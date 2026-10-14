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
import { create as createStore } from "zustand";
import { cleanAgentTitle, useWorkspaces, type Pane } from "@/stores/workspaces";
import { reportInput, reportOutput, reportStatus } from "@/features/status/agentStatus";
import { createWorktree } from "@/features/worktree/worktree";
import { isAppChord } from "@/commands/keymap";
import { setTask } from "@/features/progress/progress";

const IS_MAC = navigator.userAgent.includes("Mac");

const DARK: ITheme = {
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

/** Same hues, darkened so agent output stays readable on white. */
const LIGHT: ITheme = {
  background: "#fbfbfc",
  foreground: "#1b1b21",
  cursor: "#1b1b21",
  cursorAccent: "#fbfbfc",
  selectionBackground: "#4f5ff033",
  black: "#1b1b21",
  red: "#c62f2f",
  green: "#1f8a46",
  yellow: "#9a6a00",
  blue: "#2f5fd0",
  magenta: "#8a3fc0",
  cyan: "#0f7f8c",
  white: "#d4d4da",
  brightBlack: "#6b6b74",
  brightRed: "#e04848",
  brightGreen: "#2aa35a",
  brightYellow: "#b88400",
  brightBlue: "#4a78e8",
  brightMagenta: "#a55ad8",
  brightCyan: "#1897a6",
  brightWhite: "#ffffff",
};

const themeFor = (theme: "dark" | "light") => (theme === "light" ? LIGHT : DARK);

// Recolor every live terminal when the app theme changes; refit them all when zoomed.
useWorkspaces.subscribe((s, prev) => {
  if (s.theme !== prev.theme) for (const entry of entries.values()) entry.term.options.theme = themeFor(s.theme);
  if (s.terminalFontSize !== prev.terminalFontSize) {
    for (const entry of entries.values()) {
      entry.term.options.fontSize = s.terminalFontSize;
      scheduleFit(entry); // new rows/cols reach the agent through onResize
    }
  }
});

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
    fontSize: useWorkspaces.getState().terminalFontSize,
    lineHeight: 1.2,
    cursorBlink: true,
    scrollback: 10_000,
    macOptionIsMeta: true,
    allowProposedApi: true,
    theme: themeFor(useWorkspaces.getState().theme),
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

  // What the user types, line by line: a submitted line is the agent's new task (progress bars).
  let typed = "";
  term.onData((data) => {
    const entry = entries.get(pane.id);
    if (!entry?.spawned || entry.exited) return;
    void invoke("pty_write", { paneId: pane.id, data });
    if (!data.startsWith("\x1b")) {
      for (const ch of data) {
        if (ch === "\r") {
          if (pane.provider !== "shell") setTask(pane.id, typed);
          typed = "";
        } else if (ch === "\x7f") typed = typed.slice(0, -1);
        else if (ch >= " " || ch === "\n") typed += ch;
      }
    }
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
  // Agents name their terminal after what they're doing (Claude Code: the session topic).
  term.onTitleChange((raw) => {
    const s = useWorkspaces.getState();
    const provider = s.providers.find((p) => p.id === pane.provider);
    const title = cleanAgentTitle(raw, [provider?.name ?? "", provider?.command ?? "", pane.provider]);
    const current = s.workspaces.flatMap((w) => w.panes).find((p) => p.id === pane.id);
    if (title && current && current.agentTitle !== title) s.updatePane(pane.id, { agentTitle: title });
  });
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
  // Ctrl/⌘ + wheel (and trackpad pinch, which arrives as Ctrl + wheel) zooms every terminal.
  let pinch = 0;
  host.addEventListener(
    "wheel",
    (e) => {
      if (!e.ctrlKey && !e.metaKey) return;
      e.preventDefault();
      e.stopPropagation();
      pinch -= e.deltaY;
      if (Math.abs(pinch) < 40) return;
      const s = useWorkspaces.getState();
      s.setTerminalFontSize(s.terminalFontSize + Math.sign(pinch) * 0.5);
      pinch = 0;
    },
    { passive: false, capture: true },
  );

  return { pane, term, fit, search, searchResults: { index: -1, count: 0 }, host, opened: false, spawned: false, exited: false, lastOutput: 0, decoder: new TextDecoder() };
}

/**
 * Panes whose process is still starting, until it prints something: shown as a loading
 * overlay on the pane ("Creating worktree…", "Starting Claude Code…").
 */
export const useStarting = createStore<Record<string, "worktree" | "process">>(() => ({}));

function setStarting(paneId: string, phase: "worktree" | "process" | null) {
  useStarting.setState((s) => {
    if (phase === null) {
      if (!(paneId in s)) return s;
      const { [paneId]: _, ...rest } = s;
      return rest;
    }
    return { ...s, [paneId]: phase };
  }, true);
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
  const id = entry.pane.id;
  setStarting(id, entry.pane.wantsWorktree && !entry.pane.worktree ? "worktree" : "process");
  // Some programs stay silent until they get input; don't leave the overlay up forever.
  const giveUp = window.setTimeout(() => setStarting(id, null), 15_000);
  await prepareWorktree(entry);
  if (entries.get(id) === entry) setStarting(id, "process");
  const channel = new Channel<ArrayBuffer>();
  channel.onmessage = (data) => {
    if (!entry.lastOutput) {
      clearTimeout(giveUp);
      setStarting(id, null);
    }
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
    clearTimeout(giveUp);
    setStarting(id, null);
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

/** Id of a workspace's side-panel shell (it isn't one of the workspace's panes). */
export const sideTerminalId = (workspaceId: string) => `side-terminal-${workspaceId}`;

/** Kills the PTY and disposes the terminal. */
export function closeTerminal(paneId: string) {
  const entry = entries.get(paneId);
  void invoke("pty_kill", { paneId });
  setStarting(paneId, null);
  if (!entry) return;
  entry.observer?.disconnect();
  entry.term.dispose();
  entry.host.remove();
  entries.delete(paneId);
}
