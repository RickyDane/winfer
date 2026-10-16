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
}

export const listBranches = (dir: string) => invoke<Branch[]>("worktree_branches", { dir });

/** A worktree for `branch`, or for a new branch `newBranch` started from it. */
export const worktreeFromBranch = (dir: string, branch: string, newBranch: string | null) =>
  invoke<WorktreeInfo>("worktree_from_branch", { dir, branch, newBranch });

export const fetchBranches = (dir: string) => invoke<void>("worktree_fetch", { dir });

/** Linked worktrees of the repo containing `dir` (not the main checkout). */
export const listWorktrees = (dir: string) => invoke<ExistingWorktree[]>("worktree_list", { dir });

export const createWorktree = (dir: string, provider: string) =>
  invoke<WorktreeInfo | null>("worktree_create", { dir, provider });

export const worktreeDiff = (info: WorktreeInfo) => invoke<WorktreeDiff>("worktree_diff", { info });

export const mergeWorktree = (info: WorktreeInfo) => invoke<string>("worktree_merge", { info });

/** Commits whatever the agent left uncommitted (no-op when clean). */
export const commitWorktree = (info: WorktreeInfo, message: string) => invoke<void>("worktree_commit", { info, message });

export const removeWorktree = (info: WorktreeInfo, deleteBranch: boolean) =>
  invoke<void>("worktree_remove", { info, deleteBranch });

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
