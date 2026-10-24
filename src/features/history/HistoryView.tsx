import { ask } from "@tauri-apps/plugin-dialog";
import { ChevronRight, Copy, GitMerge, History, Search, Trash2, X } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { keysFor } from "@/commands/keymap";
import { cn } from "@/lib/cn";
import { ProviderIcon } from "@/features/providers/ProviderIcon";
import { providerName, useWorkspaces, useWorkspacesPick } from "@/stores/workspaces";
import { copyWithToast } from "@/stores/toasts";
import { formatCost, formatTokens } from "@/features/usage/history";
import { deleteHistoryEntry, searchHistory, type MergeRecord } from "./history";

const dateFormat = new Intl.DateTimeFormat(undefined, { dateStyle: "medium", timeStyle: "short" });

/** Highlights the search words in `text`. */
function Marked({ text, words }: { text: string; words: string[] }) {
  if (words.length === 0) return <>{text}</>;
  const pattern = new RegExp(`(${words.map((w) => w.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")).join("|")})`, "gi");
  return (
    <>
      {text.split(pattern).map((part, i) =>
        i % 2 === 1 ? (
          <mark key={i} className="rounded-sm bg-accent-soft text-fg">
            {part}
          </mark>
        ) : (
          part
        ),
      )}
    </>
  );
}

function Entry({ merge, words, onDeleted }: { merge: MergeRecord; words: string[]; onDeleted: () => void }) {
  const providers = useWorkspaces((s) => s.providers);
  const [open, setOpen] = useState(false);
  return (
    <article className="anim-rise rounded-lg border border-border bg-panel-raised">
      <button onClick={() => setOpen((o) => !o)} className="flex w-full items-start gap-2.5 px-3 py-2.5 text-left">
        <ChevronRight size={13} className={cn("mt-0.5 shrink-0 text-fg-subtle transition-transform", open && "rotate-90")} />
        <div className="min-w-0 flex-1 space-y-1">
          <div className="text-[13px] font-medium text-fg">
            <Marked text={merge.title} words={words} />
          </div>
          <div className="flex flex-wrap items-center gap-x-2 gap-y-0.5 text-[11px] text-fg-subtle">
            <span className="flex items-center gap-1">
              <ProviderIcon provider={merge.provider} size={11} /> {providerName(providers, merge.provider)}
            </span>
            <span>{merge.workspace}</span>
            <span className="flex items-center gap-1 font-mono">
              <GitMerge size={10} /> <Marked text={merge.branch.replace(/^winfer\//, "")} words={words} /> → {merge.base}
            </span>
            <span className="font-mono">
              {merge.files.length} files <span className="text-success">+{merge.adds}</span> <span className="text-danger">−{merge.dels}</span>
            </span>
            {merge.tests && <span className={merge.tests.endsWith("passed") ? "text-success" : "text-danger"}>{merge.tests}</span>}
            {merge.cost !== null && merge.cost !== undefined && (
              <span className="font-mono" title={`What the agents spent on it at API prices${merge.tokens ? ` (${formatTokens(merge.tokens)} tokens)` : ""}`}>
                {formatCost(merge.cost)}
              </span>
            )}
          </div>
        </div>
        <span className="shrink-0 text-[11px] text-fg-subtle">{dateFormat.format(merge.mergedAt)}</span>
      </button>
      {open && (
        <div className="space-y-2.5 border-t border-border px-3 py-2.5 pl-9 text-xs">
          {merge.summary && (
            <p className="leading-relaxed text-fg">
              <Marked text={merge.summary} words={words} />
            </p>
          )}
          {merge.task && (
            <p className="text-fg-muted">
              <span className="text-fg-subtle">Task: </span>
              <Marked text={merge.task} words={words} />
            </p>
          )}
          {merge.files.length > 0 && (
            <div className="flex flex-wrap gap-1">
              {merge.files.map((f) => (
                <span key={f} className="max-w-72 truncate rounded bg-hover px-1.5 py-0.5 font-mono text-[10px] text-fg-muted" title={f}>
                  <Marked text={f} words={words} />
                </span>
              ))}
            </div>
          )}
          <div className="flex items-center gap-2 text-[11px] text-fg-subtle">
            <span className="truncate font-mono" title={merge.repo}>
              {merge.repo}
            </span>
            {merge.commitSha && (
              <button
                onClick={() => copyWithToast(merge.commitSha!, "Copied commit SHA")}
                className="flex items-center gap-1 rounded px-1.5 py-0.5 font-mono hover:bg-hover hover:text-fg"
                title="Copy the merge commit"
              >
                <Copy size={10} /> {merge.commitSha}
              </button>
            )}
            <span className="flex-1" />
            <button
              onClick={async () => {
                if (!(await ask(`Remove “${merge.title}” from the history? The merge itself stays in git.`, { title: "Remove from history", okLabel: "Remove" }))) return;
                await deleteHistoryEntry(merge.id);
                onDeleted();
              }}
              className="flex items-center gap-1 rounded px-1.5 py-0.5 hover:bg-hover hover:text-danger"
            >
              <Trash2 size={10} /> Remove
            </button>
          </div>
        </div>
      )}
    </article>
  );
}

/** Searchable history of merged features (⌘⇧H). */
export function HistoryView() {
  const { overlay, setOverlay, activeWorkspaceId, workspaces } = useWorkspacesPick("overlay", "setOverlay", "activeWorkspaceId", "workspaces");
  const [query, setQuery] = useState("");
  const [scope, setScope] = useState<"workspace" | "all">("workspace");
  const [results, setResults] = useState<MergeRecord[] | null>(null);
  const [version, setVersion] = useState(0);
  const input = useRef<HTMLInputElement>(null);
  const isOpen = overlay === "history";

  // Opened from the overview, search every workspace; from a workspace, that one.
  const fromOverview = useWorkspaces((s) => s.view === "overview");
  useEffect(() => {
    if (isOpen) setScope(fromOverview ? "all" : "workspace");
  }, [isOpen, fromOverview]);

  useEffect(() => {
    if (isOpen) input.current?.focus();
  }, [isOpen]);

  useEffect(() => {
    if (!isOpen) return;
    const timer = window.setTimeout(() => {
      searchHistory(query, scope === "workspace" ? activeWorkspaceId : null).then(setResults, () => setResults([]));
    }, 120);
    return () => window.clearTimeout(timer);
  }, [isOpen, query, scope, activeWorkspaceId, version]);

  if (!isOpen) return null;
  const words = query.split(/\s+/).filter(Boolean);
  const workspace = workspaces.find((w) => w.id === activeWorkspaceId);

  return (
    <div className="anim-backdrop fixed inset-0 z-40 flex justify-center bg-black/40 pt-[8vh]" onMouseDown={() => setOverlay(null)}>
      <div className="anim-dialog flex h-fit max-h-[84vh] w-[820px] flex-col rounded-xl border border-border-strong bg-panel shadow-2xl" onMouseDown={(e) => e.stopPropagation()}>
        <header className="flex items-center gap-2 border-b border-border px-5 py-3">
          <History size={14} className="text-accent" />
          <h1 className="flex-1 text-sm font-semibold text-fg">Merged features</h1>
          <span className="text-xs text-fg-subtle">{keysFor({ type: "ui.overlay", overlay: "history" })}</span>
          <button onClick={() => setOverlay(null)} className="rounded p-1 text-fg-subtle hover:bg-hover hover:text-fg">
            <X size={14} />
          </button>
        </header>
        <div className="flex items-center gap-2 border-b border-border px-5 py-2.5">
          <div className="flex flex-1 items-center gap-2 rounded-md border border-border bg-panel-raised px-2.5 focus-within:border-accent">
            <Search size={13} className="text-fg-subtle" />
            <input
              ref={input}
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              placeholder="Search titles, summaries, tasks, branches, files…"
              className="h-8 flex-1 bg-transparent text-[13px] text-fg outline-none placeholder:text-fg-subtle"
            />
            {query && (
              <button onClick={() => setQuery("")} className="text-fg-subtle hover:text-fg">
                <X size={12} />
              </button>
            )}
          </div>
          <div className="flex rounded-md border border-border p-0.5 text-xs">
            {(["workspace", "all"] as const).map((s) => (
              <button
                key={s}
                onClick={() => setScope(s)}
                className={cn("rounded px-2 py-1 transition-colors duration-150", scope === s ? "bg-active font-medium text-fg" : "text-fg-muted hover:text-fg")}
              >
                {s === "workspace" ? (workspace?.name ?? "This workspace") : "All workspaces"}
              </button>
            ))}
          </div>
        </div>
        <div className="space-y-2 overflow-y-auto p-4">
          {results === null ? (
            [0, 1, 2].map((i) => <div key={i} className="h-16 animate-pulse rounded-lg border border-border bg-panel-raised" />)
          ) : results.length === 0 ? (
            <p className="py-10 text-center text-xs text-fg-subtle">
              {query ? "Nothing matches." : "No merged features yet. Merging from the review inbox, the pane menu, by voice or by picking a race winner adds them here."}
            </p>
          ) : (
            results.map((m) => <Entry key={m.id} merge={m} words={words} onDeleted={() => setVersion((v) => v + 1)} />)
          )}
        </div>
      </div>
    </div>
  );
}
