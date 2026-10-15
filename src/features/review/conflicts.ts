/**
 * Conflict preview for the review inbox: which finished branches collide with their base as it
 * is now, and which touch the same files as another agent's worktree (working or finished), so
 * the merge order can be chosen before anything breaks.
 */
import { create } from "zustand";
import { worktreeConflicts, type ConflictItem, type ConflictPair, type ConflictReport } from "@/features/worktree/worktree";
import { normDir } from "@/features/usage/taskCost";
import { useWorkspaces, type WorktreeInfo } from "@/stores/workspaces";

interface ConflictState {
  report: ConflictReport | null;
  loading: boolean;
}

export const useConflicts = create<ConflictState>(() => ({ report: null, loading: false }));

let generation = 0;

/** Checks every agent worktree in the repos of `repos` against its base and the others. */
export async function refreshConflicts(repos: string[]) {
  const wanted = new Set(repos.map(normDir));
  const seen = new Set<string>();
  const infos: WorktreeInfo[] = [];
  for (const pane of useWorkspaces.getState().workspaces.flatMap((w) => w.panes)) {
    const wt = pane.worktree;
    if (!wt || !wanted.has(normDir(wt.repo)) || seen.has(normDir(wt.path))) continue;
    seen.add(normDir(wt.path));
    infos.push(wt);
  }
  const run = ++generation;
  if (infos.length === 0) return useConflicts.setState({ report: { items: [], pairs: [] }, loading: false });
  useConflicts.setState({ loading: true });
  try {
    const report = await worktreeConflicts(infos);
    if (run === generation) useConflicts.setState({ report, loading: false });
  } catch (err) {
    console.error("worktree_conflicts failed", err);
    if (run === generation) useConflicts.setState({ loading: false });
  }
}

export interface WorktreeConflicts {
  item: ConflictItem | undefined;
  /** Other worktrees touching the same files, with the path of the other one. */
  others: (ConflictPair & { other: string })[];
}

/** What the last check found for one worktree. */
export function conflictsFor(report: ConflictReport | null, path: string): WorktreeConflicts {
  const p = normDir(path);
  if (!report) return { item: undefined, others: [] };
  return {
    item: report.items.find((i) => normDir(i.path) === p),
    others: report.pairs.flatMap((pair) => (normDir(pair.a) === p ? [{ ...pair, other: pair.b }] : normDir(pair.b) === p ? [{ ...pair, other: pair.a }] : [])),
  };
}
