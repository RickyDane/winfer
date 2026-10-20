import { execute } from "@/commands/bus";
import { leaves, setSizesAt } from "@/features/layout/tree";
import { TileView } from "@/features/layout/TileView";
import { PaneFrame } from "@/features/terminal/PaneFrame";
import { livePanes, useWorkspaces } from "@/stores/workspaces";
import { AgentBar } from "./AgentBar";

/**
 * Every session of every workspace, tiled like a workspace: same panes, gutters, zoom and
 * drag and drop. Each pane is labeled with its workspace; focusing one makes its workspace
 * active, so new panes and commands go there.
 */
export function Overview() {
  const { workspaces, overview, focusedPaneId, updateOverview } = useWorkspaces();
  const panes = new Map(workspaces.flatMap((w) => livePanes(w).map((p) => [p.id, p] as const)));
  const names = new Map(workspaces.map((w) => [w.id, w.name]));
  const order = overview.layout ? leaves(overview.layout) : [];
  const occupied = workspaces.filter((w) => livePanes(w).length > 0).length;

  const renderLeaf = (id: string) => {
    const pane = panes.get(id);
    if (!pane) return null;
    return (
      <PaneFrame
        pane={pane}
        index={order.indexOf(id) + 1}
        focused={id === focusedPaneId}
        zoomed={id === overview.zoomedPaneId}
        workspaceName={names.get(pane.workspaceId)}
        onFocus={() => {
          if (id !== useWorkspaces.getState().focusedPaneId) execute({ type: "pane.focus", target: { id } });
        }}
      />
    );
  };

  return (
    <div className="flex min-h-0 min-w-0 flex-1 flex-col">
      <AgentBar panes={[...panes.values()]} scope={{ workspaces: occupied }} />
      <div className="flex min-h-0 min-w-0 flex-1">
        {overview.layout && overview.zoomedPaneId ? (
          renderLeaf(overview.zoomedPaneId)
        ) : overview.layout ? (
          <TileView
            node={overview.layout}
            renderLeaf={renderLeaf}
            onResize={(path, sizes) => updateOverview((o) => ({ layout: o.layout && setSizesAt(o.layout, path, sizes) }))}
          />
        ) : (
          <div className="flex h-full w-full items-center justify-center rounded-[var(--radius-pane)] border border-dashed border-border text-sm text-fg-muted">
            No sessions in any workspace
          </div>
        )}
      </div>
    </div>
  );
}
