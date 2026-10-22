/**
 * The undocked browser: the browser panel in a window of its own (say on another screen).
 *
 * The window comes from `window.open`, which the backend turns into a native window sharing
 * this webview's process (undock.rs). It is same-origin, so the app renders the panel into it
 * with a React portal: same stores, same Tauri channels, nothing to sync. This module owns the
 * window: its styles (mirrored from the main window), shortcuts and closing.
 */
import { WebviewWindow } from "@tauri-apps/api/webviewWindow";
import { create } from "zustand";
import { handleShortcut } from "@/commands/keymap";
import { useWorkspaces } from "@/stores/workspaces";

/** Must match LABEL in src-tauri/src/undock.rs. */
const LABEL = "browser";
/** Must match UNDOCK_URL in src-tauri/src/undock.rs. */
const UNDOCK_URL = "about:blank#winfer-browser";
const ROOT_ID = "winfer-undocked";

interface UndockState {
  /** The undocked window, null while the browser sits in the side panel. */
  win: Window | null;
  /** The element the panel renders into. */
  root: HTMLElement | null;
}

export const useUndock = create<UndockState>(() => ({ win: null, root: null }));

let cleanup: (() => void) | null = null;

/** Copies the main window's stylesheets and <html> theme attributes, and keeps them in sync (theme switches, dev HMR). */
function mirrorStyles(target: Document): () => void {
  const copyHead = () => {
    target.head.querySelectorAll("[data-mirrored]").forEach((n) => n.remove());
    for (const node of document.head.querySelectorAll('style, link[rel="stylesheet"]')) {
      const copy = target.importNode(node, true) as HTMLElement;
      // Absolute, so it resolves the same from about:blank.
      if (copy instanceof HTMLLinkElement) copy.href = (node as HTMLLinkElement).href;
      copy.dataset.mirrored = "";
      target.head.appendChild(copy);
    }
  };
  const copyAttrs = () => {
    const from = document.documentElement;
    const to = target.documentElement;
    for (const { name } of [...to.attributes]) if (!from.hasAttribute(name)) to.removeAttribute(name);
    for (const { name, value } of [...from.attributes]) to.setAttribute(name, value);
    // The native title bar follows the theme too.
    const mode = from.dataset.mode;
    if (mode === "light" || mode === "dark") void nativeWindow().then((w) => w?.setTheme(mode).catch(() => {}));
  };
  copyHead();
  copyAttrs();
  const head = new MutationObserver(copyHead);
  head.observe(document.head, { childList: true, subtree: true, characterData: true });
  const attrs = new MutationObserver(copyAttrs);
  attrs.observe(document.documentElement, { attributes: true });
  return () => {
    head.disconnect();
    attrs.disconnect();
  };
}

/** Moves the browser into its own window, or brings that window to the front. */
export function undockBrowser(): boolean {
  const open = useUndock.getState().win;
  if (open && !open.closed) {
    raise();
    return true;
  }
  // Place and size come from the backend, which remembers them (undock.rs).
  const win = window.open(UNDOCK_URL, "winfer-browser");
  if (!win) return false;

  let stopMirror = () => {};
  /**
   * Fills the window's document. WebKit swaps the first, empty document for the loaded
   * about:blank one after `window.open` returns, so this runs again whenever the content is gone.
   */
  const mount = () => {
    const doc = win.document;
    if (doc.getElementById(ROOT_ID)) return;
    stopMirror();
    doc.title = "winfer browser";
    const meta = doc.createElement("meta");
    meta.setAttribute("charset", "utf-8");
    doc.head.appendChild(meta);
    const base = doc.createElement("base");
    base.href = location.href;
    doc.head.appendChild(base);
    stopMirror = mirrorStyles(doc);
    const root = doc.createElement("div");
    root.id = ROOT_ID;
    root.className = "flex h-full flex-col overflow-hidden bg-panel";
    doc.body.style.height = "100%";
    doc.documentElement.style.height = "100%";
    doc.body.appendChild(root);
    // App shortcuts (⌘L, ⌘⇧C, …) work from this window too. A new document has a new inner window.
    win.addEventListener("keydown", handleShortcut, { capture: true });
    useUndock.setState({ win, root });
  };
  mount();

  const onMainUnload = () => win.close();
  window.addEventListener("beforeunload", onMainUnload);
  // The native close button gives no reliable event to the opener: watch for it.
  const poll = window.setInterval(() => (win.closed ? dockBrowser() : mount()), 150);

  cleanup = () => {
    window.clearInterval(poll);
    window.removeEventListener("beforeunload", onMainUnload);
    stopMirror();
    if (!win.closed) win.close();
  };
  // The screen-recorder plugin looks for the browser canvas here when it isn't in the main window.
  (window as unknown as { winferBrowserWindow?: Window }).winferBrowserWindow = win;
  return true;
}

/** Puts the browser back into the side panel and closes its window. */
export function dockBrowser() {
  if (!useUndock.getState().win) return;
  const done = cleanup;
  cleanup = null;
  useUndock.setState({ win: null, root: null });
  delete (window as unknown as { winferBrowserWindow?: Window }).winferBrowserWindow;
  done?.();
  const ws = useWorkspaces.getState();
  if (ws.browserOpen && ws.sidePanel === "browser") return;
  ws.setSidePanel("browser");
}

export const toggleUndock = () => (useUndock.getState().win ? dockBrowser() : void undockBrowser());

/** Brings the undocked window forward; false while the browser is docked. */
export function focusUndocked(): boolean {
  const { win } = useUndock.getState();
  if (!win || win.closed) return false;
  raise();
  return true;
}

/** `window.focus()` from the opener doesn't raise a native window; Tauri's API does. */
const nativeWindow = () => WebviewWindow.getByLabel(LABEL).catch(() => null);

function raise() {
  void nativeWindow().then(async (w) => {
    if (!w) return;
    await w.unminimize().catch(() => {});
    await w.setFocus().catch(() => {});
  });
}
