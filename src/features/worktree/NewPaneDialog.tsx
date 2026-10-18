import { FolderOpen, GitBranch, Plus, Search, Sparkles, X } from "lucide-react";
import { Spinner } from "@/components/Spinner";
import { useEffect, useRef, useState } from "react";
import { formatChord, IS_MAC } from "@/commands/keymap";
import { cn } from "@/lib/cn";
import { relativeTime, logActivity } from "@/stores/activity";
import { defaultProviderId, providerName, useActiveWorkspace, useWorkspaces } from "@/stores/workspaces";
import { listBranches, listWorktrees, worktreeFromBranch, type Branch, type ExistingWorktree } from "./worktree";

const folderName = (path: string) => path.split(/[\\/]/).filter(Boolean).pop() ?? path;
const samePath = (a: string, b: string) => a.replace(/[\\/]+$/, "") === b.replace(/[\\/]+$/, "");

type Row = { kind: "auto" } | { kind: "new"; name: string } | { kind: "branch"; branch: Branch };

/**
 * New pane (⌘T): start the agent on a new branch in its own worktree, or pick an existing branch:
 * one already checked out somewhere opens in that folder, any other gets a worktree of its own.
 */
export function NewPaneDialog() {
  const { overlay, setOverlay, providers, newPaneProvider } = useWorkspaces();
  const workspace = useActiveWorkspace();
  const [branches, setBranches] = useState<Branch[] | null>(null);
  const [worktrees, setWorktrees] = useState<ExistingWorktree[]>([]);
  const [query, setQuery] = useState("");
  const [index, setIndex] = useState(0);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const input = useRef<HTMLInputElement>(null);
  const list = useRef<HTMLDivElement>(null);
  const isOpen = overlay === "newPane";
  const dir = workspace?.rootDir;

  useEffect(() => {
    if (!isOpen || !dir) return;
    setQuery("");
    setIndex(0);
    setError(null);
    setBusy(false);
    setBranches(null);
    listBranches(dir).then(setBranches, () => setBranches([]));
    listWorktrees(dir).then(setWorktrees, () => setWorktrees([]));
    input.current?.focus();
  }, [isOpen, dir]);

  useEffect(() => {
    list.current?.querySelector(`[data-row="${index}"]`)?.scrollIntoView({ block: "nearest" });
  }, [index]);

  if (!isOpen || !workspace || !dir) return null;

  const close = () => setOverlay(null);
  const isRepo = branches === null || branches.length > 0;
  const name = query.trim();
  const words = query.toLowerCase().split(/\s+/).filter(Boolean);
  const matches = (branches ?? []).filter((b) => words.every((w) => b.name.toLowerCase().includes(w)));
  const exact = (branches ?? []).some((b) => b.name === name || (b.remote && b.name.split("/").slice(1).join("/") === name));
  const rows: Row[] = [
    name ? (exact ? null : { kind: "new" as const, name }) : { kind: "auto" as const },
    ...matches.map((branch) => ({ kind: "branch" as const, branch })),
  ].filter((r): r is Row => r !== null);
  const current = Math.min(index, Math.max(rows.length - 1, 0));
  const provider = newPaneProvider ?? defaultProviderId();
  const agent = providerName(providers, provider);

  const open = async (row: Row) => {
    const s = useWorkspaces.getState();
    const opts = { workspaceId: workspace.id };
    if (row.kind === "auto" || !isRepo) {
      s.addPane(provider, opts);
      logActivity(workspace.id, `Opened ${agent}`, provider);
      return close();
    }
    const at = row.kind === "branch" ? row.branch.checkedOutAt : null;
    if (at) {
      // Already checked out: open the agent where it lives (the project folder itself works without a worktree).
      const wt = worktrees.find((w) => samePath(w.info.path, at));
      if (wt && !wt.isMain) s.addPane(provider, { ...opts, existingWorktree: wt.info });
      else s.addPane(provider, { ...opts, worktree: false, cwd: at });
      logActivity(workspace.id, `Opened ${agent} on ${row.kind === "branch" ? row.branch.name : ""}`, provider);
      return close();
    }
    setBusy(true);
    setError(null);
    try {
      // A new branch starts from the project folder's current branch.
      const info =
        row.kind === "new"
          ? await worktreeFromBranch(dir, "HEAD", row.name)
          : await worktreeFromBranch(dir, row.branch.name, null);
      s.addPane(provider, { ...opts, existingWorktree: info });
      logActivity(workspace.id, `Worktree for ${info.branch}`, provider);
      close();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const onKey = (e: React.KeyboardEvent) => {
    if (e.key === "Escape") close();
    else if (e.key === "ArrowDown") setIndex(Math.min(current + 1, rows.length - 1));
    else if (e.key === "ArrowUp") setIndex(Math.max(current - 1, 0));
    else if (e.key === "Enter" && !busy) {
      if (e.metaKey || e.ctrlKey) {
        // Skip the worktree: the agent works in the project folder.
        useWorkspaces.getState().addPane(provider, { workspaceId: workspace.id, worktree: false });
        close();
      } else if (rows[current]) void open(rows[current]);
    } else return;
    e.preventDefault();
  };

  const rowClass = (i: number) =>
    cn("flex w-full items-center gap-2 rounded-md px-2.5 py-1.5 text-left text-[13px]", i === current ? "bg-accent-soft text-fg" : "text-fg-muted hover:bg-hover hover:text-fg");

  return (
    <div className="anim-backdrop fixed inset-0 z-40 flex justify-center bg-black/40 pt-[10vh]" onMouseDown={close}>
      <div className="anim-dialog flex h-fit max-h-[78vh] w-[560px] flex-col rounded-xl border border-border-strong bg-panel shadow-2xl" onMouseDown={(e) => e.stopPropagation()}>
        <header className="flex items-center gap-2 border-b border-border px-5 py-3">
          <Plus size={14} className="text-accent" />
          <h1 className="flex-1 text-sm font-semibold text-fg">New {agent} pane</h1>
          <span className="truncate text-xs text-fg-subtle">{workspace.name}</span>
          <button onClick={close} className="rounded p-1 text-fg-subtle hover:bg-hover hover:text-fg">
            <X size={14} />
          </button>
        </header>

        <div className="border-b border-border px-5 py-2.5">
          <div className="flex items-center gap-2 rounded-md border border-border bg-panel-raised px-2.5 focus-within:border-accent">
            <Search size={13} className="text-fg-subtle" />
            <input
              ref={input}
              value={query}
              onChange={(e) => {
                setQuery(e.target.value);
                setIndex(0);
                setError(null);
              }}
              onKeyDown={onKey}
              disabled={busy}
              placeholder="New branch name, or find a branch…"
              className="h-8 flex-1 bg-transparent font-mono text-[12px] text-fg outline-none placeholder:font-sans placeholder:text-fg-subtle"
            />
            {busy && <Spinner size={12} className="text-fg-subtle" />}
          </div>
        </div>

        <div ref={list} className="min-h-24 flex-1 overflow-y-auto p-2">
          {rows.map((row, i) => (
            <button key={row.kind === "branch" ? row.branch.name : row.kind} data-row={i} onMouseEnter={() => setIndex(i)} onClick={() => !busy && void open(row)} className={rowClass(i)}>
              {row.kind === "auto" ? (
                <>
                  <Sparkles size={12} className="shrink-0 text-accent" />
                  <span className="flex-1">{isRepo ? "New worktree, auto-named branch" : "New pane"}</span>
                </>
              ) : row.kind === "new" ? (
                <>
                  <Plus size={12} className="shrink-0 text-accent" />
                  <span className="min-w-0 flex-1 truncate">
                    New branch <span className="font-mono text-[12px] text-fg">{row.name}</span>
                  </span>
                </>
              ) : (
                <>
                  <GitBranch size={12} className="shrink-0 text-fg-subtle" />
                  <span className="min-w-0 flex-1 truncate font-mono text-[12px]">{row.branch.name}</span>
                  {row.branch.remote && <span className="shrink-0 rounded bg-hover px-1 text-[10px] text-fg-subtle">remote</span>}
                  {row.branch.checkedOutAt && (
                    <span className="flex max-w-40 shrink-0 items-center gap-1 truncate text-[10px] text-fg-subtle" title={row.branch.checkedOutAt}>
                      <FolderOpen size={10} /> {folderName(row.branch.checkedOutAt)}
                    </span>
                  )}
                  {row.branch.lastCommitAt && <span className="shrink-0 text-[11px] text-fg-subtle">{relativeTime(row.branch.lastCommitAt)}</span>}
                </>
              )}
            </button>
          ))}
          {branches === null && (
            <p className="flex items-center justify-center gap-2 py-4 text-xs text-fg-subtle">
              <Spinner size={12} /> Reading branches…
            </p>
          )}
        </div>

        <footer className="space-y-1.5 border-t border-border px-5 py-2.5">
          {error && <p className="text-xs text-danger">{error}</p>}
          <p className="text-[11px] text-fg-subtle">↑↓ choose · {formatChord("Enter")} open · {formatChord(IS_MAC ? "Meta+Enter" : "Ctrl+Enter")} without worktree · esc cancel</p>
        </footer>
      </div>
    </div>
  );
}
