import { FolderOpen, RotateCw } from "lucide-react";
import { useMemo } from "react";
import { useActiveWorkspace, type Pane } from "@/stores/workspaces";
import { TerminalHost } from "./TerminalHost";
import { restartTerminal, sideTerminalId } from "./xtermRegistry";

/**
 * A plain shell in the side panel (next to Browser and Android), in the workspace's folder:
 * for quick commands without adding a pane to the layout. One per workspace; it keeps
 * running while another tab or workspace is shown.
 */
export function SideTerminal() {
  const workspace = useActiveWorkspace();
  const pane = useMemo<Pane | null>(
    () =>
      workspace ? {
        id: sideTerminalId(workspace.id),
        workspaceId: workspace.id,
        provider: "shell",
        title: "Terminal",
        cwd: workspace.rootDir,
        status: "idle",
        worktree: null,
        wantsWorktree: false,
        resume: false,
        unseen: false,
        devUrl: null,
        raceId: null,
        prompt: null,
        model: null,
        agentTitle: null,
      } : null,
    [workspace?.id, workspace?.rootDir],
  );
  if (!pane) return null;
  return (
    <div className="flex min-h-0 flex-1 flex-col">
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
        {/* Keyed so switching workspace attaches that workspace's shell. */}
        <TerminalHost key={pane.id} pane={pane} focused />
      </div>
    </div>
  );
}
