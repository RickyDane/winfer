import { Mic, PanelLeft, PanelRight } from "lucide-react";
import { useEffect } from "react";
import { BrowserPanel } from "@/features/browser/BrowserPanel";
import { NewPaneMenu } from "@/features/panes/NewPaneMenu";
import { CheatSheet } from "@/features/palette/CheatSheet";
import { CommandPalette } from "@/features/palette/CommandPalette";
import { DiffView } from "@/features/worktree/DiffView";
import { useKeymap } from "@/commands/keymap";
import { startStatusTracking } from "@/features/status/agentStatus";
import { leaves, setSizesAt } from "@/features/layout/tree";
import { TileView } from "@/features/layout/TileView";
import { PaneFrame } from "@/features/terminal/PaneFrame";
import { Sidebar } from "@/features/workspaces/Sidebar";
import { useActiveWorkspace, useWorkspaces } from "@/stores/workspaces";

function TitleBar() {
  const { toggleSidebar, toggleBrowser } = useWorkspaces();
  const workspace = useActiveWorkspace();
  if (!workspace) return null;
  return (
    // pl-20 leaves room for the macOS traffic lights (titleBarStyle: Overlay).
    <div data-tauri-drag-region className="flex h-10 shrink-0 items-center gap-2 pr-3 pl-20 text-fg-subtle">
      <button onClick={toggleSidebar} className="rounded p-1 hover:bg-hover hover:text-fg" title="Toggle sidebar (⌘B)">
        <PanelLeft size={15} />
      </button>
      <span data-tauri-drag-region className="flex-1 text-center text-xs text-fg-muted">
        {workspace.name}
      </span>
      <NewPaneMenu />
      <button className="flex items-center gap-1.5 rounded-md border border-border px-2 py-1 text-xs hover:bg-hover hover:text-fg" title="Push to talk (hold ⌥Space)">
        <Mic size={13} />
        <span>⌥Space</span>
      </button>
      <button onClick={toggleBrowser} className="rounded p-1 hover:bg-hover hover:text-fg" title="Toggle browser (⌘⇧B)">
        <PanelRight size={15} />
      </button>
    </div>
  );
}

function EmptyWorkspace() {
  return (
    <div className="flex h-full w-full flex-col items-center justify-center gap-2 rounded-[var(--radius-pane)] border border-dashed border-border text-fg-subtle">
      <span className="text-sm text-fg-muted">No agents running</span>
      <span className="text-xs">⌘T new pane · ⌘⇧T pick provider · ⌘K commands · ⌘/ shortcuts</span>
    </div>
  );
}

export function AppShell() {
  const { sidebarOpen, browserOpen, focusedPaneId, focusPane, loadProviders, updateActive, hydrate } = useWorkspaces();
  useKeymap();

  useEffect(() => {
    void loadProviders();
    void hydrate();
    startStatusTracking();
  }, [loadProviders, hydrate]);
  const workspace = useActiveWorkspace();
  if (!workspace) return <div className="h-full" data-tauri-drag-region />;
  const panes = new Map(workspace.panes.map((p) => [p.id, p]));
  const order = workspace.layout ? leaves(workspace.layout) : [];
  const zoomed = workspace.zoomedPaneId;
  const renderLeaf = (id: string) => (
    <PaneFrame
      pane={panes.get(id)!}
      index={order.indexOf(id) + 1}
      focused={id === focusedPaneId}
      zoomed={id === zoomed}
      onFocus={() => focusPane(id)}
    />
  );

  return (
    <div className="flex h-full flex-col">
      <TitleBar />
      <main className="flex min-h-0 flex-1 gap-2 px-2 pb-2">
        {sidebarOpen && <Sidebar />}
        <div className="flex min-h-0 min-w-0 flex-1">
          {workspace.layout && zoomed ? (
            renderLeaf(zoomed)
          ) : workspace.layout ? (
            <TileView
              node={workspace.layout}
              renderLeaf={renderLeaf}
              onResize={(path, sizes) => updateActive((w) => ({ layout: w.layout && setSizesAt(w.layout, path, sizes) }))}
            />
          ) : (
            <EmptyWorkspace />
          )}
        </div>
        {browserOpen && <BrowserPanel />}
      </main>
      <CommandPalette />
      <CheatSheet />
      <DiffView />
    </div>
  );
}
