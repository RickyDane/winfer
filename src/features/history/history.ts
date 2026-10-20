/**
 * History of merged features: every merge (review inbox, pane menu, voice, race winner) is
 * recorded with what the review knew about it, and can be searched later.
 */
import { invoke } from "@tauri-apps/api/core";
import { useProgress } from "@/features/progress/progress";
import { diffStats, reviewFor, type DiffStats } from "@/features/review/reviews";
import { refreshTaskCosts } from "@/features/usage/taskCost";
import { worktreeDiff } from "@/features/worktree/worktree";
import { useWorkspaces, type Pane, type WorktreeInfo } from "@/stores/workspaces";

export interface MergeRecord {
  id: string;
  mergedAt: number;
  workspaceId: string | null;
  workspace: string;
  repo: string;
  branch: string;
  base: string;
  commitSha: string | null;
  provider: string;
  title: string;
  summary: string | null;
  task: string | null;
  files: string[];
  adds: number;
  dels: number;
  tests: string | null;
  /** What the agents spent on it at API prices (US dollars), when the usage logs knew. */
  cost: number | null;
  tokens: number | null;
}

/** What to remember about a branch, taken before merging (afterwards its diff is empty). */
export type MergeSnapshot = Omit<MergeRecord, "id" | "mergedAt" | "commitSha">;

/** First sentence of a summary, as a headline. */
const headline = (text: string) => {
  const first = text.split(/(?<=[.!?])\s/)[0].trim();
  return first.length > 120 ? `${first.slice(0, 117)}…` : first;
};

export async function snapshotForHistory(pane: Pane & { worktree: WorktreeInfo }): Promise<MergeSnapshot> {
  const s = useWorkspaces.getState();
  const review = reviewFor(pane.id);
  const progress = useProgress.getState().byPane[pane.id];
  let stats: DiffStats | null = review?.stats ?? null;
  if (!stats) stats = await worktreeDiff(pane.worktree).then(diffStats, () => null);
  const [usage] = await refreshTaskCosts([pane.worktree.path]);
  const summary = review?.summary.state === "done" ? review.summary.value : null;
  const task = progress?.task ?? pane.prompt ?? null;
  const tests = review?.tests.state === "done" && review.tests.value ? `${review.tests.value.command} ${review.tests.value.ok ? "passed" : "failed"}` : null;
  return {
    workspaceId: pane.workspaceId,
    workspace: s.workspaces.find((w) => w.id === pane.workspaceId)?.name ?? "",
    repo: pane.worktree.repo,
    branch: pane.worktree.branch,
    base: pane.worktree.base,
    provider: pane.provider,
    title: pane.customTitle ?? progress?.title ?? pane.agentTitle ?? (summary ? headline(summary) : null) ?? task?.split("\n")[0].slice(0, 120) ?? pane.worktree.branch,
    summary,
    task,
    files: stats?.files ?? [],
    adds: stats?.adds ?? 0,
    dels: stats?.dels ?? 0,
    tests,
    cost: usage?.calls ? usage.cost : null,
    tokens: usage?.calls ? usage.tokens : null,
  };
}

export async function recordMerge(snapshot: MergeSnapshot) {
  const merge: MergeRecord = { ...snapshot, id: crypto.randomUUID(), mergedAt: Date.now(), commitSha: null };
  await invoke("history_add", { merge }).catch((err) => console.error("history_add failed", err));
}

export const searchHistory = (query: string, workspaceId: string | null) =>
  invoke<MergeRecord[]>("history_search", { query, workspaceId });

export const deleteHistoryEntry = (id: string) => invoke<void>("history_delete", { id });
