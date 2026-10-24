import { BranchDialog } from "@/features/worktree/BranchDialog";
import { NewPaneDialog } from "@/features/worktree/NewPaneDialog";
import { HistoryView } from "@/features/history/HistoryView";
import { AgentBar } from "@/features/overview/AgentBar";
import { Overview } from "@/features/overview/Overview";
import { ArchiveView } from "@/features/archive/ArchiveView";
import { SessionsOverview } from "@/features/progress/SessionsOverview";
import { startProgressTracking } from "@/features/progress/progress";
import { ReviewInbox } from "@/features/review/ReviewInbox";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { Archive, AudioLines, Layers, LayoutGrid, PanelLeft, PanelRight } from "lucide-react";
import { useEffect } from "react";
import { cn } from "@/lib/cn";
import { execute } from "@/commands/bus";
import { BrowserResizer, SidePanel, UndockedBrowser } from "@/features/browser/BrowserPanel";
import { NewPaneMenu } from "@/features/panes/NewPaneMenu";
import { CheatSheet } from "@/features/palette/CheatSheet";
import { CommandPalette } from "@/features/palette/CommandPalette";
import { DiffView } from "@/features/worktree/DiffView";
import { SettingsModal } from "@/features/settings/SettingsModal";
import { LiveVoiceHUD } from "@/features/voice/live/LiveVoiceHUD";
import { toggleLive, useLiveVoice } from "@/features/voice/live/session";
import { BroadcastBar } from "@/features/orchestration/BroadcastBar";
import { RaceView } from "@/features/orchestration/RaceView";
import { RaceResults } from "@/features/orchestration/RaceResults";
import { startRunTimes } from "@/features/orchestration/runTimes";
import { RecipesView } from "@/features/recipes/RecipesView";
import { ModelPicker } from "@/features/models/ModelPicker";
import { DragLayer } from "@/features/layout/DragLayer";
import { useEscapeToClose } from "@/features/modals/escape";
import { Toaster } from "@/components/Toaster";
import { UsageMeter } from "@/features/usage/UsageMeter";
import { startUsageTracking } from "@/features/usage/usage";
import { startWakes } from "@/features/usage/agentUsage";
import { startUpdateChecks } from "@/features/updates/updater";
import { startProviderUpdateChecks } from "@/features/providers/updates";
import { WhatsNew } from "@/features/updates/WhatsNew";
import { QuestionDialog } from "@/features/questions/QuestionDialog";
import { ApprovalDialog } from "@/features/plugins/ApprovalDialog";
import { IS_MAC, keysFor, useKeymap } from "@/commands/keymap";
import { startStatusTracking } from "@/features/status/agentStatus";
import { startMobileBridge } from "@/features/mobile/mobile";
import { startAgentApi } from "@/features/mcp/agentApi";
import { startBgNotices } from "@/features/mcp/bgNotices";
import { startTerminalEvents } from "@/features/terminal/xtermRegistry";
import { loadHome } from "@/lib/home";
import { loadEditors } from "@/features/editors/editors";
import { leaves, setSizesAt } from "@/features/layout/tree";
import { TileView } from "@/features/layout/TileView";
import { PaneFrame } from "@/features/terminal/PaneFrame";
import { Sidebar, SidebarResizer } from "@/features/workspaces/Sidebar";
import { livePanes, useActiveWorkspace, useWorkspaces, useWorkspacesPick } from "@/stores/workspaces";
import { applyTheme, themeById } from "@/features/theme/themes";
import { loadPlugins, startPluginEvents } from "@/features/plugins/plugins";
import { PluginPageView, usePluginPages } from "@/features/plugins/PluginPage";
import { PageTabs } from "@/features/plugins/PageTabs";

function TitleBar() {
  const { toggleSidebar, toggleBrowser } = useWorkspacesPick("toggleSidebar", "toggleBrowser");
  const workspace = useActiveWorkspace();
  if (!workspace) return null;
  return (
    // On macOS the window controls (traffic lights) overlay the title bar's left edge
    // (titleBarStyle: Overlay); Windows draws its own title bar above, so no gap there.
    // Equal 1fr side columns inside symmetric padding keep the tabs centered on the window.
    <div data-tauri-drag-region className="grid h-10 shrink-0 grid-cols-[1fr_auto_1fr] items-center gap-2 px-3 text-fg-subtle">
      <div data-tauri-drag-region className={cn("flex min-w-0 items-center", IS_MAC && "pl-[68px]")}>
        <button onClick={toggleSidebar} className="rounded p-1 hover:bg-hover hover:text-fg" title={`Toggle sidebar (${keysFor({ type: "ui.toggleSidebar" })})`}>
          <PanelLeft size={15} />
        </button>
      </div>
      <ViewTabs workspace={workspace.name} />
      <div data-tauri-drag-region className="flex min-w-0 items-center justify-end gap-2">
        <UsageMeter />
        <NewPaneMenu />
        <LiveButton />
        <button onClick={toggleBrowser} className="rounded p-1 hover:bg-hover hover:text-fg" title={`Toggle browser (${keysFor({ type: "ui.toggleBrowser" })})`}>
          <PanelRight size={15} />
        </button>
      </div>
    </div>
  );
}

/** Title bar tabs: the active workspace's tiles, the overview of every workspace's sessions, the archive, and plugin pages. */
function ViewTabs({ workspace }: { workspace: string }) {
  const view = useWorkspaces((s) => s.view);
  const all = useWorkspaces((s) => s.workspaces).flatMap((w) => w.panes);
  const sessions = all.filter((p) => !p.archived);
  const archived = all.filter((p) => p.archived);
  const waiting = sessions.filter((p) => p.status === "needs-input").length;
  const archivedWaiting = archived.filter((p) => p.status === "needs-input").length;
  const tab = (active: boolean) =>
    cn("flex items-center gap-1.5 rounded-md px-2.5 py-1 text-xs transition-colors duration-150", active ? "bg-active font-medium text-fg" : "text-fg-muted hover:bg-hover hover:text-fg");
  return (
    <div role="tablist" className="flex items-center gap-0.5 rounded-lg border border-border bg-panel p-0.5">
      <button role="tab" aria-selected={view === "workspace"} onClick={() => execute({ type: "ui.view", view: "workspace" })} className={tab(view === "workspace")}>
        <LayoutGrid size={12} />
        <span className="max-w-48 truncate">{workspace}</span>
      </button>
      <button
        role="tab"
        aria-selected={view === "overview"}
        onClick={() => execute({ type: "ui.view", view: "overview" })}
        className={tab(view === "overview")}
        title={`All workspaces and sessions (${keysFor({ type: "ui.view", view: "toggle" })})`}
      >
        <Layers size={12} />
        Overview
        <span className="font-mono text-[10px] text-fg-subtle">{sessions.length}</span>
        {waiting > 0 && <span className="anim-ring size-1.5 rounded-full bg-warning text-warning" title={`${waiting} waiting for you`} />}
      </button>
      {(archived.length > 0 || view === "archive") && (
        <button
          role="tab"
          aria-selected={view === "archive"}
          onClick={() => execute({ type: "ui.view", view: "archive" })}
          className={tab(view === "archive")}
          title={`Archived sessions: still running, out of the way (${keysFor({ type: "ui.view", view: "archive" })})`}
        >
          <Archive size={12} />
          Archive
          <span className="font-mono text-[10px] text-fg-subtle">{archived.length}</span>
          {archivedWaiting > 0 && <span className="anim-ring size-1.5 rounded-full bg-warning text-warning" title={`${archivedWaiting} waiting for you`} />}
        </button>
      )}
      <PageTabs tab={tab} />
    </div>
  );
}

/** Starts or ends the live voice conversation. */
function LiveButton() {
  const on = useLiveVoice((s) => s.state !== "off" && s.state !== "error");
  return (
    <button
      onClick={toggleLive}
      className={cn("rounded-md border px-1.5 py-1 hover:bg-hover hover:text-fg", on ? "border-accent text-accent" : "border-border")}
      title={`${on ? "End" : "Start"} live voice conversation (${keysFor({ type: "voice.live" })})`}
    >
      <AudioLines size={13} />
    </button>
  );
}

function EmptyWorkspace() {
  return (
    <div className="flex h-full w-full flex-col items-center justify-center gap-2 rounded-[var(--radius-pane)] border border-dashed border-border text-fg-subtle">
      <span className="text-sm text-fg-muted">No agents running</span>
      <span className="text-xs">
        {keysFor({ type: "ui.overlay", overlay: "newPane" })} new pane · {keysFor({ type: "ui.palette", query: "New pane: " })} pick provider ·{" "}
        {keysFor({ type: "ui.palette" })} commands · {keysFor({ type: "ui.cheatSheet" })} shortcuts
      </span>
    </div>
  );
}

export function AppShell() {
  const { sidebarOpen, browserOpen, focusedPaneId, focusPane, loadProviders, updateActive, hydrate, theme, view, glassPanes } = useWorkspacesPick("sidebarOpen", "browserOpen", "focusedPaneId", "focusPane", "loadProviders", "updateActive", "hydrate", "theme", "view", "glassPanes");
  useKeymap();
  useEscapeToClose();
  const pages = usePluginPages();
  // A page whose plugin was turned off (or hasn't loaded yet) falls back to the workspace.
  const page = view.startsWith("page:") ? pages.find((p) => `page:${p.key}` === view) : undefined;

  useEffect(() => {
    applyTheme(theme);
    // Native window chrome (Windows title bar, macOS controls) follows the app.
    void getCurrentWindow().setTheme(themeById(theme).mode).catch(() => {});
  }, [theme]);

  useEffect(() => {
    void loadProviders();
    void hydrate();
    void loadHome();
    void loadEditors();
    startTerminalEvents();
    startStatusTracking();
    startMobileBridge();
    startRunTimes();
    startAgentApi();
    startBgNotices();
    startProgressTracking();
    startUsageTracking();
    startWakes();
    startUpdateChecks();
    startProviderUpdateChecks();
    startPluginEvents();
    void loadPlugins().catch((err) => console.error("plugins failed to load", err));
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
      onFocus={focusPane}
    />
  );

  return (
    <div className={cn("flex h-full flex-col", glassPanes && "glass-stage")}>
      <TitleBar />
      <main className="flex min-h-0 flex-1 gap-2 px-2 pb-2">
        {sidebarOpen && (
          <>
            <Sidebar />
            <SidebarResizer />
          </>
        )}
        <div className="flex min-h-0 min-w-0 flex-1 flex-col">
          {view === "workspace" && livePanes(workspace).some((p) => p.provider !== "shell") && (
            <AgentBar panes={livePanes(workspace)} scope={{ workspace: workspace.name }} />
          )}
          {/* Keyed so switching workspace or view fades the new content in. */}
          <div key={page ? view : `${view}:${view === "overview" || view === "archive" ? "" : workspace.id}`} className={cn("anim-fade flex min-h-0 min-w-0 flex-1", glassPanes && "pt-1")}>
            {page ? (
              <PluginPageView page={page} />
            ) : view === "overview" ? (
              <Overview />
            ) : view === "archive" ? (
              <ArchiveView />
            ) : workspace.layout && zoomed ? (
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
        </div>
        {browserOpen && (
          <>
            <BrowserResizer />
            <SidePanel />
          </>
        )}
      </main>
      <UndockedBrowser />
      <CommandPalette />
      <CheatSheet />
      <DiffView />
      <SettingsModal />
      <WhatsNew />
      <ApprovalDialog />
      <QuestionDialog />
      <RaceView />
      <RaceResults />
      <ReviewInbox />
      <HistoryView />
      <BranchDialog />
      <NewPaneDialog />
      <SessionsOverview />
      <BroadcastBar />
      <RecipesView />
      <ModelPicker />
      <DragLayer />
      <LiveVoiceHUD />
      <Toaster />
    </div>
  );
}
