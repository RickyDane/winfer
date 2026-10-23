import { Archive, ArchiveRestore, ExternalLink, GitBranch, Maximize2, Minimize2, Plus, X } from "lucide-react";
import { Spinner } from "@/components/Spinner";
import { useState } from "react";
import { execute } from "@/commands/bus";
import { keysFor } from "@/commands/keymap";
import { cn } from "@/lib/cn";
import { ProviderIcon } from "@/features/providers/ProviderIcon";
import { StatusDot } from "./StatusDot";
import { PaneMenu } from "./PaneMenu";
import { PaneTitleInput } from "./PaneTitleInput";
import { PaneSearch } from "./PaneSearch";
import { beginDrag, useDrag } from "@/features/layout/drag";
import { TerminalHost } from "./TerminalHost";
import { CodePane } from "@/features/code/CodePane";
import { paneUsesWeb } from "@/features/code/styles";
import { useStarting } from "./xtermRegistry";
import { paneLabel, providerName, useWorkspaces, type Pane } from "@/stores/workspaces";
import { progressTitle, useProgress } from "@/features/progress/progress";
import { ProgressBar, progressLabel } from "@/features/progress/SessionsOverview";
import { ContextMeter } from "@/features/usage/UsageMeter";
import { PaneCost } from "@/features/usage/TaskCostChip";
import { GlassPaneHeader } from "./GlassPaneHeader";

const HeaderButton = ({ title, onClick, children }: { title: string; onClick?: () => void; children: React.ReactNode }) => (
  <button
    title={title}
    onClick={onClick}
    onMouseDown={(e) => e.stopPropagation()}
    className="rounded p-1 text-fg-subtle hover:bg-hover hover:text-fg"
  >
    {children}
  </button>
);

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
  onFocus: () => void;
  /** Shown in the overview, where panes from every workspace sit side by side. */
  workspaceName?: string;
}

export function PaneFrame({ pane, index, focused, zoomed, onFocus, workspaceName }: Props) {
  const target = { id: pane.id };
  const searching = useWorkspaces((s) => s.searchPaneId === pane.id);
  const beingDragged = useDrag((s) => s.paneId === pane.id);
  const progress = useProgress((s) => s.byPane[pane.id]);
  const showProgress = pane.provider !== "shell" && progress?.percent != null;
  const starting = useStarting((s) => s[pane.id]);
  const name = useWorkspaces((s) => providerName(s.providers, pane.provider));
  const label = useWorkspaces((s) => paneLabel(pane, s.providers, progressTitle(progress), progress?.title));
  const [renaming, setRenaming] = useState(false);
  const glass = useWorkspaces((s) => s.glassPanes);
  const [isNew] = useState(() => !seen.has(pane.id));
  seen.add(pane.id);

  return (
    <section
      data-pane-id={pane.id}
      onMouseDown={onFocus}
      className={cn(
        "relative flex h-full min-h-0 w-full min-w-0 flex-col overflow-hidden border transition-[border-color,opacity,box-shadow,background-color] duration-150",
        glass ? "pane-glass" : "rounded-[var(--radius-pane)] bg-panel",
        isNew && "anim-grow",
        glass ? focused && "pane-glass-focused" : focused ? "border-accent/70" : "border-border",
        beingDragged && "opacity-40",
      )}
    >
      {/* Archived panes aren't tiled, so there's nowhere to drag them. */}
      {glass ? (
        <GlassPaneHeader
          {...{ pane, index, focused, zoomed, onFocus, workspaceName, name, label, renaming, setRenaming, progress, showProgress }}
        />
      ) : (
      <header
        onPointerDown={pane.archived ? undefined : (e) => beginDrag(e, pane.id)}
        title={pane.archived ? undefined : "Drag to move this pane"}
        className={cn("@container relative flex h-8 shrink-0 items-center gap-2 border-b border-border px-3", !pane.archived && "cursor-grab active:cursor-grabbing")}
      >
        {showProgress && (
          <span title={progressLabel(progress)} className="absolute inset-x-0 -bottom-px">
            <ProgressBar progress={progress} status={pane.status} className="h-0.5 rounded-none bg-transparent" />
          </span>
        )}
        <span className="min-w-3 font-mono text-[11px] text-fg-subtle">{index}</span>
        <StatusDot status={pane.status} />
        <ProviderIcon provider={pane.provider} />
        {workspaceName && (
          <span className="max-w-32 shrink-0 truncate rounded bg-hover px-1.5 py-0.5 text-[10px] font-medium text-fg-muted" title={`Workspace ${workspaceName}`}>
            {workspaceName}
          </span>
        )}
        {renaming ? (
          <PaneTitleInput pane={pane} onDone={() => setRenaming(false)} className="h-5 flex-1 font-mono text-xs" />
        ) : (
          <span
            className="flex-1 truncate font-mono text-xs text-fg-muted"
            title={`${label === name ? name : `${name}: ${label}`}\nDouble-click to rename`}
            onDoubleClick={() => setRenaming(true)}
          >
            {label}
          </span>
        )}
        <ContextMeter paneId={pane.id} provider={pane.provider} />
        <PaneCost pane={pane} />
        {showProgress && (
          <span title={progressLabel(progress)} className="font-mono text-[11px] text-fg-subtle">
            {progress.percent}%
          </span>
        )}
        {pane.worktree && (
          <button
            onMouseDown={(e) => e.stopPropagation()}
            onClick={() => execute({ type: "pane.showChanges", target })}
            title={`${pane.worktree.path}\nClick to show changes`}
            className="flex max-w-40 items-center gap-1 truncate rounded px-1.5 py-0.5 font-mono text-[11px] text-fg-subtle hover:bg-hover hover:text-fg"
          >
            <GitBranch size={11} />
            <span className="truncate">{pane.worktree.branch.replace(/^winfer\//, "")}</span>
          </button>
        )}
        {pane.model && (
          <button
            onMouseDown={(e) => e.stopPropagation()}
            onClick={() => {
              onFocus();
              execute({ type: "ui.overlay", overlay: "model" });
            }}
            title="Model · click to change"
            className="max-w-28 truncate rounded px-1.5 py-0.5 font-mono text-[11px] text-fg-subtle hover:bg-hover hover:text-fg"
          >
            {pane.model}
          </button>
        )}
        {pane.devUrl && (
          <button
            onMouseDown={(e) => e.stopPropagation()}
            onClick={() => execute({ type: "browser.open", url: pane.devUrl! })}
            title={`Open ${pane.devUrl} in the browser panel`}
            className="flex items-center gap-1 rounded px-1.5 py-0.5 font-mono text-[11px] text-accent hover:bg-accent-soft"
          >
            {pane.devUrl.replace(/^https?:\/\//, "").replace(/\/$/, "")}
            <ExternalLink size={10} />
          </button>
        )}
        <PaneMenu pane={pane} onRename={() => setRenaming(true)} />
        {pane.archived ? (
          <button
            onMouseDown={(e) => e.stopPropagation()}
            onClick={() => execute({ type: "pane.archive", target, restore: true })}
            title={`Put back into ${workspaceName ?? "its workspace"}`}
            className="flex items-center gap-1 rounded px-1.5 py-0.5 text-[11px] text-fg-muted hover:bg-hover hover:text-fg"
          >
            <ArchiveRestore size={12} />
            Restore
          </button>
        ) : (
          <>
            <HeaderButton title="Archive: keep it running, out of the way" onClick={() => execute({ type: "pane.archive", target })}>
              <Archive size={13} />
            </HeaderButton>
            <HeaderButton title={`${zoomed ? "Restore" : "Zoom"} (${keysFor({ type: "pane.zoom", target: "focused" })})`} onClick={() => execute({ type: "pane.zoom", target })}>
              {zoomed ? <Minimize2 size={13} /> : <Maximize2 size={13} />}
            </HeaderButton>
            <HeaderButton
              title={`Split right (${keysFor({ type: "pane.split", dir: "row" })})`}
              onClick={() => {
                onFocus();
                execute({ type: "pane.split", dir: "row" });
              }}
            >
              <Plus size={14} />
            </HeaderButton>
          </>
        )}
        <HeaderButton title={`Close (${keysFor({ type: "pane.close", target: "focused" })})`} onClick={() => execute({ type: "pane.close", target })}>
          <X size={14} />
        </HeaderButton>
      </header>
      )}

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
        <div className={cn("anim-fade pointer-events-none absolute inset-x-0 bottom-0 flex items-center justify-center", glass ? "top-20" : "top-8")}>
          <span className="flex items-center gap-2 rounded-md border border-border bg-panel-raised px-3 py-1.5 text-xs text-fg-muted shadow-lg">
            <Spinner size={13} className="text-accent" />
            {starting === "worktree" ? "Creating worktree…" : `Starting ${name}…`}
          </span>
        </div>
      )}
    </section>
  );
}
