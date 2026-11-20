/**
 * Which settings can be reset, and whether each differs from its default. Rows show a mark and
 * a reset button for their entry, the navigation dots pages that have changes, and "Reset page"
 * resets them all. Native's settings.rs has the same list (`RESETS`).
 */
import { DEFAULT_THEME } from "@/features/theme/themes";
import { DEFAULT_TERMINAL_FONT_SIZE, useWorkspaces } from "@/stores/workspaces";

type State = ReturnType<typeof useWorkspaces.getState>;

export interface ResetEntry {
  id: string;
  /** The settings page it's on. */
  page: string;
  label: string;
  changed: (s: State) => boolean;
  reset: (s: State) => void;
}

const DEFAULT_PROGRESS_INTERVAL = 60;

export const RESETS: ResetEntry[] = [
  { id: "terminal.size", page: "general", label: "Text size", changed: (s) => s.terminalFontSize !== DEFAULT_TERMINAL_FONT_SIZE, reset: (s) => s.setTerminalFontSize(DEFAULT_TERMINAL_FONT_SIZE) },
  { id: "terminal.gpu", page: "general", label: "GPU acceleration", changed: (s) => !s.terminalGpu, reset: (s) => s.setTerminalGpu(true) },
  { id: "editor", page: "general", label: "Code editor", changed: (s) => s.editor !== null, reset: (s) => s.setEditor(null) },
  { id: "shortcuts", page: "general", label: "Keyboard shortcuts", changed: (s) => Object.keys(s.shortcuts).length > 0, reset: (s) => s.resetShortcuts() },
  {
    id: "theme",
    page: "theme",
    label: "Light or dark",
    changed: (s) => s.theme !== DEFAULT_THEME || s.themeMode !== "manual" || s.themePicks.light !== undefined || (s.themePicks.dark !== undefined && s.themePicks.dark !== DEFAULT_THEME),
    reset: (s) => {
      s.setTheme(DEFAULT_THEME);
      useWorkspaces.setState({ themePicks: {}, themeMode: "manual" });
    },
  },
  { id: "agents.default", page: "agents", label: "Default agent", changed: (s) => s.defaultProvider !== null, reset: (s) => s.setDefaultProvider(null) },
  { id: "agents.models", page: "agents", label: "Models", changed: (s) => Object.keys(s.agentModels).length > 0, reset: () => useWorkspaces.setState({ agentModels: {} }) },
  { id: "agents.autoApprove", page: "agents", label: "Auto-approve", changed: (s) => s.autoApprove.length > 0, reset: () => useWorkspaces.setState({ autoApprove: [] }) },
  { id: "agents.prompt", page: "agents", label: "System prompt", changed: (s) => s.systemPrompt !== null, reset: (s) => s.setSystemPrompt(null) },
  { id: "agents.tools", page: "agents", label: "Optional tools", changed: (s) => s.enabledTools.length > 0, reset: () => useWorkspaces.setState({ enabledTools: [] }) },
  { id: "review.tests", page: "agents", label: "Run tests when an agent finishes", changed: (s) => s.reviewConfig.autoTests === true, reset: (s) => s.setReviewConfig({ autoTests: false }) },
  { id: "review.screenshots", page: "agents", label: "Screenshot the dev server", changed: (s) => s.reviewConfig.screenshots === false, reset: (s) => s.setReviewConfig({ screenshots: true }) },
  { id: "progress.enabled", page: "agents", label: "Estimate how far working agents are", changed: (s) => s.progressConfig.enabled === false, reset: (s) => s.setProgressConfig({ enabled: true }) },
  {
    id: "progress.interval",
    page: "agents",
    label: "Check every",
    changed: (s) => s.progressConfig.intervalSec !== undefined && s.progressConfig.intervalSec !== DEFAULT_PROGRESS_INTERVAL,
    reset: (s) => s.setProgressConfig({ intervalSec: DEFAULT_PROGRESS_INTERVAL }),
  },
  { id: "voice.readback", page: "voice", label: "Speak agent updates aloud", changed: (s) => s.voiceConfig.readback === true, reset: (s) => s.setVoiceConfig({ readback: false }) },
  { id: "usage.cost", page: "usage", label: "Show task cost on panes", changed: (s) => s.showPaneCost, reset: (s) => s.setShowPaneCost(false) },
  { id: "plugins.icons", page: "plugins", label: "Hide names on plugin tabs", changed: (s) => s.pageTabIconsOnly, reset: (s) => s.setPageTabIconsOnly(false) },
];

const byId = new Map(RESETS.map((r) => [r.id, r]));

export const resetEntry = (id: string): ResetEntry | undefined => byId.get(id);

/** The entries on `page` that differ from their defaults. */
export const changedOnPage = (s: State, page: string): ResetEntry[] => RESETS.filter((r) => r.page === page && r.changed(s));

/** How many settings differ from their defaults, per page id. */
export function changedCounts(s: State): Record<string, number> {
  const counts: Record<string, number> = {};
  for (const r of RESETS) if (r.changed(s)) counts[r.page] = (counts[r.page] ?? 0) + 1;
  return counts;
}

/** Resets every changed setting on `page`; returns how many. */
export function resetPage(page: string): number {
  const entries = changedOnPage(useWorkspaces.getState(), page);
  for (const r of entries) r.reset(useWorkspaces.getState());
  return entries.length;
}
