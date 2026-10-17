import { FolderOpen, Plus, RotateCw, X } from "lucide-react";
import { useEffect, useMemo } from "react";
import { create } from "zustand";
import { cn } from "@/lib/cn";
import { useWorkspaces, type Pane, type Workspace } from "@/stores/workspaces";
import { TerminalHost } from "./TerminalHost";
import { closeTerminal, restartTerminal, sideTerminalId } from "./xtermRegistry";

/** Workspaces with a side shell open, and the one the panel shows. */
const useSideTerminals = create<{ open: string[]; shown: string | null }>(() => ({ open: [], shown: null }));

function openSideTerminal(workspaceId: string) {
  useSideTerminals.setState((s) => ({ open: s.open.includes(workspaceId) ? s.open : [...s.open, workspaceId], shown: workspaceId }));
}

function closeSideTerminal(workspaceId: string) {
  closeTerminal(sideTerminalId(workspaceId));
  useSideTerminals.setState((s) => {
    const open = s.open.filter((id) => id !== workspaceId);
    return { open, shown: s.shown === workspaceId ? (open.at(-1) ?? null) : s.shown };
  });
}

const shellPane = (workspace: Workspace): Pane => ({
  id: sideTerminalId(workspace.id),
  workspaceId: workspace.id,
  provider: "shell",
  title: "Terminal",
  cwd: workspace.rootDir,
  status: "idle",
  worktree: null,
  wantsWorktree: false,
  resume: false,
  sessionId: null,
  unseen: false,
  devUrl: null,
  raceId: null,
  plain: false,
  prompt: null,
  model: null,
  effort: null,
  route: null,
  agentTitle: null,
  customTitle: null,
});

/**
 * A plain shell in the side panel (next to Browser and Android), in a workspace's folder:
 * for quick commands without adding a pane to the layout. One per workspace, each a tab; the
 * panel keeps showing its shell when you switch workspaces, and shells keep running while hidden.
 */
export function SideTerminal() {
  const workspaces = useWorkspaces((s) => s.workspaces);
  const activeId = useWorkspaces((s) => s.activeWorkspaceId);
  const { open, shown } = useSideTerminals();
  // Removed workspaces' shells are closed by the Command Bus; drop their tabs.
  const tabs = open.flatMap((id) => workspaces.find((w) => w.id === id) ?? []);
  const workspace = tabs.find((w) => w.id === shown) ?? tabs.at(-1) ?? null;

  useEffect(() => {
    // Opens the active workspace's shell the first time the tab is shown.
    if (tabs.length === 0) openSideTerminal(activeId);
  }, [tabs.length, activeId]);

  const pane = useMemo(() => (workspace ? shellPane(workspace) : null), [workspace?.id, workspace?.rootDir]);
  if (!pane) return null;
  const active = workspaces.find((w) => w.id === activeId);
  const tab = (selected: boolean) =>
    cn("group flex h-6 max-w-40 shrink-0 items-center gap-1.5 rounded-md px-2 text-xs", selected ? "bg-active text-fg" : "text-fg-muted hover:bg-hover");
  return (
    <div className="flex min-h-0 flex-1 flex-col">
      {(tabs.length > 1 || activeId !== workspace!.id) && (
        <div className="flex h-8 shrink-0 items-center gap-1 border-b border-border px-1.5">
          <div className="flex min-w-0 flex-1 gap-1 overflow-x-auto">
            {tabs.map((w) => (
              <button key={w.id} onClick={() => openSideTerminal(w.id)} title={w.rootDir} className={tab(w.id === workspace!.id)}>
                <span className="truncate">{w.name}</span>
                <X
                  size={11}
                  className="shrink-0 opacity-0 group-hover:opacity-60 hover:opacity-100"
                  onClick={(e) => {
                    e.stopPropagation();
                    closeSideTerminal(w.id);
                  }}
                />
              </button>
            ))}
          </div>
          {active && !open.includes(active.id) && (
            <button onClick={() => openSideTerminal(active.id)} title={`Open a shell in ${active.name}`} className="rounded p-1 text-fg-subtle hover:bg-hover hover:text-fg">
              <Plus size={14} />
            </button>
          )}
        </div>
      )}
      <div className="flex shrink-0 items-center gap-2 border-b border-border px-3 py-1.5 text-xs text-fg-subtle">
        <FolderOpen size={12} className="shrink-0" />
        <span className="min-w-0 flex-1 truncate font-mono text-[11px]" title={pane.cwd}>
          {pane.cwd}
        </span>
        <button onClick={() => restartTerminal(pane)} title="Restart the shell" className="rounded p-1 hover:bg-hover hover:text-fg">
          <RotateCw size={12} />
        </button>
      </div>
      <div className="min-h-0 flex-1 py-1.5 pr-1 pl-2.5">
        {/* Keyed so switching tab attaches that workspace's shell. */}
        <TerminalHost key={pane.id} pane={pane} focused />
      </div>
    </div>
  );
}
