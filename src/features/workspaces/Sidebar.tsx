import { getVersion } from "@tauri-apps/api/app";
import { useEffect, useState } from "react";
import logoUrl from "@/assets/logo.svg";
import { Activity, ChevronRight, GitBranch, History, Inbox, Moon, Plus, Settings, Sun } from "lucide-react";
import { useProgress } from "@/features/progress/progress";
import { useReviews } from "@/features/review/reviews";
import { cn } from "@/lib/cn";
import { execute } from "@/commands/bus";
import { keysFor } from "@/commands/keymap";
import { StatusDot } from "@/features/terminal/StatusDot";
import { ActivityFeed } from "./ActivityFeed";
import { ContextMenu, useContextMenu, type MenuItem } from "@/components/ContextMenu";
import { paneMenu, workspaceMenu } from "./sidebarMenus";
import { paneLabel, providerName, useWorkspaces, type Pane, type Workspace } from "@/stores/workspaces";
import { useSortable } from "./useSortable";
import { ProviderIcon } from "@/features/providers/ProviderIcon";
import { WorktreeList } from "@/features/worktree/WorktreeList";
import { ProgressBar, progressLabel } from "@/features/progress/SessionsOverview";
import { themeById } from "@/features/theme/themes";
import { UsageButton } from "@/features/usage/UsageMeter";

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

/** Thin progress bar under an agent's sidebar row. */
function PaneProgress({ pane }: { pane: Pane }) {
  const progress = useProgress((s) => s.byPane[pane.id]);
  if (pane.provider === "shell" || progress?.percent == null) return null;
  return (
    <span className="mt-1.5 block" title={progressLabel(progress)}>
      <ProgressBar progress={progress} status={pane.status} className="h-[3px]" />
    </span>
  );
}

/** Percent done of a working agent, from the progress estimates. */
function PanePercent({ pane }: { pane: Pane }) {
  const percent = useProgress((s) => s.byPane[pane.id]?.percent);
  if (pane.provider === "shell" || percent == null || pane.status === "idle") return null;
  return <span className={cn("shrink-0 font-mono text-[10px] tabular-nums", percent === 100 ? "text-success" : "text-fg-subtle")}>{percent}%</span>;
}

/** Second line of an agent row: its branch and what it is doing right now. */
function PaneDetail({ pane }: { pane: Pane }) {
  const step = useProgress((s) => s.byPane[pane.id]?.step);
  const branch = pane.worktree?.branch.replace(/^winfer\//, "");
  const doing = pane.status === "needs-input" ? "Needs your input" : step;
  if (!branch && !doing) return null;
  return (
    <span className="mt-0.5 flex min-w-0 items-center gap-1 text-[11px] text-fg-subtle">
      {branch && (
        <span className="flex min-w-0 shrink items-center gap-1 font-mono text-[10px]">
          <GitBranch size={10} className="shrink-0" />
          <span className="truncate">{branch}</span>
        </span>
      )}
      {branch && doing && <span className="shrink-0">·</span>}
      {doing && <span className={cn("min-w-0 flex-1 truncate", pane.status === "needs-input" && "text-warning")}>{doing}</span>}
    </span>
  );
}

/** A colored monogram per workspace, so they can be told apart at a glance. */
function WorkspaceAvatar({ name, active }: { name: string; active: boolean }) {
  let hash = 0;
  for (const ch of name) hash = (hash * 31 + ch.charCodeAt(0)) | 0;
  const hue = Math.abs(hash) % 360;
  const color = `hsl(${hue} 65% 62%)`;
  return (
    <span
      className={cn("flex size-5 shrink-0 items-center justify-center rounded-md text-[11px] font-semibold uppercase transition-opacity", !active && "opacity-80")}
      style={{ color, background: `color-mix(in srgb, ${color} 18%, transparent)` }}
    >
      {/* The last word: "orbit-web" and "orbit-api" get W and A, not two O's. */}
      {name.split(/[-_ ./]+/).filter(Boolean).pop()?.replace(/[^a-z0-9]/gi, "").charAt(0) || "·"}
    </span>
  );
}

/** One dot per agent of a collapsed workspace: its sessions' state without opening it. */
function WorkspaceDots({ panes }: { panes: Pane[] }) {
  const agents = panes.filter((p) => p.provider !== "shell");
  if (agents.length === 0) return panes.length > 0 ? <span className="text-[11px] text-fg-subtle">{panes.length}</span> : null;
  const shown = agents.slice(0, 5);
  return (
    <span className="flex items-center gap-1" title={`${agents.length} agent${agents.length === 1 ? "" : "s"}`}>
      {shown.map((p) => (
        <StatusDot key={p.id} status={p.status} />
      ))}
      {agents.length > shown.length && <span className="text-[10px] text-fg-subtle">+{agents.length - shown.length}</span>}
    </span>
  );
}

/** Small uppercase label above a sidebar section. */
function SectionLabel({ children, action }: { children: React.ReactNode; action?: React.ReactNode }) {
  return (
    <div className="flex h-7 items-center justify-between pr-1 pl-2.5">
      <span className="text-[10.5px] font-medium tracking-[0.08em] text-fg-subtle uppercase">{children}</span>
      {action}
    </div>
  );
}

const navItem = "flex w-full items-center gap-2.5 rounded-md px-2.5 py-1.5 text-left text-[13px] text-fg-muted transition-colors duration-150 hover:bg-hover hover:text-fg";

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
      className={navItem}
    >
      <Activity size={14} className="shrink-0" />
      <span className="flex-1">Sessions</span>
      {working.length > 0 && (
        <span className="flex items-center gap-1.5 font-mono text-[11px] text-fg-subtle">
          <span className="size-1.5 animate-pulse rounded-full bg-success" />
          {working.length}
          {average !== null && <span className="text-fg-subtle/70">· {average}%</span>}
        </span>
      )}
    </button>
  );
}

/** The logo and the running build's version, bottom left. */
function AppVersion() {
  const [version, setVersion] = useState<string | null>(null);
  useEffect(() => {
    getVersion().then(setVersion, () => setVersion(null));
  }, []);
  return (
    <span className="flex min-w-0 flex-1 items-center gap-2.5" title={version ? `winfer ${version}` : "winfer"}>
      <img src={logoUrl} alt="" className="size-7 shrink-0" draggable={false} />
      <span className="flex min-w-0 flex-col leading-tight">
        <span className="text-[13px] font-medium text-fg">winfer</span>
        {version && <span className="truncate font-mono text-[10px] text-fg-subtle">v{version}</span>}
      </span>
    </span>
  );
}

/** Opens the review inbox; the count is finished work waiting in this workspace (in the overview: in any). */
function ReviewEntry() {
  const workspaceId = useWorkspaces((s) => s.activeWorkspaceId);
  const everywhere = useWorkspaces((s) => s.view === "overview");
  const count = useReviews((s) => s.reviews.filter((r) => (everywhere || r.workspaceId === workspaceId) && r.status === "open").length);
  return (
    <button onClick={() => execute({ type: "ui.overlay", overlay: "review" })} title={`Review finished work (${keysFor({ type: "ui.overlay", overlay: "review" })})`} className={navItem}>
      <Inbox size={14} className="shrink-0" />
      <span className="flex-1">Review</span>
      {count > 0 && (
        <span key={count} className="anim-grow min-w-[18px] rounded-full bg-accent px-1.5 text-center text-[10.5px] leading-[18px] font-semibold text-on-accent">
          {count}
        </span>
      )}
    </button>
  );
}

/** Where a dragged sidebar row would land. */
function DropLine({ at }: { at: "top" | "bottom" }) {
  return <span className={cn("pointer-events-none absolute inset-x-1 z-10 h-0.5 rounded-full bg-accent", at === "top" ? "-top-[3px]" : "-bottom-[3px]")} />;
}

type OpenMenu = (e: React.MouseEvent, target: { title: string; items: MenuItem[] }) => void;

/** An expanded workspace's sessions, in the order the user dragged them into. */
function WorkspacePanes({ workspace: ws, focusedPaneId, openMenu }: { workspace: Workspace; focusedPaneId: string | null; openMenu: OpenMenu }) {
  const movePaneInList = useWorkspaces((s) => s.movePaneInList);
  const { drag, begin, dropLine } = useSortable(movePaneInList);
  return (
    <div className="relative pr-1 pb-1.5 pl-[17px]">
      {/* Guide line tying the sessions to their workspace. */}
      <span className="absolute top-0 bottom-2.5 left-[17px] w-px bg-border" />
      {ws.panes.map((pane, i) => {
        const focused = pane.id === focusedPaneId;
        const line = dropLine(i, ws.panes.length);
        return (
          <button
            key={pane.id}
            data-sort-id={pane.id}
            onClick={() => execute({ type: "pane.focus", target: { id: pane.id } })}
            onPointerDown={(e) => begin(e, pane.id)}
            onContextMenu={(e) => openMenu(e, { title: pane.worktree?.branch ?? pane.cwd, items: paneMenu(pane) })}
            className={cn(
              "anim-slide relative ml-2 block w-[calc(100%-0.5rem)] rounded-md px-2 py-1.5 text-left text-[13px] transition-colors duration-150",
              focused ? "bg-active text-fg" : "text-fg-muted hover:bg-hover hover:text-fg",
              drag?.id === pane.id && "opacity-50",
            )}
          >
            {line && <DropLine at={line} />}
            {/* Marks the focused session on the guide line. */}
            {focused && <span className="absolute top-1.5 bottom-1.5 -left-2 w-0.5 -translate-x-1/2 rounded-full bg-accent" />}
            {/* Keyed by status: a short highlight when an agent finishes or starts waiting. */}
            {(pane.status === "done" || pane.status === "needs-input") && (
              <span key={pane.status} className="anim-flash pointer-events-none absolute inset-0 rounded-md" />
            )}
            <span className="flex items-center gap-2">
              <StatusDot status={pane.status} />
              <ProviderIcon provider={pane.provider} size={12} />
              <PaneName pane={pane} className={cn("flex-1", (pane.unseen || focused) && "font-medium text-fg")} />
              <PanePercent pane={pane} />
            </span>
            <span className="block pl-[26px]">
              <PaneDetail pane={pane} />
              <PaneProgress pane={pane} />
            </span>
          </button>
        );
      })}
      {ws.panes.length === 0 && <span className="ml-2 block px-2 py-1.5 text-[12px] text-fg-subtle">No sessions yet — {keysFor({ type: "ui.overlay", overlay: "newPane" })} starts one</span>}
      <WorktreeList workspace={ws} />
    </div>
  );
}

const iconButton = "rounded-md p-1.5 text-fg-subtle transition-colors hover:bg-hover hover:text-fg";

export function Sidebar() {
  const { workspaces, activeWorkspaceId, focusedPaneId, expandedWorkspaces, toggleWorkspaceExpanded, moveWorkspace, theme, toggleTheme } = useWorkspaces();
  const [menu, openMenu, closeMenu] = useContextMenu<{ title: string; items: MenuItem[] }>();
  const { drag, begin, dropLine } = useSortable(moveWorkspace);

  return (
    <aside className="anim-slide flex h-full w-64 shrink-0 flex-col overflow-hidden rounded-[var(--radius-pane)] border border-border bg-panel">
      <div className="flex flex-col gap-px p-2">
        <SessionsEntry />
        <ReviewEntry />
        <button onClick={() => execute({ type: "ui.overlay", overlay: "history" })} title={`Merged features (${keysFor({ type: "ui.overlay", overlay: "history" })})`} className={navItem}>
          <History size={14} className="shrink-0" />
          <span className="flex-1">History</span>
        </button>
      </div>

      <div className="mx-3 border-t border-border" />

      <nav className="flex-1 overflow-y-auto px-2 pt-2 pb-2 select-none">
        <SectionLabel
          action={
            <button onClick={() => execute({ type: "workspace.create" })} className="rounded p-1 text-fg-subtle hover:bg-hover hover:text-fg" title="New workspace">
              <Plus size={13} />
            </button>
          }
        >
          Workspaces
        </SectionLabel>
        {workspaces.map((ws, i) => {
          const active = ws.id === activeWorkspaceId;
          const expanded = expandedWorkspaces.includes(ws.id);
          const line = dropLine(i, workspaces.length);
          return (
            <div
              key={ws.id}
              data-sort-id={ws.id}
              className={cn(
                "relative mb-1 rounded-lg transition-[background-color,opacity] duration-150",
                active && "bg-panel-raised ring-1 ring-border",
                drag?.id === ws.id && "opacity-50",
              )}
            >
              {line && <DropLine at={line} />}
              <button
                onClick={() => toggleWorkspaceExpanded(ws.id)}
                onDoubleClick={() => execute({ type: "workspace.switch", target: { id: ws.id } })}
                onPointerDown={(e) => begin(e, ws.id)}
                onContextMenu={(e) => openMenu(e, { title: ws.rootDir, items: workspaceMenu(ws) })}
                title={`${ws.rootDir}\nDouble-click to open · drag to reorder`}
                className={cn(
                  "group flex w-full items-center gap-2.5 rounded-lg px-2 py-1.5 text-left text-[13px] transition-colors duration-150",
                  active ? "font-semibold text-fg" : "text-fg-muted hover:bg-hover hover:text-fg",
                )}
              >
                <WorkspaceAvatar name={ws.name} active={active} />
                <span className="flex-1 truncate">{ws.name}</span>
                {!expanded && <WorkspaceDots panes={ws.panes} />}
                <ChevronRight
                  size={13}
                  className={cn("shrink-0 text-fg-subtle transition-transform duration-200", expanded ? "rotate-90" : "opacity-0 group-hover:opacity-100")}
                />
              </button>

              {expanded && <WorkspacePanes workspace={ws} focusedPaneId={active ? focusedPaneId : null} openMenu={openMenu} />}
            </div>
          );
        })}
      </nav>

      {menu && <ContextMenu x={menu.x} y={menu.y} title={menu.target.title} items={menu.target.items} onClose={closeMenu} />}
      <ActivityFeed />
      <footer className="flex items-center gap-1 border-t border-border bg-panel-raised/40 px-3 py-2.5">
        <AppVersion />
        <button onClick={toggleTheme} className={iconButton} title={`Switch to ${themeById(themeById(theme).pair).name} (more themes in Settings)`}>
          <span key={theme} className="anim-grow block">
            {themeById(theme).mode === "dark" ? <Sun size={14} /> : <Moon size={14} />}
          </span>
        </button>
        <UsageButton onOpen={() => execute({ type: "ui.settings", page: "usage" })} title="Usage" />
        <button onClick={() => execute({ type: "ui.settings" })} className={iconButton} title={`Settings (${keysFor({ type: "ui.settings" })})`}>
          <Settings size={14} />
        </button>
      </footer>
    </aside>
  );
}
