import { invoke } from "@tauri-apps/api/core";
import { create } from "zustand";
import { fetchModels, fetchProviders, type ModelInfo, type Provider, type ProviderId } from "@/features/providers/providers";
import { buildPreset, insertDwindle, leaves, PRESETS, removeLeaf, type LayoutNode } from "@/features/layout/tree";
import type { Recipe } from "@/features/recipes/recipes";

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
  /** Latest local dev-server URL printed in this pane (e.g. http://localhost:5173/). */
  devUrl: string | null;
  /** Panes started together by race mode share an id. */
  raceId: string | null;
  /** Initial prompt given when the agent was started (not persisted). */
  prompt: string | null;
  /** Model the agent runs with; null means the CLI's default. */
  model: string | null;
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
  /** Pane whose worktree changes are shown in the diff view. */
  diffPaneId: string | null;
  settingsOpen: boolean;
  /** Open overlay for agent orchestration and recipes. */
  overlay: "race" | "broadcast" | "recipes" | "model" | "review" | "progress" | null;
  /** User-saved recipes (persisted). */
  recipes: Recipe[];
  /** Models each provider offers (from its CLI), loaded in the background. */
  models: Record<ProviderId, ModelInfo[]>;
  /** Default model per provider for new panes (persisted). */
  agentModels: Record<ProviderId, string>;
  /** Agent for new panes (⌘T, splits, "open a window"); null = Claude Code if installed. */
  defaultProvider: ProviderId | null;
  /** Shortcut overrides by binding id (see src/commands/keymap.ts); "" = switched off. */
  shortcuts: Record<string, string>;
  /** Browser panel width in px. */
  browserWidth: number;
  theme: Theme;
  /** Main area: the active workspace's tiles, or the overview of every workspace's sessions. */
  view: "workspace" | "overview";
  /** The overview tiles every session of every workspace; same shape as a workspace's layout. */
  overview: OverviewLayout;
  /** Review inbox: run tests / take screenshots automatically when an agent finishes. */
  reviewConfig: { autoTests?: boolean; screenshots?: boolean };
  /** LLM progress estimates for working agents. */
  progressConfig: { enabled?: boolean; intervalSec?: number };
  /** Pane whose scrollback search bar is open (⌘F). */
  searchPaneId: string | null;
  /** Voice model choices, persisted and pushed to the backend on launch. */
  voiceConfig: { sttModel?: string; clefModel?: string; readback?: boolean; language?: "en" | "de" | "auto"; confirm?: boolean; writer?: "off" | "workers" | "codex"; writerModel?: string; inputDevice?: string };
  hydrate: () => Promise<void>;
  loadProviders: () => Promise<void>;
  setActiveWorkspace: (id: string) => void;
  createWorkspace: (name: string, rootDir: string) => string;
  removeWorkspace: (id: string) => void;
  focusPane: (id: string) => void;
  addPane: (
    provider: ProviderId,
    opts?: { splitFrom?: string; worktree?: boolean; prompt?: string; raceId?: string; workspaceId?: string; model?: string },
  ) => string;
  removePane: (id: string) => void;
  updatePane: (id: string, patch: Partial<Pane>) => void;
  setPaneStatus: (id: string, status: AgentStatus) => void;
  /** Applies a change to the active workspace. */
  updateActive: (fn: (w: Workspace) => Partial<Workspace>) => void;
  openPalette: (query?: string) => void;
  closePalette: () => void;
  setCheatSheet: (open: boolean) => void;
  setDiffPane: (id: string | null) => void;
  setSettings: (open: boolean) => void;
  setOverlay: (overlay: "race" | "broadcast" | "recipes" | "model" | "review" | "progress" | null) => void;
  setRecipes: (recipes: Recipe[]) => void;
  loadModels: (provider: ProviderId) => Promise<ModelInfo[]>;
  setAgentModel: (provider: ProviderId, model: string | null) => void;
  setDefaultProvider: (provider: ProviderId | null) => void;
  /** Sets a binding's chord; null restores the default. */
  setShortcut: (id: string, chord: string | null) => void;
  resetShortcuts: () => void;
  setBrowserWidth: (width: number) => void;
  toggleTheme: () => void;
  setView: (view: "workspace" | "overview") => void;
  updateOverview: (fn: (o: OverviewLayout) => Partial<OverviewLayout>) => void;
  setReviewConfig: (config: { autoTests?: boolean; screenshots?: boolean }) => void;
  setProgressConfig: (config: { enabled?: boolean; intervalSec?: number }) => void;
  setSearchPane: (id: string | null) => void;
  setVoiceConfig: (config: { sttModel?: string; clefModel?: string; readback?: boolean; language?: "en" | "de" | "auto"; confirm?: boolean; writer?: "off" | "workers" | "codex"; writerModel?: string; inputDevice?: string }) => void;
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

export const DEFAULT_BROWSER_WIDTH = 420;

export interface OverviewLayout {
  layout: LayoutNode | null;
  zoomedPaneId: string | null;
  presetIndex: number;
}

const GRID = PRESETS.indexOf("grid");

/** Every session, grouped by workspace, each workspace in its own tile order. */
export function overviewOrder(workspaces: Workspace[]): string[] {
  return workspaces.flatMap((w) => (w.layout ? leaves(w.layout) : w.panes.map((p) => p.id)));
}

export type Theme = "dark" | "light";

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
  diffPaneId: null,
  settingsOpen: false,
  overlay: null,
  recipes: [],
  models: {},
  agentModels: {},
  defaultProvider: null,
  shortcuts: {},
  browserWidth: DEFAULT_BROWSER_WIDTH,
  theme: "dark",
  view: "workspace",
  overview: { layout: null, zoomedPaneId: null, presetIndex: GRID },
  reviewConfig: {},
  progressConfig: {},
  searchPaneId: null,
  voiceConfig: {},

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
        return { ...p, raceId: p.raceId ?? null, model: p.model ?? null, workspaceId: w.id, status: "idle", wantsWorktree: false, resume, unseen: false, devUrl: null, prompt: null };
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
      voiceConfig: ui.voice ?? {},
      recipes: ui.recipes ?? [],
      agentModels: ui.agentModels ?? {},
      defaultProvider: ui.defaultProvider ?? null,
      shortcuts: ui.shortcuts ?? {},
      browserWidth: ui.browserWidth ?? DEFAULT_BROWSER_WIDTH,
      theme: ui.theme ?? "dark",
      view: ui.view ?? "workspace",
      overview: { layout: ui.overviewLayout ?? null, zoomedPaneId: null, presetIndex: GRID },
      reviewConfig: ui.review ?? {},
      progressConfig: ui.progress ?? {},
    });
  },

  loadProviders: async () => {
    const providers = await fetchProviders();
    set({ providers });
    // Warm the model lists in the background; some CLIs take a few seconds to list them.
    for (const p of providers.filter((p) => p.installed && p.modelArg)) void get().loadModels(p.id);
  },

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
    // In a zoomed workspace the focused pane must be the visible one, so the zoom follows.
    set((s) => ({
      focusedPaneId: id,
      overview: s.overview.zoomedPaneId && s.overview.zoomedPaneId !== id ? { ...s.overview, zoomedPaneId: id } : s.overview,
      workspaces: s.workspaces.map((w) =>
        w.zoomedPaneId && w.zoomedPaneId !== id && w.panes.some((p) => p.id === id) ? { ...w, zoomedPaneId: id } : w,
      ),
    }));
    get().updatePane(id, { unseen: false });
  },

  addPane: (provider, opts) => {
    const { activeWorkspaceId, focusedPaneId } = get();
    const workspaceId = opts?.workspaceId ?? activeWorkspaceId;
    const background = workspaceId !== activeWorkspaceId;
    const id = newId("p");
    set((s) => ({
      focusedPaneId: background ? s.focusedPaneId : id,
      workspaces: updateWorkspace(s.workspaces, workspaceId, (w) => ({
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
            devUrl: null,
            raceId: opts?.raceId ?? null,
            prompt: opts?.prompt ?? null,
            model: opts?.model ?? get().agentModels[provider] ?? null,
          },
        ],
        layout: insertDwindle(w.layout, id, opts?.splitFrom ?? (background ? null : focusedPaneId)),
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
  // One modal at a time: they share a layer, so a later one would hide behind an earlier one.
  setCheatSheet: (open) => set(open ? { cheatSheetOpen: true, paletteQuery: null, settingsOpen: false, overlay: null } : { cheatSheetOpen: false }),
  setDiffPane: (id) => set({ diffPaneId: id, paletteQuery: null, cheatSheetOpen: false }),
  setSettings: (open) => set({ settingsOpen: open, paletteQuery: null, cheatSheetOpen: false }),
  setOverlay: (overlay) => set({ overlay, paletteQuery: null, cheatSheetOpen: false }),
  setRecipes: (recipes) => set({ recipes }),
  loadModels: async (provider) => {
    const cached = get().models[provider];
    if (cached) return cached;
    const list = await fetchModels(provider).catch(() => []);
    set((s) => ({ models: { ...s.models, [provider]: list } }));
    return list;
  },
  setAgentModel: (provider, model) =>
    set((s) => {
      const agentModels = { ...s.agentModels };
      if (model) agentModels[provider] = model;
      else delete agentModels[provider];
      return { agentModels };
    }),
  setDefaultProvider: (provider) => set({ defaultProvider: provider }),
  setShortcut: (id, chord) =>
    set((s) => {
      const shortcuts = { ...s.shortcuts };
      if (chord === null) delete shortcuts[id];
      else shortcuts[id] = chord;
      return { shortcuts };
    }),
  resetShortcuts: () => set({ shortcuts: {} }),
  setBrowserWidth: (width) => set({ browserWidth: Math.round(width) }),
  toggleTheme: () => set((s) => ({ theme: s.theme === "dark" ? "light" : "dark" })),
  setView: (view) => set({ view }),
  updateOverview: (fn) => set((s) => ({ overview: { ...s.overview, ...fn(s.overview) } })),
  setReviewConfig: (config) => set((s) => ({ reviewConfig: { ...s.reviewConfig, ...config } })),
  setProgressConfig: (config) => set((s) => ({ progressConfig: { ...s.progressConfig, ...config } })),
  setSearchPane: (id) => set({ searchPaneId: id }),
  setVoiceConfig: (config) => set((s) => ({ voiceConfig: { ...s.voiceConfig, ...config } })),

  toggleSidebar: () => set((s) => ({ sidebarOpen: !s.sidebarOpen })),
  toggleBrowser: () => set((s) => ({ browserOpen: !s.browserOpen })),
}));

export const useActiveWorkspace = () =>
  useWorkspaces((s) => s.workspaces.find((w) => w.id === s.activeWorkspaceId));

export const providerName = (providers: Provider[], id: ProviderId) =>
  providers.find((p) => p.id === id)?.name ?? id;

// ---------------------------------------------------------------------------------------------
// Persistence (SQLite via src-tauri/src/store.rs)

type PanePersisted = Pick<Pane, "id" | "provider" | "title" | "cwd" | "worktree"> & { raceId?: string | null; model?: string | null };

interface Snapshot {
  workspaces: (Omit<Workspace, "panes"> & { panes: PanePersisted[] })[];
  ui: {
    activeWorkspaceId?: string;
    sidebarOpen?: boolean;
    browserOpen?: boolean;
    voice?: { sttModel?: string; clefModel?: string; readback?: boolean; language?: "en" | "de" | "auto"; confirm?: boolean; writer?: "off" | "workers" | "codex"; writerModel?: string; inputDevice?: string };
    recipes?: Recipe[];
    agentModels?: Record<ProviderId, string>;
    defaultProvider?: ProviderId | null;
    shortcuts?: Record<string, string>;
    browserWidth?: number;
    theme?: Theme;
    view?: "workspace" | "overview";
    overviewLayout?: LayoutNode | null;
    review?: { autoTests?: boolean; screenshots?: boolean };
    progress?: { enabled?: boolean; intervalSec?: number };
  } | null;
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
      panes: panes.map(({ id, provider, title, cwd, worktree, raceId, model }) => ({ id, provider, title, cwd, worktree, raceId, model })),
    })),
    ui: { activeWorkspaceId: s.activeWorkspaceId, sidebarOpen: s.sidebarOpen, browserOpen: s.browserOpen, voice: s.voiceConfig, recipes: s.recipes, agentModels: s.agentModels, defaultProvider: s.defaultProvider, shortcuts: s.shortcuts, browserWidth: s.browserWidth, theme: s.theme, view: s.view, overviewLayout: s.overview.layout, review: s.reviewConfig, progress: s.progressConfig },
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

/** The agent new panes open with: the chosen default if it's installed, else Claude Code, else the shell. */
export function defaultProviderId(s: Pick<WorkspaceState, "providers" | "defaultProvider"> = useWorkspaces.getState()): ProviderId {
  const installed = (id: ProviderId | null) => id !== null && s.providers.some((p) => p.id === id && p.installed);
  if (installed(s.defaultProvider)) return s.defaultProvider!;
  return installed("claude") ? "claude" : "shell";
}

/**
 * Keeps the overview's tiles in step with the sessions: closed ones leave (other tiles keep
 * their sizes); when sessions are added, the overview re-tiles with its current preset.
 */
useWorkspaces.subscribe((s, prev) => {
  if (s.workspaces === prev.workspaces && s.overview === prev.overview) return;
  const ids = overviewOrder(s.workspaces);
  const current = s.overview.layout ? leaves(s.overview.layout) : [];
  if (ids.length === current.length && ids.every((id) => current.includes(id))) return;
  const added = ids.some((id) => !current.includes(id));
  const layout = added
    ? buildPreset(PRESETS[s.overview.presetIndex], ids)
    : current.filter((id) => !ids.includes(id)).reduce<LayoutNode | null>((tree, id) => removeLeaf(tree, id), s.overview.layout);
  const zoomedPaneId = s.overview.zoomedPaneId && ids.includes(s.overview.zoomedPaneId) ? s.overview.zoomedPaneId : null;
  useWorkspaces.setState({ overview: { ...s.overview, layout, zoomedPaneId } });
});
