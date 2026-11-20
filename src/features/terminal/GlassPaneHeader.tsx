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
import { PaneNote } from "./PaneNote";
import { PaneTitleInput } from "./PaneTitleInput";
import { displayModelName } from "@/lib/modelName";
import { BgIndicator } from "@/features/code/BgIndicator";

const stop = (e: React.MouseEvent) => e.stopPropagation();

const GlassButton = ({ title, onClick, danger, children }: { title: string; onClick?: () => void; danger?: boolean; children: React.ReactNode }) => (
  <button
    title={title}
    onClick={onClick}
    onMouseDown={stop}
    className={cn(
      "grid size-6 place-items-center rounded-md text-fg-subtle transition-colors",
      danger ? "hover:bg-danger/15 hover:text-danger" : "hover:bg-hover hover:text-fg",
    )}
  >
    {children}
  </button>
);

const Pill = ({ className, ...props }: React.HTMLAttributes<HTMLSpanElement>) => (
  <span {...props} className={cn("glass-pill inline-flex min-w-0 shrink-0 items-center gap-1 truncate rounded-full px-1.5 py-px", className)} />
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
 * Single Spatial Island pane header: one floating frosted capsule inset 8px from the top, left,
 * and right edges of the pane. Left: provider emblem + live status dot and a two-line identity &
 * metadata stack. Right: live background tasks, dev server launch, telemetry pill, divider, and
 * action icons.
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
        "group/header @container glass-header relative mx-2 mt-2 mb-0 flex shrink-0 items-center gap-2.5 rounded-xl px-2 py-[7px] transition-colors",
        needs
          ? "bg-[color-mix(in_srgb,var(--color-warning)_14%,var(--color-panel-raised))]"
          : error
            ? "bg-[color-mix(in_srgb,var(--color-danger)_14%,var(--color-panel-raised))]"
            : focused
              ? "glass-header-focused"
              : "glass-header-rest",
        !pane.archived && "cursor-grab active:cursor-grabbing",
      )}
    >
      {showProgress && progress && (
        <span title={progressLabel(progress)} className="pointer-events-none absolute inset-0 overflow-hidden rounded-[11px]">
          <ProgressBar progress={progress} status={pane.status} className="absolute inset-x-0 bottom-0 h-0.5 rounded-none bg-transparent" />
        </span>
      )}

      {/* Left: Provider badge + live status dot */}
      <span
        title={name}
        className="relative grid size-[30px] shrink-0 place-items-center rounded-lg shadow-[inset_0_1px_0_rgb(255_255_255/0.22)]"
        style={{ background: `linear-gradient(135deg, color-mix(in srgb, ${color} 32%, transparent), color-mix(in srgb, ${color} 14%, transparent))` }}
      >
        <ProviderIcon provider={pane.provider} size={14} />
        <span className="absolute -right-[3px] -bottom-[3px] grid place-items-center rounded-full bg-panel p-[2px]">
          <StatusDot status={pane.status} />
        </span>
      </span>

      {/* Center: Two-line identity & inline metadata stack */}
      <div className="flex min-w-0 flex-1 flex-col gap-0.5">
        <div className="flex min-w-0 items-center gap-1.5">
          <span
            className={cn(
              "shrink-0 rounded-[4px] px-[4.5px] py-px font-mono text-[10px] leading-[1.3] font-semibold",
              focused ? "bg-accent/15 text-accent" : "glass-pill text-fg-subtle",
            )}
          >
            {index}
          </span>
          {renaming ? (
            <PaneTitleInput pane={pane} onDone={() => setRenaming(false)} className="h-5 flex-1 text-[13px] font-semibold" />
          ) : (
            <span
              className={cn("min-w-0 flex-1 truncate text-[13px] tracking-[-0.012em]", focused ? "font-semibold text-fg" : "font-medium text-fg-muted")}
              title={`${label === name ? name : `${name}: ${label}`}\nDouble-click to rename`}
              onDoubleClick={() => setRenaming(true)}
            >
              {label}
            </span>
          )}
        </div>

        <div className="hidden min-w-0 items-center gap-1.5 overflow-hidden text-[11px] text-fg-subtle @[260px]:flex">
          {workspaceName && (
            <Pill className="max-w-28 bg-hover font-medium text-fg text-[10.5px]" title={`Workspace ${workspaceName}`}>
              {workspaceName}
            </Pill>
          )}
          {needs ? (
            <span className="min-w-0 truncate font-mono text-[10.5px] text-warning">Waiting for your input</span>
          ) : error ? (
            <span className="min-w-0 truncate font-mono text-[10.5px] text-danger">The process stopped with an error</span>
          ) : (
            <>
              {pane.worktree ? (
                <button
                  onMouseDown={stop}
                  onClick={() => execute({ type: "pane.showChanges", target })}
                  aria-label={`Show changes for ${branch}`}
                  title={`Branch: ${branch}\nOwn worktree: ${pane.worktree.path}\nClick to show changes`}
                  className="min-w-0 shrink-0 @[420px]:shrink"
                >
                  <Pill className="glass-branch max-w-full font-mono text-[10.5px] @[420px]:max-w-44">
                    <FolderGit2 size={11} className="shrink-0" />
                    <span className="hidden truncate @[420px]:inline">{branch}</span>
                  </Pill>
                </button>
              ) : (
                <span className="inline-flex shrink-0 items-center gap-1 text-[11px] text-fg-subtle" title="Works in the project folder, not its own worktree">
                  <GitBranch size={11} className="shrink-0" />
                  <span className="hidden @[420px]:inline">shared folder</span>
                </span>
              )}
              {pane.model && (
                <>
                  <span className="hidden text-[10px] opacity-40 @[460px]:inline">·</span>
                  <button
                    onMouseDown={stop}
                    onClick={() => {
                      onFocus();
                      execute({ type: "ui.overlay", overlay: "model" });
                    }}
                    title={`${pane.model} · click to change`}
                    className="hidden max-w-28 shrink-0 truncate text-[11px] text-fg-muted hover:text-fg @[460px]:inline"
                  >
                    {displayModelName(pane.model)}
                  </button>
                </>
              )}
            </>
          )}
        </div>
      </div>

      {/* Right: Live background tasks, launch, unified telemetry pill, divider, actions */}
      <div className="flex shrink-0 items-center gap-1.5">
        <BgIndicator pane={pane} />
        {pane.devUrl && (
          <button
            onMouseDown={stop}
            onClick={() => execute({ type: "browser.open", url: pane.devUrl!, paneId: pane.id })}
            aria-label={`Open ${pane.devUrl} in the browser panel`}
            className="group/launch relative shrink-0"
          >
            <span className="inline-flex h-6 items-center gap-1 rounded-full border border-accent/30 bg-accent/15 px-2 text-[11px] font-medium text-accent transition-colors hover:bg-accent/25">
              Launch
              <ExternalLink size={10} />
            </span>
            {/* The URL itself, shown right away on hover (native titles take a second and are easy to miss). */}
            <span className="pointer-events-none absolute top-full right-0 z-50 mt-1.5 max-w-80 truncate rounded-md bg-panel-raised px-2 py-1 font-mono text-[11px] whitespace-nowrap text-fg opacity-0 shadow-lg transition-opacity delay-150 group-hover/launch:opacity-100">
              {pane.devUrl}
            </span>
          </button>
        )}
        <div className="glass-pill hidden h-6 shrink-0 items-center gap-1.5 rounded-full px-2 has-[*]:inline-flex">
          <PaneCost pane={pane} />
          <ContextMeter paneId={pane.id} provider={pane.provider} ring />
        </div>
        <span className="h-4 w-px shrink-0 bg-border" />
        <div className={cn("flex shrink-0 items-center gap-px transition-opacity group-hover/header:opacity-100", focused ? "opacity-100" : "opacity-50")}>
          <PaneMenu pane={pane} onRename={() => setRenaming(true)} />
          <PaneNote paneId={pane.id} />
          {pane.archived ? (
            <button
              onMouseDown={stop}
              onClick={() => execute({ type: "pane.archive", target, restore: true })}
              title={`Put back into ${workspaceName ?? "its workspace"}`}
              className="glass-pill ml-1 flex h-6 items-center gap-1 rounded-full border border-border px-2.5 text-[11px] font-medium text-fg hover:bg-hover"
            >
              <ArchiveRestore size={12} />
              Restore
            </button>
          ) : (
            <span className="hidden items-center gap-px @[300px]:flex">
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
          <GlassButton danger title={`Close (${keysFor({ type: "pane.close", target: "focused" })})`} onClick={() => execute({ type: "pane.close", target })}>
            <X size={14} />
          </GlassButton>
        </div>
      </div>
    </header>
  );
}
