import { ChevronDown, ChevronRight, Moon, Plus, Settings } from "lucide-react";
import { cn } from "@/lib/cn";
import { execute } from "@/commands/bus";
import { keysFor } from "@/commands/keymap";
import { StatusDot } from "@/features/terminal/StatusDot";
import { ActivityFeed } from "./ActivityFeed";
import { WorkspaceMenu } from "./WorkspaceMenu";
import { useCallback, useState } from "react";
import { providerName, useWorkspaces } from "@/stores/workspaces";

export function Sidebar() {
  const { providers, workspaces, activeWorkspaceId, focusedPaneId, setActiveWorkspace, focusPane } = useWorkspaces();
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
                onClick={() => setActiveWorkspace(ws.id)}
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
                    onClick={() => focusPane(pane.id)}
                    className={cn(
                      "flex w-full items-center gap-2 rounded-md py-1.5 pr-2 pl-5 text-left text-[13px]",
                      pane.id === focusedPaneId ? "bg-hover text-fg" : "text-fg-muted hover:bg-hover hover:text-fg",
                    )}
                  >
                    <StatusDot status={pane.status} />
                    <span className={cn("truncate", pane.unseen && "font-semibold text-fg")}>
                      {providerName(providers, pane.provider)}
                    </span>
                    {pane.worktree && (
                      <span className="ml-auto truncate font-mono text-[10px] text-fg-subtle">{pane.worktree.branch.replace(/^winfer\//, "")}</span>
                    )}
                  </button>
                ))}
            </div>
          );
        })}
      </nav>

      {menu && menuWorkspace && <WorkspaceMenu ws={menuWorkspace} x={menu.x} y={menu.y} onClose={closeMenu} />}
      <ActivityFeed />
      <footer className="flex items-center gap-2 border-t border-border px-4 py-3">
        <span className="flex size-6 items-center justify-center rounded-full bg-accent text-[11px] font-semibold text-white">
          W
        </span>
        <span className="flex-1 text-xs text-fg-muted">Local</span>
        <button className="rounded p-1 text-fg-subtle hover:bg-hover hover:text-fg" title="Theme">
          <Moon size={14} />
        </button>
        <button onClick={() => execute({ type: "ui.settings" })} className="rounded p-1 text-fg-subtle hover:bg-hover hover:text-fg" title={`Settings (${keysFor({ type: "ui.settings" })})`}>
          <Settings size={14} />
        </button>
      </footer>
    </aside>
  );
}
