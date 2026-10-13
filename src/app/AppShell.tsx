import { Mic, PanelLeft, PanelRight } from "lucide-react";
import { useEffect } from "react";
import { cn } from "@/lib/cn";
import { BrowserPanel } from "@/features/browser/BrowserPanel";
import { NewPaneMenu } from "@/features/panes/NewPaneMenu";
import { CheatSheet } from "@/features/palette/CheatSheet";
import { CommandPalette } from "@/features/palette/CommandPalette";
import { DiffView } from "@/features/worktree/DiffView";
import { SettingsModal } from "@/features/settings/SettingsModal";
import { VoiceHUD } from "@/features/voice/VoiceHUD";
import { BroadcastBar } from "@/features/orchestration/BroadcastBar";
import { RaceView } from "@/features/orchestration/RaceView";
import { RecipesView } from "@/features/recipes/RecipesView";
import { ModelPicker } from "@/features/models/ModelPicker";
import { startListening, startVoice, stopListening, useVoice } from "@/features/voice/voice";
import { useKeymap } from "@/commands/keymap";
import { startStatusTracking } from "@/features/status/agentStatus";
import { startAgentApi } from "@/features/mcp/agentApi";
import { startTerminalEvents } from "@/features/terminal/xtermRegistry";
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
      <MicButton />
      <button onClick={toggleBrowser} className="rounded p-1 hover:bg-hover hover:text-fg" title="Toggle browser (⌘⇧B)">
        <PanelRight size={15} />
      </button>
    </div>
  );
}

/** Hold to talk (same as holding ⌥Space). */
function MicButton() {
  const listening = useVoice((s) => s.phase === "listening");
  return (
    <button
      onMouseDown={() => startListening()}
      onMouseUp={stopListening}
      onMouseLeave={() => listening && stopListening()}
      className={cn(
        "flex items-center gap-1.5 rounded-md border px-2 py-1 text-xs hover:bg-hover hover:text-fg",
        listening ? "border-accent text-accent" : "border-border",
      )}
      title="Hold to talk (or hold ⌥Space anywhere; ⌥⇧Space to dictate)"
    >
      <Mic size={13} />
      <span>⌥Space</span>
    </button>
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
    startTerminalEvents();
    startStatusTracking();
    startVoice();
    startAgentApi();
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
      <SettingsModal />
      <RaceView />
      <BroadcastBar />
      <RecipesView />
      <ModelPicker />
      <VoiceHUD />
    </div>
  );
}
