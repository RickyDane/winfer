import { invoke } from "@tauri-apps/api/core";
import type { WorktreeInfo } from "@/stores/workspaces";

export interface WorktreeDiff {
  diff: string;
  untracked: string[];
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
}

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
