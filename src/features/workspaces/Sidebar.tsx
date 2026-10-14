import { Activity, ChevronDown, ChevronRight, History, Inbox, Moon, Plus, Settings, Sun } from "lucide-react";
import { useProgress } from "@/features/progress/progress";
import { useReviews } from "@/features/review/reviews";
import { cn } from "@/lib/cn";
import { execute } from "@/commands/bus";
import { keysFor } from "@/commands/keymap";
import { StatusDot } from "@/features/terminal/StatusDot";
import { ActivityFeed } from "./ActivityFeed";
import { WorkspaceMenu } from "./WorkspaceMenu";
import { useCallback, useState } from "react";
import { paneLabel, providerName, useWorkspaces, type Pane } from "@/stores/workspaces";
import { ProviderIcon } from "@/features/providers/ProviderIcon";
import { WorktreeList } from "@/features/worktree/WorktreeList";
import { ProgressBar, progressLabel } from "@/features/progress/SessionsOverview";

/** The agent's own title (or its task), with the CLI's name in the tooltip. */
function PaneName({ pane, className }: { pane: Pane; className?: string }) {
  const providers = useWorkspaces((s) => s.providers);
  const task = useProgress((s) => s.byPane[pane.id]?.title ?? s.byPane[pane.id]?.task);
  const label = paneLabel(pane, providers, task);
  const name = providerName(providers, pane.provider);
  return (
    <span className={cn("min-w-0 truncate", className)} title={label === name ? name : `${name}: ${label}`}>
      {label}
    </span>
  );
}

/** Thin progress bar along the bottom of an agent's sidebar row. */
function PaneProgress({ pane }: { pane: Pane }) {
  const progress = useProgress((s) => s.byPane[pane.id]);
  if (pane.provider === "shell" || (progress?.percent == null && pane.status !== "working")) return null;
  return (
    <span className="absolute right-2 bottom-0.5 left-5" title={progressLabel(progress)}>
      <ProgressBar progress={progress} status={pane.status} className="h-0.5" />
    </span>
  );
}

/** Percent done of a working agent, from the progress estimates. */
function PanePercent({ pane }: { pane: Pane }) {
  const percent = useProgress((s) => s.byPane[pane.id]?.percent);
  if (pane.provider === "shell" || percent == null || pane.status === "idle") return null;
  return <span className={cn("font-mono text-[10px]", !pane.worktree && "ml-auto", percent === 100 ? "text-success" : "text-fg-subtle")}>{percent}%</span>;
}

/** Opens the sessions overview; shows how many agents are working, with their average progress. */
function SessionsEntry() {
  const workspaces = useWorkspaces((s) => s.workspaces);
  const byPane = useProgress((s) => s.byPane);
  const working = workspaces.flatMap((w) => w.panes).filter((p) => p.provider !== "shell" && p.status === "working");
  const known = working.map((p) => byPane[p.id]?.percent).filter((n): n is number => n != null);
  const average = known.length ? Math.round(known.reduce((a, b) => a + b, 0) / known.length) : null;
  return (
    <button
      onClick={() => execute({ type: "ui.overlay", overlay: "progress" })}
      title={`Session progress (${keysFor({ type: "ui.overlay", overlay: "progress" })})`}
      className="mx-2 flex items-center gap-2 rounded-md px-2 py-1.5 text-left text-[13px] text-fg-muted hover:bg-hover hover:text-fg"
    >
      <Activity size={14} />
      <span className="flex-1">Sessions</span>
      {working.length > 0 && (
        <span className="font-mono text-[11px] text-fg-subtle">
          {working.length} working{average !== null && ` · ${average}%`}
        </span>
      )}
    </button>
  );
}

/** Opens the review inbox; the count is finished work waiting in this workspace. */
function ReviewEntry() {
  const workspaceId = useWorkspaces((s) => s.activeWorkspaceId);
  const count = useReviews((s) => s.reviews.filter((r) => r.workspaceId === workspaceId && r.status === "open").length);
  return (
    <button
      onClick={() => execute({ type: "ui.overlay", overlay: "review" })}
      title={`Review finished work (${keysFor({ type: "ui.overlay", overlay: "review" })})`}
      className="mx-2 mb-2 flex items-center gap-2 rounded-md px-2 py-1.5 text-left text-[13px] text-fg-muted hover:bg-hover hover:text-fg"
    >
      <Inbox size={14} />
      <span className="flex-1">Review</span>
      {count > 0 && <span className="rounded bg-accent px-1.5 text-[11px] font-medium text-white">{count}</span>}
    </button>
  );
}

export function Sidebar() {
  const { workspaces, activeWorkspaceId, focusedPaneId, theme, toggleTheme } = useWorkspaces();
  const [menu, setMenu] = useState<{ id: string; x: number; y: number } | null>(null);
  const closeMenu = useCallback(() => setMenu(null), []);
  const menuWorkspace = menu && workspaces.find((w) => w.id === menu.id);

  return (
    <aside className="flex h-full w-60 shrink-0 flex-col rounded-[var(--radius-pane)] border border-border bg-panel">
      <div className="flex items-center justify-between px-4 pt-4 pb-2">
        <span className="text-xs font-medium tracking-wide text-fg-subtle">Workspaces</span>
        <button
          onClick={() => execute({ type: "workspace.create" })}
          className="rounded p-1 text-fg-subtle hover:bg-hover hover:text-fg"
          title="New workspace"
        >
          <Plus size={14} />
        </button>
      </div>

      <nav className="flex-1 overflow-y-auto px-2">
        {workspaces.map((ws) => {
          const active = ws.id === activeWorkspaceId;
          const count = ws.panes.length;
          const attention = ws.panes.some((p) => p.unseen);
          return (
            <div key={ws.id} className="mb-0.5">
              <button
                onClick={() => execute({ type: "workspace.switch", target: { id: ws.id } })}
                onContextMenu={(e) => {
                  e.preventDefault();
                  setMenu({ id: ws.id, x: e.clientX, y: e.clientY });
                }}
                title={ws.rootDir}
                className={cn(
                  "flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left text-[13px]",
                  active ? "bg-active font-semibold text-fg" : "text-fg-muted hover:bg-hover hover:text-fg",
                )}
              >
                <span className="flex-1 truncate">{ws.name}</span>
                {attention && <span className="size-1.5 rounded-full bg-warning" title="An agent needs attention" />}
                {count > 0 && (
                  <span className="rounded bg-hover px-1.5 text-[11px] font-normal text-fg-muted">{count}</span>
                )}
                {active ? <ChevronDown size={13} /> : count > 0 && <ChevronRight size={13} />}
              </button>

              {active &&
                ws.panes.map((pane) => (
                  <button
                    key={pane.id}
                    onClick={() => execute({ type: "pane.focus", target: { id: pane.id } })}
                    className={cn(
                      "relative flex w-full items-center gap-2 rounded-md py-1.5 pr-2 pl-5 text-left text-[13px]",
                      pane.id === focusedPaneId ? "bg-hover text-fg" : "text-fg-muted hover:bg-hover hover:text-fg",
                    )}
                  >
                    <StatusDot status={pane.status} />
                    <ProviderIcon provider={pane.provider} size={12} />
                    <PaneName pane={pane} className={cn(pane.unseen && "font-semibold text-fg")} />
                    {pane.worktree && (
                      <span className="ml-auto truncate font-mono text-[10px] text-fg-subtle">{pane.worktree.branch.replace(/^winfer\//, "")}</span>
                    )}
                    <PanePercent pane={pane} />
                    <PaneProgress pane={pane} />
                  </button>
                ))}
              {active && <WorktreeList workspace={ws} />}
            </div>
          );
        })}
      </nav>

      <SessionsEntry />
      <ReviewEntry />
      <button
        onClick={() => execute({ type: "ui.overlay", overlay: "history" })}
        title={`Merged features (${keysFor({ type: "ui.overlay", overlay: "history" })})`}
        className="mx-2 mb-2 -mt-2 flex items-center gap-2 rounded-md px-2 py-1.5 text-left text-[13px] text-fg-muted hover:bg-hover hover:text-fg"
      >
        <History size={14} />
        <span className="flex-1">History</span>
      </button>
      {menu && menuWorkspace && <WorkspaceMenu ws={menuWorkspace} x={menu.x} y={menu.y} onClose={closeMenu} />}
      <ActivityFeed />
      <footer className="flex items-center gap-2 border-t border-border px-4 py-3">
        <span className="flex size-6 items-center justify-center rounded-full bg-accent text-[11px] font-semibold text-white">
          W
        </span>
        <span className="flex-1 text-xs text-fg-muted">Local</span>
        <button
          onClick={toggleTheme}
          className="rounded p-1 text-fg-subtle hover:bg-hover hover:text-fg"
          title={theme === "dark" ? "Switch to light mode" : "Switch to dark mode"}
        >
          {theme === "dark" ? <Sun size={14} /> : <Moon size={14} />}
        </button>
        <button onClick={() => execute({ type: "ui.settings" })} className="rounded p-1 text-fg-subtle hover:bg-hover hover:text-fg" title={`Settings (${keysFor({ type: "ui.settings" })})`}>
          <Settings size={14} />
        </button>
      </footer>
    </aside>
  );
}
