import { create } from "zustand";
import { fetchProviders, type Provider, type ProviderId } from "@/features/providers/providers";
import { insertDwindle, leaves, removeLeaf, type LayoutNode } from "@/features/layout/tree";

export type AgentStatus = "idle" | "working" | "needs-input" | "done" | "error";

export interface Pane {
  id: string;
  workspaceId: string;
  provider: ProviderId;
  title: string;
  cwd: string;
  status: AgentStatus;
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
  unread: number;
}

interface WorkspaceState {
  providers: Provider[];
  workspaces: Workspace[];
  activeWorkspaceId: string;
  focusedPaneId: string | null;
  sidebarOpen: boolean;
  browserOpen: boolean;
  paletteQuery: string | null;
  cheatSheetOpen: boolean;
  loadProviders: () => Promise<void>;
  setActiveWorkspace: (id: string) => void;
  focusPane: (id: string) => void;
  addPane: (provider: ProviderId, opts?: { splitFrom?: string }) => string;
  removePane: (id: string) => void;
  setPaneStatus: (id: string, status: AgentStatus) => void;
  /** Applies a change to the active workspace. */
  updateActive: (fn: (w: Workspace) => Partial<Workspace>) => void;
  openPalette: (query?: string) => void;
  closePalette: () => void;
  setCheatSheet: (open: boolean) => void;
  toggleSidebar: () => void;
  toggleBrowser: () => void;
}

// Workspaces are hard-coded until M3 adds SQLite persistence and a "new workspace" flow.
const seed: Workspace[] = [
  {
    id: "ws-winfer",
    name: "winfer",
    rootDir: "~/Coding/winfer",
    unread: 0,
    panes: [{ id: "p-1", workspaceId: "ws-winfer", provider: "shell", title: "winfer", cwd: "~/Coding/winfer", status: "idle" }],
    layout: { kind: "leaf", paneId: "p-1" },
    zoomedPaneId: null,
    presetIndex: 0,
  },
  { id: "ws-home", name: "home", rootDir: "~", unread: 0, panes: [], layout: null, zoomedPaneId: null, presetIndex: 0 },
];

let nextPane = 2;

const updateWorkspace = (workspaces: Workspace[], id: string, fn: (w: Workspace) => Workspace) =>
  workspaces.map((w) => (w.id === id ? fn(w) : w));

export const useWorkspaces = create<WorkspaceState>((set, get) => ({
  providers: [],
  workspaces: seed,
  activeWorkspaceId: seed[0].id,
  focusedPaneId: "p-1",
  sidebarOpen: true,
  browserOpen: true,
  paletteQuery: null,
  cheatSheetOpen: false,

  loadProviders: async () => set({ providers: await fetchProviders() }),

  setActiveWorkspace: (id) => {
    const ws = get().workspaces.find((w) => w.id === id);
    set({ activeWorkspaceId: id, focusedPaneId: ws?.layout ? leaves(ws.layout)[0] : null });
  },

  focusPane: (id) => set({ focusedPaneId: id }),

  addPane: (provider, opts) => {
    const { activeWorkspaceId, focusedPaneId } = get();
    const id = `p-${nextPane++}`;
    set((s) => ({
      focusedPaneId: id,
      workspaces: updateWorkspace(s.workspaces, activeWorkspaceId, (w) => ({
        ...w,
        panes: [
          ...w.panes,
          { id, workspaceId: w.id, provider, title: w.name, cwd: w.rootDir, status: "idle" },
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

  setPaneStatus: (id, status) =>
    set((s) => ({
      workspaces: s.workspaces.map((w) => ({
        ...w,
        panes: w.panes.map((p) => (p.id === id ? { ...p, status } : p)),
      })),
    })),

  updateActive: (fn) =>
    set((s) => ({ workspaces: updateWorkspace(s.workspaces, s.activeWorkspaceId, (w) => ({ ...w, ...fn(w) })) })),

  openPalette: (query = "") => set({ paletteQuery: query, cheatSheetOpen: false }),
  closePalette: () => set({ paletteQuery: null }),
  setCheatSheet: (open) => set({ cheatSheetOpen: open, paletteQuery: null }),

  toggleSidebar: () => set((s) => ({ sidebarOpen: !s.sidebarOpen })),
  toggleBrowser: () => set((s) => ({ browserOpen: !s.browserOpen })),
}));

export const useActiveWorkspace = () =>
  useWorkspaces((s) => s.workspaces.find((w) => w.id === s.activeWorkspaceId)!);

export const providerName = (providers: Provider[], id: ProviderId) =>
  providers.find((p) => p.id === id)?.name ?? id;
