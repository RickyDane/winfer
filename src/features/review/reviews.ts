/**
 * Review inbox: when an agent finishes, collect what it did into a card — diff stats, an LLM
 * summary, test results and a screenshot of its dev server — so the user can merge, discard or
 * send it back without visiting each pane.
 */
import { llmCompose, llmEnabled, replyLanguage, wrongScript } from "@/features/llm/llm";
import { invoke } from "@tauri-apps/api/core";
import { create } from "zustand";
import { readTerminal } from "@/features/terminal/xtermRegistry";
import { worktreeDiff, type WorktreeDiff } from "@/features/worktree/worktree";
import { speak } from "@/features/voice/readback";
import { providerName, useWorkspaces, type Pane, type WorktreeInfo } from "@/stores/workspaces";
import { discardPane, mergePane } from "@/commands/bus";
import { recordMerge, snapshotForHistory } from "@/features/history/history";
import { refreshTaskCosts } from "@/features/usage/taskCost";

export interface DiffStats {
  files: string[];
  adds: number;
  dels: number;
  ahead: number;
  dirty: boolean;
}

export interface TestRun {
  command: string;
  ok: boolean;
  output: string;
  ms: number;
}

type Load<T> = { state: "idle" } | { state: "loading" } | { state: "done"; value: T } | { state: "error"; error: string };

export interface Review {
  id: string;
  paneId: string;
  workspaceId: string;
  provider: string;
  raceId: string | null;
  /** The task the agent was started with, if any. */
  prompt: string | null;
  finishedAt: number;
  branch: string | null;
  /** Where the agent's changes live (its worktree, or the pane's folder). */
  dir: string;
  /** Null for agents without a worktree: their changes can't be told apart. */
  stats: DiffStats | null;
  /** Tail of the agent's terminal: its last message. */
  lastOutput: string;
  summary: Load<string> | { state: "off" };
  /** `value: null` when the project has no test command. */
  tests: Load<TestRun | null>;
  testCommand: string | null;
  screenshot: Load<string> & { url?: string };
  status: "open" | "merged" | "discarded";
}

interface ReviewState {
  reviews: Review[];
}

export const useReviews = create<ReviewState>(() => ({ reviews: [] }));

const update = (id: string, patch: Partial<Review>) =>
  useReviews.setState((s) => ({ reviews: s.reviews.map((r) => (r.id === id ? { ...r, ...patch } : r)) }));

export const reviewById = (id: string) => useReviews.getState().reviews.find((r) => r.id === id);

export const reviewFor = (paneId: string) => useReviews.getState().reviews.find((r) => r.paneId === paneId && r.status === "open");

/** Open reviews of a workspace, newest first. */
export const openReviews = (workspaceId: string) =>
  useReviews
    .getState()
    .reviews.filter((r) => r.workspaceId === workspaceId && r.status === "open")
    .sort((a, b) => b.finishedAt - a.finishedAt);

export function removeReview(id: string) {
  useReviews.setState((s) => ({ reviews: s.reviews.filter((r) => r.id !== id) }));
}

export const setReviewStatus = (id: string, status: Review["status"]) => update(id, { status });

/** Files, line counts and commit state from a worktree diff. */
export function diffStats(d: WorktreeDiff): DiffStats {
  const files = new Set<string>();
  let adds = 0;
  let dels = 0;
  for (const line of d.diff.split("\n")) {
    const header = /^diff --git a\/(.+) b\/(.+)$/.exec(line);
    if (header) files.add(header[2]);
    else if (line.startsWith("+") && !line.startsWith("+++")) adds++;
    else if (line.startsWith("-") && !line.startsWith("---")) dels++;
  }
  d.untracked.forEach((f) => files.add(f));
  return { files: [...files], adds, dels, ahead: d.ahead, dirty: d.dirty };
}

/** The agent's last words, without blank lines and prompt chrome. */
function lastOutput(paneId: string): string {
  const text = readTerminal(paneId, 60) ?? "";
  return text
    .split("\n")
    .map((l) => l.trimEnd())
    .filter((l) => l.trim() && !/^[─━═\s>│╭╮╰╯]+$/.test(l))
    .slice(-25)
    .join("\n")
    .slice(-1800);
}

const SUMMARY_SYSTEM = [
  "You review the work a coding agent just finished, for a developer deciding whether to merge it.",
  "From the diff and the agent's last output, write 2–3 short sentences: what changed and why, then anything risky,",
  "unfinished or worth checking (failing steps, TODOs, deleted code). Name files only when it helps.",
  "Output only the summary.",
].join(" ");

async function summarize(review: Review, diff: string) {
  if (!llmEnabled()) return update(review.id, { summary: { state: "off" } });
  update(review.id, { summary: { state: "loading" } });
  const user = [
    review.prompt && `Task: ${review.prompt}`,
    review.stats && `Files changed: ${review.stats.files.join(", ") || "none"} (+${review.stats.adds} −${review.stats.dels})`,
    diff && `Diff:\n${diff.slice(0, 12_000)}${diff.length > 12_000 ? "\n…(truncated)" : ""}`,
    `Agent's last output:\n${review.lastOutput}`,
  ]
    .filter(Boolean)
    .join("\n\n");
  try {
    const task = review.prompt ?? "";
    const system = `${SUMMARY_SYSTEM} Write it in ${replyLanguage(task)}.`;
    let text = await llmCompose(system, user);
    // Models sometimes switch languages anyway; one retry usually fixes it.
    if (wrongScript(text, task)) text = await llmCompose(system, user);
    update(review.id, { summary: { state: "done", value: text.trim() } });
  } catch (err) {
    update(review.id, { summary: { state: "error", error: String(err) } });
  }
}

export async function runTests(id: string) {
  const review = useReviews.getState().reviews.find((r) => r.id === id);
  if (!review) return;
  update(id, { tests: { state: "loading" } });
  try {
    const value = await invoke<TestRun | null>("review_run_tests", { dir: review.dir });
    update(id, { tests: { state: "done", value } });
  } catch (err) {
    update(id, { tests: { state: "error", error: String(err) } });
  }
}

export async function captureScreenshot(id: string) {
  const review = useReviews.getState().reviews.find((r) => r.id === id);
  const pane = review && paneById(review.paneId);
  if (!review || !pane?.devUrl) return;
  const url = pane.devUrl;
  update(id, { screenshot: { state: "loading", url } });
  try {
    const value = await invoke<string>("browser_screenshot", { workspaceId: review.workspaceId, url });
    update(id, { screenshot: { state: "done", value, url } });
  } catch (err) {
    update(id, { screenshot: { state: "error", error: String(err), url } });
  }
}

const paneById = (id: string): Pane | undefined =>
  useWorkspaces
    .getState()
    .workspaces.flatMap((w) => w.panes)
    .find((p) => p.id === id);

/** Builds (or refreshes) the card for an agent that just finished. */
export async function collectReview(paneId: string) {
  const pane = paneById(paneId);
  if (!pane || pane.provider === "shell") return;

  let diff = "";
  let stats: DiffStats | null = null;
  if (pane.worktree) {
    try {
      const d = await worktreeDiff(pane.worktree);
      diff = d.diff;
      stats = diffStats(d);
    } catch {
      return; // worktree gone (discarded meanwhile)
    }
    // A turn that changed nothing (a question, a plan) isn't work to review.
    if (stats.files.length === 0 && stats.ahead === 0) return;
  }

  const existing = reviewFor(paneId);
  const s = useWorkspaces.getState();
  const config = s.reviewConfig;
  const review: Review = {
    id: existing?.id ?? crypto.randomUUID(),
    paneId,
    workspaceId: pane.workspaceId,
    provider: pane.provider,
    raceId: pane.raceId ?? null,
    prompt: pane.prompt ?? null,
    finishedAt: Date.now(),
    branch: pane.worktree?.branch ?? null,
    dir: pane.worktree?.path ?? pane.cwd,
    stats,
    lastOutput: lastOutput(paneId),
    summary: { state: "idle" },
    tests: { state: "idle" },
    testCommand: null,
    screenshot: { state: "idle" },
    status: "open",
  };
  useReviews.setState((st) => ({ reviews: [review, ...st.reviews.filter((r) => r.id !== review.id)] }));

  void summarize(review, diff);
  if (pane.worktree) void refreshTaskCosts([pane.worktree.path]);
  void invoke<string | null>("review_test_command", { dir: review.dir }).then(
    (testCommand) => {
      update(review.id, { testCommand });
      if (testCommand && config.autoTests) void runTests(review.id);
    },
    () => {},
  );
  if (pane.devUrl && config.screenshots !== false) void captureScreenshot(review.id);
}

// Hooks fire per turn, and output-idle detection can flap: wait for the agent to stay finished.
const pending = new Map<string, number>();
export function agentFinished(paneId: string) {
  clearTimeout(pending.get(paneId));
  pending.set(
    paneId,
    window.setTimeout(() => {
      pending.delete(paneId);
      const pane = paneById(paneId);
      if (pane && pane.status !== "working") void collectReview(paneId);
    }, 1500),
  );
}

/** Reads the open reviews of the active workspace aloud ("what's done?"). */
export function readReviewsAloud() {
  const s = useWorkspaces.getState();
  const reviews = openReviews(s.activeWorkspaceId);
  if (reviews.length === 0) return speak("Nothing is waiting for review.", { force: true });
  const lines = reviews.map((r, i) => {
    const name = providerName(s.providers, r.provider);
    const summary = r.summary.state === "done" ? r.summary.value : r.stats ? `${r.stats.files.length} files changed` : "finished";
    const tests = r.tests.state === "done" && r.tests.value ? (r.tests.value.ok ? " Tests pass." : " Tests fail.") : "";
    return `${i + 1}: ${name}. ${summary}${tests}`;
  });
  speak(`${reviews.length} ready for review. ${lines.join(" ")}`, { force: true });
}

/** First sentence of the summary, as a commit message for changes the agent didn't commit. */
export function commitMessage(review: Review | undefined): string | undefined {
  if (review?.summary.state !== "done") return review?.prompt ? `winfer: ${review.prompt.slice(0, 72)}` : undefined;
  const first = review.summary.value.split(/(?<=[.!?])\s/)[0];
  return first.length > 100 ? `${first.slice(0, 97)}…` : first;
}

/** Merge from a card, the pane menu or voice; the card (if any) shows the result. */
export async function mergeReviewed(pane: { id: string; provider: string; worktree: WorktreeInfo }) {
  const review = reviewFor(pane.id);
  // Taken before merging: afterwards the branch has no diff left to describe.
  const full = paneById(pane.id);
  const snapshot = full?.worktree ? await snapshotForHistory(full as Pane & { worktree: WorktreeInfo }) : null;
  if (await mergePane(pane, commitMessage(review))) {
    if (review) setReviewStatus(review.id, "merged");
    if (snapshot) void recordMerge(snapshot);
  }
}

export async function discardReviewed(pane: { id: string; worktree: WorktreeInfo }) {
  const review = reviewFor(pane.id);
  if (await discardPane(pane)) {
    if (review) setReviewStatus(review.id, "discarded");
  }
}
