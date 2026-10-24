import { Archive, ArchiveRestore, ExternalLink, FolderGit2, GitBranch, Maximize2, Minimize2, Plus, X } from "lucide-react";
import { execute } from "@/commands/bus";
import { keysFor } from "@/commands/keymap";
import { cn } from "@/lib/cn";
import { ProviderIcon } from "@/features/providers/ProviderIcon";
import { PROVIDER_COLORS } from "@/features/providers/providers";
import { beginDrag } from "@/features/layout/drag";
import { ProgressBar, progressLabel } from "@/features/progress/SessionsOverview";
import { ContextMeter } from "@/features/usage/UsageMeter";
import { PaneCost } from "@/features/usage/TaskCostChip";
import type { Progress } from "@/features/progress/progress";
import type { Pane } from "@/stores/workspaces";
import { StatusDot } from "./StatusDot";
import { PaneMenu } from "./PaneMenu";
import { PaneTitleInput } from "./PaneTitleInput";
import { displayModelName } from "@/lib/modelName";
import { BgIndicator } from "@/features/code/BgIndicator";

const stop = (e: React.MouseEvent) => e.stopPropagation();

const GlassButton = ({ title, onClick, children }: { title: string; onClick?: () => void; children: React.ReactNode }) => (
  <button title={title} onClick={onClick} onMouseDown={stop} className="grid size-6 place-items-center rounded-lg text-fg-muted hover:bg-white/10 hover:text-fg">
    {children}
  </button>
);

const Pill = ({ className, ...props }: React.HTMLAttributes<HTMLSpanElement>) => (
  <span {...props} className={cn("inline-flex min-w-0 shrink-0 items-center gap-1 truncate rounded-full border border-white/[0.07] bg-white/[0.07] px-2 py-px", className)} />
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
 * The opt-in "glass" pane header (Settings → On panes): a floating card inside the pane with
 * title and actions on top, and the pane's facts (agent, git, model, links, cost, context) below.
 * Needs input and errors replace the facts row with a tinted notice.
 */
export function GlassPaneHeader({ pane, index, focused, zoomed, onFocus, workspaceName, name, label, renaming, setRenaming, progress, showProgress }: Props) {
  const target = { id: pane.id };
  const color = PROVIDER_COLORS[pane.provider] ?? "#6b7280";
  const needs = pane.status === "needs-input";
  const error = pane.status === "error";
  const branch = pane.worktree?.branch.replace(/^winfer\//, "");

  return (
    <header
      onPointerDown={pane.archived ? undefined : (e) => beginDrag(e, pane.id)}
      title={pane.archived ? undefined : "Drag to move this pane"}
      className={cn(
        "group/header @container relative m-2 mb-0 flex shrink-0 flex-col gap-1.5 rounded-xl border px-3 py-2.5 shadow-[inset_0_1px_0_rgb(255_255_255/0.12),0_8px_20px_-10px_rgb(0_0_0/0.6)] transition-colors",
        needs
          ? "border-warning/40 bg-[linear-gradient(rgb(251_191_36/0.15),rgb(251_191_36/0.05))]"
          : error
            ? "border-danger/40 bg-[linear-gradient(rgb(248_113_113/0.14),rgb(248_113_113/0.04))]"
            : "border-white/10 bg-white/[0.07]",
        !pane.archived && "cursor-grab active:cursor-grabbing",
      )}
    >
      {showProgress && progress && (
        <span title={progressLabel(progress)} className="absolute inset-x-0 -bottom-px">
          {/* Clipped to the header's rounded shape so the bar runs edge to edge along the bottom border. */}
          <span className="pointer-events-none absolute inset-x-0 bottom-0 h-12 overflow-hidden rounded-b-xl">
            <ProgressBar progress={progress} status={pane.status} className="absolute inset-x-0 bottom-0 h-0.5 rounded-none bg-transparent" />
          </span>
          {/* The percentage rides the end of the fill, so the bar carries its own number. */}
          {progress.percent != null && (
            <span
              className={cn(
                "absolute top-1/2 -translate-y-1/2 rounded-full border bg-panel px-1 font-mono text-[9px] leading-[13px] transition-[left] duration-700 ease-out",
                pane.status === "done" || progress.percent === 100
                  ? "border-success/40 text-success"
                  : needs
                    ? "border-warning/40 text-warning"
                    : "border-accent/40 text-accent",
              )}
              style={{ left: `clamp(12px, calc(${Math.max(progress.percent, 2)}% - 16px), calc(100% - 44px))` }}
            >
              {progress.percent}%
            </span>
          )}
        </span>
      )}
      <div className="flex min-w-0 items-center gap-2.5">
        <span className={cn("w-2.5 shrink-0 font-mono text-[10px] font-semibold", focused ? "text-accent" : "text-fg-subtle")}>{index}</span>
        <span
          title={name}
          className="relative grid size-[26px] shrink-0 place-items-center rounded-lg shadow-[inset_0_1px_0_rgb(255_255_255/0.25)]"
          style={{ background: `linear-gradient(135deg, color-mix(in srgb, ${color} 38%, transparent), color-mix(in srgb, ${color} 14%, transparent))` }}
        >
          <ProviderIcon provider={pane.provider} size={14} />
          <span className="absolute -right-1 -bottom-1 grid place-items-center rounded-full bg-panel p-[2px]">
            <StatusDot status={pane.status} />
          </span>
        </span>
        {renaming ? (
          <PaneTitleInput pane={pane} onDone={() => setRenaming(false)} className="h-6 flex-1 text-[13px] font-semibold" />
        ) : (
          <span
            className={cn("min-w-0 flex-1 truncate text-[13.5px] font-semibold tracking-[-0.01em]", focused ? "text-fg" : "text-fg-muted")}
            title={`${label === name ? name : `${name}: ${label}`}\nDouble-click to rename`}
            onDoubleClick={() => setRenaming(true)}
          >
            {label}
          </span>
        )}
        <BgIndicator pane={pane} />
        {/* Narrow panes keep a single row: the context reading moves up next to the title. */}
        <span className="@[300px]:hidden">
          <ContextMeter paneId={pane.id} provider={pane.provider} ring />
        </span>
        {pane.devUrl && (
          <button
            onMouseDown={stop}
            onClick={() => execute({ type: "browser.open", url: pane.devUrl! })}
            aria-label={`Open ${pane.devUrl} in the browser panel`}
            className="group/launch relative shrink-0"
          >
            <Pill className="border-accent/30 bg-accent/15 text-[11px] text-accent hover:bg-accent/25">
              Launch
              <ExternalLink size={10} />
            </Pill>
            {/* The URL itself, shown right away on hover (native titles take a second and are easy to miss). */}
            <span className="pointer-events-none absolute top-full right-0 z-50 mt-1.5 max-w-80 truncate rounded-md border border-border-strong bg-panel-raised px-2 py-1 font-mono text-[11px] whitespace-nowrap text-fg opacity-0 shadow-lg transition-opacity delay-150 group-hover/launch:opacity-100">
              {pane.devUrl}
            </span>
          </button>
        )}
        <div className={cn("flex shrink-0 items-center transition-opacity group-hover/header:opacity-100", focused ? "opacity-100" : "opacity-55")}>
          <PaneMenu pane={pane} onRename={() => setRenaming(true)} />
          {pane.archived ? (
            <button
              onMouseDown={stop}
              onClick={() => execute({ type: "pane.archive", target, restore: true })}
              title={`Put back into ${workspaceName ?? "its workspace"}`}
              className="ml-1 flex items-center gap-1 rounded-full border border-white/10 bg-white/[0.07] px-2.5 py-0.5 text-[11px] text-fg-muted hover:bg-white/15 hover:text-fg"
            >
              <ArchiveRestore size={12} />
              Restore
            </button>
          ) : (
            <span className="hidden items-center @[300px]:flex">
              <GlassButton title="Archive: keep it running, out of the way" onClick={() => execute({ type: "pane.archive", target })}>
                <Archive size={13} />
              </GlassButton>
              <GlassButton title={`${zoomed ? "Restore" : "Zoom"} (${keysFor({ type: "pane.zoom", target: "focused" })})`} onClick={() => execute({ type: "pane.zoom", target })}>
                {zoomed ? <Minimize2 size={13} /> : <Maximize2 size={13} />}
              </GlassButton>
              <GlassButton
                title={`Split right (${keysFor({ type: "pane.split", dir: "row" })})`}
                onClick={() => {
                  onFocus();
                  execute({ type: "pane.split", dir: "row" });
                }}
              >
                <Plus size={14} />
              </GlassButton>
            </span>
          )}
          <GlassButton title={`Close (${keysFor({ type: "pane.close", target: "focused" })})`} onClick={() => execute({ type: "pane.close", target })}>
            <X size={14} />
          </GlassButton>
        </div>
      </div>

      <div className="hidden min-w-0 items-center gap-1.5 overflow-hidden pl-5 text-[11px] text-fg-muted @[300px]:flex">
        {workspaceName && (
          <Pill className="max-w-32 bg-white/15 font-medium text-fg" title={`Workspace ${workspaceName}`}>
            {workspaceName}
          </Pill>
        )}
        {needs ? (
          <span className="min-w-0 truncate font-mono text-warning">Waiting for your input</span>
        ) : error ? (
          <span className="min-w-0 truncate font-mono text-danger">The process stopped with an error</span>
        ) : (
          <>
            {pane.worktree ? (
              <button onMouseDown={stop} onClick={() => execute({ type: "pane.showChanges", target })} title={`Own worktree: ${pane.worktree.path}\nClick to show changes`} className="min-w-0 shrink">
                <Pill className="max-w-44 border-teal-400/30 bg-teal-400/10 text-teal-200 hover:bg-teal-400/20">
                  <FolderGit2 size={11} className="shrink-0" />
                  <span className="truncate">{branch}</span>
                </Pill>
              </button>
            ) : (
              <Pill className="border-dashed bg-transparent text-fg-subtle" title="Works in the project folder, not its own worktree">
                <GitBranch size={11} className="shrink-0" />
                shared folder
              </Pill>
            )}
            {pane.model && (
              <button
                onMouseDown={stop}
                onClick={() => {
                  onFocus();
                  execute({ type: "ui.overlay", overlay: "model" });
                }}
                title={`${pane.model} · click to change`}
              >
                <Pill className="max-w-28 hover:bg-white/15 hover:text-fg">{displayModelName(pane.model)}</Pill>
              </button>
            )}
          </>
        )}
        <span className="flex-1" />
        <PaneCost pane={pane} />
        <ContextMeter paneId={pane.id} provider={pane.provider} ring />
      </div>
    </header>
  );
}
