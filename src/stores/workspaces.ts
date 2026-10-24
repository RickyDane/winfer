import { invoke } from "@tauri-apps/api/core";
import { create } from "zustand";
import { useShallow } from "zustand/react/shallow";
import { fetchModels, fetchProviders, type ModelInfo, type Provider, type ProviderId } from "@/features/providers/providers";
import { buildPreset, insertDwindle, leaves, PRESETS, removeLeaf, type LayoutNode } from "@/features/layout/tree";
import type { Recipe } from "@/features/recipes/recipes";
import { getHome } from "@/lib/home";
import { DEFAULT_THEME, isThemeId, themeById } from "@/features/theme/themes";

/** Main area: workspace tiles, the overview, the archive, or a plugin page (`page:<plugin>/<page>`). */
export type View = "workspace" | "overview" | "archive" | `page:${string}`;

export type AgentStatus = "idle" | "working" | "needs-input" | "done" | "error";

export interface WorktreeInfo {
  repo: string;
  path: string;
  branch: string;
  base: string;
}

export type SidePanelTab = "browser" | "device" | "terminal" | "viewer";

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
  /** The agent's own session id (reported by its hooks), resumed after a restart. */
  sessionId: string | null;
  /** Status changed while the user wasn't looking at this pane. */
  unseen: boolean;
  /** Latest local dev-server URL printed in this pane (e.g. http://localhost:5173/). */
  devUrl: string | null;
  /** Panes started together by race mode share an id. */
  raceId: string | null;
  /** A/B test baseline: runs without winfer's tools, the browser and the user's MCP servers and skills. */
  plain: boolean;
  /** Initial prompt given when the agent was started (not persisted). */
  prompt: string | null;
  /** Model the agent runs with; null means the CLI's default. */
  model: string | null;
  /** Title the agent gave its terminal (Claude Code: the session's topic), cleaned up. */
  agentTitle: string | null;
  /** Name the user gave the session; replaces the agent's title wherever the pane is shown. */
  customTitle: string | null;
  /** Moved to the archive: still running, but out of the workspace's tiles until restored. */
  archived: boolean;
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
  /** Workspaces whose sessions are listed in the sidebar (persisted). */
  expandedWorkspaces: string[];
  browserOpen: boolean;
  paletteQuery: string | null;
  cheatSheetOpen: boolean;
  /** Pane whose worktree changes are shown in the diff view. */
  diffPaneId: string | null;
  settingsOpen: boolean;
  /** Settings page shown; kept after closing so reopening lands where you left off. */
  settingsPage: string;
  /** Open overlay for agent orchestration and recipes. */
  overlay: "race" | "abtest" | "broadcast" | "recipes" | "model" | "review" | "progress" | "history" | "branch" | "newPane" | null;
  /** Agent the new pane dialog starts; null for the default agent. */
  newPaneProvider: ProviderId | null;
  /** User-saved recipes (persisted). */
  recipes: Recipe[];
  /** Models each provider offers (from its CLI), loaded in the background. */
  models: Record<ProviderId, ModelInfo[]>;
  /** Default model per provider for new panes (persisted). */
  agentModels: Record<ProviderId, string>;
  /** Agents started with their permission prompts skipped (persisted). */
  autoApprove: ProviderId[];
  /** Codex CLI panes start lean and with an output cap (persisted, default off; winfer code's Codex has its own setting). */
  leanCodex: boolean;
  /** Added to every agent's system prompt at start (persisted); null = DEFAULT_SYSTEM_PROMPT, "" = none. */
  systemPrompt: string | null;
  /** OPTIONAL_TOOLS the user turned back on (persisted); the others are left out of agents' tool lists. */
  enabledTools: string[];
  /** Agent for new panes (⌘T, splits, "open a window"); null = Claude Code if installed. */
  defaultProvider: ProviderId | null;
  /** Code editor "Open in editor" uses (see features/editors); null = the first one installed. */
  editor: string | null;
  /** Shortcut overrides by binding id (see src/commands/keymap.ts); "" = switched off. */
  shortcuts: Record<string, string>;
  /** Browser panel width in px. */
  browserWidth: number;
  /** Left sidebar width in px. */
  sidebarWidth: number;
  /** What the side panel shows: the browser, a mirrored Android device, a terminal or the file viewer. */
  sidePanel: SidePanelTab;
  /** Files open in the side panel's viewer (absolute paths) and the one shown. */
  viewer: { docs: string[]; active: string | null };
  /** Device the Android panel mirrors (adb serial). */
  androidSerial: string | null;
  /** Color theme id (see features/theme/themes.ts). */
  theme: Theme;
  /** Main area: the active workspace's tiles, or the overview of every workspace's sessions. */
  view: View;
  /** Terminal font size in px (zoom with Mod+= / Mod+- / Mod+0 or Ctrl+wheel). */
  terminalFontSize: number;
  /** Plugin page keys shown as title bar tabs, in order; the rest live in the Apps menu. null = not chosen yet. */
  pinnedPages: string[] | null;
  /** The overview tiles every session of every workspace; same shape as a workspace's layout. */
  overview: OverviewLayout;
  /** Review inbox: run tests / take screenshots automatically when an agent finishes. */
  reviewConfig: { autoTests?: boolean; screenshots?: boolean };
  /** LLM progress estimates for working agents. */
  progressConfig: { enabled?: boolean; intervalSec?: number };
  /** Show what each worktree pane's task has cost so far in its header (opt-in). */
  showPaneCost: boolean;
  /** Opt-in: the floating glass pane header design. */
  glassPanes: boolean;
  /** Pinned plugin page tabs show only their icon; the title is in the tooltip. */
  pageTabIconsOnly: boolean;
  /** Pane whose scrollback search bar is open (⌘F). */
  searchPaneId: string | null;
  /** Last session id per agent and folder (`provider:path`), so a reopened worktree resumes it (persisted). */
  recentSessions: Record<string, string>;
  /** Each pane's task and its short title (see progress.ts), kept so session titles survive restarts. */
  paneTasks: Record<string, PaneTask>;
  /** The LLM's title per agent session id (persisted), so a resumed session keeps its title. */
  sessionTitles: Record<string, string>;
  /** Voice model choices, persisted and pushed to the backend on launch. */
  voiceConfig: { readback?: boolean; writer?: "off" | "chatgpt" | "codex" | "claude"; writerModel?: string; inputDevice?: string; liveModel?: string; liveVoice?: string; livePrompt?: string; liveAuth?: "auto" | "apikey" | "chatgpt"; liveChatgptVoice?: string };
  hydrate: () => Promise<void>;
  loadProviders: () => Promise<void>;
  setActiveWorkspace: (id: string) => void;
  createWorkspace: (name: string, rootDir: string) => string;
  removeWorkspace: (id: string) => void;
  toggleWorkspaceExpanded: (id: string) => void;
  collapseAllWorkspaces: () => void;
  /** Moves a workspace to `index` in the sidebar order. */
  moveWorkspace: (id: string, index: number) => void;
  /** Moves a pane to `index` within its workspace's sidebar list. */
  movePaneInList: (id: string, index: number) => void;
  focusPane: (id: string) => void;
  addPane: (
    provider: ProviderId,
    opts?: { splitFrom?: string; worktree?: boolean; prompt?: string; raceId?: string; plain?: boolean; customTitle?: string; workspaceId?: string; model?: string; existingWorktree?: WorktreeInfo; cwd?: string; resumeSession?: string },
  ) => string;
  removePane: (id: string) => void;
  /** Moves a pane out of its workspace's tiles into the archive; its process keeps running. */
  archivePane: (id: string) => void;
  /** Puts an archived pane back into its workspace's tiles. */
  unarchivePane: (id: string) => void;
  updatePane: (id: string, patch: Partial<Pane>) => void;
  /** Names a session; an empty name goes back to the agent's own title. */
  renamePane: (id: string, name: string) => void;
  setPaneStatus: (id: string, status: AgentStatus) => void;
  /** Records the agent session a pane runs, reported by the agent's hooks. */
  setPaneSession: (id: string, sessionId: string) => void;
  /** Applies a change to the active workspace. */
  updateActive: (fn: (w: Workspace) => Partial<Workspace>) => void;
  openPalette: (query?: string) => void;
  closePalette: () => void;
  setCheatSheet: (open: boolean) => void;
  setDiffPane: (id: string | null) => void;
  /** Opens or closes settings, optionally on a given page. */
  setSettings: (open: boolean, page?: string) => void;
  setOverlay: (overlay: "race" | "abtest" | "broadcast" | "recipes" | "model" | "review" | "progress" | "history" | "branch" | "newPane" | null, newPaneProvider?: ProviderId | null) => void;
  setRecipes: (recipes: Recipe[]) => void;
  loadModels: (provider: ProviderId) => Promise<ModelInfo[]>;
  setAgentModel: (provider: ProviderId, model: string | null) => void;
  setAutoApprove: (provider: ProviderId, on: boolean) => void;
  setLeanCodex: (on: boolean) => void;
  setToolEnabled: (name: string, on: boolean) => void;
  setSystemPrompt: (prompt: string | null) => void;
  setDefaultProvider: (provider: ProviderId | null) => void;
  setEditor: (editor: string | null) => void;
  /** Sets a binding's chord; null restores the default. */
  setShortcut: (id: string, chord: string | null) => void;
  resetShortcuts: () => void;
  setBrowserWidth: (width: number) => void;
  setSidebarWidth: (width: number) => void;
  setSidePanel: (panel: SidePanelTab) => void;
  setViewer: (viewer: { docs: string[]; active: string | null }) => void;
  setAndroidSerial: (serial: string | null) => void;
  setTheme: (theme: Theme) => void;
  /** Switch to the theme's light/dark counterpart. */
  toggleTheme: () => void;
  setView: (view: View) => void;
  setTerminalFontSize: (size: number) => void;
  setPinnedPages: (keys: string[]) => void;
  updateOverview: (fn: (o: OverviewLayout) => Partial<OverviewLayout>) => void;
  setReviewConfig: (config: { autoTests?: boolean; screenshots?: boolean }) => void;
  setProgressConfig: (config: { enabled?: boolean; intervalSec?: number }) => void;
  setShowPaneCost: (show: boolean) => void;
  setGlassPanes: (on: boolean) => void;
  setPageTabIconsOnly: (iconsOnly: boolean) => void;
  setSearchPane: (id: string | null) => void;
  setVoiceConfig: (config: { readback?: boolean; writer?: "off" | "chatgpt" | "codex" | "claude"; writerModel?: string; inputDevice?: string; liveModel?: string; liveVoice?: string; livePrompt?: string; liveAuth?: "auto" | "apikey" | "chatgpt"; liveChatgptVoice?: string }) => void;
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

/** `items` with the one whose id is `id` moved to `index`. */
function moveItem<T extends { id: string }>(items: T[], id: string, index: number): T[] {
  const from = items.findIndex((x) => x.id === id);
  if (from < 0 || from === index) return items;
  const next = items.filter((x) => x.id !== id);
  next.splice(Math.max(0, Math.min(index, next.length)), 0, items[from]);
  return next;
}

const firstPane = (ws?: Workspace) => (ws?.layout ? leaves(ws.layout)[0] : null);

export const DEFAULT_BROWSER_WIDTH = 420;
export const DEFAULT_SIDEBAR_WIDTH = 256;
export const DEFAULT_TERMINAL_FONT_SIZE = 12.5;
export const DEFAULT_SYSTEM_PROMPT =
  "Use your tools (MCP, browser, skills, subagents, shell) wherever they're faster or more reliable than by hand, and to verify your work. Prefer winfer's integrated tools over external ones. Open and test web pages only in winfer's browser (its browser tools or open_url), never via `open`/`xdg-open`/`start` or other browser automation. Keep tool output small, it fills your context: search with line numbers before reading, read only the lines you need (your file-read tool with an offset and limit rather than printing whole files with cat or sed), don't re-read a file you haven't changed, and trim long command output to the part you need. Change existing files with your edit tool (only the changed lines), not by rewriting whole files with Write or shell heredocs. Run dev servers, watchers, loops and other long-running commands with winfer's bg tool, not by backgrounding them in your shell (&, nohup, run_in_background, a kept-open terminal session): wait on them with bg read until and stop them when done.";

/**
 * MCP tools agents rarely use, off by default: each tool an agent is offered costs context in
 * every request. Settings → Agent tools turns them back on one by one.
 */
export const OPTIONAL_TOOLS: { name: string; server: string; hint: string; /** Plugin that provides the tool; shown only while it's installed and enabled. */ plugin?: string }[] = [
  { name: "browser_drag", server: "Browser", hint: "Drag and drop between two elements." },
  { name: "browser_drop", server: "Browser", hint: "Drop files or data onto an element from outside the page." },
  { name: "browser_emulate_media", server: "Browser", hint: "Emulate dark/light scheme, reduced motion or print media." },
  { name: "browser_navigate_back", server: "Browser", hint: "Go back in the page history (agents can also navigate to the URL)." },
  { name: "kb_explain", server: "Knowledge Base", plugin: "knowledge-base", hint: "Explain one node of the knowledge graph and its connections." },
  { name: "kb_save", server: "Knowledge Base", plugin: "knowledge-base", hint: "Save an answer to the knowledge graph's memory." },
];

/** The MCP tools agents start without: the optional ones the user hasn't turned on. */
export const hiddenToolsOf = (s: { enabledTools: string[] }) => OPTIONAL_TOOLS.map((t) => t.name).filter((n) => !s.enabledTools.includes(n));

/** The system prompt agents start with: the user's, else the default. */
export const systemPromptOf = (s: { systemPrompt: string | null }) => s.systemPrompt ?? DEFAULT_SYSTEM_PROMPT;
export const MIN_TERMINAL_FONT_SIZE = 8;
export const MAX_TERMINAL_FONT_SIZE = 28;

export interface OverviewLayout {
  layout: LayoutNode | null;
  zoomedPaneId: string | null;
  presetIndex: number;
}

const GRID = PRESETS.indexOf("grid");

/** Every session, grouped by workspace, each workspace in its own tile order. */
export function overviewOrder(workspaces: Workspace[]): string[] {
  return workspaces.flatMap((w) => (w.layout ? leaves(w.layout) : w.panes.filter((p) => !p.archived).map((p) => p.id)));
}

/** Every archived pane, grouped by workspace in sidebar order. */
export const archivedPanes = (workspaces: Workspace[]): Pane[] => workspaces.flatMap((w) => w.panes.filter((p) => p.archived));

/** The panes shown in a workspace's tiles (all but the archived ones). */
export const livePanes = (w: Workspace): Pane[] => w.panes.filter((p) => !p.archived);

export type Theme = string;

export interface PaneTask {
  task: string | null;
  title: string | null;
}

export const useWorkspaces = create<WorkspaceState>((set, get) => ({
  hydrated: false,
  providers: [],
  workspaces: [],
  activeWorkspaceId: "",
  focusedPaneId: null,
  sidebarOpen: true,
  expandedWorkspaces: [],
  browserOpen: true,
  paletteQuery: null,
  cheatSheetOpen: false,
  diffPaneId: null,
  settingsOpen: false,
  settingsPage: "general",
  overlay: null,
  newPaneProvider: null,
  recipes: [],
  models: {},
  agentModels: {},
  autoApprove: [],
  leanCodex: false,
  systemPrompt: null,
  enabledTools: [],
  defaultProvider: null,
  editor: null,
  shortcuts: {},
  browserWidth: DEFAULT_BROWSER_WIDTH,
  sidebarWidth: DEFAULT_SIDEBAR_WIDTH,
  sidePanel: "browser",
  viewer: { docs: [], active: null },
  androidSerial: null,
  theme: DEFAULT_THEME,
  view: "workspace",
  terminalFontSize: DEFAULT_TERMINAL_FONT_SIZE,
  pinnedPages: null,
  overview: { layout: null, zoomedPaneId: null, presetIndex: GRID },
  reviewConfig: {},
  progressConfig: {},
  showPaneCost: false,
  glassPanes: false,
  pageTabIconsOnly: false,
  searchPaneId: null,
  recentSessions: {},
  paneTasks: {},
  sessionTitles: {},
  voiceConfig: {},

  hydrate: async () => {
    hydration ??= loadSnapshot();
    const snapshot = await hydration;
    if (get().hydrated) return;
    if (snapshot.workspaces.length === 0) {
      const home = newWorkspace("home", "~");
      set({ hydrated: true, workspaces: [home], activeWorkspaceId: home.id, expandedWorkspaces: [home.id] });
      return;
    }

    // Panes resume their own session by id. Without one (the agent never reported it), only one
    // restored pane per provider + directory resumes the folder's last session; others sharing
    // that directory would all resume the same conversation.
    const resumed = new Set<string>();
    const workspaces: Workspace[] = snapshot.workspaces.map((w) => ({
      ...w,
      panes: w.panes.map((p) => {
        const key = `${p.provider}:${p.cwd}`;
        const resume = !!p.sessionId || !resumed.has(key);
        if (!p.sessionId) resumed.add(key);
        return { ...p, sessionId: p.sessionId ?? null, raceId: p.raceId ?? null, plain: p.plain ?? false, model: p.model ?? null, agentTitle: p.agentTitle ?? null, customTitle: p.customTitle ?? null, archived: p.archived ?? false, workspaceId: w.id, status: "idle", wantsWorktree: false, resume, unseen: false, devUrl: null, prompt: null };
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
      expandedWorkspaces: ui.expandedWorkspaces?.filter((id) => workspaces.some((w) => w.id === id)) ?? [active.id],
      browserOpen: ui.browserOpen ?? true,
      voiceConfig: ui.voice ?? {},
      recipes: ui.recipes ?? [],
      agentModels: ui.agentModels ?? {},
      autoApprove: ui.autoApprove ?? [],
      leanCodex: ui.leanCodex ?? false,
      systemPrompt: ui.systemPrompt ?? null,
      enabledTools: ui.enabledTools ?? [],
      defaultProvider: ui.defaultProvider ?? null,
      editor: ui.editor ?? null,
      shortcuts: ui.shortcuts ?? {},
      browserWidth: ui.browserWidth ?? DEFAULT_BROWSER_WIDTH,
      sidebarWidth: ui.sidebarWidth ?? DEFAULT_SIDEBAR_WIDTH,
      sidePanel: ui.sidePanel ?? "browser",
      viewer: { docs: ui.viewer?.docs ?? [], active: ui.viewer?.active ?? null },
      androidSerial: ui.androidSerial ?? null,
      theme: isThemeId(ui.theme) ? ui.theme : DEFAULT_THEME,
      view: ui.view ?? "workspace",
      terminalFontSize: ui.terminalFontSize ?? DEFAULT_TERMINAL_FONT_SIZE,
      pinnedPages: ui.pinnedPages ?? null,
      overview: { layout: ui.overviewLayout ?? null, zoomedPaneId: null, presetIndex: GRID },
      reviewConfig: ui.review ?? {},
      progressConfig: ui.progress ?? {},
      showPaneCost: ui.showPaneCost ?? false,
      glassPanes: ui.glassPanes ?? false,
      pageTabIconsOnly: ui.pageTabIconsOnly ?? false,
      recentSessions: ui.recentSessions ?? {},
      paneTasks: ui.paneTasks ?? {},
      sessionTitles: ui.sessionTitles ?? {},
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
    // The active workspace's sessions are always listed.
    set((s) => ({ activeWorkspaceId: id, focusedPaneId: firstPane(ws), expandedWorkspaces: s.expandedWorkspaces.includes(id) ? s.expandedWorkspaces : [...s.expandedWorkspaces, id] }));
  },

  createWorkspace: (name, rootDir) => {
    const ws = newWorkspace(name, rootDir);
    set((s) => ({ workspaces: [...s.workspaces, ws], activeWorkspaceId: ws.id, focusedPaneId: null, expandedWorkspaces: [...s.expandedWorkspaces, ws.id] }));
    return ws.id;
  },

  removeWorkspace: (id) =>
    set((s) => {
      const workspaces = s.workspaces.filter((w) => w.id !== id);
      if (workspaces.length === 0) workspaces.push(newWorkspace("home", "~"));
      if (s.activeWorkspaceId !== id) return { workspaces };
      return { workspaces, activeWorkspaceId: workspaces[0].id, focusedPaneId: firstPane(workspaces[0]) };
    }),

  toggleWorkspaceExpanded: (id) =>
    set((s) => ({
      expandedWorkspaces: s.expandedWorkspaces.includes(id) ? s.expandedWorkspaces.filter((w) => w !== id) : [...s.expandedWorkspaces, id],
    })),

  collapseAllWorkspaces: () => set({ expandedWorkspaces: [] }),

  moveWorkspace: (id, index) => set((s) => ({ workspaces: moveItem(s.workspaces, id, index) })),

  // `index` counts the sidebar's list, which shows archived panes apart, after the others.
  movePaneInList: (id, index) =>
    set((s) => ({
      workspaces: s.workspaces.map((w) => {
        const pane = w.panes.find((p) => p.id === id);
        if (!pane) return w;
        const group = w.panes.filter((p) => p.archived === pane.archived);
        const rest = w.panes.filter((p) => p.archived !== pane.archived);
        const moved = moveItem(group, id, index);
        return { ...w, panes: pane.archived ? [...rest, ...moved] : [...moved, ...rest] };
      }),
    })),

  focusPane: (id) => {
    // In a zoomed workspace the focused pane must be the visible one, so the zoom follows.
    const rezoom = (w: Workspace) => !!w.zoomedPaneId && w.zoomedPaneId !== id && w.panes.some((p) => p.id === id);
    // Every click in a pane lands here: unchanged state stays the same object, so nothing re-renders.
    set((s) => {
      const overview = s.overview.zoomedPaneId && s.overview.zoomedPaneId !== id ? { ...s.overview, zoomedPaneId: id } : s.overview;
      const workspaces = s.workspaces.some(rezoom) ? s.workspaces.map((w) => (rezoom(w) ? { ...w, zoomedPaneId: id } : w)) : s.workspaces;
      if (s.focusedPaneId === id && overview === s.overview && workspaces === s.workspaces) return s;
      return { focusedPaneId: id, overview, workspaces };
    });
    get().updatePane(id, { unseen: false });
  },

  addPane: (provider, opts) => {
    const { activeWorkspaceId, focusedPaneId } = get();
    const workspaceId = opts?.workspaceId ?? activeWorkspaceId;
    const background = workspaceId !== activeWorkspaceId;
    const id = newId("p");
    set((s) => ({
      focusedPaneId: background ? s.focusedPaneId : id,
      // A new pane opens in the tiles, so the archive makes way.
      view: !background && s.view === "archive" ? "workspace" : s.view,
      workspaces: updateWorkspace(s.workspaces, workspaceId, (w) => ({
        ...w,
        panes: [
          ...w.panes,
          {
            id,
            workspaceId: w.id,
            provider,
            title: w.name,
            cwd: opts?.existingWorktree?.path ?? opts?.cwd ?? w.rootDir,
            status: "idle",
            worktree: opts?.existingWorktree ?? null,
            wantsWorktree: !opts?.existingWorktree && provider !== "shell" && (opts?.worktree ?? w.useWorktrees),
            resume: !!opts?.resumeSession,
            sessionId: opts?.resumeSession ?? null,
            unseen: false,
            devUrl: null,
            agentTitle: null,
            customTitle: opts?.customTitle ?? null,
            raceId: opts?.raceId ?? null,
            plain: opts?.plain ?? false,
            prompt: opts?.prompt ?? null,
            model: opts?.model ?? get().agentModels[provider] ?? null,
            archived: false,
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

  archivePane: (id) =>
    set((s) => {
      const workspaces = s.workspaces.map((w) =>
        w.panes.some((p) => p.id === id && !p.archived)
          ? {
              ...w,
              panes: w.panes.map((p) => (p.id === id ? { ...p, archived: true } : p)),
              layout: removeLeaf(w.layout, id),
              zoomedPaneId: w.zoomedPaneId === id ? null : w.zoomedPaneId,
            }
          : w,
      );
      // In the archive the pane stays in view (and focused); elsewhere focus moves to a visible one.
      if (s.view === "archive" || s.focusedPaneId !== id) return { workspaces };
      const active = workspaces.find((w) => w.id === s.activeWorkspaceId);
      return { workspaces, focusedPaneId: active?.layout ? leaves(active.layout).at(-1)! : null };
    }),

  unarchivePane: (id) =>
    set((s) => ({
      workspaces: s.workspaces.map((w) =>
        w.panes.some((p) => p.id === id && p.archived)
          ? {
              ...w,
              panes: w.panes.map((p) => (p.id === id ? { ...p, archived: false } : p)),
              layout: insertDwindle(w.layout, id, w.id === s.activeWorkspaceId && s.view === "workspace" ? s.focusedPaneId : null),
              zoomedPaneId: null,
            }
          : w,
      ),
    })),

  renamePane: (id, name) => get().updatePane(id, { customTitle: name.replace(/\s+/g, " ").trim().slice(0, 80) || null }),

  updatePane: (id, patch) =>
    set((s) => {
      // A patch that changes nothing (or a pane that's gone) leaves the state as it is.
      const pane = s.workspaces.find((w) => w.panes.some((p) => p.id === id))?.panes.find((p) => p.id === id);
      if (!pane || (Object.keys(patch) as (keyof Pane)[]).every((k) => Object.is(pane[k], patch[k]))) return s;
      return {
        workspaces: s.workspaces.map((w) =>
          w.panes.some((p) => p.id === id) ? { ...w, panes: w.panes.map((p) => (p.id === id ? { ...p, ...patch } : p)) } : w,
        ),
      };
    }),

  setPaneStatus: (id, status) => {
    const s = get();
    const visible = s.focusedPaneId === id && document.hasFocus();
    s.updatePane(id, { status, unseen: !visible && (status === "needs-input" || status === "done" || status === "error") });
  },

  setPaneSession: (id, sessionId) => {
    const pane = get().workspaces.flatMap((w) => w.panes).find((p) => p.id === id);
    if (!pane || pane.provider === "shell") return;
    const key = sessionKey(pane.provider, pane.worktree?.path ?? pane.cwd);
    if (pane.sessionId === sessionId && get().recentSessions[key] === sessionId) return;
    get().updatePane(id, { sessionId });
    set((s) => {
      // Newest last; only the most recent folders are kept.
      const { [key]: _, ...rest } = s.recentSessions;
      const entries = [...Object.entries(rest), [key, sessionId]].slice(-MAX_RECENT_SESSIONS);
      return { recentSessions: Object.fromEntries(entries) };
    });
  },

  updateActive: (fn) =>
    set((s) => ({ workspaces: updateWorkspace(s.workspaces, s.activeWorkspaceId, (w) => ({ ...w, ...fn(w) })) })),

  openPalette: (query = "") => set({ paletteQuery: query, cheatSheetOpen: false }),
  closePalette: () => set({ paletteQuery: null }),
  // One modal at a time: they share a layer, so a later one would hide behind an earlier one.
  setCheatSheet: (open) => set(open ? { cheatSheetOpen: true, paletteQuery: null, settingsOpen: false, overlay: null } : { cheatSheetOpen: false }),
  setDiffPane: (id) => set({ diffPaneId: id, paletteQuery: null, cheatSheetOpen: false }),
  setSettings: (open, page) => set({ settingsOpen: open, paletteQuery: null, cheatSheetOpen: false, ...(page ? { settingsPage: page } : {}) }),
  setOverlay: (overlay, newPaneProvider = null) => set({ overlay, newPaneProvider, paletteQuery: null, cheatSheetOpen: false }),
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
  setAutoApprove: (provider, on) => set((s) => ({ autoApprove: [...s.autoApprove.filter((p) => p !== provider), ...(on ? [provider] : [])] })),
  setLeanCodex: (leanCodex) => set({ leanCodex }),
  setToolEnabled: (name, on) => set((s) => ({ enabledTools: on ? [...new Set([...s.enabledTools, name])] : s.enabledTools.filter((t) => t !== name) })),
  setSystemPrompt: (systemPrompt) => set({ systemPrompt }),
  setDefaultProvider: (provider) => set({ defaultProvider: provider }),
  setEditor: (editor) => set({ editor }),
  setShortcut: (id, chord) =>
    set((s) => {
      const shortcuts = { ...s.shortcuts };
      if (chord === null) delete shortcuts[id];
      else shortcuts[id] = chord;
      return { shortcuts };
    }),
  resetShortcuts: () => set({ shortcuts: {} }),
  setBrowserWidth: (width) => set({ browserWidth: Math.round(width) }),
  setSidebarWidth: (width) => set({ sidebarWidth: Math.round(width) }),
  setSidePanel: (panel) => set({ sidePanel: panel, browserOpen: true }),
  setViewer: (viewer) => set({ viewer }),
  setAndroidSerial: (serial) => set({ androidSerial: serial }),
  setTheme: (theme) => set({ theme: isThemeId(theme) ? theme : DEFAULT_THEME }),
  toggleTheme: () => set((s) => ({ theme: themeById(s.theme).pair })),
  setView: (view) =>
    set((s) => {
      // Leaving the archive: focus a pane that's on screen again.
      const focused = s.workspaces.flatMap((w) => w.panes).find((p) => p.id === s.focusedPaneId);
      if (view === "archive" || !focused?.archived) return { view };
      return { view, focusedPaneId: firstPane(s.workspaces.find((w) => w.id === s.activeWorkspaceId)) };
    }),
  setTerminalFontSize: (size) =>
    set({ terminalFontSize: Math.round(Math.min(MAX_TERMINAL_FONT_SIZE, Math.max(MIN_TERMINAL_FONT_SIZE, size)) * 2) / 2 }),
  setPinnedPages: (pinnedPages) => set({ pinnedPages: [...new Set(pinnedPages)] }),
  updateOverview: (fn) => set((s) => ({ overview: { ...s.overview, ...fn(s.overview) } })),
  setReviewConfig: (config) => set((s) => ({ reviewConfig: { ...s.reviewConfig, ...config } })),
  setProgressConfig: (config) => set((s) => ({ progressConfig: { ...s.progressConfig, ...config } })),
  setShowPaneCost: (showPaneCost) => set({ showPaneCost }),
  setGlassPanes: (glassPanes) => set({ glassPanes }),
  setPageTabIconsOnly: (pageTabIconsOnly) => set({ pageTabIconsOnly }),
  setSearchPane: (id) => set({ searchPaneId: id }),
  setVoiceConfig: (config) => set((s) => ({ voiceConfig: { ...s.voiceConfig, ...config } })),

  toggleSidebar: () => set((s) => ({ sidebarOpen: !s.sidebarOpen })),
  toggleBrowser: () => set((s) => ({ browserOpen: !s.browserOpen })),
}));

/**
 * Some of the store's fields, re-rendering only when one of them changes. Destructuring the
 * whole store (`useWorkspaces()`) re-renders on every change, down to each click in a pane.
 */
export function useWorkspacesPick<K extends keyof WorkspaceState>(...keys: K[]): Pick<WorkspaceState, K> {
  return useWorkspaces(
    useShallow((s) => {
      const picked = {} as Pick<WorkspaceState, K>;
      for (const k of keys) picked[k] = s[k];
      return picked;
    }),
  );
}

const MAX_RECENT_SESSIONS = 200;

/** Key of `recentSessions`: the agent and the folder it ran in ("~" expanded, so both spellings match). */
export function sessionKey(provider: ProviderId, path: string): string {
  const home = getHome();
  const full = home && (path === "~" || path.startsWith("~/")) ? home + path.slice(1) : path;
  return `${provider}:${full.replace(/(.)\/+$/, "$1")}`;
}

/** The agent session last run in `path` that no open pane is running, if any. */
export function recentSession(provider: ProviderId, path: string, s: Pick<WorkspaceState, "recentSessions" | "workspaces"> = useWorkspaces.getState()): string | null {
  const id = s.recentSessions[sessionKey(provider, path)];
  if (!id || s.workspaces.some((w) => w.panes.some((p) => p.sessionId === id))) return null;
  return id;
}

export const useActiveWorkspace = () =>
  useWorkspaces((s) => s.workspaces.find((w) => w.id === s.activeWorkspaceId));

export const providerName = (providers: Provider[], id: ProviderId) =>
  providers.find((p) => p.id === id)?.name ?? id;

// ---------------------------------------------------------------------------------------------
// Persistence (SQLite via src-tauri/src/store.rs)

type PanePersisted = Pick<Pane, "id" | "provider" | "title" | "cwd" | "worktree"> & { archived?: boolean; raceId?: string | null; plain?: boolean; model?: string | null; agentTitle?: string | null; customTitle?: string | null; sessionId?: string | null };

interface Snapshot {
  workspaces: (Omit<Workspace, "panes"> & { panes: PanePersisted[] })[];
  ui: {
    activeWorkspaceId?: string;
    sidebarOpen?: boolean;
    expandedWorkspaces?: string[];
    browserOpen?: boolean;
    voice?: { readback?: boolean; writer?: "off" | "chatgpt" | "codex" | "claude"; writerModel?: string; inputDevice?: string; liveModel?: string; liveVoice?: string; livePrompt?: string; liveAuth?: "auto" | "apikey" | "chatgpt"; liveChatgptVoice?: string };
    recipes?: Recipe[];
    agentModels?: Record<ProviderId, string>;
    autoApprove?: ProviderId[];
    leanCodex?: boolean;
    systemPrompt?: string | null;
    enabledTools?: string[];
    defaultProvider?: ProviderId | null;
    editor?: string | null;
    shortcuts?: Record<string, string>;
    browserWidth?: number;
    sidebarWidth?: number;
    sidePanel?: SidePanelTab;
    viewer?: { docs?: string[]; active?: string | null };
    androidSerial?: string | null;
    theme?: Theme;
    view?: View;
    terminalFontSize?: number;
    pinnedPages?: string[] | null;
    overviewLayout?: LayoutNode | null;
    review?: { autoTests?: boolean; screenshots?: boolean };
    progress?: { enabled?: boolean; intervalSec?: number };
    showPaneCost?: boolean;
    glassPanes?: boolean;
    pageTabIconsOnly?: boolean;
    recentSessions?: Record<string, string>;
    paneTasks?: Record<string, PaneTask>;
    sessionTitles?: Record<string, string>;
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

/** Saved tasks of the panes still open (closed panes' entries are dropped). */
function openPaneTasks(s: WorkspaceState): Record<string, PaneTask> {
  const open = new Set(s.workspaces.flatMap((w) => w.panes.map((p) => p.id)));
  return Object.fromEntries(Object.entries(s.paneTasks).filter(([id]) => open.has(id)));
}

function toSnapshot(s: WorkspaceState): Snapshot {
  return {
    workspaces: s.workspaces.map(({ panes, ...w }) => ({
      ...w,
      panes: panes.map(({ id, provider, title, cwd, worktree, raceId, plain, model, agentTitle, customTitle, sessionId, archived }) => ({ id, provider, title, cwd, worktree, raceId, plain, model, agentTitle, customTitle, sessionId, archived })),
    })),
    ui: { activeWorkspaceId: s.activeWorkspaceId, sidebarOpen: s.sidebarOpen, expandedWorkspaces: s.expandedWorkspaces, browserOpen: s.browserOpen, voice: s.voiceConfig, recipes: s.recipes, agentModels: s.agentModels, autoApprove: s.autoApprove, leanCodex: s.leanCodex, systemPrompt: s.systemPrompt, enabledTools: s.enabledTools, defaultProvider: s.defaultProvider, editor: s.editor, shortcuts: s.shortcuts, browserWidth: s.browserWidth, sidebarWidth: s.sidebarWidth, sidePanel: s.sidePanel, viewer: s.viewer, androidSerial: s.androidSerial, theme: s.theme, view: s.view, terminalFontSize: s.terminalFontSize, pinnedPages: s.pinnedPages, overviewLayout: s.overview.layout, review: s.reviewConfig, progress: s.progressConfig, showPaneCost: s.showPaneCost, glassPanes: s.glassPanes, pageTabIconsOnly: s.pageTabIconsOnly, recentSessions: s.recentSessions, paneTasks: openPaneTasks(s), sessionTitles: s.sessionTitles },
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

/**
 * Cleans a terminal title set by an agent: drops spinner/status glyphs ("✳ ", "⠂ ") and titles
 * that only repeat the CLI's name. Null when nothing useful is left.
 */
export function cleanAgentTitle(raw: string, providerNames: string[]): string | null {
  const title = raw.replace(/^[^\p{L}\p{N}]+/u, "").replace(/\s+/g, " ").trim().slice(0, 80);
  if (!title) return null;
  const lower = title.toLowerCase();
  if (providerNames.some((n) => n.toLowerCase() === lower)) return null;
  // A console's default title is the program's path (Windows: "C:\WINDOWS\system32\cmd.exe",
  // maybe after "Administrator: "): not a topic.
  if (/^(?:[^:]*:\s*)?(?:[a-z]:)?[\\/][^\s]*$|\.exe$/i.test(raw.trim())) return null;
  return title;
}

/** The pane's title: the user's name for it, else the agent's own title. */
export const paneTitle = (pane: Pane): string | null => pane.customTitle ?? pane.agentTitle;

/** Sessions whose titles are kept; older ones are dropped. */
export const MAX_SESSION_TITLES = 500;

/**
 * What to call a pane in lists: the user's name for it, else the LLM's session title (the same
 * for every provider), else the agent's own terminal title, else its task, else its CLI's name.
 */
export function paneLabel(pane: Pane, providers: Provider[], task?: string | null, sessionTitle?: string | null): string {
  const fallback = task ?? pane.prompt;
  return pane.customTitle ?? sessionTitle ?? pane.agentTitle ?? (fallback ? fallback.split("\n")[0].slice(0, 80) : providerName(providers, pane.provider));
}
