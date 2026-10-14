import { ReviewInbox } from "@/features/review/ReviewInbox";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { Mic, PanelLeft, PanelRight } from "lucide-react";
import { useEffect } from "react";
import { cn } from "@/lib/cn";
import { BrowserPanel, BrowserResizer } from "@/features/browser/BrowserPanel";
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
import { DragLayer } from "@/features/layout/DragLayer";
import { startListening, startVoice, stopListening, useVoice } from "@/features/voice/voice";
import { keysFor, useKeymap, voiceKeys } from "@/commands/keymap";
import { startStatusTracking } from "@/features/status/agentStatus";
import { startAgentApi } from "@/features/mcp/agentApi";
import { startTerminalEvents } from "@/features/terminal/xtermRegistry";
import { loadHome } from "@/lib/home";
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
      <button onClick={toggleSidebar} className="rounded p-1 hover:bg-hover hover:text-fg" title={`Toggle sidebar (${keysFor({ type: "ui.toggleSidebar" })})`}>
        <PanelLeft size={15} />
      </button>
      <span data-tauri-drag-region className="flex-1 text-center text-xs text-fg-muted">
        {workspace.name}
      </span>
      <NewPaneMenu />
      <MicButton />
      <button onClick={toggleBrowser} className="rounded p-1 hover:bg-hover hover:text-fg" title={`Toggle browser (${keysFor({ type: "ui.toggleBrowser" })})`}>
        <PanelRight size={15} />
      </button>
    </div>
  );
}

/** Hold to talk (same as holding the push-to-talk keys). */
function MicButton() {
  const listening = useVoice((s) => s.phase === "listening");
  const shortcutError = useVoice((s) => s.status?.shortcutError);
  const keys = voiceKeys(useWorkspaces((s) => s.shortcuts));
  return (
    <button
      onMouseDown={() => startListening()}
      onMouseUp={stopListening}
      onMouseLeave={() => listening && stopListening()}
      className={cn(
        "flex items-center gap-1.5 rounded-md border px-2 py-1 text-xs hover:bg-hover hover:text-fg",
        listening ? "border-accent text-accent" : "border-border",
      )}
      title={shortcutError ?? `Hold to talk (or hold ${keys.command} anywhere; ${keys.dictation} to dictate)`}
    >
      <Mic size={13} />
      <span className={cn(shortcutError && "line-through")}>{keys.command}</span>
    </button>
  );
}

function EmptyWorkspace() {
  return (
    <div className="flex h-full w-full flex-col items-center justify-center gap-2 rounded-[var(--radius-pane)] border border-dashed border-border text-fg-subtle">
      <span className="text-sm text-fg-muted">No agents running</span>
      <span className="text-xs">
        {keysFor({ type: "pane.spawn", provider: "default" })} new pane · {keysFor({ type: "ui.palette", query: "New pane: " })} pick provider ·{" "}
        {keysFor({ type: "ui.palette" })} commands · {keysFor({ type: "ui.cheatSheet" })} shortcuts
      </span>
    </div>
  );
}

export function AppShell() {
  const { sidebarOpen, browserOpen, focusedPaneId, focusPane, loadProviders, updateActive, hydrate, theme } = useWorkspaces();
  useKeymap();

  useEffect(() => {
    document.documentElement.dataset.theme = theme;
    // Native window chrome (Windows title bar, macOS controls) follows the app.
    void getCurrentWindow().setTheme(theme).catch(() => {});
  }, [theme]);

  useEffect(() => {
    void loadProviders();
    void hydrate();
    void loadHome();
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
        {browserOpen && (
          <>
            <BrowserResizer />
            <BrowserPanel />
          </>
        )}
      </main>
      <CommandPalette />
      <CheatSheet />
      <DiffView />
      <SettingsModal />
      <RaceView />
      <ReviewInbox />
      <BroadcastBar />
      <RecipesView />
      <ModelPicker />
      <DragLayer />
      <VoiceHUD />
    </div>
  );
}
