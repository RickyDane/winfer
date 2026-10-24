import { create } from "zustand";
import { persist } from "zustand/middleware";

/** Your messages: a right-aligned accent bubble, a full-width prompt band, or the plain rule. */
export type UserStyle = "bubble" | "band" | "rule";

/** How winfer code's web UI looks and what it shows (Settings → winfer code). */
export interface CodeWebPrefs {
  /** winfer code panes show the web UI (`winfer code --serve`) instead of the terminal TUI. */
  web: boolean;
  /** The session's cost in the status line (when the backend reports one). */
  showCost: boolean;
  /** Tokens per second in the status line while a turn streams. */
  showRate: boolean;
  /** The context meter in the status line. */
  showContext: boolean;
  /** The plan (todos) and running subagents above the composer. */
  showPlan: boolean;
  /** Reasoning stays open once the model finished thinking. */
  expandThinking: boolean;
  /** Shell output is open while the command runs. */
  expandShell: boolean;
  /** Tighter spacing in the transcript. */
  compact: boolean;
  /** How your own messages stand out in the transcript. */
  userStyle: UserStyle;
  /** Text size multiplier (1 = default); only text scales, not spacing. */
  textScale: number;
  /** Models starred in the model picker (`backend/model` specs), in the order they were added. */
  favoriteModels: string[];
  /** What model aliases last resolved to (`claude/opus` → `claude-opus-5-5`), for versioned names. */
  resolvedModels: Record<string, string>;
}

interface CodeWebSettings extends CodeWebPrefs {
  set: (patch: Partial<CodeWebPrefs>) => void;
  /** @deprecated use `set({ web })` */
  setWeb: (on: boolean) => void;
}

export const DEFAULT_PREFS: CodeWebPrefs = {
  web: false,
  showCost: true,
  showRate: true,
  showContext: true,
  showPlan: true,
  expandThinking: false,
  expandShell: false,
  compact: false,
  userStyle: "bubble",
  textScale: 1,
  favoriteModels: [],
  resolvedModels: {},
};

export const MIN_TEXT_SCALE = 0.8;
export const MAX_TEXT_SCALE = 1.5;
export const TEXT_SCALE_STEP = 0.05;

/** Sets the web UI's text scale, clamped and rounded to TEXT_SCALE_STEP. */
export function setTextScale(v: number) {
  const clamped = Math.min(MAX_TEXT_SCALE, Math.max(MIN_TEXT_SCALE, v));
  useCodeWeb.getState().set({ textScale: Math.round(clamped / TEXT_SCALE_STEP) / Math.round(1 / TEXT_SCALE_STEP) });
}

/** Steps the text scale by `delta` steps; 0 resets it. */
export function stepTextScale(delta: number) {
  setTextScale(delta === 0 ? 1 : useCodeWeb.getState().textScale + delta * TEXT_SCALE_STEP);
}

export const useCodeWeb = create<CodeWebSettings>()(
  persist(
    (set) => ({ ...DEFAULT_PREFS, set: (patch) => set(patch), setWeb: (web) => set({ web }) }),
    {
      name: "winfer-code-web",
      // v1: the TUI became the default (and thinking collapsed); reset both once for existing installs.
      // v2: shell cards start collapsed; reset expandShell once.
      version: 2,
      migrate: (persisted, version) => {
        let prefs = (persisted ?? {}) as Partial<CodeWebPrefs>;
        if (version < 1) prefs = { ...prefs, web: false, expandThinking: false };
        if (version < 2) prefs = { ...prefs, expandShell: false };
        return prefs;
      },
      partialize: ({ set: _s, setWeb: _w, ...prefs }) => prefs,
    },
  ),
);

/** The `?codeweb` browser preview: web UI panes driven by a scripted session. */
export const CODE_WEB_DEMO = typeof location !== "undefined" && new URLSearchParams(location.search).has("codeweb");

/** Whether winfer code panes show the web UI instead of the terminal TUI. */
export const webCodeEnabled = () => CODE_WEB_DEMO || useCodeWeb.getState().web;

const chosen = new Map<string, boolean>();
/** Decided once per pane, so flipping the setting doesn't swap a running session's process. */
export function paneUsesWeb(paneId: string) {
  if (!chosen.has(paneId)) chosen.set(paneId, webCodeEnabled());
  return chosen.get(paneId)!;
}
