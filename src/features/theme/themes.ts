/**
 * Color themes. Each theme sets the app's design tokens (the --color-* variables from
 * globals.css) and the terminal palette, so panes and chrome always match.
 */
import type { ITheme } from "@xterm/xterm";

export type ThemeMode = "dark" | "light";

interface Ui {
  app: string;
  panel: string;
  panelRaised: string;
  hover: string;
  active: string;
  border: string;
  borderStrong: string;
  fg: string;
  fgMuted: string;
  fgSubtle: string;
  accent: string;
  /** Text on a filled accent button. */
  onAccent: string;
  success: string;
  warning: string;
  danger: string;
  claude: string;
}

/** black, red, green, yellow, blue, magenta, cyan, white, then the bright variants. */
type Ansi = [string, string, string, string, string, string, string, string, string, string, string, string, string, string, string, string];

export interface ThemeDef {
  id: string;
  name: string;
  mode: ThemeMode;
  /** The theme the sidebar's sun/moon button switches to. */
  pair: string;
  ui: Ui;
  ansi: Ansi;
}

const def = (id: string, name: string, mode: ThemeMode, pair: string, ui: Ui, ansi: Ansi): ThemeDef => ({ id, name, mode, pair, ui, ansi });

export const THEMES: ThemeDef[] = [
  def(
    "dark", "Winfer Dark", "dark", "light",
    { app: "#0b0b0d", panel: "#121215", panelRaised: "#17171b", hover: "#1d1d22", active: "#24242a", border: "#26262c", borderStrong: "#33333b", fg: "#e6e6ea", fgMuted: "#9a9aa3", fgSubtle: "#6b6b74", accent: "#6d7cff", onAccent: "#ffffff", success: "#4ade80", warning: "#f5b544", danger: "#f26d6d", claude: "#d97757" },
    ["#1d1d22", "#f26d6d", "#4ade80", "#f5b544", "#6d9cff", "#c08cff", "#5fd4e0", "#d4d4da", "#6b6b74", "#ff8a8a", "#7cf0a4", "#ffd07a", "#94b6ff", "#d6b0ff", "#8ce6ee", "#ffffff"],
  ),
  def(
    "light", "Winfer Light", "light", "dark",
    { app: "#e9e9ee", panel: "#fbfbfc", panelRaised: "#ffffff", hover: "#ececf1", active: "#e2e2e9", border: "#dcdce3", borderStrong: "#c9c9d2", fg: "#1b1b21", fgMuted: "#55555f", fgSubtle: "#85858f", accent: "#4f5ff0", onAccent: "#ffffff", success: "#16a34a", warning: "#b7791f", danger: "#dc3b3b", claude: "#c2603f" },
    // Same hues as Winfer Dark, darkened so agent output stays readable on white.
    ["#1b1b21", "#c62f2f", "#1f8a46", "#9a6a00", "#2f5fd0", "#8a3fc0", "#0f7f8c", "#d4d4da", "#6b6b74", "#e04848", "#2aa35a", "#b88400", "#4a78e8", "#a55ad8", "#1897a6", "#ffffff"],
  ),
  def(
    "catppuccin-mocha", "Catppuccin Mocha", "dark", "catppuccin-latte",
    { app: "#11111b", panel: "#1e1e2e", panelRaised: "#232334", hover: "#2a2b3c", active: "#313244", border: "#313244", borderStrong: "#45475a", fg: "#cdd6f4", fgMuted: "#a6adc8", fgSubtle: "#7f849c", accent: "#cba6f7", onAccent: "#11111b", success: "#a6e3a1", warning: "#f9e2af", danger: "#f38ba8", claude: "#fab387" },
    ["#45475a", "#f38ba8", "#a6e3a1", "#f9e2af", "#89b4fa", "#f5c2e7", "#94e2d5", "#bac2de", "#585b70", "#f38ba8", "#a6e3a1", "#f9e2af", "#89b4fa", "#f5c2e7", "#94e2d5", "#a6adc8"],
  ),
  def(
    "catppuccin-latte", "Catppuccin Latte", "light", "catppuccin-mocha",
    { app: "#dce0e8", panel: "#eff1f5", panelRaised: "#f5f6f9", hover: "#e6e9ef", active: "#dce0e8", border: "#ccd0da", borderStrong: "#bcc0cc", fg: "#4c4f69", fgMuted: "#6c6f85", fgSubtle: "#8c8fa1", accent: "#8839ef", onAccent: "#eff1f5", success: "#40a02b", warning: "#df8e1d", danger: "#d20f39", claude: "#fe640b" },
    ["#5c5f77", "#d20f39", "#40a02b", "#df8e1d", "#1e66f5", "#ea76cb", "#179299", "#acb0be", "#6c6f85", "#d20f39", "#40a02b", "#df8e1d", "#1e66f5", "#ea76cb", "#179299", "#bcc0cc"],
  ),
  def(
    "tokyo-night", "Tokyo Night", "dark", "tokyo-night-day",
    { app: "#16161e", panel: "#1a1b26", panelRaised: "#1f2030", hover: "#24283b", active: "#292e42", border: "#292e42", borderStrong: "#3b4261", fg: "#c0caf5", fgMuted: "#a9b1d6", fgSubtle: "#737aa2", accent: "#7aa2f7", onAccent: "#16161e", success: "#9ece6a", warning: "#e0af68", danger: "#f7768e", claude: "#ff9e64" },
    ["#15161e", "#f7768e", "#9ece6a", "#e0af68", "#7aa2f7", "#bb9af7", "#7dcfff", "#a9b1d6", "#414868", "#f7768e", "#9ece6a", "#e0af68", "#7aa2f7", "#bb9af7", "#7dcfff", "#c0caf5"],
  ),
  def(
    "tokyo-night-day", "Tokyo Night Day", "light", "tokyo-night",
    { app: "#d0d5e3", panel: "#e1e2e7", panelRaised: "#e9e9ed", hover: "#d5d6db", active: "#c4c8da", border: "#c4c8da", borderStrong: "#a8aecb", fg: "#3760bf", fgMuted: "#6172b0", fgSubtle: "#848cb5", accent: "#2e7de9", onAccent: "#ffffff", success: "#587539", warning: "#8c6c3e", danger: "#f52a65", claude: "#b15c00" },
    ["#b4b5b9", "#f52a65", "#587539", "#8c6c3e", "#2e7de9", "#9854f1", "#007197", "#6172b0", "#a1a6c5", "#f52a65", "#587539", "#8c6c3e", "#2e7de9", "#9854f1", "#007197", "#3760bf"],
  ),
  def(
    "dracula", "Dracula", "dark", "light",
    { app: "#21222c", panel: "#282a36", panelRaised: "#2d2f3d", hover: "#343746", active: "#44475a", border: "#343746", borderStrong: "#44475a", fg: "#f8f8f2", fgMuted: "#b6b8c8", fgSubtle: "#6272a4", accent: "#bd93f9", onAccent: "#21222c", success: "#50fa7b", warning: "#f1fa8c", danger: "#ff5555", claude: "#ffb86c" },
    ["#21222c", "#ff5555", "#50fa7b", "#f1fa8c", "#bd93f9", "#ff79c6", "#8be9fd", "#f8f8f2", "#6272a4", "#ff6e6e", "#69ff94", "#ffffa5", "#d6acff", "#ff92df", "#a4ffff", "#ffffff"],
  ),
  def(
    "nord", "Nord", "dark", "light",
    { app: "#242933", panel: "#2e3440", panelRaised: "#343a47", hover: "#3b4252", active: "#434c5e", border: "#3b4252", borderStrong: "#4c566a", fg: "#eceff4", fgMuted: "#d8dee9", fgSubtle: "#7b88a1", accent: "#88c0d0", onAccent: "#2e3440", success: "#a3be8c", warning: "#ebcb8b", danger: "#bf616a", claude: "#d08770" },
    ["#3b4252", "#bf616a", "#a3be8c", "#ebcb8b", "#81a1c1", "#b48ead", "#88c0d0", "#e5e9f0", "#4c566a", "#bf616a", "#a3be8c", "#ebcb8b", "#81a1c1", "#b48ead", "#8fbcbb", "#eceff4"],
  ),
  def(
    "gruvbox-dark", "Gruvbox Dark", "dark", "gruvbox-light",
    { app: "#1d2021", panel: "#282828", panelRaised: "#2e2c2b", hover: "#32302f", active: "#3c3836", border: "#3c3836", borderStrong: "#504945", fg: "#ebdbb2", fgMuted: "#bdae93", fgSubtle: "#928374", accent: "#83a598", onAccent: "#1d2021", success: "#b8bb26", warning: "#fabd2f", danger: "#fb4934", claude: "#fe8019" },
    ["#3c3836", "#cc241d", "#98971a", "#d79921", "#458588", "#b16286", "#689d6a", "#a89984", "#928374", "#fb4934", "#b8bb26", "#fabd2f", "#83a598", "#d3869b", "#8ec07c", "#ebdbb2"],
  ),
  def(
    "gruvbox-light", "Gruvbox Light", "light", "gruvbox-dark",
    { app: "#f2e5bc", panel: "#fbf1c7", panelRaised: "#f9f5d7", hover: "#f2e5bc", active: "#ebdbb2", border: "#ebdbb2", borderStrong: "#d5c4a1", fg: "#3c3836", fgMuted: "#665c54", fgSubtle: "#928374", accent: "#076678", onAccent: "#fbf1c7", success: "#79740e", warning: "#b57614", danger: "#9d0006", claude: "#af3a03" },
    ["#3c3836", "#cc241d", "#98971a", "#d79921", "#458588", "#b16286", "#689d6a", "#7c6f64", "#928374", "#9d0006", "#79740e", "#b57614", "#076678", "#8f3f71", "#427b58", "#3c3836"],
  ),
  def(
    "solarized-dark", "Solarized Dark", "dark", "solarized-light",
    { app: "#00212b", panel: "#002b36", panelRaised: "#04313d", hover: "#073642", active: "#0a4050", border: "#073642", borderStrong: "#174652", fg: "#93a1a1", fgMuted: "#839496", fgSubtle: "#586e75", accent: "#268bd2", onAccent: "#fdf6e3", success: "#859900", warning: "#b58900", danger: "#dc322f", claude: "#cb4b16" },
    ["#073642", "#dc322f", "#859900", "#b58900", "#268bd2", "#d33682", "#2aa198", "#eee8d5", "#586e75", "#cb4b16", "#859900", "#b58900", "#268bd2", "#6c71c4", "#2aa198", "#fdf6e3"],
  ),
  def(
    "solarized-light", "Solarized Light", "light", "solarized-dark",
    { app: "#eee8d5", panel: "#fdf6e3", panelRaised: "#fffbee", hover: "#f5efdc", active: "#eee8d5", border: "#eee8d5", borderStrong: "#d9d2bf", fg: "#586e75", fgMuted: "#657b83", fgSubtle: "#93a1a1", accent: "#268bd2", onAccent: "#fdf6e3", success: "#859900", warning: "#b58900", danger: "#dc322f", claude: "#cb4b16" },
    ["#073642", "#dc322f", "#859900", "#b58900", "#268bd2", "#d33682", "#2aa198", "#93a1a1", "#657b83", "#cb4b16", "#859900", "#b58900", "#268bd2", "#6c71c4", "#2aa198", "#586e75"],
  ),
  def(
    "one-dark", "One Dark", "dark", "light",
    { app: "#21252b", panel: "#282c34", panelRaised: "#2c313a", hover: "#2f343e", active: "#3e4451", border: "#333842", borderStrong: "#3e4451", fg: "#abb2bf", fgMuted: "#8b929e", fgSubtle: "#5c6370", accent: "#61afef", onAccent: "#21252b", success: "#98c379", warning: "#e5c07b", danger: "#e06c75", claude: "#d19a66" },
    ["#3f4451", "#e06c75", "#98c379", "#e5c07b", "#61afef", "#c678dd", "#56b6c2", "#d7dae0", "#5c6370", "#e9969d", "#b3d39c", "#edd4a6", "#8fc6f4", "#d7a1e7", "#7bc6d0", "#ffffff"],
  ),
  def(
    "rose-pine", "Rosé Pine", "dark", "rose-pine-dawn",
    { app: "#13111e", panel: "#191724", panelRaised: "#1f1d2e", hover: "#21202e", active: "#26233a", border: "#26233a", borderStrong: "#403d52", fg: "#e0def4", fgMuted: "#908caa", fgSubtle: "#6e6a86", accent: "#c4a7e7", onAccent: "#191724", success: "#9ccfd8", warning: "#f6c177", danger: "#eb6f92", claude: "#ebbcba" },
    ["#26233a", "#eb6f92", "#31748f", "#f6c177", "#9ccfd8", "#c4a7e7", "#ebbcba", "#e0def4", "#6e6a86", "#eb6f92", "#31748f", "#f6c177", "#9ccfd8", "#c4a7e7", "#ebbcba", "#e0def4"],
  ),
  def(
    "rose-pine-dawn", "Rosé Pine Dawn", "light", "rose-pine",
    { app: "#f2e9e1", panel: "#faf4ed", panelRaised: "#fffaf3", hover: "#f4ede8", active: "#dfdad9", border: "#dfdad9", borderStrong: "#cecacd", fg: "#575279", fgMuted: "#797593", fgSubtle: "#9893a5", accent: "#907aa9", onAccent: "#faf4ed", success: "#286983", warning: "#ea9d34", danger: "#b4637a", claude: "#d7827e" },
    ["#cecacd", "#b4637a", "#286983", "#ea9d34", "#56949f", "#907aa9", "#d7827e", "#575279", "#9893a5", "#b4637a", "#286983", "#ea9d34", "#56949f", "#907aa9", "#d7827e", "#575279"],
  ),
  def(
    "github-dark", "GitHub Dark", "dark", "github-light",
    { app: "#010409", panel: "#0d1117", panelRaised: "#161b22", hover: "#1c2128", active: "#262c36", border: "#21262d", borderStrong: "#30363d", fg: "#e6edf3", fgMuted: "#9198a1", fgSubtle: "#6e7681", accent: "#2f81f7", onAccent: "#ffffff", success: "#3fb950", warning: "#d29922", danger: "#f85149", claude: "#db6d28" },
    ["#484f58", "#ff7b72", "#3fb950", "#d29922", "#58a6ff", "#bc8cff", "#39c5cf", "#b1bac4", "#6e7681", "#ffa198", "#56d364", "#e3b341", "#79c0ff", "#d2a8ff", "#56d4dd", "#ffffff"],
  ),
  def(
    "github-light", "GitHub Light", "light", "github-dark",
    { app: "#f6f8fa", panel: "#ffffff", panelRaised: "#ffffff", hover: "#f3f4f6", active: "#eaeef2", border: "#d8dee4", borderStrong: "#d0d7de", fg: "#1f2328", fgMuted: "#59636e", fgSubtle: "#818b98", accent: "#0969da", onAccent: "#ffffff", success: "#1a7f37", warning: "#9a6700", danger: "#d1242f", claude: "#bc4c00" },
    ["#24292f", "#cf222e", "#116329", "#4d2d00", "#0969da", "#8250df", "#1b7c83", "#6e7781", "#57606a", "#a40e26", "#1a7f37", "#633c01", "#218bff", "#a475f9", "#3192aa", "#8c959f"],
  ),
];

export const DEFAULT_THEME = "dark";

const byId = new Map(THEMES.map((t) => [t.id, t]));

export const isThemeId = (id: unknown): id is string => typeof id === "string" && byId.has(id);

export const themeById = (id: string): ThemeDef => byId.get(id) ?? byId.get(DEFAULT_THEME)!;

/** The xterm palette for a theme: terminals sit on the panel color. */
export function terminalTheme(id: string): ITheme {
  const { ui, ansi, mode } = themeById(id);
  const [black, red, green, yellow, blue, magenta, cyan, white, brightBlack, brightRed, brightGreen, brightYellow, brightBlue, brightMagenta, brightCyan, brightWhite] = ansi;
  return {
    background: ui.panel,
    foreground: ui.fg,
    cursor: ui.fg,
    cursorAccent: ui.panel,
    selectionBackground: ui.accent + (mode === "light" ? "33" : "55"),
    black, red, green, yellow, blue, magenta, cyan, white,
    brightBlack, brightRed, brightGreen, brightYellow, brightBlue, brightMagenta, brightCyan, brightWhite,
  };
}

const TOKENS: Record<keyof Ui, string> = {
  app: "--color-app",
  panel: "--color-panel",
  panelRaised: "--color-panel-raised",
  hover: "--color-hover",
  active: "--color-active",
  border: "--color-border",
  borderStrong: "--color-border-strong",
  fg: "--color-fg",
  fgMuted: "--color-fg-muted",
  fgSubtle: "--color-fg-subtle",
  accent: "--color-accent",
  onAccent: "--color-on-accent",
  success: "--color-success",
  warning: "--color-warning",
  danger: "--color-danger",
  claude: "--color-claude",
};

/**
 * xterm's minimumContrastRatio for a theme. Agent TUIs often pick colors for a dark
 * background (near-white truecolor text, ANSI white), which vanish on light themes, so
 * light themes make xterm darken any text that falls below WCAG AA contrast.
 */
export const terminalMinContrast = (id: string): number => (themeById(id).mode === "light" ? 4.5 : 1);

/** Writes a theme's tokens onto <html>, overriding the defaults in globals.css. */
export function applyTheme(id: string) {
  const { ui, mode } = themeById(id);
  const root = document.documentElement;
  for (const key of Object.keys(TOKENS) as (keyof Ui)[]) root.style.setProperty(TOKENS[key], ui[key]);
  root.style.setProperty("--color-accent-soft", ui.accent + (mode === "light" ? "22" : "26"));
  root.style.colorScheme = mode;
  root.dataset.theme = id;
  root.dataset.mode = mode;
}
