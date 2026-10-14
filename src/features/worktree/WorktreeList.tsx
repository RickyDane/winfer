import { ask, message } from "@tauri-apps/plugin-dialog";
import { GitBranch, Play, Trash2 } from "lucide-react";
import { useCallback, useEffect, useState } from "react";
import { cn } from "@/lib/cn";
import { logActivity } from "@/stores/activity";
import { defaultProviderId, useWorkspaces, type Workspace } from "@/stores/workspaces";
import { listWorktrees, removeWorktree, type ExistingWorktree } from "./worktree";

const REFRESH_MS = 20_000;

/**
 * Worktrees of the workspace's repo that no pane is using: left behind by closed agents or
 * made outside winfer. Clicking one opens the default agent inside it.
 */
export function WorktreeList({ workspace }: { workspace: Workspace }) {
  const [found, setFound] = useState<ExistingWorktree[]>([]);
  const inUse = useWorkspaces((s) => new Set(s.workspaces.flatMap((w) => w.panes.flatMap((p) => (p.worktree ? [p.worktree.path] : [])))).size);

  const refresh = useCallback(() => {
    listWorktrees(workspace.rootDir).then(setFound, () => setFound([]));
  }, [workspace.rootDir]);

  // Re-check when the workspace or its panes change, when winfer gets focus, and now and then.
  useEffect(() => {
    refresh();
    const timer = window.setInterval(refresh, REFRESH_MS);
    window.addEventListener("focus", refresh);
    return () => {
      window.clearInterval(timer);
      window.removeEventListener("focus", refresh);
    };
  }, [refresh, inUse]);

  const used = new Set(useWorkspaces.getState().workspaces.flatMap((w) => w.panes.flatMap((p) => (p.worktree ? [p.worktree.path] : []))));
  const free = found.filter((w) => !used.has(w.info.path));
  if (free.length === 0) return null;

  const open = (wt: ExistingWorktree) => {
    const s = useWorkspaces.getState();
    if (s.activeWorkspaceId !== workspace.id) s.setActiveWorkspace(workspace.id);
    s.addPane(defaultProviderId(), { workspaceId: workspace.id, existingWorktree: wt.info });
    logActivity(workspace.id, `Opened worktree ${wt.info.branch}`);
  };

  const remove = async (wt: ExistingWorktree) => {
    const ok = await ask(
      wt.prunable
        ? `The folder of ${wt.info.branch} is gone. Remove it from git's worktree list?`
        : `Delete the worktree at ${wt.info.path}${wt.byWinfer ? ` and its branch ${wt.info.branch}` : ""}? Uncommitted changes in it are lost${wt.byWinfer ? ", and so are commits that weren't merged" : ""}.`,
      { title: "Remove worktree", kind: "warning", okLabel: "Remove" },
    );
    if (!ok) return;
    try {
      // Only winfer's own branches go too; a branch made by hand may still be wanted.
      await removeWorktree(wt.info, wt.byWinfer);
      logActivity(workspace.id, `Removed worktree ${wt.info.branch}`);
    } catch (err) {
      await message(String(err), { title: "Couldn't remove worktree", kind: "error" });
    }
    refresh();
  };

  return (
    <div className="mt-1 mb-1 pl-5">
      <div className="px-0 pt-1 pb-0.5 text-[10px] font-medium tracking-wide text-fg-subtle uppercase">Worktrees</div>
      {free.map((wt) => (
        <div key={wt.info.path} className="group flex items-center gap-1 rounded-md pr-1 hover:bg-hover">
          <button
            onClick={() => !wt.prunable && open(wt)}
            disabled={wt.prunable}
            title={wt.prunable ? `${wt.info.path} (folder missing)` : `${wt.info.path}\nClick to open an agent here`}
            className={cn("flex min-w-0 flex-1 items-center gap-2 py-1.5 pr-1 text-left text-[13px] text-fg-muted group-hover:text-fg", wt.prunable && "line-through")}
          >
            <GitBranch size={12} className="shrink-0 text-fg-subtle" />
            <span className="truncate font-mono text-[12px]">{wt.info.branch.replace(/^winfer\//, "")}</span>
          </button>
          {!wt.prunable && (
            <button onClick={() => open(wt)} title="Open an agent in this worktree" className="rounded p-0.5 text-fg-subtle opacity-0 group-hover:opacity-100 hover:text-fg">
              <Play size={11} />
            </button>
          )}
          <button
            onClick={() => void remove(wt)}
            title="Remove this worktree"
            className="rounded p-0.5 text-fg-subtle opacity-0 group-hover:opacity-100 hover:text-danger"
          >
            <Trash2 size={11} />
          </button>
        </div>
      ))}
    </div>
  );
}
