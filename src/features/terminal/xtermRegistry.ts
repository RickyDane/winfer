/**
 * Owns every xterm instance outside React. Panes attach/detach the terminal's host element
 * to their slot, so layout changes and workspace switches never recreate a terminal (and the
 * agent behind it keeps running). Each terminal spawns its PTY once, on first attach.
 */
import { Channel, invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { openUrl } from "@tauri-apps/plugin-opener";
import { FitAddon } from "@xterm/addon-fit";
import { SearchAddon } from "@xterm/addon-search";
import { WebLinksAddon } from "@xterm/addon-web-links";
import { WebglAddon } from "@xterm/addon-webgl";
import { Terminal, type ITheme } from "@xterm/xterm";
import "@xterm/xterm/css/xterm.css";
import { useWorkspaces, type Pane } from "@/stores/workspaces";
import { reportInput, reportOutput, reportStatus } from "@/features/status/agentStatus";
import { createWorktree } from "@/features/worktree/worktree";
import { isAppChord } from "@/commands/keymap";

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
  search: SearchAddon;
  searchResults: { index: number; count: number };
  host: HTMLDivElement;
  opened: boolean;
  spawned: boolean;
  exited: boolean;
  lastOutput: number;
  decoder: TextDecoder;
  observer?: ResizeObserver;
  frame?: number;
  resizeTimer?: number;
}

const entries = new Map<string, Entry>();

let started = false;

/** Subscribes to PTY exit events. Called once at startup. */
export function startTerminalEvents() {
  if (started) return;
  started = true;
  void listen<{ paneId: string; code: number }>("pty://exit", ({ payload }) => {
    const entry = entries.get(payload.paneId);
    if (!entry) return;
    entry.exited = true;
    entry.term.write(`\r\n\x1b[2m[process exited with code ${payload.code}]\x1b[0m\r\n`);
    reportStatus(payload.paneId, payload.code === 0 ? "done" : "error", `exited with code ${payload.code}`);
  });
}

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
  const search = new SearchAddon();
  term.loadAddon(search);
  search.onDidChangeResults(({ resultIndex, resultCount }) => {
    const entry = entries.get(pane.id);
    if (entry) entry.searchResults = { index: resultIndex, count: resultCount };
  });
  term.loadAddon(new WebLinksAddon((_event, uri) => void openUrl(uri)));

  // App shortcuts (⌘… on macOS, Ctrl+Shift+… on Windows, plus any the user assigned) must reach
  // the app, not the PTY. Returning false skips xterm only; the browser still fires copy/paste events.
  term.attachCustomKeyEventHandler((e) => !((IS_MAC ? e.metaKey : e.ctrlKey && e.shiftKey) || isAppChord(e)));

  term.onData((data) => {
    const entry = entries.get(pane.id);
    if (!entry?.spawned || entry.exited) return;
    void invoke("pty_write", { paneId: pane.id, data });
    if (data.includes("\r")) reportInput(pane.id);
  });

  // Terminal notifications: OSC 9 (iTerm2/ConEmu style), OSC 777 (notify;title;body), bell.
  term.parser.registerOscHandler(9, (data) => {
    reportStatus(pane.id, "needs-input", data);
    return true;
  });
  term.parser.registerOscHandler(777, (data) => {
    const [kind, title, body] = data.split(";");
    if (kind === "notify") reportStatus(pane.id, "needs-input", [title, body].filter(Boolean).join(": "));
    return true;
  });
  // Shells beep on tab completion; only agents' bells mean "look at me".
  term.onBell(() => pane.provider !== "shell" && reportStatus(pane.id, "needs-input"));

  term.onResize(({ cols, rows }) => {
    const entry = entries.get(pane.id);
    if (!entry?.spawned) return;
    clearTimeout(entry.resizeTimer);
    entry.resizeTimer = window.setTimeout(() => void invoke("pty_resize", { paneId: pane.id, cols, rows }), 50);
  });

  const host = document.createElement("div");
  host.style.width = "100%";
  host.style.height = "100%";

  return { pane, term, fit, search, searchResults: { index: -1, count: 0 }, host, opened: false, spawned: false, exited: false, lastOutput: 0, decoder: new TextDecoder() };
}

/** Gives an agent pane its own git worktree before it starts (if the folder is a repo). */
async function prepareWorktree(entry: Entry) {
  const pane = entry.pane;
  if (!pane.wantsWorktree || pane.worktree) return;
  try {
    const worktree = await createWorktree(pane.cwd, pane.provider);
    const patch = worktree ? { worktree, cwd: worktree.path, wantsWorktree: false } : { wantsWorktree: false };
    useWorkspaces.getState().updatePane(pane.id, patch);
    entry.pane = { ...pane, ...patch };
    if (worktree) entry.term.write(`\x1b[2mworktree ${worktree.branch} (from ${worktree.base}) at ${worktree.path}\x1b[0m\r\n`);
  } catch (err) {
    useWorkspaces.getState().updatePane(pane.id, { wantsWorktree: false });
    entry.term.write(`\x1b[33mCouldn't create a worktree, starting in ${pane.cwd}: ${err}\x1b[0m\r\n`);
  }
}

async function spawn(entry: Entry) {
  entry.spawned = true;
  await prepareWorktree(entry);
  const channel = new Channel<ArrayBuffer>();
  channel.onmessage = (data) => {
    entry.term.write(new Uint8Array(data));
    entry.lastOutput = Date.now();
    reportOutput(entry.pane.id);
    detectDevServer(entry, data);
  };
  try {
    await invoke("pty_spawn", {
      req: {
        paneId: entry.pane.id,
        providerId: entry.pane.provider,
        cwd: entry.pane.cwd,
        workspaceId: entry.pane.workspaceId,
        resume: entry.pane.resume,
        prompt: entry.pane.prompt,
        model: entry.pane.model,
        cols: entry.term.cols,
        rows: entry.term.rows,
      },
      onData: channel,
    });
    useWorkspaces.getState().setPaneStatus(entry.pane.id, entry.pane.provider === "shell" ? "idle" : "working");
  } catch (err) {
    entry.exited = true;
    entry.term.write(`\x1b[31mFailed to start ${entry.pane.provider}: ${err}\x1b[0m\r\n`);
    useWorkspaces.getState().setPaneStatus(entry.pane.id, "error");
  }
}

// Dev servers print e.g. "Local: http://localhost:5173/" — often with color codes inside the URL.
const ANSI = /\x1b\[[0-9;?]*[A-Za-z]/g;
const LOCAL_URL = /https?:\/\/(?:localhost|127\.0\.0\.1|0\.0\.0\.0|\[::1\]):\d{2,5}(?:\/[^\s"'<>)\]]*)?/g;

function detectDevServer(entry: Entry, data: ArrayBuffer) {
  const text = entry.decoder.decode(data, { stream: true }).replace(ANSI, "");
  const urls = text.match(LOCAL_URL);
  if (!urls) return;
  const url = urls.at(-1)!.replace("0.0.0.0", "localhost");
  const pane = useWorkspaces.getState().workspaces.flatMap((w) => w.panes).find((p) => p.id === entry.pane.id);
  if (pane && pane.devUrl !== url) useWorkspaces.getState().updatePane(pane.id, { devUrl: url });
}

function scheduleFit(entry: Entry) {
  cancelAnimationFrame(entry.frame ?? 0);
  entry.frame = requestAnimationFrame(() => {
    if (!entry.host.isConnected || entry.host.clientWidth === 0 || entry.host.clientHeight === 0) return;
    entry.fit.fit();
    if (!entry.spawned) void spawn(entry);
  });
}

/** Starts a pane's process without showing it (e.g. an agent started in a background workspace). */
export function startDetached(pane: Pane) {
  if (entries.has(pane.id)) return;
  const entry = create(pane);
  entries.set(pane.id, entry);
  entry.term.resize(120, 32);
  void spawn(entry);
}

/** Last `lines` lines of a pane's screen + scrollback as plain text. */
export function readTerminal(paneId: string, lines: number): string | null {
  const entry = entries.get(paneId);
  if (!entry) return null;
  const buffer = entry.term.buffer.active;
  const out: string[] = [];
  for (let i = Math.max(0, buffer.length - lines); i < buffer.length; i++) out.push(buffer.getLine(i)?.translateToString(true) ?? "");
  while (out.length && !out.at(-1)!.trim()) out.pop();
  return out.join("\n");
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

const SEARCH_DECORATIONS = {
  matchBackground: "#6d7cff55",
  matchOverviewRuler: "#6d7cff",
  activeMatchBackground: "#f5b544aa",
  activeMatchColorOverviewRuler: "#f5b544",
};

/** Searches a pane's scrollback; returns the active match index and total count. */
export function searchTerminal(paneId: string, query: string, direction: "next" | "previous" | "clear") {
  const entry = entries.get(paneId);
  if (!entry) return { index: -1, count: 0 };
  if (direction === "clear" || !query) {
    entry.search.clearDecorations();
    entry.searchResults = { index: -1, count: 0 };
    return entry.searchResults;
  }
  const options = { caseSensitive: false, decorations: SEARCH_DECORATIONS };
  if (direction === "next") entry.search.findNext(query, options);
  else entry.search.findPrevious(query, options);
  return entry.searchResults;
}

/** Writes text to a pane's process as if typed. */
export function sendText(paneId: string, text: string) {
  const entry = entries.get(paneId);
  if (!entry?.spawned || entry.exited) return;
  void invoke("pty_write", { paneId, data: text });
  if (text.includes("\r")) reportInput(paneId);
}

/**
 * Pastes text into a pane using bracketed paste, so multi-line text lands in an agent's prompt
 * as one block instead of each line being submitted.
 */
export function pasteText(paneId: string, text: string) {
  sendText(paneId, `\x1b[200~${text}\x1b[201~`);
}

/** Milliseconds since the pane last produced output. */
export function outputAge(paneId: string): number {
  const entry = entries.get(paneId);
  return entry ? Date.now() - entry.lastOutput : Infinity;
}

export function focusTerminal(paneId: string) {
  entries.get(paneId)?.term.focus();
}

/** Restarts a pane's process in place (e.g. with a different model), resuming its session. */
export function restartTerminal(pane: Pane) {
  const slot = entries.get(pane.id)?.host.parentElement ?? null;
  closeTerminal(pane.id);
  if (slot) attachTerminal(pane, slot);
  else startDetached(pane);
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
