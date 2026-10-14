import { ExternalLink, GitBranch, Maximize2, Minimize2, Plus, X } from "lucide-react";
import { execute } from "@/commands/bus";
import { keysFor } from "@/commands/keymap";
import { cn } from "@/lib/cn";
import { ProviderIcon } from "@/features/providers/ProviderIcon";
import { StatusDot } from "./StatusDot";
import { PaneMenu } from "./PaneMenu";
import { PaneSearch } from "./PaneSearch";
import { beginDrag, useDrag } from "@/features/layout/drag";
import { TerminalHost } from "./TerminalHost";
import { useWorkspaces, type Pane } from "@/stores/workspaces";
import { useProgress } from "@/features/progress/progress";
import { ProgressBar, progressLabel } from "@/features/progress/SessionsOverview";

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
  const showProgress = pane.provider !== "shell" && (pane.status === "working" || pane.status === "needs-input" || progress?.percent != null);

  return (
    <section
      data-pane-id={pane.id}
      onMouseDown={onFocus}
      className={cn(
        "relative flex h-full min-h-0 w-full min-w-0 flex-col overflow-hidden rounded-[var(--radius-pane)] border bg-panel",
        focused ? "border-accent/70" : "border-border",
        beingDragged && "opacity-40",
      )}
    >
      <header
        onPointerDown={(e) => beginDrag(e, pane.id)}
        title="Drag to move this pane"
        className="relative flex h-8 shrink-0 cursor-grab items-center gap-2 border-b border-border px-3 active:cursor-grabbing"
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
        <span className="flex-1 truncate font-mono text-xs text-fg-muted">{pane.title}</span>
        {showProgress && progress?.percent != null && (
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
        <PaneMenu pane={pane} />
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
        <HeaderButton title={`Close (${keysFor({ type: "pane.close", target: "focused" })})`} onClick={() => execute({ type: "pane.close", target })}>
          <X size={14} />
        </HeaderButton>
      </header>

      {searching && <PaneSearch paneId={pane.id} />}
      <div className="min-h-0 flex-1 py-1.5 pl-2.5 pr-1">
        <TerminalHost pane={pane} focused={focused} />
      </div>
    </section>
  );
}
