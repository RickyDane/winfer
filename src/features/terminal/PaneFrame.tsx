import { ExternalLink, GitBranch, Loader2, Maximize2, Minimize2, Plus, X } from "lucide-react";
import { useState } from "react";
import { execute } from "@/commands/bus";
import { keysFor } from "@/commands/keymap";
import { cn } from "@/lib/cn";
import { ProviderIcon } from "@/features/providers/ProviderIcon";
import { StatusDot } from "./StatusDot";
import { PaneMenu } from "./PaneMenu";
import { PaneSearch } from "./PaneSearch";
import { beginDrag, useDrag } from "@/features/layout/drag";
import { TerminalHost } from "./TerminalHost";
import { useStarting } from "./xtermRegistry";
import { providerName, useWorkspaces, type Pane } from "@/stores/workspaces";
import { useProgress } from "@/features/progress/progress";
import { ProgressBar, progressLabel } from "@/features/progress/SessionsOverview";
import { ContextMeter, UsageRing } from "@/features/usage/UsageMeter";
import { tightest, useUsage } from "@/features/usage/usage";

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
  const showProgress = pane.provider !== "shell" && (pane.status === "working" || pane.status === "needs-input" || progress?.percent != null);
  const starting = useStarting((s) => s[pane.id]);
  const hasUsage = useUsage((s) => tightest(s.snapshots[pane.provider]) !== null);
  const name = useWorkspaces((s) => providerName(s.providers, pane.provider));
  const [isNew] = useState(() => !seen.has(pane.id));
  seen.add(pane.id);

  return (
    <section
      data-pane-id={pane.id}
      onMouseDown={onFocus}
      className={cn(
        "relative flex h-full min-h-0 w-full min-w-0 flex-col overflow-hidden rounded-[var(--radius-pane)] border bg-panel transition-[border-color,opacity] duration-150",
        isNew && "anim-grow",
        focused ? "border-accent/70" : "border-border",
        beingDragged && "opacity-40",
      )}
    >
      <header
        onPointerDown={(e) => beginDrag(e, pane.id)}
        title="Drag to move this pane"
        className="@container relative flex h-8 shrink-0 cursor-grab items-center gap-2 border-b border-border px-3 active:cursor-grabbing"
      >
        {showProgress && (
          <span title={progressLabel(progress)} className="absolute inset-x-0 -bottom-px">
            <ProgressBar progress={progress} status={pane.status} className="h-0.5 rounded-none bg-transparent" />
          </span>
        )}
        <span className="min-w-3 font-mono text-[11px] text-fg-subtle">{index}</span>
        <StatusDot status={pane.status} />
        {hasUsage ? <UsageRing provider={pane.provider} size={20} /> : <ProviderIcon provider={pane.provider} />}
        {workspaceName && (
          <span className="max-w-32 shrink-0 truncate rounded bg-hover px-1.5 py-0.5 text-[10px] font-medium text-fg-muted" title={`Workspace ${workspaceName}`}>
            {workspaceName}
          </span>
        )}
        <span className="flex-1 truncate font-mono text-xs text-fg-muted" title={pane.agentTitle ?? pane.title}>
          {pane.agentTitle ?? pane.title}
        </span>
        <ContextMeter paneId={pane.id} />
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
      {starting && (
        <div className="anim-fade pointer-events-none absolute inset-x-0 top-8 bottom-0 flex items-center justify-center">
          <span className="flex items-center gap-2 rounded-md border border-border bg-panel-raised px-3 py-1.5 text-xs text-fg-muted shadow-lg">
            <Loader2 size={13} className="animate-spin text-accent" />
            {starting === "worktree" ? "Creating worktree…" : `Starting ${name}…`}
          </span>
        </div>
      )}
    </section>
  );
}
