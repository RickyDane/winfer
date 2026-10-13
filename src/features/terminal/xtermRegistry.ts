/**
 * Owns every xterm instance outside React. Panes attach/detach the terminal's host element
 * to their slot, so layout changes and workspace switches never recreate a terminal (and the
 * agent behind it keeps running). Each terminal spawns its PTY once, on first attach.
 */
import { Channel, invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { openUrl } from "@tauri-apps/plugin-opener";
import { FitAddon } from "@xterm/addon-fit";
import { WebLinksAddon } from "@xterm/addon-web-links";
import { WebglAddon } from "@xterm/addon-webgl";
import { Terminal, type ITheme } from "@xterm/xterm";
import "@xterm/xterm/css/xterm.css";
import { useWorkspaces, type Pane } from "@/stores/workspaces";

const IS_MAC = navigator.userAgent.includes("Mac");

const THEME: ITheme = {
  background: "#121215",
  foreground: "#e6e6ea",
  cursor: "#e6e6ea",
  cursorAccent: "#121215",
  selectionBackground: "#6d7cff55",
  black: "#1d1d22",
  red: "#f26d6d",
  green: "#4ade80",
  yellow: "#f5b544",
  blue: "#6d9cff",
  magenta: "#c08cff",
  cyan: "#5fd4e0",
  white: "#d4d4da",
  brightBlack: "#6b6b74",
  brightRed: "#ff8a8a",
  brightGreen: "#7cf0a4",
  brightYellow: "#ffd07a",
  brightBlue: "#94b6ff",
  brightMagenta: "#d6b0ff",
  brightCyan: "#8ce6ee",
  brightWhite: "#ffffff",
};

interface Entry {
  pane: Pane;
  term: Terminal;
  fit: FitAddon;
  host: HTMLDivElement;
  opened: boolean;
  spawned: boolean;
  exited: boolean;
  observer?: ResizeObserver;
  frame?: number;
  resizeTimer?: number;
}

const entries = new Map<string, Entry>();

listen<{ paneId: string; code: number }>("pty://exit", ({ payload }) => {
  const entry = entries.get(payload.paneId);
  if (!entry) return;
  entry.exited = true;
  entry.term.write(`\r\n\x1b[2m[process exited with code ${payload.code}]\x1b[0m\r\n`);
  useWorkspaces.getState().setPaneStatus(payload.paneId, payload.code === 0 ? "done" : "error");
});

function create(pane: Pane): Entry {
  const term = new Terminal({
    fontFamily: '"JetBrains Mono", "SF Mono", ui-monospace, Menlo, Consolas, monospace',
    fontSize: 12.5,
    lineHeight: 1.2,
    cursorBlink: true,
    scrollback: 10_000,
    macOptionIsMeta: true,
    allowProposedApi: true,
    theme: THEME,
  });
  const fit = new FitAddon();
  term.loadAddon(fit);
  term.loadAddon(new WebLinksAddon((_event, uri) => void openUrl(uri)));

  // App shortcuts (⌘… on macOS, Ctrl+Shift+… on Windows) must reach the app, not the PTY.
  // Returning false skips xterm only; the browser still fires copy/paste events.
  term.attachCustomKeyEventHandler((e) => !(IS_MAC ? e.metaKey : e.ctrlKey && e.shiftKey));

  term.onData((data) => {
    const entry = entries.get(pane.id);
    if (entry?.spawned && !entry.exited) void invoke("pty_write", { paneId: pane.id, data });
  });

  term.onResize(({ cols, rows }) => {
    const entry = entries.get(pane.id);
    if (!entry?.spawned) return;
    clearTimeout(entry.resizeTimer);
    entry.resizeTimer = window.setTimeout(() => void invoke("pty_resize", { paneId: pane.id, cols, rows }), 50);
  });

  const host = document.createElement("div");
  host.style.width = "100%";
  host.style.height = "100%";

  return { pane, term, fit, host, opened: false, spawned: false, exited: false };
}

async function spawn(entry: Entry) {
  entry.spawned = true;
  const channel = new Channel<ArrayBuffer>();
  channel.onmessage = (data) => entry.term.write(new Uint8Array(data));
  try {
    await invoke("pty_spawn", {
      req: {
        paneId: entry.pane.id,
        providerId: entry.pane.provider,
        cwd: entry.pane.cwd,
        workspaceId: entry.pane.workspaceId,
        cols: entry.term.cols,
        rows: entry.term.rows,
      },
      onData: channel,
    });
    useWorkspaces.getState().setPaneStatus(entry.pane.id, "working");
  } catch (err) {
    entry.exited = true;
    entry.term.write(`\x1b[31mFailed to start ${entry.pane.provider}: ${err}\x1b[0m\r\n`);
    useWorkspaces.getState().setPaneStatus(entry.pane.id, "error");
  }
}

function scheduleFit(entry: Entry) {
  cancelAnimationFrame(entry.frame ?? 0);
  entry.frame = requestAnimationFrame(() => {
    if (!entry.host.isConnected || entry.host.clientWidth === 0 || entry.host.clientHeight === 0) return;
    entry.fit.fit();
    if (!entry.spawned) void spawn(entry);
  });
}

export function attachTerminal(pane: Pane, slot: HTMLElement) {
  let entry = entries.get(pane.id);
  if (!entry) {
    entry = create(pane);
    entries.set(pane.id, entry);
  }
  slot.appendChild(entry.host);

  if (!entry.opened) {
    entry.term.open(entry.host);
    entry.opened = true;
    try {
      const webgl = new WebglAddon();
      webgl.onContextLoss(() => webgl.dispose());
      entry.term.loadAddon(webgl);
    } catch {
      // Falls back to the DOM renderer.
    }
  }

  const e = entry;
  e.observer?.disconnect();
  e.observer = new ResizeObserver(() => scheduleFit(e));
  e.observer.observe(slot);
  scheduleFit(e);
}

export function detachTerminal(paneId: string) {
  const entry = entries.get(paneId);
  if (!entry) return;
  entry.observer?.disconnect();
  entry.host.remove();
}

export function focusTerminal(paneId: string) {
  entries.get(paneId)?.term.focus();
}

/** Kills the PTY and disposes the terminal. */
export function closeTerminal(paneId: string) {
  const entry = entries.get(paneId);
  void invoke("pty_kill", { paneId });
  if (!entry) return;
  entry.observer?.disconnect();
  entry.term.dispose();
  entry.host.remove();
  entries.delete(paneId);
}
