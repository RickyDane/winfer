import { ask, message } from "@tauri-apps/plugin-dialog";
import { GitBranch, Play, Trash2 } from "lucide-react";
import { useCallback, useEffect, useState } from "react";
import { cn } from "@/lib/cn";
import { logActivity, relativeTime } from "@/stores/activity";
import { defaultProviderId, useWorkspaces, type Workspace } from "@/stores/workspaces";
import { listWorktrees, removeWorktree, type ExistingWorktree } from "./worktree";
import { ContextMenu, useContextMenu } from "@/components/ContextMenu";
import { openInWorktree, worktreeMenu } from "@/features/workspaces/sidebarMenus";

const REFRESH_MS = 20_000;

/** Last activity: the branch's last commit, else when the folder was made. */
const lastActivity = (wt: ExistingWorktree) => wt.lastCommitAt ?? wt.createdAt;

function describeState(wt: ExistingWorktree): string {
  const parts = [];
  if (wt.merged) parts.push("Nothing unmerged — safe to remove");
  if (wt.ahead > 0) parts.push(`${wt.ahead} commit${wt.ahead === 1 ? "" : "s"} not on ${wt.info.base}`);
  if (wt.dirty) parts.push("uncommitted changes");
  if (wt.lastCommitAt) parts.push(`last commit ${new Date(wt.lastCommitAt).toLocaleString()}`);
  if (wt.createdAt) parts.push(`created ${new Date(wt.createdAt).toLocaleString()}`);
  return parts.join(" · ");
}

/** "2d · 3 ahead · uncommitted" or "✓ merged · 5d": how old it is and whether it can go. */
function WorktreeState({ wt }: { wt: ExistingWorktree }) {
  if (wt.prunable) return <span className="block text-[10px] text-fg-subtle">folder missing</span>;
  const at = lastActivity(wt);
  const age = at ? relativeTime(at) : null;
  return (
    <span className="flex flex-wrap items-center gap-x-1.5 text-[10px] whitespace-nowrap text-fg-subtle">
      {wt.merged && <span className="text-success">✓ merged</span>}
      {age && <span title={describeState(wt)}>{age === "now" ? "just now" : `${age} ago`}</span>}
      {wt.ahead > 0 && <span>{wt.ahead} ahead</span>}
      {wt.dirty && <span className="text-warning">uncommitted</span>}
    </span>
  );
}

/**
 * Worktrees of the workspace's repo that no pane is using: left behind by closed agents or
 * made outside winfer. Clicking one opens the default agent inside it.
 */
export function WorktreeList({ workspace }: { workspace: Workspace }) {
  const [found, setFound] = useState<ExistingWorktree[]>([]);
  const [menu, openMenu, closeMenu] = useContextMenu<ExistingWorktree>();
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
  // Newest activity first; finished (merged) ones sink to the bottom, ready to clean up.
  const free = found
    .filter((w) => !used.has(w.info.path))
    .sort((a, b) => Number(a.merged) - Number(b.merged) || (lastActivity(b) ?? 0) - (lastActivity(a) ?? 0));
  if (free.length === 0) return null;

  const open = (wt: ExistingWorktree) => {
    openInWorktree(workspace, wt, defaultProviderId());
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
      {menu && (
        <ContextMenu
          x={menu.x}
          y={menu.y}
          title={menu.target.info.path}
          items={worktreeMenu(workspace, menu.target, () => void remove(menu.target))}
          onClose={closeMenu}
        />
      )}
      <div className="px-0 pt-1 pb-0.5 text-[10px] font-medium tracking-wide text-fg-subtle uppercase">Worktrees</div>
      {free.map((wt) => (
        <div
          key={wt.info.path}
          onContextMenu={(e) => openMenu(e, wt)}
          className="group flex items-center gap-1 rounded-md pr-1 hover:bg-hover"
        >
          <button
            onClick={() => !wt.prunable && open(wt)}
            disabled={wt.prunable}
            title={wt.prunable ? `${wt.info.path} (folder missing)` : `${wt.info.path}\n${describeState(wt)}\nClick to open an agent here`}
            className="flex min-w-0 flex-1 items-start gap-2 py-1 pr-1 text-left text-[13px] text-fg-muted group-hover:text-fg"
          >
            <GitBranch size={12} className="mt-1 shrink-0 text-fg-subtle" />
            <span className="min-w-0 flex-1">
              <span className={cn("block truncate font-mono text-[12px]", wt.prunable && "line-through")}>{wt.info.branch.replace(/^winfer\//, "")}</span>
              <WorktreeState wt={wt} />
            </span>
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
