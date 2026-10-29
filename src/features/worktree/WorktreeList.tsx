import { ask } from "@tauri-apps/plugin-dialog";
import { FolderGit2, GitBranch, ListChecks, Play, Plus, Trash2, X } from "lucide-react";
import { Spinner } from "@/components/Spinner";
import { useCallback, useEffect, useRef, useState } from "react";
import { cn } from "@/lib/cn";
import { logActivity, relativeTime } from "@/stores/activity";
import { toast, updateToast, withToast } from "@/stores/toasts";
import { defaultProviderId, useWorkspaces, type Workspace } from "@/stores/workspaces";
import { isStale, lastActivity, listWorktrees, removeWorktree, removeWorktrees, STALE_DAYS, worktreeInUse, type ExistingWorktree } from "./worktree";
import { ContextMenu, useContextMenu, type MenuItem } from "@/components/ContextMenu";
import { openBranchDialog, openInWorktree, worktreeMenu } from "@/features/workspaces/sidebarMenus";

/** How often the active workspace's worktrees are re-listed; other expanded workspaces' less often. */
const REFRESH_MS = 20_000;
const IDLE_REFRESH_MS = 120_000;
/** Focusing winfer re-lists only when the last listing is at least this old. */
const FOCUS_REFRESH_MS = 10_000;

const plural = (n: number, one: string, many = `${one}s`) => `${n} ${n === 1 ? one : many}`;

/** The confirmation for removing `list`: what goes, and what would be lost. */
function bulkWarning(list: ExistingWorktree[]): string {
  const own = list.filter((w) => w.byWinfer).length;
  const dirty = list.filter((w) => w.dirty).length;
  const unmerged = list.filter((w) => w.byWinfer && w.ahead > 0).length;
  const lines = [
    `Delete ${plural(list.length, "worktree")}${own ? `, and the branches of the ${own} winfer made` : ""}?`,
    "",
    ...list.slice(0, 12).map((w) => `• ${w.info.branch}`),
    ...(list.length > 12 ? [`• …and ${list.length - 12} more`] : []),
  ];
  const lost = [dirty && `${plural(dirty, "worktree")} with uncommitted changes`, unmerged && `${plural(unmerged, "branch", "branches")} with unmerged commits`].filter(Boolean);
  if (lost.length) lines.push("", `Lost for good: ${lost.join(" and ")}.`);
  if (own < list.length) lines.push("", "Branches not made by winfer are kept.");
  return lines.join("\n");
}

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
  // null until the first listing for this repo comes back.
  const [found, setFound] = useState<ExistingWorktree[] | null>(null);
  const [menu, openMenu, closeMenu] = useContextMenu<ExistingWorktree>();
  const [removing, setRemoving] = useState<Set<string>>(new Set());
  const [cleanupMenu, openCleanupMenu, closeCleanupMenu] = useContextMenu<true>();
  /** Picking worktrees to remove together; paths of the picked ones. */
  const [selecting, setSelecting] = useState(false);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const active = useWorkspaces((s) => s.activeWorkspaceId === workspace.id);
  // This workspace's agents in worktrees: one closing frees its worktree, which then belongs in the list.
  const ownWorktrees = useWorkspaces((s) => s.workspaces.find((w) => w.id === workspace.id)?.panes.filter((p) => p.worktree).length ?? 0);
  /** Folder being listed right now, if any. */
  const busy = useRef<string | null>(null);
  const lastRefresh = useRef(0);
  const currentDir = useRef(workspace.rootDir);
  currentDir.current = workspace.rootDir;

  // Each listing runs several git commands per worktree, slow on big repos and on Windows: never
  // two at once, and none while winfer is in the background.
  const refresh = useCallback(() => {
    const dir = workspace.rootDir;
    if (busy.current === dir) return;
    busy.current = dir;
    lastRefresh.current = Date.now();
    const show = (list: ExistingWorktree[]) => currentDir.current === dir && setFound(list);
    listWorktrees(dir)
      .then(show, () => show([]))
      .finally(() => busy.current === dir && (busy.current = null));
  }, [workspace.rootDir]);

  useEffect(() => {
    setFound(null);
    setSelecting(false);
    setSelected(new Set());
  }, [workspace.rootDir]);

  useEffect(() => {
    if (!selecting) return;
    const esc = (e: KeyboardEvent) => e.key === "Escape" && setSelecting(false);
    window.addEventListener("keydown", esc);
    return () => window.removeEventListener("keydown", esc);
  }, [selecting]);

  // Now, when the workspace becomes active, now and then (the active one more often), and on
  // focus unless it was just done.
  useEffect(() => {
    refresh();
    const timer = window.setInterval(() => !document.hidden && refresh(), active ? REFRESH_MS : IDLE_REFRESH_MS);
    const onFocus = () => Date.now() - lastRefresh.current >= FOCUS_REFRESH_MS && refresh();
    window.addEventListener("focus", onFocus);
    return () => {
      window.clearInterval(timer);
      window.removeEventListener("focus", onFocus);
    };
  }, [refresh, active]);

  // A moment after this workspace's agents come and go (several often close at once).
  const firstCount = useRef(true);
  useEffect(() => {
    if (firstCount.current) {
      firstCount.current = false;
      return;
    }
    const timer = window.setTimeout(refresh, 1000);
    return () => window.clearTimeout(timer);
  }, [refresh, ownWorktrees]);

  // Re-render when any pane's folder or worktree changes, in any workspace (one may share the repo).
  useWorkspaces((s) => s.workspaces.map((w) => w.panes.map((p) => `${p.cwd}\0${p.worktree?.path ?? ""}`).join("\n")).join("\n"));
  const panes = useWorkspaces.getState().workspaces.flatMap((w) => w.panes);
  // Newest activity first; finished (merged) ones sink to the bottom, ready to clean up.
  // The project folder (main checkout) always comes first; worktrees no pane uses follow.
  const loading = found === null;
  const main = found?.find((w) => w.isMain);
  const free = (found ?? [])
    .filter((w) => !w.isMain && !worktreeInUse(w.info, panes, workspace.id))
    .sort((a, b) => Number(a.merged) - Number(b.merged) || (lastActivity(b) ?? 0) - (lastActivity(a) ?? 0));
  const rows = main ? [main, ...free] : free;
  if (!loading && rows.length === 0) return null;

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
    const path = wt.info.path;
    setRemoving((s) => new Set(s).add(path));
    try {
      // Only winfer's own branches go too; a branch made by hand may still be wanted.
      await withToast(removeWorktree(wt.info, wt.byWinfer), {
        loading: `Removing ${wt.info.branch}…`,
        success: `Removed ${wt.info.branch}`,
        error: "Couldn't remove worktree",
      });
      logActivity(workspace.id, `Removed worktree ${wt.info.branch}`);
      // Gone right away, even if a listing that still has it is under way.
      setFound((f) => f && f.filter((w) => w.info.path !== path));
    } catch {
      // Shown in the toast.
    }
    setRemoving((s) => {
      const next = new Set(s);
      next.delete(path);
      return next;
    });
    refresh();
  };

  /** Removes `list` together (winfer's own branches too), after one confirmation. */
  const removeMany = async (list: ExistingWorktree[]) => {
    if (list.length === 0) return;
    if (!(await ask(bulkWarning(list), { title: `Remove ${plural(list.length, "worktree")}`, kind: "warning", okLabel: "Remove" }))) return;
    const paths = new Set(list.map((w) => w.info.path));
    setRemoving((s) => new Set([...s, ...paths]));
    const id = toast(`Removing ${plural(list.length, "worktree")}…`, "loading");
    const failed = await removeWorktrees(list.map((w) => ({ info: w.info, deleteBranch: w.byWinfer }))).catch((e) =>
      list.map((w) => ({ path: w.info.path, branch: w.info.branch, error: String(e) })),
    );
    const done = list.length - failed.length;
    if (failed.length) {
      updateToast(id, {
        kind: "error",
        text: done ? `Removed ${done} of ${list.length} worktrees` : "Couldn't remove worktrees",
        detail: failed.map((f) => `${f.branch}: ${f.error}`).join("\n"),
      });
    } else {
      updateToast(id, { kind: "success", text: `Removed ${plural(done, "worktree")}` });
    }
    if (done) logActivity(workspace.id, `Removed ${plural(done, "worktree")}`);
    const kept = new Set(failed.map((f) => f.path));
    setFound((f) => f && f.filter((w) => !paths.has(w.info.path) || kept.has(w.info.path)));
    setRemoving((s) => new Set([...s].filter((p) => !paths.has(p))));
    setSelected(new Set());
    setSelecting(false);
    refresh();
  };

  const stale = free.filter((w) => isStale(w));
  const merged = free.filter((w) => w.merged && !w.prunable);
  const missing = free.filter((w) => w.prunable);
  const picked = free.filter((w) => selected.has(w.info.path));
  const toggle = (path: string) =>
    setSelected((s) => {
      const next = new Set(s);
      if (!next.delete(path)) next.add(path);
      return next;
    });
  const startSelecting = (initial: ExistingWorktree[] = []) => {
    setSelected(new Set(initial.map((w) => w.info.path)));
    setSelecting(true);
  };

  const cleanupItems: MenuItem[] = [
    { label: "Select worktrees to remove…", disabled: free.length === 0, run: () => startSelecting() },
    { label: `Select older than ${STALE_DAYS} days…`, disabled: stale.length === 0, run: () => startSelecting(stale) },
    "separator",
    { label: `Remove older than ${STALE_DAYS} days (${stale.length})…`, disabled: stale.length === 0, danger: true, run: () => void removeMany(stale) },
    { label: `Remove merged (${merged.length})…`, disabled: merged.length === 0, danger: true, run: () => void removeMany(merged) },
    ...(missing.length ? [{ label: `Forget missing folders (${missing.length})…`, danger: true, run: () => void removeMany(missing) }] : []),
  ];

  const headerButton = "rounded px-1 py-0.5 text-[10px] font-medium text-fg-subtle hover:bg-hover hover:text-fg disabled:opacity-40";

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
      {cleanupMenu && <ContextMenu x={cleanupMenu.x} y={cleanupMenu.y} title="Clean up worktrees" items={cleanupItems} onClose={closeCleanupMenu} />}
      {selecting ? (
        <div className="flex items-center gap-0.5 pt-1 pb-0.5 pr-1">
          <span className="min-w-0 flex-1 truncate text-[10px] font-medium whitespace-nowrap text-fg-subtle">{picked.length} selected</span>
          <button
            onClick={() => setSelected(picked.length === free.length ? new Set() : new Set(free.map((w) => w.info.path)))}
            title={picked.length === free.length ? "Select none" : "Select every worktree"}
            className={headerButton}
          >
            {picked.length === free.length ? "None" : "All"}
          </button>
          <button onClick={() => setSelected(new Set(stale.map((w) => w.info.path)))} disabled={stale.length === 0} title={`Select the ones with no activity for ${STALE_DAYS} days`} className={headerButton}>
            &gt;{STALE_DAYS}d
          </button>
          <button onClick={() => void removeMany(picked)} disabled={picked.length === 0} title="Remove the selected worktrees" className={cn(headerButton, "text-danger hover:text-danger")}>
            Remove
          </button>
          <button onClick={() => setSelecting(false)} title="Stop selecting (Esc)" aria-label="Stop selecting" className="rounded p-0.5 text-fg-subtle hover:bg-hover hover:text-fg">
            <X size={12} />
          </button>
        </div>
      ) : (
        <div className="flex items-center pt-1 pb-0.5 pr-1">
          <span className="flex flex-1 items-center gap-1.5 text-[10px] font-medium tracking-wide text-fg-subtle uppercase">
            Branches
            {loading && <Spinner size={10} aria-label="Loading worktrees" />}
          </span>
          {free.length > 0 && (
            <button onClick={(e) => openCleanupMenu(e, true)} title="Clean up worktrees…" className="rounded p-0.5 text-fg-subtle hover:bg-hover hover:text-fg">
              <ListChecks size={12} />
            </button>
          )}
          <button onClick={() => openBranchDialog(workspace)} title="New worktree from a branch…" className="rounded p-0.5 text-fg-subtle hover:bg-hover hover:text-fg">
            <Plus size={12} />
          </button>
        </div>
      )}
      {rows.map((wt) => (
        <div
          key={wt.info.path}
          onContextMenu={(e) => !selecting && openMenu(e, wt)}
          className={cn(
            "anim-slide group flex items-center gap-1 rounded-md pr-1 transition-opacity hover:bg-hover",
            removing.has(wt.info.path) && "pointer-events-none opacity-50",
            selecting && wt.isMain && "pointer-events-none opacity-40",
          )}
        >
          <button
            onClick={() => (selecting ? toggle(wt.info.path) : !wt.prunable && open(wt))}
            disabled={!selecting && wt.prunable}
            title={
              wt.isMain
                ? `${wt.info.path}\nThe project folder itself, on ${wt.info.branch}\nClick to open an agent here (no worktree)`
                : wt.prunable
                  ? `Branch: ${wt.info.branch}\nWorktree: ${wt.info.path} (folder missing)`
                  : `Branch: ${wt.info.branch}\nWorktree: ${wt.info.path}\nBase branch: ${wt.info.base}\n${describeState(wt)}\n${selecting ? "Click to select" : "Click to open an agent here"}`
            }
            className="flex min-w-0 flex-1 items-start gap-2 py-1 pr-1 text-left text-[13px] text-fg-muted group-hover:text-fg"
          >
            {selecting && !wt.isMain ? (
              <input type="checkbox" readOnly tabIndex={-1} checked={selected.has(wt.info.path)} className="pointer-events-none mt-1 size-3 shrink-0 accent-accent" />
            ) : wt.isMain ? (
              <FolderGit2 size={12} className="mt-1 shrink-0 text-accent" />
            ) : (
              <GitBranch size={12} className="mt-1 shrink-0 text-fg-subtle" />
            )}
            <span className="min-w-0 flex-1">
              <span className="flex items-center gap-1.5">
                <span className={cn("truncate font-mono text-[12px]", wt.prunable && "line-through")}>{wt.info.branch.replace(/^winfer\//, "")}</span>
                {wt.isMain && <span className="shrink-0 rounded bg-accent-soft px-1 text-[9px] font-medium text-accent uppercase">project</span>}
              </span>
              <WorktreeState wt={wt} />
            </span>
          </button>
          {!selecting && !wt.prunable && (
            <button onClick={() => open(wt)} title="Open an agent in this worktree" className="rounded p-0.5 text-fg-subtle opacity-0 group-hover:opacity-100 hover:text-fg">
              <Play size={11} />
            </button>
          )}
          {removing.has(wt.info.path) ? (
            <Spinner size={11} className="mr-0.5 text-fg-subtle" />
          ) : !wt.isMain && !selecting && (
            <button
              onClick={() => void remove(wt)}
              title="Remove this worktree"
              className="rounded p-0.5 text-fg-subtle opacity-0 group-hover:opacity-100 hover:text-danger"
            >
              <Trash2 size={11} />
            </button>
          )}
        </div>
      ))}
    </div>
  );
}
