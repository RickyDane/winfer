/**
 * Browser panel state and actions. Each workspace has its own headless Chromium
 * (src-tauri/src/browser); tabs come from CDP target discovery, so tabs agents open show up too.
 * The panel lists every workspace's tabs and shows one of them; it doesn't follow the active
 * workspace, so switching workspaces (or working in the overview) leaves the page in place.
 */
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { create } from "zustand";
import { useWorkspaces } from "@/stores/workspaces";
import { pasteText } from "@/features/terminal/xtermRegistry";
import { normalizeUrl } from "./url";
import { relatedTab } from "./related";
import { followedTab } from "./follow";

export { normalizeUrl };

export interface Tab {
  id: string;
  url: string;
  title: string;
}

export type Device = "desktop" | "mobile";

/** An agent using a tab; `adopted` when it took over a tab it didn't open (one the user had open). */
export interface AgentUse {
  adopted: boolean;
}

/** A tab's name in the tab strip. */
export function tabLabel(tab: Tab) {
  if (tab.title && tab.title !== tab.url) return tab.title;
  if (!tab.url || tab.url === "about:blank") return "New tab";
  try {
    return new URL(tab.url).host || tab.url;
  } catch {
    return tab.url;
  }
}

/** Who's using a workspace's browser: its one working agent by name, else "An agent". */
export function agentName(s: Pick<ReturnType<typeof useWorkspaces.getState>, "workspaces" | "providers">, workspaceId: string) {
  const working = s.workspaces.find((w) => w.id === workspaceId)?.panes.filter((p) => p.provider !== "shell" && p.status === "working") ?? [];
  return working.length === 1 ? (s.providers.find((p) => p.id === working[0].provider)?.name ?? working[0].provider) : "An agent";
}

export interface PageSize {
  width: number;
  height: number;
}

export interface ConsoleEntry {
  level: "error" | "warning" | "info" | "log";
  text: string;
  source: string | null;
  timestamp: number;
}
export type Availability = "unknown" | "missing" | "installing" | "ready" | "error";

interface BrowserState {
  availability: Availability;
  error: string | null;
  tabs: Record<string, Tab[]>;
  activeTab: Record<string, string | null>;
  device: Record<string, Device>;
  /** Page sizes set by dragging the page's edges or by an agent, per tab id; absent fits the panel. */
  sizes: Record<string, PageSize>;
  /** Bumped to ask the URL bar to take focus (⌘L). */
  focusUrlBar: number;
  /** Console/network entries per tab id. */
  console: Record<string, ConsoleEntry[]>;
  consoleOpen: boolean;
  picking: boolean;
  /** Short confirmation shown in the panel ("Sent to Claude Code"). */
  notice: string | null;
  /** The workspace whose tab the panel shows; null until the panel first opens. */
  shown: string | null;
  /** Tabs an agent is using right now (acted on in the last few seconds), by tab id. */
  agentTabs: Record<string, AgentUse>;
}

export const useBrowser = create<BrowserState>(() => ({
  availability: "unknown",
  error: null,
  tabs: {},
  activeTab: {},
  device: {},
  sizes: {},
  focusUrlBar: 0,
  console: {},
  consoleOpen: false,
  picking: false,
  notice: null,
  shown: null,
  agentTabs: {},
}));

/** How long a tab stays marked as used after an agent's last action in it. */
const AGENT_LINGER_MS = 3500;
const agentTimers = new Map<string, number>();
/** Tabs agents opened through winfer (open_url): using them isn't a takeover. */
const agentOpened = new Set<string>();
/** Tabs whose takeover was announced, so it's said once. */
const announced = new Set<string>();

export function markAgentOpened(tabId: string) {
  agentOpened.add(tabId);
}

/** An agent acted on a tab (the backend's CDP proxy saw it): mark it until it goes quiet. */
export function markAgentTab(tabId: string, workspaceId: string, adopted: boolean) {
  adopted &&= !agentOpened.has(tabId);
  const tab = useBrowser.getState().tabs[workspaceId]?.find((t) => t.id === tabId);
  if (adopted && tab && !announced.has(tabId)) {
    announced.add(tabId);
    flash(`${agentName(useWorkspaces.getState(), workspaceId)} took over ${tabLabel(tab)}`);
  }
  clearTimeout(agentTimers.get(tabId));
  agentTimers.set(
    tabId,
    window.setTimeout(() => {
      agentTimers.delete(tabId);
      useBrowser.setState((s) => {
        const { [tabId]: _, ...rest } = s.agentTabs;
        return { agentTabs: rest };
      });
    }, AGENT_LINGER_MS),
  );
  if (useBrowser.getState().agentTabs[tabId]?.adopted !== adopted) useBrowser.setState((s) => ({ agentTabs: { ...s.agentTabs, [tabId]: { adopted } } }));
}

const activeWorkspaceId = () => useWorkspaces.getState().activeWorkspaceId;

/** Records a browser problem in browser.log (app data folder) for later debugging. */
export function logBrowser(level: "error" | "warn" | "info", message: string) {
  if (level === "error") console.error(`[browser] ${message}`);
  void invoke("browser_log", { level, message }).catch(() => {});
}

/** The workspace whose browser the panel shows (the active one until a tab is picked). */
export function shownWorkspaceId() {
  const s = useWorkspaces.getState();
  const shown = useBrowser.getState().shown;
  return shown && s.workspaces.some((w) => w.id === shown) ? shown : s.activeWorkspaceId;
}

/** Where new tabs go: the workspace on screen, or in the overview and plugin pages, the one the panel shows. */
function targetWorkspaceId() {
  return useWorkspaces.getState().view === "workspace" ? activeWorkspaceId() : shownWorkspaceId();
}

function setTabs(workspaceId: string, tabs: Tab[]) {
  const ws = useWorkspaces.getState();
  // Follow agents in the workspace the panel shows, or the one on screen.
  const watched = workspaceId === shownWorkspaceId() || (ws.view === "workspace" && workspaceId === ws.activeWorkspaceId);
  useBrowser.setState((s) => {
    const current = s.activeTab[workspaceId];
    const stillOpen = tabs.some((t) => t.id === current);
    const follow = watched ? followedTab(s.tabs[workspaceId], tabs, current) : undefined;
    // The shown workspace's last tab closed: show another workspace's tab instead of nothing.
    const other = tabs.length === 0 && s.shown === workspaceId ? Object.keys(s.tabs).find((id) => id !== workspaceId && s.tabs[id].length > 0) : undefined;
    return {
      tabs: { ...s.tabs, [workspaceId]: tabs },
      activeTab: { ...s.activeTab, [workspaceId]: follow?.id ?? (stillOpen ? current! : (tabs.at(-1)?.id ?? null)) },
      shown: follow ? workspaceId : (other ?? s.shown),
    };
  });
}

let listening = false;
function listenForTabs() {
  if (listening) return;
  listening = true;
  void listen<{ workspaceId: string; tabs: Tab[] }>("browser://tabs", ({ payload }) => setTabs(payload.workspaceId, payload.tabs));
  void listen<{ workspaceId: string; targetId: string; adopted: boolean }>("browser://agent", ({ payload }) =>
    markAgentTab(payload.targetId, payload.workspaceId, payload.adopted),
  );
  void listen<{ targetId: string; width: number; height: number }>("browser://viewport", ({ payload }) => setPageSize(payload.targetId, payload));
  void listen<{ targetId: string; entry: ConsoleEntry }>("browser://console", ({ payload }) =>
    useBrowser.setState((s) => ({ console: { ...s.console, [payload.targetId]: [...(s.console[payload.targetId] ?? []), payload.entry].slice(-200) } })),
  );
  void listen<Picked>("browser://picked", ({ payload }) => {
    useBrowser.setState({ picking: false });
    deliverPicked(payload);
  });
  void listen<string>("browser://pick-error", ({ payload }) => {
    useBrowser.setState({ picking: false });
    flash(`Couldn't read that element: ${payload}`);
  });
}

interface Picked {
  url: string;
  selector: string;
  html: string;
  text: string;
  screenshot: string | null;
}

let noticeTimer: number | undefined;
function flash(notice: string) {
  clearTimeout(noticeTimer);
  useBrowser.setState({ notice });
  noticeTimer = window.setTimeout(() => useBrowser.setState({ notice: null }), 4000);
}

/** The agent that should receive browser context: the shown tab's workspace's focused agent pane, else its first one. */
function targetAgent() {
  const s = useWorkspaces.getState();
  const ws = s.workspaces.find((w) => w.id === shownWorkspaceId());
  const agents = ws?.panes.filter((p) => p.provider !== "shell") ?? [];
  return agents.find((p) => p.id === s.focusedPaneId) ?? agents[0] ?? null;
}

function sendToAgent(text: string, what: string) {
  const pane = targetAgent();
  if (!pane) {
    flash("Open an agent pane first");
    return;
  }
  pasteText(pane.id, text);
  useWorkspaces.getState().focusPane(pane.id);
  const name = useWorkspaces.getState().providers.find((p) => p.id === pane.provider)?.name ?? pane.provider;
  flash(`Sent ${what} to ${name}`);
}

function deliverPicked(p: Picked) {
  const lines = [`Element on ${p.url}`, `selector: ${p.selector}`];
  if (p.text) lines.push(`text: "${p.text.replace(/\s+/g, " ")}"`);
  lines.push("html:", p.html);
  if (p.screenshot) lines.push(`screenshot: ${p.screenshot}`);
  sendToAgent(`${lines.join("\n")}\n`, p.selector);
}

export async function startPick() {
  const targetId = activeTabId();
  if (!targetId) return;
  useBrowser.setState({ picking: true });
  try {
    await invoke("browser_pick_start", { workspaceId: shownWorkspaceId(), targetId });
  } catch (err) {
    useBrowser.setState({ picking: false });
    flash(String(err));
  }
}

export async function cancelPick() {
  const targetId = activeTabId();
  useBrowser.setState({ picking: false });
  if (targetId) await invoke("browser_pick_cancel", { workspaceId: shownWorkspaceId(), targetId }).catch(() => {});
}

export function togglePick() {
  return useBrowser.getState().picking ? cancelPick() : startPick();
}

/** Sends recent console errors/warnings of the active tab to the agent. */
export function sendConsoleToAgent() {
  const targetId = activeTabId();
  const tab = (useBrowser.getState().tabs[shownWorkspaceId()] ?? []).find((t) => t.id === targetId);
  const entries = (useBrowser.getState().console[targetId ?? ""] ?? []).filter((e) => e.level === "error" || e.level === "warning").slice(-20);
  if (entries.length === 0) {
    flash("No console errors to send");
    return;
  }
  const body = entries.map((e) => `- [${e.level}] ${e.text}${e.source ? ` (${e.source})` : ""}`).join("\n");
  sendToAgent(`Browser console errors on ${tab?.url ?? "the page"}:\n${body}\n`, `${entries.length} console error${entries.length === 1 ? "" : "s"}`);
}

export async function clearConsole() {
  const targetId = activeTabId();
  if (!targetId) return;
  useBrowser.setState((s) => ({ console: { ...s.console, [targetId]: [] } }));
  await invoke("browser_console_clear", { workspaceId: shownWorkspaceId(), targetId }).catch(() => {});
}

async function fail(err: unknown) {
  logBrowser("error", `start failed: ${String(err)}`);
  useBrowser.setState({ availability: "error", error: String(err) });
}

const opening = new Map<string, Promise<boolean>>();

/** Checks for a browser and starts the workspace's instance; concurrent calls share one start. */
export function ensureBrowser(workspaceId = shownWorkspaceId()): Promise<boolean> {
  let pending = opening.get(workspaceId);
  if (!pending) {
    pending = startBrowser(workspaceId).finally(() => opening.delete(workspaceId));
    opening.set(workspaceId, pending);
  }
  return pending;
}

async function startBrowser(workspaceId: string): Promise<boolean> {
  listenForTabs();
  const s = useBrowser.getState();
  if (s.availability === "unknown" || s.availability === "error") {
    try {
      const status = await invoke<{ executable: unknown | null }>("browser_status");
      useBrowser.setState({ availability: status.executable ? "ready" : "missing", error: null });
    } catch (err) {
      await fail(err);
      return false;
    }
  }
  if (useBrowser.getState().availability !== "ready") return false;
  if (useBrowser.getState().tabs[workspaceId]) return true;
  try {
    const tabs = await invoke<Tab[]>("browser_open", { workspaceId });
    setTabs(workspaceId, tabs);
    return true;
  } catch (err) {
    await fail(err);
    return false;
  }
}

export async function installBrowser() {
  useBrowser.setState({ availability: "installing", error: null });
  try {
    await invoke("browser_install");
    useBrowser.setState({ availability: "ready" });
    await ensureBrowser();
  } catch (err) {
    await fail(err);
  }
}

export function activeTabId(workspaceId = shownWorkspaceId()) {
  return useBrowser.getState().activeTab[workspaceId] ?? null;
}

/** Shows a tab in the panel (and its workspace's browser). */
export function selectTab(tabId: string, workspaceId = shownWorkspaceId()) {
  useBrowser.setState((s) => ({ activeTab: { ...s.activeTab, [workspaceId]: tabId }, shown: workspaceId }));
}

/** Where `openUrl` opened a page: the tab, and the open tab it took over (one already on that site), if any. */
export interface Opened {
  id: string;
  tookOver?: Tab;
}

/**
 * Opens a URL in a workspace's browser (by default the one on screen) and shows it: takes over
 * a tab already showing that site (see `relatedTab`) unless `newTab`, else a blank active tab.
 */
export async function openUrl(input: string, opts: { newTab?: boolean; workspaceId?: string } = {}): Promise<Opened | undefined> {
  if (!input.trim()) {
    focusUrlBar();
    return undefined;
  }
  const ws = useWorkspaces.getState();
  if (!ws.browserOpen) ws.toggleBrowser();
  const workspaceId = opts.workspaceId ?? targetWorkspaceId();
  if (!(await ensureBrowser(workspaceId))) return;
  const url = normalizeUrl(input);
  const tabs = useBrowser.getState().tabs[workspaceId] ?? [];
  const existing = relatedTab(tabs, url);
  const current = activeTabId(workspaceId);
  const blank = tabs.find((t) => t.id === current && (t.url === "about:blank" || t.url === ""));

  try {
    if (!opts.newTab && existing) {
      selectTab(existing.id, workspaceId);
      if (existing.url !== url) await invoke("browser_navigate", { workspaceId, targetId: existing.id, url });
      return { id: existing.id, tookOver: existing };
    } else if (!opts.newTab && blank) {
      selectTab(blank.id, workspaceId);
      await invoke("browser_navigate", { workspaceId, targetId: blank.id, url });
      return { id: blank.id };
    } else {
      const id = await invoke<string>("browser_new_tab", { workspaceId, url });
      selectTab(id, workspaceId);
      return { id };
    }
  } catch (err) {
    // The backend logged the details; say so in the panel rather than failing silently.
    flash(`Couldn't open ${url}: ${String(err)}`);
  }
}

export async function newTab() {
  await openUrl("about:blank", { newTab: true });
  useBrowser.setState((s) => ({ focusUrlBar: s.focusUrlBar + 1 }));
}

export async function closeTab(tabId = activeTabId(), workspaceId = shownWorkspaceId()) {
  if (tabId) await invoke("browser_close_tab", { workspaceId, targetId: tabId }).catch((err) => flash(`Couldn't close the tab: ${String(err)}`));
}

/** -1 back, +1 forward, 0 reload. */
export async function history(delta: -1 | 0 | 1) {
  const targetId = activeTabId();
  if (targetId) await invoke("browser_history", { workspaceId: shownWorkspaceId(), targetId, delta }).catch((err) => flash(String(err)));
}

export function setDevice(device: Device | "toggle") {
  const workspaceId = shownWorkspaceId();
  const tabId = activeTabId(workspaceId);
  if (tabId) setPageSize(tabId, null);
  useBrowser.setState((s) => {
    const current = s.device[workspaceId] ?? "desktop";
    const next = device === "toggle" ? (current === "desktop" ? "mobile" : "desktop") : device;
    return { device: { ...s.device, [workspaceId]: next } };
  });
}

/** Gives a tab's page a fixed size, or (null) lets it fit the panel again. */
export function setPageSize(tabId: string, size: PageSize | null) {
  useBrowser.setState((s) => {
    const { [tabId]: _, ...rest } = s.sizes;
    return { sizes: size ? { ...rest, [tabId]: { width: Math.round(size.width), height: Math.round(size.height) } } : rest };
  });
}

export function focusUrlBar() {
  const ws = useWorkspaces.getState();
  if (!ws.browserOpen) ws.toggleBrowser();
  useBrowser.setState((s) => ({ focusUrlBar: s.focusUrlBar + 1 }));
}
