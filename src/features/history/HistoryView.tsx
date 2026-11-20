import { ask } from "@tauri-apps/plugin-dialog";
import { Clock, Copy, GitMerge, Search, Trash2, X } from "lucide-react";
import { relativeTime } from "@/stores/activity";
import { useEffect, useRef, useState } from "react";
import { keysFor } from "@/commands/keymap";
import { cn } from "@/lib/cn";
import { ProviderIcon } from "@/features/providers/ProviderIcon";
import { providerName, useWorkspaces, useWorkspacesPick } from "@/stores/workspaces";
import { copyWithToast } from "@/stores/toasts";
import { formatCost, formatTokens } from "@/features/usage/history";
import { deleteHistoryEntry, searchHistory, type MergeRecord } from "./history";

const dateFormat = new Intl.DateTimeFormat(undefined, { dateStyle: "medium", timeStyle: "short" });

/** "Today", "Yesterday", "3 days ago", … for grouping merges (same as the native app). */
export function dayLabel(at: number, now = Date.now()): string {
  const day = (t: number) => Math.floor(new Date(t).setHours(0, 0, 0, 0) / 86_400_000);
  const days = Math.max(0, Math.round(day(now) - day(at)));
  if (days === 0) return "Today";
  if (days === 1) return "Yesterday";
  if (days < 7) return `${days} days ago`;
  if (days < 14) return "Last week";
  return `${Math.floor(days / 7)} weeks ago`;
}

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
    <article className={cn("anim-rise rounded-lg border", open ? "border-border bg-panel" : "border-transparent hover:bg-hover")}>
      <button onClick={() => setOpen((o) => !o)} className="flex w-full items-center gap-2.5 px-3 py-2.5 text-left">
        <span className="grid size-[26px] shrink-0 place-items-center rounded-md bg-success/12 text-success">
          <GitMerge size={13} />
        </span>
        <div className="min-w-0 flex-1">
          <div className="truncate text-[13px] font-medium text-fg">
            <Marked text={merge.title} words={words} />
          </div>
          <div className="flex min-w-0 items-center gap-1.5 font-mono text-[11px] text-fg-subtle">
            <ProviderIcon provider={merge.provider} size={11} />
            <span className="truncate" title={providerName(providers, merge.provider)}>
              {merge.workspace} · <Marked text={merge.branch} words={words} />
            </span>
          </div>
        </div>
        <span className="flex shrink-0 items-center gap-1.5 text-[11px]">
          {merge.tests && <span className={merge.tests.endsWith("passed") ? "text-success" : "text-danger"}>{merge.tests.endsWith("passed") ? "tests pass" : "tests fail"}</span>}
          {merge.cost !== null && merge.cost !== undefined && (
            <span className="font-mono text-fg-subtle" title={`What the agents spent on it at API prices${merge.tokens ? ` (${formatTokens(merge.tokens)} tokens)` : ""}`}>
              {formatCost(merge.cost)}
            </span>
          )}
          <span className="text-fg-subtle">{merge.files.length} files</span>
          <span className="text-success">+{merge.adds}</span>
          <span className="text-danger">−{merge.dels}</span>
          <span className="w-14 text-right text-fg-subtle" title={dateFormat.format(merge.mergedAt)}>
            {relativeTime(merge.mergedAt)}
          </span>
        </span>
      </button>
      {open && (
        <div className="space-y-2.5 px-3 pt-0.5 pb-3 pl-[46px] text-xs">
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

  const total = (results ?? []).reduce((t, m) => ({ files: t.files + m.files.length, adds: t.adds + m.adds, dels: t.dels + m.dels }), { files: 0, adds: 0, dels: 0 });
  const stat = (label: string, value: string, tone = "text-fg") => (
    <div className="flex-1 rounded-lg border border-border bg-panel p-2.5">
      <div className={cn("text-[17px] font-semibold", tone)}>{value}</div>
      <div className="text-[11px] text-fg-subtle">{label}</div>
    </div>
  );
  let lastDay = "";

  return (
    <div className="anim-backdrop fixed inset-0 z-40 flex justify-center bg-black/40 pt-[8vh]" onMouseDown={() => setOverlay(null)}>
      <div className="anim-dialog flex h-[min(640px,84vh)] w-[820px] max-w-[94vw] flex-col isolate overflow-hidden rounded-[14px] border border-border-strong bg-panel-raised shadow-2xl" onMouseDown={(e) => e.stopPropagation()}>
        <header className="space-y-3 border-b border-border px-5 pt-4 pb-3">
          <div className="flex items-center gap-2">
            <Clock size={15} className="text-accent" />
            <h1 className="flex-1 text-[15px] font-semibold text-fg">History</h1>
            <div className="flex gap-0.5 rounded-lg border border-border bg-app p-0.5 text-xs">
              {(["workspace", "all"] as const).map((sc) => (
                <button key={sc} onClick={() => setScope(sc)} className={cn("h-6 rounded-md px-2.5", scope === sc ? "bg-active text-fg" : "text-fg-muted hover:text-fg")}>
                  {sc === "workspace" ? (workspace?.name ?? "This workspace") : "All workspaces"}
                </button>
              ))}
            </div>
            <kbd className="rounded border border-border px-1 font-mono text-[10.5px] text-fg-subtle">{keysFor({ type: "ui.overlay", overlay: "history" })}</kbd>
            <button onClick={() => setOverlay(null)} aria-label="Close" className="rounded p-1 text-fg-subtle hover:bg-hover hover:text-fg">
              <X size={14} />
            </button>
          </div>
          <div className="flex gap-2">
            {stat("merged", String(results?.length ?? 0))}
            {stat("files changed", String(total.files))}
            {stat("lines added", `+${total.adds}`, "text-success")}
            {stat("lines removed", `−${total.dels}`, "text-danger")}
          </div>
          <label className="flex h-[34px] items-center gap-2 rounded-md border border-border-strong bg-app px-2.5 focus-within:border-accent">
            <Search size={13} className="text-fg-subtle" />
            <input
              ref={input}
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              placeholder="Search titles, summaries, tasks, branches, files…"
              className="flex-1 bg-transparent text-[13px] text-fg outline-none placeholder:text-fg-subtle"
            />
            {query && (
              <button onClick={() => setQuery("")} aria-label="Clear search" className="text-fg-subtle hover:text-fg">
                <X size={12} />
              </button>
            )}
          </label>
        </header>
        <div className="flex-1 space-y-1 overflow-y-auto p-2">
          {results === null ? (
            [0, 1, 2].map((i) => <div key={i} className="h-14 animate-pulse rounded-lg bg-panel" />)
          ) : results.length === 0 ? (
            <p className="py-10 text-center text-xs text-fg-subtle">
              {query ? "Nothing matches." : "No merged features yet. Merging from the review inbox, the pane menu, by voice or by picking a race winner adds them here."}
            </p>
          ) : (
            results.map((m) => {
              const day = dayLabel(m.mergedAt);
              const heading = day !== lastDay;
              lastDay = day;
              return (
                <div key={m.id}>
                  {heading && <h2 className="px-3 pt-3 pb-1 text-[11px] font-semibold tracking-wide text-fg-subtle uppercase">{day}</h2>}
                  <Entry merge={m} words={words} onDeleted={() => setVersion((v) => v + 1)} />
                </div>
              );
            })
          )}
        </div>
      </div>
    </div>
  );
}
