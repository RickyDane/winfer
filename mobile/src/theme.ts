import { applyNebulaVars } from "./nebula";
import type { Theme } from "./protocol";

/** themes.json keys → the CSS variables the PWA paints with. */
const VARS: Record<string, string> = {
  app: "--app",
  panel: "--panel",
  panelRaised: "--raised",
  hover: "--hover",
  active: "--active",
  border: "--border",
  borderStrong: "--border-strong",
  fg: "--fg",
  fgMuted: "--muted",
  fgSubtle: "--subtle",
  accent: "--accent",
  onAccent: "--on-accent",
  success: "--ok",
  warning: "--warn",
  danger: "--danger",
  claude: "--claude",
};

const KEY = "winfer.theme";

/** Wears the desktop's theme, and remembers it for the next launch. */
export function applyTheme(theme: Theme, save = true) {
  const root = document.documentElement;
  for (const [k, v] of Object.entries(VARS)) {
    const value = theme.ui[k];
    if (value) root.style.setProperty(v, value);
  }
  const light = theme.mode === "light";
  root.dataset.mode = light ? "light" : "dark";
  root.style.colorScheme = light ? "light" : "dark";
  applyNebulaVars(light);
  document.querySelector('meta[name="theme-color"]')?.setAttribute("content", theme.ui.panel ?? "#111113");
  if (save) {
    try {
      localStorage.setItem(KEY, JSON.stringify(theme));
    } catch {
      /* private mode */
    }
  }
}

export function restoreTheme() {
  try {
    const saved = localStorage.getItem(KEY);
    if (saved) {
      applyTheme(JSON.parse(saved) as Theme, false);
      return;
    }
  } catch {
    /* a broken entry: keep the default */
  }
  applyNebulaVars(false);
}
