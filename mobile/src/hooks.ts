import { useEffect, useMemo, useState, useSyncExternalStore } from "react";
import { flushSync } from "react-dom";
import type { ClientState, DesktopClient } from "./client";

export type Route =
  | { name: "inbox" }
  | { name: "pane"; id: string }
  | { name: "new"; workspaceId: string | null }
  | { name: "settings" }
  | { name: "usage" }
  | { name: "pair" }
  | { name: "panel" }
  | { name: "browser"; workspaceId: string }
  | { name: "android"; workspaceId: string | null }
  | { name: "terminal"; workspaceId: string; paneId: string }
  | { name: "files"; workspaceId: string };

export function parseRoute(hash: string): Route {
  const h = hash.replace(/^#/, "");
  const pane = h.match(/^\/pane\/(.+)$/);
  if (pane) return { name: "pane", id: decodeURIComponent(pane[1]) };
  const fresh = h.match(/^\/new(?:\/(.+))?$/);
  if (fresh) return { name: "new", workspaceId: fresh[1] ? decodeURIComponent(fresh[1]) : null };
  if (h === "/usage") return { name: "usage" };
  if (h === "/panel") return { name: "panel" };
  const android = h.match(/^\/panel\/android(?:\/([^/]+))?$/);
  if (android) return { name: "android", workspaceId: android[1] ? decodeURIComponent(android[1]) : null };
  const mod = h.match(/^\/panel\/(browser|files)\/([^/]+)$/);
  if (mod) return { name: mod[1] as "browser" | "files", workspaceId: decodeURIComponent(mod[2]) };
  const term = h.match(/^\/panel\/terminal\/([^/]+)\/(.+)$/);
  if (term) return { name: "terminal", workspaceId: decodeURIComponent(term[1]), paneId: decodeURIComponent(term[2]) };
  if (h === "/settings") return { name: "settings" };
  if (h === "/pair") return { name: "pair" };
  return { name: "inbox" };
}

// One hash listener for the app; screen changes run inside a view transition so the old screen
// slides out and shared elements (an agent's avatar and title) morph into the next screen.
const DEPTH: Record<Route["name"], number> = { inbox: 0, pair: 0, pane: 1, new: 1, settings: 0, usage: 1, panel: 0, browser: 2, android: 2, terminal: 2, files: 2 };
/** Screens reached from the bottom tab bar: switching between them cross-fades. */
export const TABS: Route["name"][] = ["inbox", "panel", "settings"];
let current = location.hash;
const routeListeners = new Set<() => void>();

window.addEventListener("hashchange", () => {
  const before = parseRoute(current);
  const next = location.hash;
  if (next === current) return;
  const after = parseRoute(next);
  const apply = () => {
    current = next;
    flushSync(() => routeListeners.forEach((fn) => fn()));
  };
  const start = (document as { startViewTransition?: (cb: () => void) => unknown }).startViewTransition?.bind(document);
  // A started session takes over the "new" screen in place: no slide, it's the same page.
  const inPlace = before.name === "new" && after.name === "pane";
  const animate = !!start && !inPlace && !matchMedia("(prefers-reduced-motion: reduce)").matches && before.name !== after.name;
  if (!animate) return apply();
  document.documentElement.dataset.nav =
    TABS.includes(before.name) && TABS.includes(after.name) ? "tab" : DEPTH[after.name] >= DEPTH[before.name] ? "forward" : "back";
  start!(apply);
});

const subscribeRoute = (fn: () => void) => {
  routeListeners.add(fn);
  return () => void routeListeners.delete(fn);
};

export function useRoute(): Route {
  const hash = useSyncExternalStore(subscribeRoute, () => current);
  return useMemo(() => parseRoute(hash), [hash]);
}

/** First messages sent from the "new" screen, shown in the thread until the desktop echoes them. */
export const firstMessages = new Map<string, string>();

export function go(path: string) {
  location.hash = path;
}

/** Where a side-panel screen's Back leads: the Panel tab, or the chat it was opened from. */
const BACK_KEY = "winfer.panelBack";
export const panelBack = () => sessionStorage.getItem(BACK_KEY) ?? "/panel";
export const setPanelBack = (path: string) => sessionStorage.setItem(BACK_KEY, path);

/** A tiny tap on phones that support it (Android); no-op elsewhere. */
export function haptic(pattern: number | number[] = 8) {
  try {
    navigator.vibrate?.(pattern);
  } catch {
    /* unsupported */
  }
}

/** CSS-ident-safe name for view-transition-name. */
export const vtName = (kind: string, id: string) => `${kind}-${id.replace(/[^a-zA-Z0-9_-]/g, "_")}`;

export function useClient(client: DesktopClient): ClientState {
  return useSyncExternalStore(client.subscribe, client.getState);
}

/** Re-render every `ms` (for relative timestamps); 0 stops. */
export function useTick(ms: number) {
  const [, set] = useState(0);
  useEffect(() => {
    if (ms <= 0) return;
    const t = setInterval(() => set((n) => n + 1), ms);
    return () => clearInterval(t);
  }, [ms]);
}

export function ago(ts: number | null): string {
  if (!ts) return "";
  const s = Math.max(0, Math.round((Date.now() - ts) / 1000));
  if (s < 45) return "now";
  const m = Math.round(s / 60);
  if (m < 60) return `${m}m`;
  const h = Math.round(m / 60);
  if (h < 24) return `${h}h`;
  return `${Math.round(h / 24)}d`;
}

export const isIOS = () => /iPad|iPhone|iPod/.test(navigator.userAgent) || (navigator.platform === "MacIntel" && navigator.maxTouchPoints > 1);
export const isStandalone = () =>
  matchMedia("(display-mode: standalone)").matches || (navigator as Navigator & { standalone?: boolean }).standalone === true;
