import { invoke } from "@tauri-apps/api/core";
import type { WorktreeInfo } from "@/stores/workspaces";

export interface WorktreeDiff {
  diff: string;
  untracked: string[];
  /** Lines in the untracked text files. */
  untrackedLines: number;
  ahead: number;
  dirty: boolean;
}

export interface ExistingWorktree {
  info: WorktreeInfo;
  /** Created by winfer (branch winfer/…). */
  byWinfer: boolean;
  /** Its folder is gone. */
  prunable: boolean;
  /** Folder creation and last commit on the branch, ms since the epoch. */
  createdAt: number | null;
  lastCommitAt: number | null;
  /** Commits not on the base yet. */
  ahead: number;
  /** Uncommitted changes in the folder. */
  dirty: boolean;
  /** Nothing on it that the base lacks: removing it loses nothing. */
  merged: boolean;
  /** The project folder itself (the repo's main checkout). */
  isMain: boolean;
}

export interface Branch {
  /** "feature/login", or "origin/feature/login" for remote ones. */
  name: string;
  remote: boolean;
  lastCommitAt: number | null;
  /** Folder where it's checked out, if anywhere. */
  checkedOutAt: string | null;
  /** Remote branch it can be updated from: its upstream, or itself for remote ones. */
  upstream: string | null;
}

export const listBranches = (dir: string) => invoke<Branch[]>("worktree_branches", { dir });

/**
 * A worktree for `branch`, or for a new branch `newBranch` started from it. With `latest`, the
 * branch's remote is fetched first and the worktree gets the remote's newest commits.
 */
export const worktreeFromBranch = (dir: string, branch: string, newBranch: string | null, latest = false) =>
  invoke<WorktreeInfo>("worktree_from_branch", { dir, branch, newBranch, latest });

export const fetchBranches = (dir: string) => invoke<void>("worktree_fetch", { dir });

/** Listings under way, by folder: callers asking at the same time share one (each runs several git commands per worktree). */
const listing = new Map<string, Promise<ExistingWorktree[]>>();

/** Linked worktrees of the repo containing `dir` (not the main checkout). */
export function listWorktrees(dir: string): Promise<ExistingWorktree[]> {
  let pending = listing.get(dir);
  if (!pending) {
    pending = invoke<ExistingWorktree[]>("worktree_list", { dir }).finally(() => listing.delete(dir));
    listing.set(dir, pending);
  }
  return pending;
}

export const createWorktree = (dir: string, provider: string) =>
  invoke<WorktreeInfo | null>("worktree_create", { dir, provider });

export const worktreeDiff = (info: WorktreeInfo) => invoke<WorktreeDiff>("worktree_diff", { info });

/** Files the worktree changed since it branched off (untracked included), without the diff itself. */
export const worktreeChangedFiles = (info: WorktreeInfo) => invoke<string[]>("worktree_changed_files", { info });

export const mergeWorktree = (info: WorktreeInfo) => invoke<string>("worktree_merge", { info });

/** Commits whatever the agent left uncommitted (no-op when clean). */
export const commitWorktree = (info: WorktreeInfo, message: string) => invoke<void>("worktree_commit", { info, message });

export const removeWorktree = (info: WorktreeInfo, deleteBranch: boolean) =>
  invoke<void>("worktree_remove", { info, deleteBranch });

export interface RemoveFailure {
  path: string;
  branch: string;
  error: string;
}

/** Removes several worktrees one after another; the ones that couldn't go come back. */
export const removeWorktrees = (items: { info: WorktreeInfo; deleteBranch: boolean }[]) =>
  invoke<RemoveFailure[]>("worktree_remove_many", { items });

/** Worktrees with no activity for this long count as stale. */
export const STALE_DAYS = 10;
const STALE_MS = STALE_DAYS * 86_400_000;

/** Last activity: the branch's last commit, else when the folder was made. */
export const lastActivity = (wt: ExistingWorktree) => wt.lastCommitAt ?? wt.createdAt;

/** No commit (or, without one, folder creation) for `STALE_DAYS` days. Never the project folder. */
export const isStale = (wt: ExistingWorktree, now = Date.now()) => {
  const at = lastActivity(wt);
  return !wt.isMain && at !== null && now - at > STALE_MS;
};

export interface ConflictItem {
  path: string;
  branch: string;
  /** Files changed since branching off, committed or not. */
  files: string[];
  /** Commits the base gained since: it moved on. */
  behind: number;
  /** Files that conflict if the committed work is merged into the base now. */
  baseConflicts: string[];
}

export interface ConflictPair {
  /** Worktree paths. */
  a: string;
  b: string;
  /** Files both changed. */
  overlap: string[];
  /** Of those, the ones whose committed changes actually conflict. */
  conflicts: string[];
}

export interface ConflictReport {
  items: ConflictItem[];
  pairs: ConflictPair[];
}

/** Which worktrees collide with their base, and with each other (same repo only). */
export const worktreeConflicts = (infos: WorktreeInfo[]) => invoke<ConflictReport>("worktree_conflicts", { infos });

export type SyncOutcome = { kind: "upToDate" } | { kind: "synced"; commits: number } | { kind: "conflicts"; files: string[] };

/** Merges the base's new commits into the worktree branch; backs out on conflicts. */
export const syncWorktree = (info: WorktreeInfo) => invoke<SyncOutcome>("worktree_sync", { info });
