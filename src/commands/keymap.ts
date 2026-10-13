/**
 * Default key bindings. `keys` use "Mod" for ⌘ on macOS; on Windows "Mod" becomes Ctrl+Shift so
 * plain Ctrl keys stay with the terminal. Bindings that would collide on Windows set `win`.
 */
import { useEffect } from "react";
import { execute, type Command, type Direction } from "./bus";
import { useWorkspaces } from "@/stores/workspaces";

export const IS_MAC = navigator.userAgent.includes("Mac");

export interface Binding {
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

export const BINDINGS: Binding[] = [
  { keys: "Mod+T", title: "New pane (default agent)", group: "Panes", command: { type: "pane.spawn", provider: "default" } },
  { keys: "Mod+Shift+T", win: "Ctrl+Shift+Alt+T", title: "New pane: pick provider", group: "Panes", command: { type: "ui.palette", query: "New pane: " } },
  ...DIGITS.map(
    (n): Binding => ({
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
  ...DIGITS.map((n): Binding => ({ keys: `Mod+${n}`, title: `Focus pane ${n}`, group: "Panes", command: { type: "pane.focus", target: { index: n } } })),
  ...ARROWS.map(([key, dir]): Binding => ({ keys: `Mod+Alt+${key}`, title: `Focus pane ${dir}`, group: "Panes", repeat: true, command: { type: "pane.focusDir", dir } })),
  ...ARROWS.map(([key, dir]): Binding => ({ keys: `Mod+Shift+Alt+${key}`, win: `Ctrl+Alt+${key}`, title: `Swap pane ${dir}`, group: "Panes", command: { type: "pane.swapDir", dir } })),
  { keys: "Mod+Ctrl+Right", title: "Make pane wider", group: "Layout", repeat: true, command: { type: "pane.resize", target: "focused", axis: "row", delta: RESIZE_STEP } },
  { keys: "Mod+Ctrl+Left", title: "Make pane narrower", group: "Layout", repeat: true, command: { type: "pane.resize", target: "focused", axis: "row", delta: -RESIZE_STEP } },
  { keys: "Mod+Ctrl+Down", title: "Make pane taller", group: "Layout", repeat: true, command: { type: "pane.resize", target: "focused", axis: "col", delta: RESIZE_STEP } },
  { keys: "Mod+Ctrl+Up", title: "Make pane shorter", group: "Layout", repeat: true, command: { type: "pane.resize", target: "focused", axis: "col", delta: -RESIZE_STEP } },
  { keys: "Mod+J", title: "Next agent needing attention", group: "Panes", command: { type: "pane.nextAttention" } },
  { keys: "Mod+Shift+I", win: "Ctrl+Shift+Alt+I", title: "Broadcast to all agents", group: "Panes", command: { type: "ui.overlay", overlay: "broadcast" } },
  { keys: "Mod+Shift+R", win: "Ctrl+Shift+Alt+R", title: "Race mode", group: "Panes", command: { type: "ui.overlay", overlay: "race" } },
  { keys: "Mod+.", title: "Interrupt agent", group: "Panes", command: { type: "pane.interrupt", target: "focused" } },
  { keys: "Mod+F", title: "Search scrollback", group: "Panes", command: { type: "pane.search" } },
  { keys: "Mod+Enter", title: "Zoom pane", group: "Layout", command: { type: "pane.zoom", target: "focused" } },
  { keys: "Mod+Ctrl+=", title: "Equalize panes", group: "Layout", command: { type: "layout.equalize" } },
  { keys: "Mod+Ctrl+L", title: "Next layout preset", group: "Layout", command: { type: "layout.preset", name: "next" } },
  ...DIGITS.map(
    (n): Binding => ({ keys: `Mod+Ctrl+${n}`, win: `Ctrl+Alt+${n}`, title: `Switch to workspace ${n}`, group: "Workspaces", command: { type: "workspace.switch", target: { index: n } } }),
  ),
  { keys: "Mod+K", title: "Command palette", group: "App", command: { type: "ui.palette" } },
  { keys: "Mod+B", title: "Toggle sidebar", group: "App", command: { type: "ui.toggleSidebar" } },
  { keys: "Mod+Shift+B", win: "Ctrl+Shift+Alt+B", title: "Toggle browser", group: "App", command: { type: "ui.toggleBrowser" } },
  { keys: "Mod+L", title: "Browser address bar", group: "App", command: { type: "browser.focusUrl" } },
  { keys: "Mod+,", title: "Settings", group: "App", command: { type: "ui.settings" } },
  { keys: "Mod+/", title: "Keyboard shortcuts", group: "App", command: { type: "ui.cheatSheet" } },
];

const MOD_ORDER = ["Ctrl", "Alt", "Shift", "Meta"];

/** Canonical chord, e.g. "Ctrl+Shift+T", with modifiers in a fixed order. */
function canonical(parts: string[]): string {
  const mods = MOD_ORDER.filter((m) => parts.includes(m));
  const key = parts.filter((p) => !MOD_ORDER.includes(p));
  return [...mods, ...key].join("+");
}

function bindingChord(b: Binding): string {
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
  Comma: ",",
  Period: ".",
  Enter: "Enter",
  NumpadEnter: "Enter",
  Space: "Space",
  Escape: "Escape",
};

/** Uses physical key codes so Shift/Alt don't change the key (Alt+1 is "¡" on macOS). */
function eventChord(e: KeyboardEvent): string | null {
  const key = e.code.startsWith("Key")
    ? e.code.slice(3)
    : e.code.startsWith("Digit")
      ? e.code.slice(5)
      : CODE_KEYS[e.code];
  if (!key) return null;
  const parts = [e.ctrlKey && "Ctrl", e.altKey && "Alt", e.shiftKey && "Shift", e.metaKey && "Meta", key].filter(Boolean) as string[];
  return canonical(parts);
}

const byChord = new Map(BINDINGS.map((b) => [bindingChord(b), b]));

/** Display form: "⌘⌃→" on macOS, "Ctrl+Shift+T" on Windows. */
export function formatKeys(b: Binding): string {
  const chord = bindingChord(b);
  if (!IS_MAC) return chord;
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

/** Installs the global shortcut handler. Runs in capture phase so terminals never see app chords. */
export function useKeymap() {
  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      const chord = eventChord(e);
      const binding = chord && byChord.get(chord);
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
