/**
 * Browser panel state and actions. Each workspace has its own headless Chromium
 * (src-tauri/src/browser); tabs come from CDP target discovery, so tabs agents open show up too.
 */
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { create } from "zustand";
import { useWorkspaces } from "@/stores/workspaces";

export interface Tab {
  id: string;
  url: string;
  title: string;
  byAgent: boolean;
}

export type Device = "desktop" | "mobile";
export type Availability = "unknown" | "missing" | "installing" | "ready" | "error";

interface BrowserState {
  availability: Availability;
  error: string | null;
  tabs: Record<string, Tab[]>;
  activeTab: Record<string, string | null>;
  device: Record<string, Device>;
  /** Bumped to ask the URL bar to take focus (⌘L). */
  focusUrlBar: number;
}

export const useBrowser = create<BrowserState>(() => ({
  availability: "unknown",
  error: null,
  tabs: {},
  activeTab: {},
  device: {},
  focusUrlBar: 0,
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

/** Turns what the user typed into a URL: localhost → http, bare domains → https, else a search. */
export function normalizeUrl(input: string): string {
  const text = input.trim();
  if (/^[a-z][a-z0-9+.-]*:/i.test(text) && !/^(localhost|[\w.-]+):\d+/i.test(text)) return text;
  if (/^(localhost|127\.0\.0\.1|0\.0\.0\.0|\[::1\])(:\d+)?(\/|$)/i.test(text)) return `http://${text}`;
  if (/^[\w-]+(\.[\w-]+)+(:\d+)?(\/.*)?$/.test(text) && !text.includes(" ")) return `https://${text}`;
  return `https://www.google.com/search?q=${encodeURIComponent(text)}`;
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
