import { CloudDownload, FolderPlus, GitBranch, Loader2, Play, Search, X } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { cn } from "@/lib/cn";
import { relativeTime, logActivity } from "@/stores/activity";
import { defaultProviderId, providerName, useActiveWorkspace, useWorkspaces } from "@/stores/workspaces";
import { fetchBranches, listBranches, worktreeFromBranch, type Branch } from "./worktree";

const folderName = (path: string) => path.split(/[\\/]/).filter(Boolean).pop() ?? path;

/**
 * New worktree from a branch: pick a local or remote branch (or start a new branch from one),
 * then open an agent in it or make it a workspace of its own.
 */
export function BranchDialog() {
  const { overlay, setOverlay, providers } = useWorkspaces();
  const workspace = useActiveWorkspace();
  const [branches, setBranches] = useState<Branch[] | null>(null);
  const [query, setQuery] = useState("");
  const [selected, setSelected] = useState<Branch | null>(null);
  const [newBranch, setNewBranch] = useState<string | null>(null);
  const [busy, setBusy] = useState<"fetch" | "agent" | "workspace" | null>(null);
  const [error, setError] = useState<string | null>(null);
  const search = useRef<HTMLInputElement>(null);
  const isOpen = overlay === "branch";
  const dir = workspace?.rootDir;

  const load = () => {
    if (!dir) return;
    listBranches(dir).then(setBranches, (e) => {
      setBranches([]);
      setError(String(e));
    });
  };

  useEffect(() => {
    if (!isOpen) return;
    setQuery("");
    setSelected(null);
    setNewBranch(null);
    setError(null);
    setBranches(null);
    load();
    search.current?.focus();
  }, [isOpen, dir]); // eslint-disable-line react-hooks/exhaustive-deps

  if (!isOpen || !workspace || !dir) return null;

  const words = query.toLowerCase().split(/\s+/).filter(Boolean);
  const shown = (branches ?? []).filter((b) => words.every((w) => b.name.toLowerCase().includes(w)));
  const blocked = selected && newBranch === null && selected.checkedOutAt;
  const agent = providerName(providers, defaultProviderId());

  const create = async (as: "agent" | "workspace") => {
    if (!selected) return;
    setBusy(as);
    setError(null);
    try {
      const info = await worktreeFromBranch(dir, selected.name, newBranch?.trim() || null);
      const s = useWorkspaces.getState();
      if (as === "workspace") {
        // Its own workspace, rooted in the worktree: agents there work on this branch directly.
        const id = s.createWorkspace(info.branch, info.path);
        useWorkspaces.setState((st) => ({ workspaces: st.workspaces.map((w) => (w.id === id ? { ...w, useWorktrees: false } : w)) }));
        logActivity(id, `Workspace for branch ${info.branch}`);
      } else {
        s.addPane(defaultProviderId(), { workspaceId: workspace.id, existingWorktree: info });
        logActivity(workspace.id, `Worktree for ${info.branch}`);
      }
      setOverlay(null);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(null);
    }
  };

  return (
    <div className="anim-backdrop fixed inset-0 z-40 flex justify-center bg-black/40 pt-[10vh]" onMouseDown={() => setOverlay(null)}>
      <div className="anim-dialog flex h-fit max-h-[78vh] w-[620px] flex-col rounded-xl border border-border-strong bg-panel shadow-2xl" onMouseDown={(e) => e.stopPropagation()}>
        <header className="flex items-center gap-2 border-b border-border px-5 py-3">
          <GitBranch size={14} className="text-accent" />
          <h1 className="flex-1 text-sm font-semibold text-fg">New worktree from a branch</h1>
          <span className="truncate text-xs text-fg-subtle">{workspace.name}</span>
          <button onClick={() => setOverlay(null)} className="rounded p-1 text-fg-subtle hover:bg-hover hover:text-fg">
            <X size={14} />
          </button>
        </header>

        <div className="flex items-center gap-2 border-b border-border px-5 py-2.5">
          <div className="flex flex-1 items-center gap-2 rounded-md border border-border bg-panel-raised px-2.5 focus-within:border-accent">
            <Search size={13} className="text-fg-subtle" />
            <input
              ref={search}
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              placeholder="Find a branch…"
              className="h-8 flex-1 bg-transparent text-[13px] text-fg outline-none placeholder:text-fg-subtle"
            />
          </div>
          <button
            onClick={async () => {
              setBusy("fetch");
              setError(null);
              await fetchBranches(dir).then(load, (e) => setError(`Fetch failed: ${e}`));
              setBusy(null);
            }}
            disabled={busy !== null}
            title="Get the latest branches from the remotes (git fetch --all --prune)"
            className="flex items-center gap-1.5 rounded-md border border-border px-2 py-1.5 text-xs text-fg-muted hover:bg-hover hover:text-fg disabled:opacity-50"
          >
            {busy === "fetch" ? <Loader2 size={12} className="animate-spin" /> : <CloudDownload size={12} />} Fetch
          </button>
        </div>

        <div className="min-h-32 flex-1 overflow-y-auto p-2">
          {branches === null ? (
            <p className="flex items-center justify-center gap-2 py-8 text-xs text-fg-subtle">
              <Loader2 size={12} className="animate-spin" /> Reading branches…
            </p>
          ) : shown.length === 0 ? (
            <p className="py-8 text-center text-xs text-fg-subtle">{branches.length === 0 ? "No branches (is this a git repository?)" : "No branch matches."}</p>
          ) : (
            shown.map((b) => (
              <button
                key={b.name}
                onClick={() => {
                  setSelected(b);
                  setError(null);
                }}
                className={cn(
                  "flex w-full items-center gap-2 rounded-md px-2.5 py-1.5 text-left text-[13px]",
                  selected?.name === b.name ? "bg-accent-soft text-fg" : "text-fg-muted hover:bg-hover hover:text-fg",
                )}
              >
                <GitBranch size={12} className="shrink-0 text-fg-subtle" />
                <span className="min-w-0 flex-1 truncate font-mono text-[12px]">{b.name}</span>
                {b.remote && <span className="shrink-0 rounded bg-hover px-1 text-[10px] text-fg-subtle">remote</span>}
                {b.checkedOutAt && (
                  <span className="max-w-40 shrink-0 truncate text-[10px] text-warning" title={b.checkedOutAt}>
                    in {folderName(b.checkedOutAt)}
                  </span>
                )}
                {b.lastCommitAt && <span className="shrink-0 text-[11px] text-fg-subtle">{relativeTime(b.lastCommitAt)}</span>}
              </button>
            ))
          )}
        </div>

        <footer className="space-y-2.5 border-t border-border px-5 py-3">
          <div className="flex items-center gap-3 text-xs text-fg-muted">
            <label className="flex items-center gap-1.5">
              <input type="radio" checked={newBranch === null} onChange={() => setNewBranch(null)} />
              Work on {selected ? <span className="font-mono text-fg">{selected.remote ? selected.name.split("/").slice(1).join("/") : selected.name}</span> : "the branch"}
            </label>
            <label className="flex flex-1 items-center gap-1.5">
              <input type="radio" checked={newBranch !== null} onChange={() => setNewBranch(newBranch ?? "")} />
              New branch from it:
              <input
                value={newBranch ?? ""}
                onChange={(e) => setNewBranch(e.target.value)}
                onFocus={() => newBranch === null && setNewBranch("")}
                placeholder="feature/my-idea"
                className="h-6 min-w-0 flex-1 rounded border border-border bg-panel-raised px-1.5 font-mono text-[11px] text-fg outline-none focus:border-accent"
              />
            </label>
          </div>
          {blocked && (
            <p className="text-xs text-warning">
              Already checked out in {folderName(selected.checkedOutAt!)}: open that one from the sidebar, or start a new branch from it.
            </p>
          )}
          {error && <p className="text-xs text-danger">{error}</p>}
          <div className="flex items-center justify-end gap-2">
            <button
              onClick={() => void create("workspace")}
              disabled={!selected || !!blocked || busy !== null || newBranch === ""}
              className="flex items-center gap-1.5 rounded-md border border-border-strong px-2.5 py-1.5 text-xs text-fg hover:bg-hover disabled:opacity-40"
              title="Create the worktree and make it a workspace of its own"
            >
              {busy === "workspace" ? <Loader2 size={12} className="animate-spin" /> : <FolderPlus size={12} />} New workspace from it
            </button>
            <button
              onClick={() => void create("agent")}
              disabled={!selected || !!blocked || busy !== null || newBranch === ""}
              className="flex items-center gap-1.5 rounded-md bg-accent px-2.5 py-1.5 text-xs font-medium text-on-accent hover:opacity-90 disabled:opacity-40"
            >
              {busy === "agent" ? <Loader2 size={12} className="animate-spin" /> : <Play size={12} />} Open {agent} in a worktree
            </button>
          </div>
        </footer>
      </div>
    </div>
  );
}
