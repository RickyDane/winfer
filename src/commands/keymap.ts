/**
 * Default key bindings. `keys` use "Mod" for ⌘ on macOS; on Windows "Mod" becomes Ctrl+Shift so
 * plain Ctrl keys stay with the terminal. Bindings that would collide on Windows set `win`.
 *
 * Users can rebind any of them (and the push-to-talk keys): overrides live in the store's
 * `shortcuts`, keyed by binding id, as canonical chords ("Ctrl+Alt+T"); "" switches one off.
 * Numbered families ("Focus pane 1–9") share one override with "#" standing for the digit.
 */
import { useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";
import { execute, type Command, type Direction } from "./bus";
import { useWorkspaces } from "@/stores/workspaces";

export const IS_MAC = navigator.userAgent.includes("Mac");

export interface Binding {
  /** Stable id for user overrides; numbered bindings share their family's id. */
  id: string;
  /** 1–9 for numbered families. */
  digit?: number;
  keys: string;
  win?: string;
  title: string;
  group: "Panes" | "Layout" | "Workspaces" | "App";
  /** A command, or a function for bindings that depend on state (e.g. "provider N"). */
  command: Command | (() => Command | null);
  /** Allow auto-repeat while the key is held (resize, focus movement). */
  repeat?: boolean;
}

const ARROWS: [string, Direction][] = [
  ["Left", "left"],
  ["Right", "right"],
  ["Up", "up"],
  ["Down", "down"],
];
const DIGITS = [1, 2, 3, 4, 5, 6, 7, 8, 9];
const RESIZE_STEP = 0.05;

type BindingSpec = Omit<Binding, "id" | "digit">;

const SPECS: BindingSpec[] = [
  { keys: "Mod+T", title: "New pane (default agent)", group: "Panes", command: { type: "pane.spawn", provider: "default" } },
  { keys: "Mod+Shift+T", win: "Ctrl+Shift+Alt+T", title: "New pane: pick provider", group: "Panes", command: { type: "ui.palette", query: "New pane: " } },
  ...DIGITS.map(
    (n): BindingSpec => ({
      keys: `Mod+Alt+${n}`,
      title: `New pane with provider ${n}`,
      group: "Panes",
      command: () => {
        const provider = useWorkspaces.getState().providers.filter((p) => p.installed)[n - 1];
        return provider ? { type: "pane.spawn", provider: provider.id } : null;
      },
    }),
  ),
  { keys: "Mod+D", title: "Split right", group: "Panes", command: { type: "pane.split", dir: "row" } },
  { keys: "Mod+Shift+D", win: "Ctrl+Shift+Alt+D", title: "Split down", group: "Panes", command: { type: "pane.split", dir: "col" } },
  { keys: "Mod+W", title: "Close pane", group: "Panes", command: { type: "pane.close", target: "focused" } },
  ...DIGITS.map((n): BindingSpec => ({ keys: `Mod+${n}`, title: `Focus pane ${n}`, group: "Panes", command: { type: "pane.focus", target: { index: n } } })),
  ...ARROWS.map(([key, dir]): BindingSpec => ({ keys: `Mod+Alt+${key}`, title: `Focus pane ${dir}`, group: "Panes", repeat: true, command: { type: "pane.focusDir", dir } })),
  ...ARROWS.map(([key, dir]): BindingSpec => ({ keys: `Mod+Shift+Alt+${key}`, win: `Ctrl+Alt+${key}`, title: `Swap pane ${dir}`, group: "Panes", command: { type: "pane.swapDir", dir } })),
  { keys: "Mod+Ctrl+Right", title: "Make pane wider", group: "Layout", repeat: true, command: { type: "pane.resize", target: "focused", axis: "row", delta: RESIZE_STEP } },
  { keys: "Mod+Ctrl+Left", title: "Make pane narrower", group: "Layout", repeat: true, command: { type: "pane.resize", target: "focused", axis: "row", delta: -RESIZE_STEP } },
  { keys: "Mod+Ctrl+Down", title: "Make pane taller", group: "Layout", repeat: true, command: { type: "pane.resize", target: "focused", axis: "col", delta: RESIZE_STEP } },
  { keys: "Mod+Ctrl+Up", title: "Make pane shorter", group: "Layout", repeat: true, command: { type: "pane.resize", target: "focused", axis: "col", delta: -RESIZE_STEP } },
  { keys: "Mod+Shift+O", win: "Ctrl+Shift+Alt+O", title: "Overview of all workspaces", group: "Workspaces", command: { type: "ui.view", view: "toggle" } },
  { keys: "Mod+O", title: "Session progress", group: "Panes", command: { type: "ui.overlay", overlay: "progress" } },
  { keys: "Mod+E", title: "Review finished work", group: "Panes", command: { type: "ui.overlay", overlay: "review" } },
  { keys: "Mod+J", title: "Next agent needing attention", group: "Panes", command: { type: "pane.nextAttention" } },
  { keys: "Mod+Shift+I", win: "Ctrl+Shift+Alt+I", title: "Broadcast to all agents", group: "Panes", command: { type: "ui.overlay", overlay: "broadcast" } },
  { keys: "Mod+Shift+R", win: "Ctrl+Shift+Alt+R", title: "Race mode", group: "Panes", command: { type: "ui.overlay", overlay: "race" } },
  { keys: "Mod+Shift+.", win: "Ctrl+Shift+Alt+.", title: "Stop all agents", group: "Panes", command: { type: "pane.interruptAll" } },
  { keys: "Mod+.", title: "Interrupt agent", group: "Panes", command: { type: "pane.interrupt", target: "focused" } },
  { keys: "Mod+F", title: "Search scrollback", group: "Panes", command: { type: "pane.search" } },
  { keys: "Mod+Enter", title: "Zoom pane", group: "Layout", command: { type: "pane.zoom", target: "focused" } },
  { keys: "Mod+Ctrl+=", title: "Equalize panes", group: "Layout", command: { type: "layout.equalize" } },
  { keys: "Mod+Ctrl+L", title: "Next layout preset", group: "Layout", command: { type: "layout.preset", name: "next" } },
  ...DIGITS.map(
    (n): BindingSpec => ({ keys: `Mod+Ctrl+${n}`, win: `Ctrl+Alt+${n}`, title: `Switch to workspace ${n}`, group: "Workspaces", command: { type: "workspace.switch", target: { index: n } } }),
  ),
  { keys: "Mod+K", title: "Command palette", group: "App", command: { type: "ui.palette" } },
  { keys: "Mod+B", title: "Toggle sidebar", group: "App", command: { type: "ui.toggleSidebar" } },
  { keys: "Mod+Shift+B", win: "Ctrl+Shift+Alt+B", title: "Toggle browser", group: "App", command: { type: "ui.toggleBrowser" } },
  { keys: "Mod+Shift+C", win: "Ctrl+Shift+Alt+C", title: "Pick element for agent", group: "App", command: { type: "browser.pick" } },
  { keys: "Mod+L", title: "Browser address bar", group: "App", command: { type: "browser.focusUrl" } },
  { keys: "Mod+,", title: "Settings", group: "App", command: { type: "ui.settings" } },
  { keys: "Mod+/", title: "Keyboard shortcuts", group: "App", command: { type: "ui.cheatSheet" } },
];

const slug = (text: string) => text.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-|-$/g, "");

export const BINDINGS: Binding[] = SPECS.map((spec) => {
  const numbered = /^(.*) (\d)$/.exec(spec.title);
  return numbered ? { ...spec, id: slug(numbered[1]), digit: Number(numbered[2]) } : { ...spec, id: slug(spec.title) };
});

const MOD_ORDER = ["Ctrl", "Alt", "Shift", "Meta"];

/** Canonical chord, e.g. "Ctrl+Shift+T", with modifiers in a fixed order. */
function canonical(parts: string[]): string {
  const mods = MOD_ORDER.filter((m) => parts.includes(m));
  const key = parts.filter((p) => !MOD_ORDER.includes(p));
  return [...mods, ...key].join("+");
}

/** The built-in chord for a binding on this platform. */
function defaultChord(b: Binding): string {
  const spec = !IS_MAC && b.win ? b.win : b.keys;
  const parts = spec.split("+").flatMap((p) => (p === "Mod" ? (IS_MAC ? ["Meta"] : ["Ctrl", "Shift"]) : [p]));
  return canonical([...new Set(parts)]);
}

const CODE_KEYS: Record<string, string> = {
  ArrowLeft: "Left",
  ArrowRight: "Right",
  ArrowUp: "Up",
  ArrowDown: "Down",
  Equal: "=",
  Minus: "-",
  Slash: "/",
  Backslash: "\\",
  Comma: ",",
  Period: ".",
  Semicolon: ";",
  Quote: "'",
  Backquote: "`",
  BracketLeft: "[",
  BracketRight: "]",
  Enter: "Enter",
  NumpadEnter: "Enter",
  Space: "Space",
  Escape: "Escape",
  Tab: "Tab",
  Backspace: "Backspace",
  Delete: "Delete",
  Home: "Home",
  End: "End",
  PageUp: "PageUp",
  PageDown: "PageDown",
};

/** Uses physical key codes so Shift/Alt don't change the key (Alt+1 is "¡" on macOS). */
export function eventChord(e: KeyboardEvent): string | null {
  const key = e.code.startsWith("Key")
    ? e.code.slice(3)
    : e.code.startsWith("Digit")
      ? e.code.slice(5)
      : /^F\d{1,2}$/.test(e.code)
        ? e.code
        : CODE_KEYS[e.code];
  if (!key) return null;
  const parts = [e.ctrlKey && "Ctrl", e.altKey && "Alt", e.shiftKey && "Shift", e.metaKey && "Meta", key].filter(Boolean) as string[];
  return canonical(parts);
}

export type Overrides = Record<string, string>;

/** The chord a binding currently uses (user override or default); null when switched off. */
export function chordFor(b: Binding, overrides: Overrides = useWorkspaces.getState().shortcuts): string | null {
  const override = overrides[b.id];
  if (override === undefined) return defaultChord(b);
  if (override === "") return null;
  return b.digit ? override.replace("#", String(b.digit)) : override;
}

/** The chord shown and recorded for a binding's row: families use "#" for the digit. */
export function rowChord(b: Binding, overrides?: Overrides): string | null {
  const chord = chordFor(b, overrides);
  return chord && b.digit ? chord.replace(/\d$/, "#") : chord;
}

/** True when `id` has a user override. */
export const isCustomized = (id: string, overrides: Overrides = useWorkspaces.getState().shortcuts) => overrides[id] !== undefined;

let chordCache: { overrides: Overrides; map: Map<string, Binding> } | undefined;
function byChord(): Map<string, Binding> {
  const overrides = useWorkspaces.getState().shortcuts;
  if (chordCache?.overrides !== overrides) {
    const map = new Map<string, Binding>();
    for (const b of BINDINGS) {
      const chord = chordFor(b, overrides);
      if (chord && !map.has(chord)) map.set(chord, b);
    }
    chordCache = { overrides, map };
  }
  return chordCache.map;
}

/** True if this key event is one of the app's shortcuts (so terminals and pages shouldn't get it). */
export function isAppChord(e: KeyboardEvent): boolean {
  const chord = eventChord(e);
  return chord !== null && byChord().has(chord);
}

/** Display form: "⌘⌃→" on macOS, "Ctrl+Shift+T" on Windows; "" when switched off. */
export function formatKeys(b: Binding): string {
  return formatChord(chordFor(b));
}

/** Formats a canonical chord ("Meta+Shift+T") for this platform; "#" shows as "1–9". */
export function formatChord(chord: string | null): string {
  if (!chord) return "";
  chord = chord.replace("#", "1–9");
  if (!IS_MAC) return chord.replace("Meta", "Win");
  const symbols: Record<string, string> = { Ctrl: "⌃", Alt: "⌥", Shift: "⇧", Meta: "⌘", Left: "←", Right: "→", Up: "↑", Down: "↓", Enter: "↩", Space: "Space" };
  // macOS convention orders modifiers ⌃⌥⇧⌘.
  return chord
    .split("+")
    .map((p) => symbols[p] ?? p)
    .join("");
}

/** Finds the binding for a command (for palette hints). */
export function bindingFor(cmd: Command): Binding | undefined {
  const json = JSON.stringify(cmd);
  return BINDINGS.find((b) => typeof b.command !== "function" && JSON.stringify(b.command) === json);
}

/** Shortcut label for a command in this platform's notation ("⌘T" / "Ctrl+Shift+T"), or "". */
export function keysFor(cmd: Command): string {
  const binding = bindingFor(cmd);
  return binding ? formatKeys(binding) : "";
}

/** Global push-to-talk keys (registered in src-tauri/src/voice/mod.rs), as override ids. */
export const VOICE_BINDINGS = [
  { id: "voice-command", title: "Hold to give a voice command", default: "Alt+Space" },
  { id: "voice-dictation", title: "Hold to dictate into the focused pane", default: "Alt+Shift+Space" },
] as const;

export function voiceChord(id: (typeof VOICE_BINDINGS)[number]["id"], overrides: Overrides = useWorkspaces.getState().shortcuts): string | null {
  const override = overrides[id];
  if (override === "") return null;
  return override ?? VOICE_BINDINGS.find((v) => v.id === id)!.default;
}

/** Push-to-talk keys for labels, e.g. { command: "⌥Space", dictation: "⌥⇧Space" }. */
export function voiceKeys(overrides?: Overrides) {
  return {
    command: formatChord(voiceChord("voice-command", overrides)) || "off",
    dictation: formatChord(voiceChord("voice-dictation", overrides)) || "off",
  };
}

/** Sends the push-to-talk keys to the backend (at startup and whenever they change). */
export function applyVoiceShortcuts(overrides: Overrides = useWorkspaces.getState().shortcuts) {
  return invoke("voice_set_shortcuts", { command: voiceChord("voice-command", overrides) ?? "", dictation: voiceChord("voice-dictation", overrides) ?? "" });
}

/** Other shortcuts (and push-to-talk keys) that already use `chord` ("#" = any digit 1–9). */
export function conflicts(id: string, chord: string, overrides: Overrides = useWorkspaces.getState().shortcuts): { id: string; title: string }[] {
  const expand = (c: string) => (c.includes("#") ? DIGITS.map((n) => c.replace("#", String(n))) : [c]);
  const wanted = new Set(expand(chord));
  const found = new Map<string, string>();
  for (const b of BINDINGS) {
    const own = chordFor(b, overrides);
    if (b.id !== id && own && wanted.has(own)) found.set(b.id, b.digit ? b.title.replace(/\d$/, "1–9") : b.title);
  }
  for (const v of VOICE_BINDINGS) {
    const own = voiceChord(v.id, overrides);
    if (v.id !== id && own && wanted.has(own)) found.set(v.id, v.title);
  }
  return [...found].map(([id, title]) => ({ id, title }));
}

let recording = false;
/** Pauses the keymap while the shortcut editor records a new chord. */
export function setRecording(on: boolean) {
  recording = on;
}

/** Installs the global shortcut handler. Runs in capture phase so terminals never see app chords. */
export function useKeymap() {
  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if (recording) return;
      const chord = eventChord(e);
      const binding = chord && byChord().get(chord);
      if (!binding) return;
      e.preventDefault();
      e.stopPropagation();
      if (e.repeat && !binding.repeat) return;
      const cmd = typeof binding.command === "function" ? binding.command() : binding.command;
      if (cmd) execute(cmd);
    };
    window.addEventListener("keydown", onKeyDown, { capture: true });
    return () => window.removeEventListener("keydown", onKeyDown, { capture: true });
  }, []);
}
