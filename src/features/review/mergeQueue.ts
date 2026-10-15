/**
 * Merge queue: finished work from the review inbox lands on its base one branch at a time.
 * Before each merge the branch takes in what the base gained meanwhile (earlier merges in the
 * queue, mostly), the tests run on that result, and only then is it merged. A branch that
 * conflicts or fails its tests stops there, and the queue goes on with the next one.
 */
import { invoke } from "@tauri-apps/api/core";
import { create } from "zustand";
import { execute } from "@/commands/bus";
import { recordMerge, snapshotForHistory } from "@/features/history/history";
import { commitWorktree, mergeWorktree, syncWorktree } from "@/features/worktree/worktree";
import { logActivity } from "@/stores/activity";
import { providerName, useWorkspaces, type Pane, type WorktreeInfo } from "@/stores/workspaces";
import { commitMessage, removeReview, reviewById, runTests, setReviewStatus, useReviews } from "./reviews";

export type QueueState = "queued" | "syncing" | "testing" | "merging" | "merged" | "conflict" | "failed";

export interface QueueItem {
  reviewId: string;
  paneId: string;
  workspaceId: string;
  branch: string;
  base: string;
  state: QueueState;
  /** Why it stopped, or what it's doing. */
  detail: string | null;
  /** Conflicting files, when `state` is "conflict". */
  files: string[];
}

interface QueueStore {
  items: QueueItem[];
  running: boolean;
  /** Run the project's tests on each branch (with the base merged in) before merging it. */
  runTests: boolean;
}

export const useMergeQueue = create<QueueStore>(() => ({ items: [], running: false, runTests: true }));

const patch = (reviewId: string, p: Partial<QueueItem>) =>
  useMergeQueue.setState((s) => ({ items: s.items.map((i) => (i.reviewId === reviewId ? { ...i, ...p } : i)) }));

const paneById = (id: string): Pane | undefined =>
  useWorkspaces
    .getState()
    .workspaces.flatMap((w) => w.panes)
    .find((p) => p.id === id);

export const queueItem = (reviewId: string) => useMergeQueue.getState().items.find((i) => i.reviewId === reviewId);

/** Waiting or being worked on (not finished or stopped). */
export const isPending = (i: QueueItem | undefined) => !!i && ["queued", "syncing", "testing", "merging"].includes(i.state);

/** Adds open reviews to the queue (in the order given) and starts it. */
export function enqueue(reviewIds: string[]) {
  const added: QueueItem[] = [];
  for (const id of new Set(reviewIds)) {
    const review = reviewById(id);
    const pane = review && paneById(review.paneId);
    if (!review || review.status !== "open" || !pane?.worktree || isPending(queueItem(id))) continue;
    added.push({ reviewId: id, paneId: pane.id, workspaceId: review.workspaceId, branch: pane.worktree.branch, base: pane.worktree.base, state: "queued", detail: null, files: [] });
  }
  if (added.length === 0) return;
  const ids = new Set(added.map((a) => a.reviewId));
  useMergeQueue.setState((s) => ({ items: [...s.items.filter((i) => !ids.has(i.reviewId)), ...added] }));
  void run();
}

/** Takes a waiting branch out of the queue, or forgets a finished or stopped one. */
export function dequeue(reviewId: string) {
  const item = queueItem(reviewId);
  if (!item || (isPending(item) && item.state !== "queued")) return;
  useMergeQueue.setState((s) => ({ items: s.items.filter((i) => i.reviewId !== reviewId) }));
}

export const retry = (reviewId: string) => enqueue([reviewId]);

export const clearFinished = () => useMergeQueue.setState((s) => ({ items: s.items.filter((i) => isPending(i)) }));

export const setQueueTests = (on: boolean) => useMergeQueue.setState({ runTests: on });

/** Hands a conflicting branch back to its agent to bring the base in and resolve. */
export function askAgentToResolve(reviewId: string) {
  const item = queueItem(reviewId);
  const pane = item && paneById(item.paneId);
  if (!item || !pane) return;
  const text = [
    `Merging your branch ${item.branch} into ${item.base} conflicts${item.files.length ? ` in ${item.files.join(", ")}` : ""}.`,
    `Merge ${item.base} into your branch (git merge ${item.base}), resolve the conflicts keeping both sides' intent, make sure the tests pass, and commit.`,
  ].join(" ");
  execute({ type: "pane.sendText", target: { id: pane.id }, text, submit: true });
  logActivity(item.workspaceId, `Asked ${providerName(useWorkspaces.getState().providers, pane.provider)} to resolve conflicts with ${item.base}`, pane.provider);
  useMergeQueue.setState((s) => ({ items: s.items.filter((i) => i.reviewId !== reviewId) }));
  removeReview(reviewId); // a fresh card arrives when the agent finishes again
}

async function process(item: QueueItem) {
  const review = reviewById(item.reviewId);
  const pane = paneById(item.paneId);
  if (!review || review.status !== "open") return patch(item.reviewId, { state: "failed", detail: "The review was closed." });
  if (!pane?.worktree) return patch(item.reviewId, { state: "failed", detail: "The agent's pane or worktree is gone." });
  const worktree: WorktreeInfo = pane.worktree;
  const name = providerName(useWorkspaces.getState().providers, pane.provider);

  patch(item.reviewId, { state: "syncing", detail: `Bringing in ${worktree.base}…` });
  await commitWorktree(worktree, commitMessage(review) ?? `Changes by ${name}`);
  // Taken before merging: afterwards the branch has no diff left to describe.
  const snapshot = await snapshotForHistory(pane as Pane & { worktree: WorktreeInfo });
  const sync = await syncWorktree(worktree);
  if (sync.kind === "conflicts") {
    return patch(item.reviewId, { state: "conflict", files: sync.files, detail: `Conflicts with ${worktree.base}` });
  }

  const before = review.tests;
  // Tests that already passed on this exact branch needn't run again.
  const fresh = sync.kind === "upToDate" && before.state === "done" && before.value?.ok;
  if (useMergeQueue.getState().runTests && !fresh) {
    const command = review.testCommand ?? (await invoke<string | null>("review_test_command", { dir: review.dir }).catch(() => null));
    if (command) {
      patch(item.reviewId, { state: "testing", detail: `Running ${command}${sync.kind === "synced" ? ` with ${worktree.base}'s ${sync.commits} new commit${sync.commits === 1 ? "" : "s"}` : ""}…` });
      await runTests(review.id);
      const tests = reviewById(review.id)?.tests;
      if (tests?.state === "error") return patch(item.reviewId, { state: "failed", detail: `Tests couldn't run: ${tests.error}` });
      if (tests?.state === "done" && tests.value && !tests.value.ok) {
        return patch(item.reviewId, { state: "failed", detail: `${tests.value.command} failed${sync.kind === "synced" ? ` after bringing in ${worktree.base}` : ""}` });
      }
    }
  }

  patch(item.reviewId, { state: "merging", detail: `Merging into ${worktree.base}…` });
  await mergeWorktree(worktree);
  setReviewStatus(review.id, "merged");
  // The test result recorded is the one taken just now, not the stale one from the snapshot.
  const tests = reviewById(review.id)?.tests;
  const testLine = tests?.state === "done" && tests.value ? `${tests.value.command} ${tests.value.ok ? "passed" : "failed"}` : snapshot.tests;
  void recordMerge({ ...snapshot, tests: testLine });
  logActivity(item.workspaceId, `Merged ${worktree.branch} into ${worktree.base} (queue)`, pane.provider);
  patch(item.reviewId, { state: "merged", detail: null });
}

/** Works through the queue, one branch at a time. */
async function run() {
  if (useMergeQueue.getState().running) return;
  useMergeQueue.setState({ running: true });
  try {
    for (;;) {
      const next = useMergeQueue.getState().items.find((i) => i.state === "queued");
      if (!next) break;
      try {
        await process(next);
      } catch (err) {
        patch(next.reviewId, { state: "failed", detail: String(err) });
      }
    }
  } finally {
    useMergeQueue.setState({ running: false });
  }
}

// A card removed from the inbox takes its waiting queue entry with it.
useReviews.subscribe((s) => {
  const ids = new Set(s.reviews.map((r) => r.id));
  const { items } = useMergeQueue.getState();
  if (items.some((i) => i.state === "queued" && !ids.has(i.reviewId))) {
    useMergeQueue.setState({ items: items.filter((i) => i.state !== "queued" || ids.has(i.reviewId)) });
  }
});
