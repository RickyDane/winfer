import { Spinner } from "@/components/Spinner";
import { memo, useState } from "react";
import { cn } from "@/lib/cn";
import { PaneSearch } from "./PaneSearch";
import { useDrag } from "@/features/layout/drag";
import { TerminalHost } from "./TerminalHost";
import { CodePane } from "@/features/code/CodePane";
import { paneUsesWeb } from "@/features/code/styles";
import { useStarting } from "./xtermRegistry";
import { paneLabel, providerName, useWorkspaces, type Pane } from "@/stores/workspaces";
import { progressTitle, useProgress } from "@/features/progress/progress";
import { GlassPaneHeader } from "./GlassPaneHeader";
import { tabPane, useBrowser } from "@/features/browser/browser";

/**
 * Panes already shown once. Layout changes remount frames, so only a pane's first
 * appearance animates in.
 */
const seen = new Set<string>();

interface Props {
  pane: Pane;
  /** 1-based position in layout order; matches ⌘1–9 and voice "focus 3". */
  index: number;
  focused: boolean;
  zoomed: boolean;
  /** Called with the pane's id. A stable function (not a fresh closure) lets the frame skip
   *  re-rendering when the layout around it re-renders. */
  onFocus: (paneId: string) => void;
  /** Shown in the overview, where panes from every workspace sit side by side. */
  workspaceName?: string;
}

export const PaneFrame = memo(function PaneFrame({ pane, index, focused, zoomed, onFocus: focusPane, workspaceName }: Props) {
  const onFocus = () => focusPane(pane.id);
  const searching = useWorkspaces((s) => s.searchPaneId === pane.id);
  const beingDragged = useDrag((s) => s.paneId === pane.id);
  const progress = useProgress((s) => s.byPane[pane.id]);
  const showProgress = pane.provider !== "shell" && progress?.percent != null;
  const starting = useStarting((s) => s[pane.id]);
  const name = useWorkspaces((s) => providerName(s.providers, pane.provider));
  const label = useWorkspaces((s) => paneLabel(pane, s.providers, progressTitle(progress), progress?.title));
  const [renaming, setRenaming] = useState(false);
  const browserVisible = useWorkspaces((s) => s.browserOpen && s.sidePanel === "browser");
  const workspaces = useWorkspaces((s) => s.workspaces);
  const browserLinked = useBrowser((s) => s.shown === pane.workspaceId && tabPane(s, pane.workspaceId, s.activeTab[pane.workspaceId], workspaces) === pane.id);
  const [isNew] = useState(() => !seen.has(pane.id));
  seen.add(pane.id);

  return (
    <section
      data-pane-id={pane.id}
      data-browser-highlighted={browserVisible && browserLinked || undefined}
      onMouseDown={onFocus}
      className={cn(
        "relative flex h-full min-h-0 w-full min-w-0 flex-col overflow-hidden border transition-[border-color,opacity,box-shadow,background-color] duration-150",
        "pane-glass",
        isNew && "anim-grow",
        focused && "pane-glass-focused",
        beingDragged && "opacity-40",
        browserVisible && browserLinked && "browser-linked-pane",
      )}
    >
      <GlassPaneHeader
        {...{ pane, index, focused, zoomed, onFocus, workspaceName, name, label, renaming, setRenaming, progress, showProgress }}
      />

      {searching && <PaneSearch paneId={pane.id} />}
      {pane.provider === "winfer" && paneUsesWeb(pane.id) ? (
        <div className="min-h-0 flex-1">
          <CodePane pane={pane} focused={focused} />
        </div>
      ) : (
        <div className="min-h-0 flex-1 py-1.5 pl-2.5 pr-1">
          <TerminalHost pane={pane} focused={focused} />
        </div>
      )}
      {starting && (
        <div className={cn("anim-fade pointer-events-none absolute inset-x-0 bottom-0 flex items-center justify-center", "top-20")}>
          <span className="flex items-center gap-2 rounded-md border border-border bg-panel-raised px-3 py-1.5 text-xs text-fg-muted shadow-lg">
            <Spinner size={13} className="text-accent" />
            {starting === "worktree" ? "Creating worktree…" : `Starting ${name}…`}
          </span>
        </div>
      )}
    </section>
  );
});
