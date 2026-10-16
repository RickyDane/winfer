/**
 * What a task cost: the agents' usage logs say which folder each model call ran in, and an
 * agent's worktree is its task, so a worktree's spend is the task's (see `dir_usage` in
 * src-tauri/src/usage_history.rs). Shown on review cards, in the merge history and per task on
 * the Usage page.
 */
import { invoke } from "@tauri-apps/api/core";
import { create } from "zustand";
import { useWorkspaces, type Pane } from "@/stores/workspaces";
import type { ModelTotal } from "./history";

export interface DirUsage {
  dir: string;
  cost: number;
  /** False when a model has no known price (its cost counts as 0). */
  priced: boolean;
  tokens: number;
  calls: number;
  firstAt: number | null;
  lastAt: number | null;
  /** Most expensive first. */
  models: ModelTotal[];
}

/** Spend in each folder since it was made. */
export const fetchDirUsage = (dirs: string[]) => invoke<DirUsage[]>("usage_dirs", { dirs });

/** Spend in each agent session (keyed by the session in `dir`). */
export const fetchSessionUsage = (sessions: string[]) => invoke<DirUsage[]>("usage_sessions", { sessions });

/** Spend per folder over the last `days`, most expensive first. */
export const fetchUsageByDir = (days: number) => invoke<DirUsage[]>("usage_by_dir", { days });

/** Folders compare with forward slashes and without a trailing slash, like the backend's. */
export const normDir = (dir: string) => dir.replace(/\\/g, "/").replace(/(.)\/+$/, "$1");

interface TaskCostState {
  /** By normalized folder. */
  byDir: Record<string, DirUsage>;
  /** By agent session id, for panes without a worktree of their own. */
  bySession: Record<string, DirUsage>;
}

export const useTaskCosts = create<TaskCostState>(() => ({ byDir: {}, bySession: {} }));

/** Reads fresh numbers for `dirs` into the store (failures keep the old ones). */
export async function refreshTaskCosts(dirs: string[]): Promise<DirUsage[]> {
  const unique = [...new Set(dirs.map(normDir))];
  if (unique.length === 0) return [];
  try {
    const found = await fetchDirUsage(unique);
    useTaskCosts.setState((s) => ({ byDir: { ...s.byDir, ...Object.fromEntries(found.map((d) => [normDir(d.dir), d])) } }));
    return found;
  } catch (err) {
    console.error("usage_dirs failed", err);
    return [];
  }
}

/** Reads fresh numbers for agent `sessions` into the store (failures keep the old ones). */
export async function refreshSessionCosts(sessions: string[]): Promise<DirUsage[]> {
  const unique = [...new Set(sessions)];
  if (unique.length === 0) return [];
  try {
    const found = await fetchSessionUsage(unique);
    useTaskCosts.setState((s) => ({ bySession: { ...s.bySession, ...Object.fromEntries(found.map((d) => [d.dir, d])) } }));
    return found;
  } catch (err) {
    console.error("usage_sessions failed", err);
    return [];
  }
}

/** The spend recorded for an agent session, if numbers were read. */
export const useSessionCost = (session: string | null | undefined) => useTaskCosts((s) => (session ? s.bySession[session] : undefined));

/** The spend recorded for a pane's worktree, if it has one and numbers were read. */
export const useTaskCost = (dir: string | null | undefined) => useTaskCosts((s) => (dir ? s.byDir[normDir(dir)] : undefined));

/** What to call a folder on the Usage page: the session working there, else its project or name. */
export function dirLabel(dir: string, panes: Pane[] = useWorkspaces.getState().workspaces.flatMap((w) => w.panes)): { title: string; detail: string | null; paneId: string | null } {
  const d = normDir(dir);
  const pane = panes.find((p) => p.worktree && normDir(p.worktree.path) === d) ?? panes.find((p) => !p.worktree && normDir(p.cwd) === d);
  const workspace = useWorkspaces.getState().workspaces.find((w) => normDir(w.rootDir) === d);
  const leaf = d.split("/").pop() || d;
  if (pane) {
    const title = pane.customTitle ?? pane.agentTitle ?? pane.worktree?.branch.replace(/^winfer\//, "") ?? leaf;
    return { title, detail: pane.worktree?.branch ?? d, paneId: pane.id };
  }
  if (workspace) return { title: workspace.name, detail: "project folder", paneId: null };
  // winfer's own worktrees: <app data>/worktrees/<repo>-<hash>/<leaf>.
  const parts = d.split("/");
  const at = parts.lastIndexOf("worktrees");
  if (at >= 0 && at + 2 < parts.length) return { title: parts[at + 2], detail: parts[at + 1].replace(/-[0-9a-f]{8}$/, ""), paneId: null };
  return { title: leaf, detail: d, paneId: null };
}
