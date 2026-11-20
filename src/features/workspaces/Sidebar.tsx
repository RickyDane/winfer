import { getVersion } from "@tauri-apps/api/app";
import { useEffect, useState } from "react";
import logoUrl from "@/assets/logo.svg";
import { Activity, Archive, ChevronRight, ChevronsDownUp, Clock, Download, GitBranch, Inbox, Moon, Plus, Search, Settings, Sun } from "lucide-react";
import { progressTitle, useProgress } from "@/features/progress/progress";
import { useReviews } from "@/features/review/reviews";
import { cn } from "@/lib/cn";
import { execute } from "@/commands/bus";
import { keysFor } from "@/commands/keymap";
import { StatusDot } from "@/features/terminal/StatusDot";
import { ActivityFeed } from "./ActivityFeed";
import { ContextMenu, useContextMenu, type MenuItem } from "@/components/ContextMenu";
import { paneMenu, workspaceMenu } from "./sidebarMenus";
import { DEFAULT_SIDEBAR_WIDTH, livePanes, paneLabel, providerName, useWorkspaces, useWorkspacesPick, type Pane, type Workspace } from "@/stores/workspaces";
import { useSortable } from "./useSortable";
import { ProviderIcon } from "@/features/providers/ProviderIcon";
import { PaneTitleInput } from "@/features/terminal/PaneTitleInput";
import { WorktreeList } from "@/features/worktree/WorktreeList";
import { themeById } from "@/features/theme/themes";
import { useUpdates } from "@/features/updates/updater";
import { checkoutDetails, paneTooltip, useSidebarCheckouts } from "./sidebarTooltips";

/** The agent's own title (or its task), with the CLI's name in the tooltip. */
export function PaneName({ pane, className, tooltip }: { pane: Pane; className?: string; tooltip?: string }) {
  const providers = useWorkspaces((s) => s.providers);
  const task = useProgress((s) => progressTitle(s.byPane[pane.id]));
  const title = useProgress((s) => s.byPane[pane.id]?.title ?? null);
  const label = paneLabel(pane, providers, task, title);
  const name = providerName(providers, pane.provider);
  return (
    <span className={cn("min-w-0 truncate", className)} title={tooltip ?? (label === name ? name : `${name}: ${label}`)}>
      {label}
    </span>
  );
}

/** Right of an agent's title: its progress, or "needs you" / "done" / "error". */
function PaneMeta({ pane }: { pane: Pane }) {
  const percent = useProgress((s) => s.byPane[pane.id]?.percent);
  if (pane.provider === "shell") return null;
  if (pane.status === "needs-input") return <span className="shrink-0 text-[10.5px] font-medium text-warning">needs you</span>;
  if (pane.status === "done") return <span className="shrink-0 text-[10.5px] text-success">done</span>;
  if (pane.status === "error") return <span className="shrink-0 text-[10.5px] text-danger">error</span>;
  if (percent == null || pane.status === "idle") return null;
  return <span className="shrink-0 font-mono text-[10.5px] text-fg-subtle tabular-nums">{percent}%</span>;
}

/** The agent's mark on a rounded tile, with its status as a dot in the corner. */
export function AgentTile({ pane, ring = "var(--color-panel)" }: { pane: Pane; ring?: string }) {
  return (
    <span className="relative grid size-[26px] shrink-0 place-items-center rounded-[7px] bg-hover">
      <ProviderIcon provider={pane.provider} size={14} />
      {pane.provider !== "shell" && (
        <span className="absolute -right-0.5 -bottom-0.5 flex rounded-full p-[2px]" style={{ background: ring }}>
          <StatusDot status={pane.status} />
        </span>
      )}
    </span>
  );
}

/** Second line of an agent row: its branch and what it is doing right now. */
export function PaneDetail({ pane }: { pane: Pane }) {
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

/** How many agents (or sessions) a collapsed workspace has. */
function WorkspaceCount({ panes }: { panes: Pane[] }) {
  const agents = panes.filter((p) => p.provider !== "shell").length;
  if (!panes.length) return null;
  return <span className="font-mono text-[10.5px] font-normal text-fg-subtle">{agents ? `${agents} agent${agents === 1 ? "" : "s"}` : panes.length}</span>;
}

/** A collapsed workspace's agents: how many work, wait for you or are done. */
function WorkspaceSummary({ panes }: { panes: Pane[] }) {
  const agents = panes.filter((p) => p.provider !== "shell");
  if (agents.length === 0) return null;
  const count = (status: Pane["status"]) => agents.filter((p) => p.status === status).length;
  const [working, waiting, done] = [count("working"), count("needs-input"), count("done")];
  return (
    <div className="-mt-1 flex flex-wrap gap-1 pb-1.5 pl-[37px] text-[11px] text-fg-subtle">
      <span>{working} working</span>
      {waiting > 0 && <span className="font-medium text-warning">· {waiting} need{waiting === 1 ? "s" : ""} you</span>}
      {done > 0 && <span className="text-success">· {done} done</span>}
    </div>
  );
}

/** Small uppercase label above a sidebar section. */
function SectionLabel({ children, action }: { children: React.ReactNode; action?: React.ReactNode }) {
  return (
    <div className="mt-2.5 flex h-[26px] items-center justify-between pl-1.5">
      <span className="text-[11px] font-semibold text-fg-subtle">{children}</span>
      {action}
    </div>
  );
}

const tab = "flex h-7 min-w-0 flex-1 items-center justify-center gap-1.5 rounded-md text-xs text-fg-muted transition-colors duration-150 hover:bg-hover hover:text-fg";
/** A tab's label: shown when the sidebar is wide enough for all three, else the icon alone (the title says which). */
const tabLabel = "hidden @[212px]:inline";

/** Search, and Sessions · Review · History. */
function SidebarTop() {
  const workspaceId = useWorkspaces((s) => s.activeWorkspaceId);
  const everywhere = useWorkspaces((s) => s.view === "overview");
  const reviews = useReviews((s) => s.reviews.filter((r) => (everywhere || r.workspaceId === workspaceId) && r.status === "open").length);
  const keys = keysFor({ type: "ui.palette" });
  return (
    <div className="@container flex flex-col gap-2 border-b border-border px-2.5 pt-2.5 pb-2">
      <button
        onClick={() => execute({ type: "ui.palette" })}
        className="flex h-8 items-center gap-2 rounded-md border border-border bg-app pr-2 pl-2.5 text-left text-[12.5px] text-fg-subtle transition-colors hover:border-border-strong hover:text-fg-muted"
      >
        <Search size={14} />
        <span className="min-w-0 flex-1 truncate">Search or jump to…</span>
        {keys && <kbd className="hidden shrink-0 rounded border border-border px-1 font-mono text-[10px] @[200px]:block">{keys}</kbd>}
      </button>
      <div className="flex gap-px rounded-lg border border-border bg-app p-0.5">
        <button onClick={() => execute({ type: "ui.overlay", overlay: "progress" })} title={`Session progress (${keysFor({ type: "ui.overlay", overlay: "progress" })})`} className={tab}>
          <Activity size={14} className="shrink-0" />
          <span className={tabLabel}>Sessions</span>
        </button>
        <button onClick={() => execute({ type: "ui.overlay", overlay: "review" })} title={`Review finished work (${keysFor({ type: "ui.overlay", overlay: "review" })})`} className={tab}>
          <Inbox size={14} className="shrink-0" />
          <span className={tabLabel}>Review</span>
          {reviews > 0 && (
            <span key={reviews} className="anim-grow min-w-4 rounded-full bg-accent px-1 text-center text-[10px] leading-4 font-semibold text-on-accent">
              {reviews}
            </span>
          )}
        </button>
        <button onClick={() => execute({ type: "ui.overlay", overlay: "history" })} title={`Merged features (${keysFor({ type: "ui.overlay", overlay: "history" })})`} className={tab}>
          <Clock size={14} className="shrink-0" />
          <span className={tabLabel}>History</span>
        </button>
      </div>
    </div>
  );
}

/** Agents waiting for you in any workspace, with Approve right there. */
function NeedsYou() {
  const workspaces = useWorkspaces((s) => s.workspaces);
  const byPane = useProgress((s) => s.byPane);
  const waiting = workspaces.flatMap((w) => livePanes(w)).filter((p) => p.provider !== "shell" && p.status === "needs-input");
  if (!waiting.length) return null;
  return (
    <section className="mt-2.5 rounded-[10px] bg-warning/[0.06] p-1">
      <div className="flex h-[26px] items-center gap-[7px] px-1.5 text-[11px] font-semibold text-warning">
        <span className="size-[7px] animate-pulse rounded-full bg-warning" />
        Needs you
        <span className="font-mono font-normal text-fg-subtle">{waiting.length}</span>
      </div>
      {waiting.map((pane) => (
        <div
          key={pane.id}
          role="button"
          tabIndex={0}
          onClick={() => execute({ type: "pane.focus", target: { id: pane.id } })}
          className="flex cursor-pointer items-center gap-[9px] rounded-md px-1.5 py-[7px] hover:bg-warning/[0.08]"
        >
          <AgentTile pane={pane} />
          <span className="flex min-w-0 flex-1 flex-col">
            <PaneName pane={pane} className="text-[12.5px] text-fg" />
            <span className="truncate text-[11.5px] text-fg-muted">{byPane[pane.id]?.step ?? "Waiting for your input"}</span>
          </span>
          <button
            onClick={(e) => {
              e.stopPropagation();
              execute({ type: "pane.approve", target: { id: pane.id } });
            }}
            title="Approve what it's asking (sends its approve key)"
            className="h-6 shrink-0 rounded-md bg-accent px-2.5 text-[11.5px] font-medium text-on-accent hover:opacity-90"
          >
            Approve
          </button>
        </div>
      ))}
    </section>
  );
}

/** The logo and the running build's version, bottom left. */
function AppVersion() {
  const [version, setVersion] = useState<string | null>(null);
  useEffect(() => {
    getVersion().then(setVersion, () => setVersion(null));
  }, []);
  return (
    <span className="flex min-w-0 items-center gap-2" title={version ? `winfer ${version}` : "winfer"}>
      <span className="grid size-[26px] shrink-0 place-items-center rounded-[7px] bg-accent-soft">
        <img src={logoUrl} alt="" className="size-[18px]" draggable={false} />
      </span>
      <span className="flex min-w-0 flex-col leading-tight">
        <span className="text-xs font-semibold text-fg">winfer</span>
        {version && <span className="truncate font-mono text-[10px] text-fg-subtle">{version}</span>}
      </span>
    </span>
  );
}

/** Only when an update is out: opens Settings → Updates. */
function UpdateButton() {
  const available = useUpdates((s) => s.available);
  if (!available) return null;
  return (
    <button
      onClick={() => execute({ type: "ui.settings", page: "updates" })}
      title={`Update to winfer ${available.version}`}
      className="relative grid size-6 shrink-0 place-items-center rounded-[7px] bg-accent-soft text-accent hover:bg-accent/25"
    >
      <Download size={12} />
      <span className="absolute -top-0.5 -right-0.5 size-[7px] rounded-full bg-accent ring-2 ring-panel" />
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
  const providers = useWorkspaces((s) => s.providers);
  const byPane = useProgress((s) => s.byPane);
  const { listings, refresh } = useSidebarCheckouts();
  const { drag, begin, dropLine } = useSortable(movePaneInList);
  const [renaming, setRenaming] = useState<string | null>(null);
  const [showArchived, setShowArchived] = useState(false);
  const live = livePanes(ws);
  const archived = ws.panes.filter((p) => p.archived);
  // Archived rows sit in their own list, dimmed and not draggable.
  const row = (pane: Pane, i: number, sortable: boolean) => {
    const focused = pane.id === focusedPaneId;
    const progress = byPane[pane.id];
    const tooltip = paneTooltip(pane, providerName(providers, pane.provider), paneLabel(pane, providers, progressTitle(progress), progress?.title), progress, listings[pane.cwd]);
    const line = sortable ? dropLine(i, live.length) : null;
    return (
      // Not a <button>: the rename field sits inside the row, and WebKit won't focus inputs in buttons.
      <div
        key={pane.id}
        role="button"
        tabIndex={0}
        title={tooltip}
        onMouseEnter={() => refresh(pane.cwd)}
        onFocus={() => refresh(pane.cwd)}
        data-sort-id={sortable ? pane.id : undefined}
        onClick={() => execute({ type: "pane.focus", target: { id: pane.id } })}
        onKeyDown={(e) => {
          if (e.target !== e.currentTarget || (e.key !== "Enter" && e.key !== " ")) return;
          e.preventDefault();
          execute({ type: "pane.focus", target: { id: pane.id } });
        }}
        onPointerDown={sortable ? (e) => begin(e, pane.id) : undefined}
        onDoubleClick={() => setRenaming(pane.id)}
        onContextMenu={(e) => openMenu(e, { title: pane.worktree?.branch ?? pane.cwd, items: paneMenu(pane, () => setRenaming(pane.id)) })}
        className={cn(
          "anim-slide relative flex w-full cursor-pointer items-center gap-[9px] rounded-md py-1.5 pr-1.5 pl-2 text-left text-[12.5px] transition-colors duration-150",
          focused ? "bg-active text-fg" : "text-fg-muted hover:bg-hover hover:text-fg",
          drag?.id === pane.id && "opacity-50",
          pane.archived && !focused && "opacity-70",
        )}
      >
        {line && <DropLine at={line} />}
        {/* Keyed by status: a short highlight when an agent finishes or starts waiting. */}
        {(pane.status === "done" || pane.status === "needs-input") && (
          <span key={pane.status} className="anim-flash pointer-events-none absolute inset-0 rounded-md" />
        )}
        <AgentTile pane={pane} ring={focused ? "var(--color-active)" : "var(--color-panel)"} />
        <span className="flex min-w-0 flex-1 flex-col">
          <span className="flex items-center gap-1.5">
            {renaming === pane.id ? (
              <PaneTitleInput pane={pane} onDone={() => setRenaming(null)} className="h-5 flex-1 text-[12.5px]" />
            ) : (
              <PaneName pane={pane} tooltip={tooltip} className={cn("flex-1", (pane.unseen || focused) && "font-medium text-fg")} />
            )}
            <PaneMeta pane={pane} />
          </span>
          <PaneDetail pane={pane} />
        </span>
      </div>
    );
  };
  return (
    <div className="flex flex-col gap-px pt-0.5 pb-1.5">
      {live.map((pane, i) => row(pane, i, true))}
      {archived.length > 0 && (
        <div>
          <button
            onClick={() => setShowArchived((v) => !v)}
            className="flex h-7 w-full items-center gap-[9px] rounded-md pr-2 pl-[13px] text-left text-[12px] text-fg-subtle hover:bg-hover hover:text-fg"
            title={showArchived ? "Hide archived sessions" : "Show archived sessions"}
          >
            <Archive size={12} />
            <span className="flex-1">Archived · {archived.length}</span>
            <ChevronRight size={12} className={cn("transition-transform duration-200", showArchived && "rotate-90")} />
          </button>
          {showArchived && archived.map((pane, i) => row(pane, i, false))}
        </div>
      )}
      <WorktreeList workspace={ws} />
      <button
        onClick={() => {
          if (useWorkspaces.getState().activeWorkspaceId !== ws.id) execute({ type: "workspace.switch", target: { id: ws.id } });
          execute({ type: "ui.overlay", overlay: "newPane" });
        }}
        className="group flex h-[30px] w-full items-center gap-[9px] rounded-md pr-2 pl-[13px] text-left text-[12px] text-fg-subtle hover:bg-hover hover:text-fg"
      >
        <Plus size={12} />
        <span className="flex-1">{ws.panes.length === 0 ? "No agents yet · start one" : "New agent"}</span>
        <kbd className="invisible font-mono text-[10px] group-hover:visible">{keysFor({ type: "ui.overlay", overlay: "newPane" })}</kbd>
      </button>
    </div>
  );
}

const iconButton = "rounded-md p-1.5 text-fg-subtle transition-colors hover:bg-hover hover:text-fg";

const MIN_SIDEBAR = 200;
const MAX_SIDEBAR = 560;
/** Room the terminals keep when the sidebar is dragged wide. */
const MIN_TERMINALS = 360;

/** The gap right of the sidebar: drag to resize, double-click for the default width. */
export function SidebarResizer() {
  const setWidth = useWorkspaces((s) => s.setSidebarWidth);
  const start = (e: React.PointerEvent) => {
    e.preventDefault();
    const startX = e.clientX;
    const { sidebarWidth: initial, browserOpen, browserWidth } = useWorkspaces.getState();
    const max = Math.max(Math.min(MAX_SIDEBAR, window.innerWidth - MIN_TERMINALS - (browserOpen ? browserWidth : 0)), MIN_SIDEBAR);
    const move = (ev: PointerEvent) => setWidth(Math.min(Math.max(initial + ev.clientX - startX, MIN_SIDEBAR), max));
    const up = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      document.body.style.cursor = "";
      document.body.classList.remove("resizing");
    };
    document.body.style.cursor = "col-resize";
    // Keeps the terminals and page canvas from eating pointer events mid-drag.
    document.body.classList.add("resizing");
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  };
  return (
    <div
      data-resizer
      onPointerDown={start}
      onDoubleClick={() => setWidth(DEFAULT_SIDEBAR_WIDTH)}
      title="Drag to resize · double-click to reset"
      className="group relative -mx-2 w-2 shrink-0 cursor-col-resize"
    >
      <div className="absolute inset-y-2 left-[3px] w-0.5 rounded-full bg-accent opacity-0 transition-opacity group-hover:opacity-60" />
    </div>
  );
}

export function Sidebar() {
  const width = useWorkspaces((s) => s.sidebarWidth);
  const { listings, refresh } = useSidebarCheckouts();
  const { workspaces, activeWorkspaceId, focusedPaneId, expandedWorkspaces, toggleWorkspaceExpanded, collapseAllWorkspaces, moveWorkspace, theme, toggleTheme } = useWorkspacesPick("workspaces", "activeWorkspaceId", "focusedPaneId", "expandedWorkspaces", "toggleWorkspaceExpanded", "collapseAllWorkspaces", "moveWorkspace", "theme", "toggleTheme");
  const [menu, openMenu, closeMenu] = useContextMenu<{ title: string; items: MenuItem[] }>();
  const { drag, begin, dropLine } = useSortable(moveWorkspace);

  return (
    <aside style={{ width }} className="anim-slide flex h-full shrink-0 flex-col overflow-hidden rounded-[var(--radius-pane)] border border-border bg-panel">
      <SidebarTop />

      <nav className="flex-1 overflow-y-auto px-2 pb-3 select-none">
        <NeedsYou />
        <SectionLabel
          action={
            <div className="flex items-center gap-px">
              <button
                onClick={collapseAllWorkspaces}
                disabled={expandedWorkspaces.length === 0}
                className="rounded p-1 text-fg-subtle hover:bg-hover hover:text-fg disabled:pointer-events-none disabled:opacity-40"
                title="Collapse all workspaces"
              >
                <ChevronsDownUp size={13} />
              </button>
              <button onClick={() => execute({ type: "workspace.create" })} className="rounded p-1 text-fg-subtle hover:bg-hover hover:text-fg" title="New workspace">
                <Plus size={13} />
              </button>
            </div>
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
                "relative transition-opacity duration-150",
                drag?.id === ws.id && "opacity-50",
              )}
            >
              {line && <DropLine at={line} />}
              <button
                onClick={() => toggleWorkspaceExpanded(ws.id)}
                onDoubleClick={() => execute({ type: "workspace.switch", target: { id: ws.id } })}
                onPointerDown={(e) => begin(e, ws.id)}
                onContextMenu={(e) => openMenu(e, { title: ws.rootDir, items: workspaceMenu(ws) })}
                title={[ws.name, `Directory: ${ws.rootDir}`, ...checkoutDetails(ws.rootDir, listings[ws.rootDir]), `New agents: ${ws.useWorktrees ? "isolated worktrees" : "shared checkout"}`, "Double-click to open · drag to reorder"].join("\n")}
                onMouseEnter={() => refresh(ws.rootDir)}
                onFocus={() => refresh(ws.rootDir)}
                className={cn(
                  "group flex h-8 w-full cursor-pointer items-center gap-[9px] rounded-md px-1.5 text-left text-[13px] transition-colors duration-150 hover:bg-hover hover:text-fg",
                  active || expanded ? "font-medium text-fg" : "text-fg-muted",
                )}
              >
                <WorkspaceAvatar name={ws.name} active={active} />
                <span className="flex-1 truncate">{ws.name}</span>
                {!expanded && <WorkspaceCount panes={livePanes(ws)} />}
                <ChevronRight
                  size={13}
                  className={cn("shrink-0 text-fg-subtle transition-transform duration-200", expanded && "rotate-90")}
                />
              </button>
              {!expanded && <WorkspaceSummary panes={livePanes(ws)} />}

              {expanded && <WorkspacePanes workspace={ws} focusedPaneId={active ? focusedPaneId : null} openMenu={openMenu} />}
            </div>
          );
        })}
      </nav>

      {menu && <ContextMenu x={menu.x} y={menu.y} title={menu.target.title} items={menu.target.items} onClose={closeMenu} />}
      <ActivityFeed />
      <footer className="flex items-center gap-2 border-t border-border py-2 pr-2 pl-3">
        <AppVersion />
        <UpdateButton />
        <span className="flex-1" />
        <button onClick={toggleTheme} className={iconButton} title={`Switch to ${themeById(themeById(theme).pair).name} (more themes in Settings)`}>
          <span key={theme} className="anim-grow block">
            {themeById(theme).mode === "dark" ? <Sun size={14} /> : <Moon size={14} />}
          </span>
        </button>
        <button onClick={() => execute({ type: "ui.settings" })} className={iconButton} title={`Settings (${keysFor({ type: "ui.settings" })})`}>
          <Settings size={14} />
        </button>
      </footer>
    </aside>
  );
}

const railButton = "relative grid size-9 place-items-center rounded-md text-fg-muted transition-colors hover:bg-hover hover:text-fg";

/** The collapsed sidebar: a 60 px icon rail with the same sections. */
export function SidebarRail() {
  const { workspaces, focusedPaneId } = useWorkspacesPick("workspaces", "focusedPaneId");
  const workspaceId = useWorkspaces((s) => s.activeWorkspaceId);
  const everywhere = useWorkspaces((s) => s.view === "overview");
  const reviews = useReviews((s) => s.reviews.filter((r) => (everywhere || r.workspaceId === workspaceId) && r.status === "open").length);
  const waiting = workspaces.flatMap((w) => livePanes(w)).filter((p) => p.provider !== "shell" && p.status === "needs-input");
  const tile = (pane: Pane) => (
    <button
      key={pane.id}
      onClick={() => execute({ type: "pane.focus", target: { id: pane.id } })}
      title={pane.title ?? pane.provider}
      className={cn("rounded-md p-[5px] hover:bg-hover", pane.id === focusedPaneId && "bg-active")}
    >
      <AgentTile pane={pane} ring={pane.id === focusedPaneId ? "var(--color-active)" : "var(--color-panel)"} />
    </button>
  );
  return (
    <aside className="anim-slide flex h-full w-[60px] shrink-0 flex-col overflow-hidden rounded-[var(--radius-pane)] border border-border bg-panel">
      <div className="flex flex-col items-center gap-1 border-b border-border pt-2.5 pb-2">
        <button onClick={() => execute({ type: "ui.palette" })} title="Search or jump to…" className={railButton}>
          <Search size={15} />
        </button>
        <button onClick={() => execute({ type: "ui.overlay", overlay: "progress" })} title="Sessions" className={railButton}>
          <Activity size={15} />
        </button>
        <button onClick={() => execute({ type: "ui.overlay", overlay: "review" })} title="Review" className={railButton}>
          <Inbox size={15} />
          {reviews > 0 && <span className="absolute top-0.5 right-0 min-w-4 rounded-full bg-accent px-1 text-center text-[10px] leading-4 font-semibold text-on-accent">{reviews}</span>}
        </button>
        <button onClick={() => execute({ type: "ui.overlay", overlay: "history" })} title="History" className={railButton}>
          <Clock size={15} />
        </button>
      </div>
      <nav className="flex flex-1 flex-col items-center gap-1 overflow-y-auto py-2">
        {waiting.length > 0 && <div className="flex w-11 flex-col items-center rounded-[10px] bg-warning/[0.06] py-1">{waiting.map(tile)}</div>}
        {workspaces.map((ws) => (
          <div key={ws.id} className="flex flex-col items-center gap-1">
            <span className="mt-1 w-9 border-t border-border" />
            <button onClick={() => execute({ type: "workspace.switch", target: { id: ws.id } })} title={ws.name} className="grid size-9 place-items-center rounded-md hover:bg-hover">
              <WorkspaceAvatar name={ws.name} active={ws.id === workspaceId} />
            </button>
            {livePanes(ws).filter((p) => p.provider !== "shell").map(tile)}
          </div>
        ))}
      </nav>
      <footer className="flex flex-col items-center gap-1 border-t border-border py-2">
        <img src={logoUrl} alt="" className="size-[22px]" draggable={false} />
        <UpdateButton />
        <button onClick={() => execute({ type: "ui.settings" })} className={railButton} title={`Settings (${keysFor({ type: "ui.settings" })})`}>
          <Settings size={15} />
        </button>
      </footer>
    </aside>
  );
}
