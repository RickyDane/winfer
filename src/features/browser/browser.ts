/**
 * Browser panel state and actions. Each workspace has its own headless Chromium
 * (src-tauri/src/browser); tabs come from CDP target discovery, so tabs agents open show up too.
 */
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { create } from "zustand";
import { useWorkspaces } from "@/stores/workspaces";
import { pasteText } from "@/features/terminal/xtermRegistry";
import { normalizeUrl } from "./url";

export { normalizeUrl };

export interface Tab {
  id: string;
  url: string;
  title: string;
  byAgent: boolean;
}

export type Device = "desktop" | "mobile";

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
  /** Bumped to ask the URL bar to take focus (⌘L). */
  focusUrlBar: number;
  /** Console/network entries per tab id. */
  console: Record<string, ConsoleEntry[]>;
  consoleOpen: boolean;
  picking: boolean;
  /** Short confirmation shown in the panel ("Sent to Claude Code"). */
  notice: string | null;
}

export const useBrowser = create<BrowserState>(() => ({
  availability: "unknown",
  error: null,
  tabs: {},
  activeTab: {},
  device: {},
  focusUrlBar: 0,
  console: {},
  consoleOpen: false,
  picking: false,
  notice: null,
}));

const activeWorkspaceId = () => useWorkspaces.getState().activeWorkspaceId;

function setTabs(workspaceId: string, tabs: Tab[]) {
  useBrowser.setState((s) => {
    const current = s.activeTab[workspaceId];
    const stillOpen = tabs.some((t) => t.id === current);
    return {
      tabs: { ...s.tabs, [workspaceId]: tabs },
      activeTab: { ...s.activeTab, [workspaceId]: stillOpen ? current! : (tabs.at(-1)?.id ?? null) },
    };
  });
}

let listening = false;
function listenForTabs() {
  if (listening) return;
  listening = true;
  void listen<{ workspaceId: string; tabs: Tab[] }>("browser://tabs", ({ payload }) => setTabs(payload.workspaceId, payload.tabs));
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

/** The agent that should receive browser context: the focused agent pane, else the first one. */
function targetAgent() {
  const s = useWorkspaces.getState();
  const ws = s.workspaces.find((w) => w.id === s.activeWorkspaceId);
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
    await invoke("browser_pick_start", { workspaceId: activeWorkspaceId(), targetId });
  } catch (err) {
    useBrowser.setState({ picking: false });
    flash(String(err));
  }
}

export async function cancelPick() {
  const targetId = activeTabId();
  useBrowser.setState({ picking: false });
  if (targetId) await invoke("browser_pick_cancel", { workspaceId: activeWorkspaceId(), targetId }).catch(() => {});
}

export function togglePick() {
  return useBrowser.getState().picking ? cancelPick() : startPick();
}

/** Sends recent console errors/warnings of the active tab to the agent. */
export function sendConsoleToAgent() {
  const targetId = activeTabId();
  const tab = (useBrowser.getState().tabs[activeWorkspaceId()] ?? []).find((t) => t.id === targetId);
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
  await invoke("browser_console_clear", { workspaceId: activeWorkspaceId(), targetId }).catch(() => {});
}

async function fail(err: unknown) {
  useBrowser.setState({ availability: "error", error: String(err) });
}

/** Checks for a browser and starts the workspace's instance. */
export async function ensureBrowser(workspaceId = activeWorkspaceId()): Promise<boolean> {
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

const sameOrigin = (a: string, b: string) => {
  try {
    return new URL(a).origin === new URL(b).origin;
  } catch {
    return false;
  }
};

export function activeTabId(workspaceId = activeWorkspaceId()) {
  return useBrowser.getState().activeTab[workspaceId] ?? null;
}

export function selectTab(tabId: string, workspaceId = activeWorkspaceId()) {
  useBrowser.setState((s) => ({ activeTab: { ...s.activeTab, [workspaceId]: tabId } }));
}

/** Opens a URL: reuses a tab already on that origin unless `newTab`, else the active tab. */
export async function openUrl(input: string, opts: { newTab?: boolean } = {}) {
  if (!input.trim()) return focusUrlBar();
  const ws = useWorkspaces.getState();
  if (!ws.browserOpen) ws.toggleBrowser();
  if (!(await ensureBrowser())) return;
  const url = normalizeUrl(input);
  const workspaceId = activeWorkspaceId();
  const tabs = useBrowser.getState().tabs[workspaceId] ?? [];
  const existing = tabs.find((t) => sameOrigin(t.url, url));
  const current = activeTabId(workspaceId);
  const blank = tabs.find((t) => t.id === current && (t.url === "about:blank" || t.url === ""));

  if (!opts.newTab && existing) {
    selectTab(existing.id, workspaceId);
    if (existing.url !== url) await invoke("browser_navigate", { workspaceId, targetId: existing.id, url });
  } else if (!opts.newTab && blank) {
    await invoke("browser_navigate", { workspaceId, targetId: blank.id, url });
  } else {
    const id = await invoke<string>("browser_new_tab", { workspaceId, url });
    selectTab(id, workspaceId);
  }
}

export async function newTab() {
  await openUrl("about:blank", { newTab: true });
  useBrowser.setState((s) => ({ focusUrlBar: s.focusUrlBar + 1 }));
}

export async function closeTab(tabId = activeTabId()) {
  if (tabId) await invoke("browser_close_tab", { workspaceId: activeWorkspaceId(), targetId: tabId });
}

/** -1 back, +1 forward, 0 reload. */
export async function history(delta: -1 | 0 | 1) {
  const targetId = activeTabId();
  if (targetId) await invoke("browser_history", { workspaceId: activeWorkspaceId(), targetId, delta });
}

export function setDevice(device: Device | "toggle") {
  const workspaceId = activeWorkspaceId();
  useBrowser.setState((s) => {
    const current = s.device[workspaceId] ?? "desktop";
    const next = device === "toggle" ? (current === "desktop" ? "mobile" : "desktop") : device;
    return { device: { ...s.device, [workspaceId]: next } };
  });
}

export function focusUrlBar() {
  const ws = useWorkspaces.getState();
  if (!ws.browserOpen) ws.toggleBrowser();
  useBrowser.setState((s) => ({ focusUrlBar: s.focusUrlBar + 1 }));
}
