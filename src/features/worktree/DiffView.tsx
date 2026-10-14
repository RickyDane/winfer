import { GitBranch, GitMerge, Loader2, RefreshCw, Trash2, X } from "lucide-react";
import { useCallback, useEffect, useMemo, useState } from "react";
import { execute } from "@/commands/bus";
import { cn } from "@/lib/cn";
import { useWorkspaces } from "@/stores/workspaces";
import { worktreeDiff, type WorktreeDiff } from "./worktree";

/** Changes an agent made in its worktree (committed + uncommitted, since the branch point). */
export function DiffView() {
  const { diffPaneId, setDiffPane, workspaces } = useWorkspaces();
  const pane = workspaces.flatMap((w) => w.panes).find((p) => p.id === diffPaneId);
  const [data, setData] = useState<WorktreeDiff | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);

  const load = useCallback(() => {
    if (!pane?.worktree) return;
    setError(null);
    setLoading(true);
    worktreeDiff(pane.worktree)
      .then(setData, (e) => setError(String(e)))
      .finally(() => setLoading(false));
  }, [pane?.worktree]);

  useEffect(() => {
    setData(null);
    load();
  }, [load]);

  useEffect(() => {
    if (!diffPaneId) return;
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && setDiffPane(null);
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [diffPaneId, setDiffPane]);

  const files = useMemo(() => splitFiles(data?.diff ?? ""), [data]);

  if (!pane?.worktree) return null;
  const { branch, base, path } = pane.worktree;
  const target = { id: pane.id };

  return (
    <div className="anim-backdrop fixed inset-0 z-50 flex items-center justify-center bg-black/40 p-10" onMouseDown={() => setDiffPane(null)}>
      <div
        className="anim-dialog flex h-full max-h-[85vh] w-full max-w-5xl flex-col overflow-hidden rounded-xl border border-border-strong bg-panel-raised shadow-2xl"
        onMouseDown={(e) => e.stopPropagation()}
      >
        <header className="flex items-center gap-3 border-b border-border px-4 py-3">
          <GitBranch size={15} className="text-accent" />
          <div className="min-w-0 flex-1">
            <div className="font-mono text-[13px] text-fg">
              {branch} <span className="text-fg-subtle">→ {base}</span>
            </div>
            <div className="truncate font-mono text-[11px] text-fg-subtle">{path}</div>
          </div>
          {data && (
            <span className="text-xs text-fg-muted">
              {data.ahead} commit{data.ahead === 1 ? "" : "s"} ahead{data.dirty && " · uncommitted changes"}
            </span>
          )}
          <button onClick={load} disabled={loading} className="rounded p-1.5 text-fg-subtle hover:bg-hover hover:text-fg" title="Refresh">
            <RefreshCw size={14} className={cn(loading && "animate-spin")} />
          </button>
          <button
            onClick={() => execute({ type: "pane.mergeWorktree", target })}
            className="flex items-center gap-1.5 rounded-md bg-accent px-2.5 py-1 text-xs font-medium text-white hover:opacity-90"
          >
            <GitMerge size={13} /> Merge into {base}
          </button>
          <button
            onClick={() => {
              setDiffPane(null);
              execute({ type: "pane.discardWorktree", target });
            }}
            className="rounded p-1.5 text-fg-subtle hover:bg-hover hover:text-danger"
            title="Discard worktree"
          >
            <Trash2 size={14} />
          </button>
          <button onClick={() => setDiffPane(null)} className="rounded p-1.5 text-fg-subtle hover:bg-hover hover:text-fg" title="Close (Esc)">
            <X size={14} />
          </button>
        </header>

        <div className="flex-1 overflow-auto font-mono text-[12px] leading-[1.55] [font-variant-ligatures:none] select-text">
          {error && <div className="p-4 text-danger">{error}</div>}
          {!data && !error && (
            <div className="flex items-center gap-2 p-4 text-fg-subtle">
              <Loader2 size={13} className="animate-spin" /> Loading changes…
            </div>
          )}
          {data && files.length === 0 && data.untracked.length === 0 && <div className="p-4 text-fg-subtle">No changes yet.</div>}
          {data && data.untracked.length > 0 && (
            <section className="border-b border-border px-4 py-3">
              <div className="mb-1 text-[11px] text-fg-subtle uppercase">New files (untracked)</div>
              {data.untracked.map((f) => (
                <div key={f} className="text-success">+ {f}</div>
              ))}
            </section>
          )}
          {files.map((file) => (
            <section key={file.name} className="border-b border-border">
              <div className="sticky top-0 bg-panel px-4 py-1.5 text-fg">{file.name}</div>
              <pre className="px-4 py-1">
                {file.lines.map((line, i) => (
                  <div
                    key={i}
                    className={cn(
                      "whitespace-pre-wrap",
                      line.startsWith("+") && "bg-success/10 text-success",
                      line.startsWith("-") && "bg-danger/10 text-danger",
                      line.startsWith("@@") && "text-accent",
                      !/^[+\-@]/.test(line) && "text-fg-muted",
                    )}
                  >
                    {line || " "}
                  </div>
                ))}
              </pre>
            </section>
          ))}
        </div>
      </div>
    </div>
  );
}

/** Splits a unified diff into files, dropping the git header lines. */
function splitFiles(diff: string) {
  const files: { name: string; lines: string[] }[] = [];
  for (const line of diff.split("\n")) {
    if (line.startsWith("diff --git ")) {
      files.push({ name: line.replace(/^diff --git a\/(.*) b\/.*$/, "$1"), lines: [] });
    } else if (files.length && !/^(index |--- |\+\+\+ |new file mode|deleted file mode|similarity index|rename (from|to) )/.test(line)) {
      files.at(-1)!.lines.push(line);
    }
  }
  return files;
}
