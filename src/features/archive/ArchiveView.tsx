import { Archive, ArchiveRestore, X } from "lucide-react";
import { useEffect } from "react";
import { execute, focusViaCommand } from "@/commands/bus";
import { cn } from "@/lib/cn";
import { ProviderIcon } from "@/features/providers/ProviderIcon";
import { PaneFrame } from "@/features/terminal/PaneFrame";
import { StatusDot } from "@/features/terminal/StatusDot";
import { PaneDetail, PaneName } from "@/features/workspaces/Sidebar";
import { archivedPanes, useWorkspaces } from "@/stores/workspaces";

const RowButton = ({ title, onClick, children }: { title: string; onClick: () => void; children: React.ReactNode }) => (
  <button
    title={title}
    onClick={(e) => {
      e.stopPropagation();
      onClick();
    }}
    className="rounded p-1 text-fg-subtle opacity-0 group-hover:opacity-100 hover:bg-hover hover:text-fg focus-visible:opacity-100"
  >
    {children}
  </button>
);

/**
 * Panes put aside once their work is done (e.g. a PR is open): still running, out of their
 * workspace's tiles. One is shown at a time, live, so it can be talked to or restored.
 */
export function ArchiveView() {
  const workspaces = useWorkspaces((s) => s.workspaces);
  const focusedPaneId = useWorkspaces((s) => s.focusedPaneId);
  const archived = archivedPanes(workspaces);
  const selected = archived.find((p) => p.id === focusedPaneId) ?? archived[0];
  const names = new Map(workspaces.map((w) => [w.id, w.name]));
  const groups = workspaces.map((w) => ({ workspace: w, panes: archived.filter((p) => p.workspaceId === w.id) })).filter((g) => g.panes.length > 0);

  // The pane shown is the focused one, so keys and "focused" commands reach it.
  useEffect(() => {
    if (selected && selected.id !== focusedPaneId) execute({ type: "pane.focus", target: { id: selected.id } });
  }, [selected?.id, focusedPaneId]);

  if (!selected) {
    return (
      <div className="flex h-full w-full flex-col items-center justify-center gap-2 rounded-[var(--radius-pane)] border border-dashed border-border text-fg-subtle">
        <Archive size={18} />
        <span className="text-sm text-fg-muted">No archived sessions</span>
        <span className="max-w-sm text-center text-xs">
          Archive a pane whose work is done from its header or … menu — it keeps running here, out of the way, until you restore it.
        </span>
      </div>
    );
  }

  return (
    <div className="flex min-h-0 min-w-0 flex-1 gap-2">
      <aside className="flex w-64 shrink-0 flex-col overflow-hidden rounded-[var(--radius-pane)] border border-border bg-panel">
        <header className="flex h-8 shrink-0 items-center gap-2 border-b border-border px-3 text-xs text-fg-muted">
          <Archive size={13} />
          <span className="flex-1 font-medium text-fg">Archive</span>
          <span className="font-mono text-[10px] text-fg-subtle">{archived.length}</span>
        </header>
        <div className="min-h-0 flex-1 overflow-y-auto p-1">
          {groups.map(({ workspace, panes }) => (
            <div key={workspace.id} className="mb-1">
              <div className="truncate px-2 pt-1.5 pb-1 text-[11px] font-medium text-fg-subtle">{workspace.name}</div>
              {panes.map((pane) => {
                const active = pane.id === selected.id;
                return (
                  <div
                    key={pane.id}
                    role="button"
                    tabIndex={0}
                    onClick={() => execute({ type: "pane.focus", target: { id: pane.id } })}
                    onKeyDown={(e) => {
                      if (e.target !== e.currentTarget || (e.key !== "Enter" && e.key !== " ")) return;
                      e.preventDefault();
                      execute({ type: "pane.focus", target: { id: pane.id } });
                    }}
                    className={cn(
                      "group block cursor-pointer rounded-md px-2 py-1.5 text-[13px] transition-colors duration-150",
                      active ? "bg-active text-fg" : "text-fg-muted hover:bg-hover hover:text-fg",
                    )}
                  >
                    <span className="flex items-center gap-2">
                      <StatusDot status={pane.status} />
                      <ProviderIcon provider={pane.provider} size={12} />
                      <PaneName pane={pane} className={cn("flex-1", (pane.unseen || active) && "font-medium text-fg")} />
                      <RowButton title="Restore to its workspace" onClick={() => execute({ type: "pane.archive", target: { id: pane.id }, restore: true })}>
                        <ArchiveRestore size={13} />
                      </RowButton>
                      <RowButton title="Close (stops the agent)" onClick={() => execute({ type: "pane.close", target: { id: pane.id } })}>
                        <X size={13} />
                      </RowButton>
                    </span>
                    <span className="block pl-[26px]">
                      <PaneDetail pane={pane} />
                    </span>
                  </div>
                );
              })}
            </div>
          ))}
        </div>
      </aside>
      <div key={selected.id} className="anim-fade flex min-h-0 min-w-0 flex-1">
        <PaneFrame
          pane={selected}
          index={archived.indexOf(selected) + 1}
          focused={selected.id === focusedPaneId}
          zoomed={false}
          workspaceName={names.get(selected.workspaceId)}
          onFocus={focusViaCommand}
        />
      </div>
    </div>
  );
}
