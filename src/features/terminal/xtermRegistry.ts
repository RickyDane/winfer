/**
 * Owns every xterm instance outside React. Panes attach/detach the terminal's host element
 * to their slot, so layout changes and workspace switches never recreate a terminal (and the
 * agent behind it keeps running). Each terminal spawns its PTY once, on first attach.
 */
import { Channel, invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { FitAddon } from "@xterm/addon-fit";
import { SearchAddon } from "@xterm/addon-search";
import { WebLinksAddon } from "@xterm/addon-web-links";
import { WebglAddon } from "@xterm/addon-webgl";
import { Terminal } from "@xterm/xterm";
import "@xterm/xterm/css/xterm.css";
import { create as createStore } from "zustand";
import { cleanAgentTitle, hiddenToolsOf, systemPromptOf, useWorkspaces, type Pane } from "@/stores/workspaces";
import { reportInput, reportOutput, reportStatus, reportTerminalNotice } from "@/features/status/agentStatus";
import { createWorktree } from "@/features/worktree/worktree";
import { isAppChord } from "@/commands/keymap";
import { setTask } from "@/features/progress/progress";
import { terminalMinContrast, terminalTheme } from "@/features/theme/themes";
import { optionTypedChar } from "./optionKey";
import { fileLinkProvider, openTerminalUri, openWebLink } from "@/features/viewer/terminalLinks";

const IS_MAC = navigator.userAgent.includes("Mac");
/** Glyph-atlas pages a terminal may grow before the WebGL renderer's atlas is cleared. */
const MAX_ATLAS_PAGES = 4;

// Recolor every live terminal when the app theme changes; refit them all when zoomed.
useWorkspaces.subscribe((s, prev) => {
  if (s.theme !== prev.theme) {
    for (const entry of entries.values()) {
      entry.term.options.theme = terminalTheme(s.theme);
      entry.term.options.minimumContrastRatio = terminalMinContrast(s.theme);
    }
  }
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
  /** Undefined until pty_spawn returns; null where the OS gives no pid. */
  pid?: number | null;
  /** Exits that arrived before pty_spawn returned, settled once the pid is known. */
  earlyExits?: ExitPayload[];
  lastOutput: number;
  decoder: TextDecoder;
  observer?: ResizeObserver;
  webgl?: WebglAddon;
  frame?: number;
  resizeTimer?: number;
}

interface ExitPayload {
  paneId: string;
  code: number;
  pid: number | null;
}

const entries = new Map<string, Entry>();

let started = false;

/** Subscribes to PTY exit events. Called once at startup. */
export function startTerminalEvents() {
  if (started) return;
  started = true;
  void listen<ExitPayload>("pty://exit", ({ payload }) => {
    const entry = entries.get(payload.paneId);
    if (!entry) return;
    // A restarted pane's old process often exits while the new one is still spawning:
    // hold the exit until the new pid is known, then decide whose it was.
    if (entry.pid === undefined) (entry.earlyExits ??= []).push(payload);
    else onExit(entry, payload);
  });
}

function onExit(entry: Entry, { code, pid }: ExitPayload) {
  // Not this entry's process (the one a restart killed).
  if (pid != null && entry.pid != null && pid !== entry.pid) return;
  entry.exited = true;
  entry.term.write(`\r\n\x1b[2m[process exited with code ${code}]\x1b[0m\r\n`);
  reportStatus(entry.pane.id, code === 0 ? "done" : "error", `exited with code ${code}`);
}

/** The pane's current folder (its worktree once it has one), where the paths it prints are relative to. */
function cwdOf(id: string, fallback: string) {
  return useWorkspaces.getState().workspaces.flatMap((w) => w.panes).find((p) => p.id === id)?.cwd ?? fallback;
}

/** The pane's workspace folder: a second place to look for the relative paths it prints. */
function rootsOf(id: string) {
  const ws = useWorkspaces.getState().workspaces.find((w) => w.panes.some((p) => p.id === id));
  return ws?.rootDir ? [ws.rootDir] : [];
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
    theme: terminalTheme(useWorkspaces.getState().theme),
    minimumContrastRatio: terminalMinContrast(useWorkspaces.getState().theme),
    // OSC 8 hyperlinks: files open in the viewer, web links in winfer's browser.
    linkHandler: {
      activate: (event, uri) => openTerminalUri(event, uri, cwdOf(pane.id, pane.cwd)),
      allowNonHttpProtocols: true,
    },
  });
  const fit = new FitAddon();
  term.loadAddon(fit);
  const search = new SearchAddon();
  term.loadAddon(search);
  search.onDidChangeResults(({ resultIndex, resultCount }) => {
    const entry = entries.get(pane.id);
    if (entry) entry.searchResults = { index: resultIndex, count: resultCount };
  });
  term.loadAddon(new WebLinksAddon((event, uri) => openWebLink(event, uri)));
  term.registerLinkProvider(fileLinkProvider(term, () => cwdOf(pane.id, pane.cwd), () => rootsOf(pane.id)));

  // App shortcuts (⌘… on macOS, Ctrl+Shift+… on Windows, plus any the user assigned) must reach
  // the app, not the PTY. Returning false skips xterm only; the browser still fires copy/paste events.
  term.attachCustomKeyEventHandler((e) => {
    if ((IS_MAC ? e.metaKey : e.ctrlKey && e.shiftKey) || isAppChord(e)) return false;
    // Option is Meta in panes, except for the characters a keyboard layout types with it
    // (Option+L is "@" on German layouts): type those.
    const typed = IS_MAC ? optionTypedChar(e) : null;
    if (typed) {
      e.preventDefault();
      term.input(typed, true);
      return false;
    }
    return true;
  });

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
    reportTerminalNotice(pane.id, data);
    return true;
  });
  term.parser.registerOscHandler(777, (data) => {
    const [kind, title, body] = data.split(";");
    if (kind === "notify") reportTerminalNotice(pane.id, [title, body].filter(Boolean).join(": "));
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
  term.onBell(() => reportTerminalNotice(pane.id));

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
  // xterm only brackets a paste when the program enabled bracketed paste mode (it can be lost,
  // e.g. after a reload, or never set); otherwise every line would be submitted separately.
  // Agent prompts always get multi-line pastes as one bracketed block.
  host.addEventListener(
    "paste",
    (e) => {
      const text = e.clipboardData?.getData("text/plain");
      if (!text || !/[\r\n]/.test(text.trimEnd()) || term.modes.bracketedPasteMode || pane.provider === "shell") return;
      e.preventDefault();
      e.stopPropagation();
      pasteText(pane.id, text.replace(/\x1b\[20[01]~/g, "").replace(/\r?\n/g, "\r"));
    },
    { capture: true },
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

/**
 * Gives an agent pane its own git worktree before it starts (if the folder is a repo).
 * Race and A/B panes must never share a folder, so for them a missing worktree is fatal:
 * returns false and the agent isn't started.
 */
async function prepareWorktree(entry: Entry): Promise<boolean> {
  const pane = entry.pane;
  if (!pane.wantsWorktree || pane.worktree) return true;
  const required = !!pane.raceId;
  try {
    const worktree = await createWorktree(pane.cwd, pane.provider);
    const patch = worktree ? { worktree, cwd: worktree.path, wantsWorktree: false } : { wantsWorktree: false };
    useWorkspaces.getState().updatePane(pane.id, patch);
    entry.pane = { ...pane, ...patch };
    if (worktree) {
      entry.term.write(`\x1b[2mworktree ${worktree.branch} (from ${worktree.base}) at ${worktree.path}\x1b[0m\r\n`);
      return true;
    }
    if (required) entry.term.write(`\x1b[31m${pane.cwd} isn't a git repository. Races and A/B tests need a worktree per agent, so this agent wasn't started.\x1b[0m\r\n`);
    return !required;
  } catch (err) {
    useWorkspaces.getState().updatePane(pane.id, { wantsWorktree: false });
    entry.term.write(
      required
        ? `\x1b[31mCouldn't create a worktree, so this agent wasn't started (races and A/B tests need one per agent): ${err}\x1b[0m\r\n`
        : `\x1b[33mCouldn't create a worktree, starting in ${pane.cwd}: ${err}\x1b[0m\r\n`,
    );
    return !required;
  }
}

const paneStatus = (id: string) =>
  useWorkspaces.getState().workspaces.flatMap((w) => w.panes).find((p) => p.id === id)?.status;

async function spawn(entry: Entry) {
  entry.spawned = true;
  const id = entry.pane.id;
  setStarting(id, entry.pane.wantsWorktree && !entry.pane.worktree ? "worktree" : "process");
  // Some programs stay silent until they get input; don't leave the overlay up forever.
  const giveUp = window.setTimeout(() => setStarting(id, null), 15_000);
  if (!(await prepareWorktree(entry))) {
    clearTimeout(giveUp);
    setStarting(id, null);
    entry.exited = true;
    useWorkspaces.getState().setPaneStatus(id, "error");
    return;
  }
  if (entries.get(id) === entry) setStarting(id, "process");
  const statusBefore = paneStatus(id);
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
    const { pid } = await invoke<{ pid: number | null }>("pty_spawn", {
      req: {
        paneId: entry.pane.id,
        providerId: entry.pane.provider,
        cwd: entry.pane.cwd,
        workspaceId: entry.pane.workspaceId,
        resume: entry.pane.resume,
        session: entry.pane.resume ? entry.pane.sessionId : null,
        prompt: entry.pane.prompt,
        model: entry.pane.model,
        autoApprove: useWorkspaces.getState().autoApprove.includes(entry.pane.provider),
        systemPrompt: systemPromptOf(useWorkspaces.getState()),
        plain: entry.pane.plain,
        lean: entry.pane.provider === "codex" && useWorkspaces.getState().leanCodex,
        hiddenTools: hiddenToolsOf(useWorkspaces.getState()),
        cols: entry.term.cols,
        rows: entry.term.rows,
      },
      onData: channel,
    });
    entry.pid = pid;
    // Only a fresh agent given a start prompt is busy right away. A resumed or promptless one
    // waits for input, and agents without a session-start hook (Codex, OpenCode) would otherwise
    // stay "working" forever. A status a hook reported while spawning wins.
    const busy = entry.pane.provider !== "shell" && !entry.pane.resume && !!entry.pane.prompt?.trim();
    if (paneStatus(id) === statusBefore) useWorkspaces.getState().setPaneStatus(id, busy ? "working" : "idle");
    entry.earlyExits?.forEach((exit) => onExit(entry, exit));
    entry.earlyExits = undefined;
  } catch (err) {
    clearTimeout(giveUp);
    setStarting(id, null);
    entry.pid = null;
    entry.earlyExits = undefined;
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

/**
 * Live WebGL addons. Terminals with the same font share one glyph atlas, but an addon's
 * clearTextureAtlas() only redraws its own terminal: the others keep pointing at atlas slots
 * that now hold different glyphs and show garbage until something forces a full redraw (a
 * resize). So the atlas is always cleared through every addon at once.
 */
const webglAddons = new Set<WebglAddon>();
/** Atlas page canvases (each addon reports the shared ones, so a set dedupes them). */
const atlasPages = new Set<HTMLCanvasElement>();
let atlasClearQueued = false;

function onAtlasPageAdded(canvas: HTMLCanvasElement) {
  atlasPages.add(canvas);
  if (atlasPages.size <= MAX_ATLAS_PAGES || atlasClearQueued) return;
  atlasClearQueued = true;
  setTimeout(clearAllTextureAtlases, 0);
}

function onAtlasPageRemoved(canvas: HTMLCanvasElement) {
  atlasPages.delete(canvas);
}

function clearAllTextureAtlases() {
  atlasClearQueued = false;
  atlasPages.clear();
  for (const webgl of webglAddons) webgl.clearTextureAtlas();
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
      webgl.onContextLoss(() => {
        webglAddons.delete(webgl);
        webgl.dispose();
      });
      // The glyph atlas grows a page whenever a program paints glyphs in colors it hasn't
      // cached yet; TUIs that animate in truecolor can add pages endlessly, and past a few,
      // xterm's WebGL renderer starts drawing garbled glyphs (and slows down). Start the atlas
      // over before that. Every addon forwards the shared atlas's events, so count them once.
      webgl.onAddTextureAtlasCanvas(onAtlasPageAdded);
      webgl.onRemoveTextureAtlasCanvas(onAtlasPageRemoved);
      webglAddons.add(webgl);
      entry.webgl = webgl;
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

/**
 * Restarts a pane's process in place (e.g. with a different model), resuming its session:
 * kills the old process group, then starts a fresh terminal and focuses it.
 */
export async function restartTerminal(pane: Pane) {
  const slot = entries.get(pane.id)?.host.parentElement ?? null;
  // Wait for the kill so the new process isn't spawned next to (or replaced by) the old one.
  await closeTerminal(pane.id);
  if (slot) {
    attachTerminal(pane, slot);
    focusTerminal(pane.id);
  } else startDetached(pane);
}

/** Id of a workspace's side-panel shell (it isn't one of the workspace's panes). */
export const sideTerminalId = (workspaceId: string) => `side-terminal-${workspaceId}`;

/** Kills the PTY and disposes the terminal. */
export function closeTerminal(paneId: string): Promise<void> {
  const entry = entries.get(paneId);
  const killed = invoke<void>("pty_kill", { paneId }).catch(() => {});
  setStarting(paneId, null);
  if (!entry) return killed;
  entry.observer?.disconnect();
  if (entry.webgl) webglAddons.delete(entry.webgl);
  entry.term.dispose();
  entry.host.remove();
  entries.delete(paneId);
  return killed;
}
