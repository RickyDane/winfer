import { invoke } from "@tauri-apps/api/core";
import { create } from "zustand";
import { fetchProviders, type Provider, type ProviderId } from "@/features/providers/providers";
import { insertDwindle, leaves, removeLeaf, type LayoutNode } from "@/features/layout/tree";

export type AgentStatus = "idle" | "working" | "needs-input" | "done" | "error";

export interface WorktreeInfo {
  repo: string;
  path: string;
  branch: string;
  base: string;
}

export interface Pane {
  id: string;
  workspaceId: string;
  provider: ProviderId;
  title: string;
  cwd: string;
  status: AgentStatus;
  /** Set once the pane runs in its own git worktree. */
  worktree: WorktreeInfo | null;
  /** Requested a worktree that hasn't been created yet. */
  wantsWorktree: boolean;
  /** Restored from a previous session: spawn with the provider's resume args. */
  resume: boolean;
  /** Status changed while the user wasn't looking at this pane. */
  unseen: boolean;
}

export interface Workspace {
  id: string;
  name: string;
  rootDir: string;
  panes: Pane[];
  layout: LayoutNode | null;
  /** Pane shown alone (⌘↩), or null. */
  zoomedPaneId: string | null;
  presetIndex: number;
  /** New agent panes get their own git worktree (on by default). */
  useWorktrees: boolean;
}

interface WorkspaceState {
  hydrated: boolean;
  providers: Provider[];
  workspaces: Workspace[];
  activeWorkspaceId: string;
  focusedPaneId: string | null;
  sidebarOpen: boolean;
  browserOpen: boolean;
  paletteQuery: string | null;
  cheatSheetOpen: boolean;
  hydrate: () => Promise<void>;
  loadProviders: () => Promise<void>;
  setActiveWorkspace: (id: string) => void;
  createWorkspace: (name: string, rootDir: string) => string;
  removeWorkspace: (id: string) => void;
  focusPane: (id: string) => void;
  addPane: (provider: ProviderId, opts?: { splitFrom?: string; worktree?: boolean }) => string;
  removePane: (id: string) => void;
  updatePane: (id: string, patch: Partial<Pane>) => void;
  setPaneStatus: (id: string, status: AgentStatus) => void;
  /** Applies a change to the active workspace. */
  updateActive: (fn: (w: Workspace) => Partial<Workspace>) => void;
  openPalette: (query?: string) => void;
  closePalette: () => void;
  setCheatSheet: (open: boolean) => void;
  toggleSidebar: () => void;
  toggleBrowser: () => void;
}

let hydration: Promise<Snapshot> | null = null;

const newId = (prefix: string) => `${prefix}-${crypto.randomUUID().slice(0, 8)}`;

function newWorkspace(name: string, rootDir: string): Workspace {
  return { id: newId("ws"), name, rootDir, panes: [], layout: null, zoomedPaneId: null, presetIndex: 0, useWorktrees: true };
}

const updateWorkspace = (workspaces: Workspace[], id: string, fn: (w: Workspace) => Workspace) =>
  workspaces.map((w) => (w.id === id ? fn(w) : w));

const firstPane = (ws?: Workspace) => (ws?.layout ? leaves(ws.layout)[0] : null);

export const useWorkspaces = create<WorkspaceState>((set, get) => ({
  hydrated: false,
  providers: [],
  workspaces: [],
  activeWorkspaceId: "",
  focusedPaneId: null,
  sidebarOpen: true,
  browserOpen: true,
  paletteQuery: null,
  cheatSheetOpen: false,

  hydrate: async () => {
    hydration ??= loadSnapshot();
    const snapshot = await hydration;
    if (get().hydrated) return;
    if (snapshot.workspaces.length === 0) {
      const home = newWorkspace("home", "~");
      set({ hydrated: true, workspaces: [home], activeWorkspaceId: home.id });
      return;
    }

    // Only one restored pane per provider + directory resumes its last session; others
    // sharing that directory would all resume the same conversation.
    const resumed = new Set<string>();
    const workspaces: Workspace[] = snapshot.workspaces.map((w) => ({
      ...w,
      panes: w.panes.map((p) => {
        const key = `${p.provider}:${p.cwd}`;
        const resume = !resumed.has(key);
        resumed.add(key);
        return { ...p, workspaceId: w.id, status: "idle", wantsWorktree: false, resume, unseen: false };
      }),
    }));
    const ui = snapshot.ui ?? {};
    const active = workspaces.find((w) => w.id === ui.activeWorkspaceId) ?? workspaces[0];
    set({
      hydrated: true,
      workspaces,
      activeWorkspaceId: active.id,
      focusedPaneId: firstPane(active),
      sidebarOpen: ui.sidebarOpen ?? true,
      browserOpen: ui.browserOpen ?? true,
    });
  },

  loadProviders: async () => set({ providers: await fetchProviders() }),

  setActiveWorkspace: (id) => {
    const ws = get().workspaces.find((w) => w.id === id);
    set({ activeWorkspaceId: id, focusedPaneId: firstPane(ws) });
  },

  createWorkspace: (name, rootDir) => {
    const ws = newWorkspace(name, rootDir);
    set((s) => ({ workspaces: [...s.workspaces, ws], activeWorkspaceId: ws.id, focusedPaneId: null }));
    return ws.id;
  },

  removeWorkspace: (id) =>
    set((s) => {
      const workspaces = s.workspaces.filter((w) => w.id !== id);
      if (workspaces.length === 0) workspaces.push(newWorkspace("home", "~"));
      if (s.activeWorkspaceId !== id) return { workspaces };
      return { workspaces, activeWorkspaceId: workspaces[0].id, focusedPaneId: firstPane(workspaces[0]) };
    }),

  focusPane: (id) => {
    set({ focusedPaneId: id });
    get().updatePane(id, { unseen: false });
  },

  addPane: (provider, opts) => {
    const { activeWorkspaceId, focusedPaneId } = get();
    const id = newId("p");
    set((s) => ({
      focusedPaneId: id,
      workspaces: updateWorkspace(s.workspaces, activeWorkspaceId, (w) => ({
        ...w,
        panes: [
          ...w.panes,
          {
            id,
            workspaceId: w.id,
            provider,
            title: w.name,
            cwd: w.rootDir,
            status: "idle",
            worktree: null,
            wantsWorktree: provider !== "shell" && (opts?.worktree ?? w.useWorktrees),
            resume: false,
            unseen: false,
          },
        ],
        layout: insertDwindle(w.layout, id, opts?.splitFrom ?? focusedPaneId),
        zoomedPaneId: null,
      })),
    }));
    return id;
  },

  removePane: (id) =>
    set((s) => {
      const workspaces = s.workspaces.map((w) =>
        w.panes.some((p) => p.id === id)
          ? {
              ...w,
              panes: w.panes.filter((p) => p.id !== id),
              layout: removeLeaf(w.layout, id),
              zoomedPaneId: w.zoomedPaneId === id ? null : w.zoomedPaneId,
            }
          : w,
      );
      const active = workspaces.find((w) => w.id === s.activeWorkspaceId);
      const focusedPaneId =
        s.focusedPaneId === id ? (active?.layout ? leaves(active.layout).at(-1)! : null) : s.focusedPaneId;
      return { workspaces, focusedPaneId };
    }),

  updatePane: (id, patch) =>
    set((s) => ({
      workspaces: s.workspaces.map((w) =>
        w.panes.some((p) => p.id === id) ? { ...w, panes: w.panes.map((p) => (p.id === id ? { ...p, ...patch } : p)) } : w,
      ),
    })),

  setPaneStatus: (id, status) => {
    const s = get();
    const visible = s.focusedPaneId === id && document.hasFocus();
    s.updatePane(id, { status, unseen: !visible && (status === "needs-input" || status === "done" || status === "error") });
  },

  updateActive: (fn) =>
    set((s) => ({ workspaces: updateWorkspace(s.workspaces, s.activeWorkspaceId, (w) => ({ ...w, ...fn(w) })) })),

  openPalette: (query = "") => set({ paletteQuery: query, cheatSheetOpen: false }),
  closePalette: () => set({ paletteQuery: null }),
  setCheatSheet: (open) => set({ cheatSheetOpen: open, paletteQuery: null }),

  toggleSidebar: () => set((s) => ({ sidebarOpen: !s.sidebarOpen })),
  toggleBrowser: () => set((s) => ({ browserOpen: !s.browserOpen })),
}));

export const useActiveWorkspace = () =>
  useWorkspaces((s) => s.workspaces.find((w) => w.id === s.activeWorkspaceId));

export const providerName = (providers: Provider[], id: ProviderId) =>
  providers.find((p) => p.id === id)?.name ?? id;

// ---------------------------------------------------------------------------------------------
// Persistence (SQLite via src-tauri/src/store.rs)

type PanePersisted = Pick<Pane, "id" | "provider" | "title" | "cwd" | "worktree">;

interface Snapshot {
  workspaces: (Omit<Workspace, "panes"> & { panes: PanePersisted[] })[];
  ui: { activeWorkspaceId?: string; sidebarOpen?: boolean; browserOpen?: boolean } | null;
}

async function loadSnapshot(): Promise<Snapshot> {
  try {
    return await invoke<Snapshot>("state_load");
  } catch (err) {
    console.error("state_load failed", err);
    return { workspaces: [], ui: null };
  }
}

function toSnapshot(s: WorkspaceState): Snapshot {
  return {
    workspaces: s.workspaces.map(({ panes, ...w }) => ({
      ...w,
      panes: panes.map(({ id, provider, title, cwd, worktree }) => ({ id, provider, title, cwd, worktree })),
    })),
    ui: { activeWorkspaceId: s.activeWorkspaceId, sidebarOpen: s.sidebarOpen, browserOpen: s.browserOpen },
  };
}

let saveTimer: number | undefined;
let lastSaved = "";

useWorkspaces.subscribe((s) => {
  if (!s.hydrated) return;
  clearTimeout(saveTimer);
  saveTimer = window.setTimeout(() => {
    const snapshot = toSnapshot(useWorkspaces.getState());
    const json = JSON.stringify(snapshot);
    if (json === lastSaved) return;
    lastSaved = json;
    invoke("state_save", { snapshot }).catch((err) => console.error("state_save failed", err));
  }, 400);
});
