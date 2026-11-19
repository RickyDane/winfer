import { Archive, ArchiveRestore, ExternalLink, FolderGit2, GitBranch, Maximize2, Minimize2, Plus, X } from "lucide-react";
import { execute } from "@/commands/bus";
import { keysFor } from "@/commands/keymap";
import { cn } from "@/lib/cn";
import { ProviderIcon } from "@/features/providers/ProviderIcon";
import { beginDrag } from "@/features/layout/drag";
import { ProgressBar, progressLabel } from "@/features/progress/SessionsOverview";
import { ContextMeter } from "@/features/usage/UsageMeter";
import { PaneCost } from "@/features/usage/TaskCostChip";
import type { Progress } from "@/features/progress/progress";
import type { Pane } from "@/stores/workspaces";
import { StatusDot } from "./StatusDot";
import { PaneMenu } from "./PaneMenu";
import { PaneNote } from "./PaneNote";
import { PaneTitleInput } from "./PaneTitleInput";
import { displayModelName } from "@/lib/modelName";
import { BgIndicator } from "@/features/code/BgIndicator";

const stop = (e: React.MouseEvent) => e.stopPropagation();

const HeaderButton = ({ title, onClick, children }: { title: string; onClick?: () => void; children: React.ReactNode }) => (
  <button title={title} onClick={onClick} onMouseDown={stop} className="grid size-6 place-items-center rounded-md text-fg-subtle hover:bg-hover hover:text-fg">
    {children}
  </button>
);

/** A fact on the status line: plain mono text, highlighted on hover when it does something. */
const Fact = ({ className, ...props }: React.HTMLAttributes<HTMLSpanElement>) => (
  <span {...props} className={cn("inline-flex min-w-0 shrink-0 items-center gap-1 truncate rounded px-1 py-px", className)} />
);

interface Props {
  pane: Pane;
  index: number;
  focused: boolean;
  zoomed: boolean;
  onFocus: () => void;
  workspaceName?: string;
  name: string;
  label: string;
  renaming: boolean;
  setRenaming: (on: boolean) => void;
  progress?: Progress;
  showProgress: boolean;
}

/**
 * The pane header (Graphite): one slim row with index, status, agent, title, progress and the
 * buttons; a 2 px progress bar along its bottom edge. Needing input or an error tints it.
 * The pane's facts (branch, model, cost, context) live in {@link PaneStatusLine} under the pane.
 */
export function PaneHeader({ pane, index, focused, zoomed, onFocus, workspaceName, name, label, renaming, setRenaming, progress, showProgress }: Props) {
  const target = { id: pane.id };
  const needs = pane.status === "needs-input";
  const error = pane.status === "error";
  const percent = showProgress && pane.status !== "idle" ? progress?.percent : null;

  return (
    <header
      onPointerDown={pane.archived ? undefined : (e) => beginDrag(e, pane.id)}
      title={pane.archived ? undefined : "Drag to move this pane"}
      className={cn(
        "group/header @container relative flex h-[34px] shrink-0 items-center gap-2 border-b border-border pr-1.5 pl-2.5 transition-colors",
        needs ? "bg-warning/[0.08]" : error ? "bg-danger/[0.08]" : "bg-panel-raised",
        !pane.archived && "cursor-grab active:cursor-grabbing",
      )}
    >
      {showProgress && progress && (
        <span title={progressLabel(progress)} className="pointer-events-none absolute inset-x-0 -bottom-px">
          <ProgressBar progress={progress} status={pane.status} className="h-0.5 rounded-none bg-transparent" />
        </span>
      )}
      <span className={cn("shrink-0 font-mono text-[10.5px]", focused ? "text-accent" : "text-fg-subtle")}>{index}</span>
      <StatusDot status={pane.status} />
      <span title={name} className="grid shrink-0 place-items-center">
        <ProviderIcon provider={pane.provider} size={13} />
      </span>
      {workspaceName && (
        <span className="max-w-28 shrink-0 truncate rounded bg-hover px-1.5 text-[10.5px] text-fg-muted" title={`Workspace ${workspaceName}`}>
          {workspaceName}
        </span>
      )}
      {renaming ? (
        <PaneTitleInput pane={pane} onDone={() => setRenaming(false)} className="h-6 flex-1 text-[13px] font-medium" />
      ) : (
        <span
          className={cn("min-w-0 flex-1 truncate text-[13px] font-medium", focused ? "text-fg" : "text-fg-muted")}
          title={`${label === name ? name : `${name}: ${label}`}\nDouble-click to rename`}
          onDoubleClick={() => setRenaming(true)}
        >
          {label}
        </span>
      )}
      <BgIndicator pane={pane} />
      {percent != null && (
        <span className="hidden shrink-0 items-center gap-1.5 font-mono text-[11px] text-fg-muted @[260px]:flex">
          <span className="h-[3px] w-8 overflow-hidden rounded-full bg-active">
            <span className={cn("block h-full rounded-full", needs ? "bg-warning" : percent >= 100 ? "bg-success" : "bg-accent")} style={{ width: `${percent}%` }} />
          </span>
          {percent}%
        </span>
      )}
      {pane.devUrl && (
        <button
          onMouseDown={stop}
          onClick={() => execute({ type: "browser.open", url: pane.devUrl!, paneId: pane.id })}
          aria-label={`Open ${pane.devUrl} in the browser panel`}
          title={`Open ${pane.devUrl} in the browser panel`}
          className="flex shrink-0 items-center gap-1 rounded px-1.5 py-0.5 font-mono text-[11px] text-accent hover:bg-accent-soft"
        >
          {pane.devUrl.replace(/^https?:\/\//, "").replace(/\/$/, "")}
          <ExternalLink size={10} />
        </button>
      )}
      <div className={cn("flex shrink-0 items-center transition-opacity group-hover/header:opacity-100", focused ? "opacity-100" : "opacity-55")}>
        <PaneNote paneId={pane.id} />
        <PaneMenu pane={pane} onRename={() => setRenaming(true)} />
        {pane.archived ? (
          <button
            onMouseDown={stop}
            onClick={() => execute({ type: "pane.archive", target, restore: true })}
            title={`Put back into ${workspaceName ?? "its workspace"}`}
            className="ml-1 flex items-center gap-1 rounded-md border border-border-strong px-2 py-0.5 text-[11px] text-fg-muted hover:bg-hover hover:text-fg"
          >
            <ArchiveRestore size={12} />
            Restore
          </button>
        ) : (
          <span className="hidden items-center @[300px]:flex">
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
          </span>
        )}
        <HeaderButton title={`Close (${keysFor({ type: "pane.close", target: "focused" })})`} onClick={() => execute({ type: "pane.close", target })}>
          <X size={14} />
        </HeaderButton>
      </div>
    </header>
  );
}

/** Under an agent's pane: its branch (click for changes), context, cost and model (click to change). */
export function PaneStatusLine({ pane, onFocus }: { pane: Pane; onFocus: () => void }) {
  if (pane.provider === "shell") return null;
  const target = { id: pane.id };
  const branch = pane.worktree?.branch.replace(/^winfer\//, "");
  return (
    <footer className="@container flex h-[26px] shrink-0 items-center gap-2 overflow-hidden border-t border-border px-2 font-mono text-[10.5px] text-fg-subtle">
      {pane.worktree ? (
        <button onMouseDown={stop} onClick={() => execute({ type: "pane.showChanges", target })} title={`Branch: ${branch}\nOwn worktree: ${pane.worktree.path}\nClick to show changes`} className="min-w-0">
          <Fact className="max-w-48 hover:bg-hover hover:text-fg">
            <GitBranch size={11} className="shrink-0" />
            <span className="truncate">{branch}</span>
          </Fact>
        </button>
      ) : (
        <Fact title="Works in the project folder, not its own worktree">
          <FolderGit2 size={11} className="shrink-0" />
          shared folder
        </Fact>
      )}
      <ContextMeter paneId={pane.id} provider={pane.provider} ring />
      <PaneCost pane={pane} />
      <span className="flex-1" />
      {pane.model && (
        <button
          onMouseDown={stop}
          onClick={() => {
            onFocus();
            execute({ type: "ui.overlay", overlay: "model" });
          }}
          title={`${pane.model} · click to change`}
          className="hidden @[260px]:block"
        >
          <Fact className="max-w-32 hover:bg-hover hover:text-fg">{displayModelName(pane.model)}</Fact>
        </button>
      )}
    </footer>
  );
}
