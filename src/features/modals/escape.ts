import { useEffect } from "react";
import { isRecording } from "@/commands/keymap";
import { closeRaceResults, useRaceResults } from "@/features/orchestration/resultsStore";
import { focusTerminal } from "@/features/terminal/xtermRegistry";
import { closeReleaseNotes, useUpdates } from "@/features/updates/updater";
import { useWorkspaces } from "@/stores/workspaces";
import { answerApproval, useApprovals } from "@/features/plugins/approvals";

const ws = () => useWorkspaces.getState();

/** Every modal, topmost first. */
const MODALS: { open: () => boolean; close: () => void }[] = [
  // Escape answers a plugin's question with "no".
  { open: () => useApprovals.getState().queue.length > 0, close: () => answerApproval(useApprovals.getState().queue[0].id, false) },
  { open: () => !!useRaceResults.getState().raceId, close: closeRaceResults },
  { open: () => !!useUpdates.getState().notesOpen, close: closeReleaseNotes },
  { open: () => ws().paletteQuery !== null, close: () => ws().closePalette() },
  { open: () => ws().cheatSheetOpen, close: () => ws().setCheatSheet(false) },
  { open: () => !!ws().overlay, close: () => ws().setOverlay(null) },
  { open: () => !!ws().diffPaneId, close: () => ws().setDiffPane(null) },
  { open: () => ws().settingsOpen, close: () => ws().setSettings(false) },
];

/** Closes the topmost open modal; false if none was open. */
export function closeTopModal(): boolean {
  const top = MODALS.find((m) => m.open());
  top?.close();
  return !!top;
}

/**
 * Escape closes the topmost modal, wherever focus is. Capture phase, so a focused terminal
 * (which swallows Escape for its PTY) or a field inside the modal can't keep it open.
 */
export function useEscapeToClose() {
  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key !== "Escape" || e.ctrlKey || e.altKey || e.metaKey || e.shiftKey || isRecording()) return;
      // A right-click menu over a modal closes first, by its own handler.
      if (document.querySelector('[role="menu"]')) return;
      if (!closeTopModal()) return;
      e.preventDefault();
      e.stopPropagation();
      if (!MODALS.some((m) => m.open())) {
        const focused = ws().focusedPaneId;
        if (focused) requestAnimationFrame(() => focusTerminal(focused));
      }
    };
    window.addEventListener("keydown", onKeyDown, { capture: true });
    return () => window.removeEventListener("keydown", onKeyDown, { capture: true });
  }, []);
}
