import { ask } from "@tauri-apps/plugin-dialog";
import { AlertTriangle, Check, CornerDownLeft, FlaskConical, GitBranch, GitMerge, Inbox, ListOrdered, ListPlus, Loader2, RotateCcw, Trash2, Trophy, X } from "lucide-react";
import { useEffect, useState } from "react";
import { execute } from "@/commands/bus";
import { keysFor } from "@/commands/keymap";
import { cn } from "@/lib/cn";
import { pickRaceWinner } from "@/features/orchestration/RaceView";
import { ProviderIcon } from "@/features/providers/ProviderIcon";
import { StatusDot } from "@/features/terminal/StatusDot";
import { closeTerminal } from "@/features/terminal/xtermRegistry";
import { removeWorktree } from "@/features/worktree/worktree";
import { logActivity, relativeTime } from "@/stores/activity";
import { providerName, useWorkspaces, type Pane } from "@/stores/workspaces";
import { withToast } from "@/stores/toasts";
import { TaskCost } from "@/features/usage/TaskCostChip";
import { refreshTaskCosts } from "@/features/usage/taskCost";
import { conflictsFor, refreshConflicts, useConflicts } from "./conflicts";
import { askAgentToResolve, clearFinished, dequeue, enqueue, isPending, retry, setQueueTests, useMergeQueue, type QueueItem } from "./mergeQueue";
import { discardReviewed, mergeReviewed, removeReview, runTests, setReviewStatus, useReviews, type Review } from "./reviews";

const button = "flex items-center gap-1 rounded-md px-2 py-1 text-xs disabled:opacity-40";

function Summary({ review }: { review: Review }) {
  const { summary } = review;
  if (summary.state === "loading" || summary.state === "idle")
    return (
      <div className="space-y-1.5">
        <p className="flex items-center gap-1.5 text-xs text-fg-subtle">
          <Loader2 size={12} className="animate-spin" /> Summarizing…
        </p>
        <span className="block h-2.5 w-11/12 animate-pulse rounded bg-hover" />
        <span className="block h-2.5 w-2/3 animate-pulse rounded bg-hover" />
      </div>
    );
  if (summary.state === "done") return <p className="anim-fade text-[13px] leading-relaxed text-fg">{summary.value}</p>;
  // Writer off or failed: the agent's own last words are the next best summary.
  return (
    <div className="space-y-1">
      {summary.state === "error" && <p className="text-[11px] text-fg-subtle">No summary ({summary.error.slice(0, 120)})</p>}
      <pre className="max-h-32 overflow-y-auto font-mono text-[11px] whitespace-pre-wrap text-fg-muted">{review.lastOutput || "(no output)"}</pre>
    </div>
  );
}

function Tests({ review }: { review: Review }) {
  const { tests, testCommand } = review;
  if (tests.state === "loading")
    return (
      <span className="flex items-center gap-1.5 text-xs text-fg-muted">
        <Loader2 size={12} className="animate-spin" /> Running {testCommand}…
      </span>
    );
  if (tests.state === "error") return <span className="text-xs text-danger">Tests couldn't run: {tests.error}</span>;
  if (tests.state === "done" && tests.value === null) return <span className="text-xs text-fg-subtle">No test command found</span>;
  if (tests.state === "done" && tests.value) {
    const run = tests.value;
    return (
      <details className="min-w-0 flex-1 text-xs">
        <summary className={cn("cursor-pointer", run.ok ? "text-success" : "text-danger")}>
          {run.ok ? "✓" : "✗"} {run.command} {run.ok ? "passed" : "failed"} · {Math.round(run.ms / 1000)}s
          <button onClick={() => void runTests(review.id)} className="ml-2 text-fg-subtle hover:text-fg">
            Rerun
          </button>
        </summary>
        <pre className="mt-1 max-h-48 overflow-auto rounded-md bg-panel p-2 font-mono text-[11px] whitespace-pre-wrap text-fg-muted">{run.output}</pre>
      </details>
    );
  }
  if (!testCommand) return null;
  return (
    <button onClick={() => void runTests(review.id)} className={cn(button, "border border-border text-fg-muted hover:bg-hover hover:text-fg")}>
      <FlaskConical size={12} /> Run {testCommand}
    </button>
  );
}

function Screenshot({ review }: { review: Review }) {
  const shot = review.screenshot;
  if (shot.state === "idle") return null;
  const open = () => {
    if (!shot.url) return;
    execute({ type: "browser.open", url: shot.url });
    useWorkspaces.getState().setOverlay(null);
  };
  return (
    <button onClick={open} title={`Open ${shot.url} in the browser`} className="group relative h-28 w-44 shrink-0 overflow-hidden rounded-md border border-border bg-panel">
      {shot.state === "done" ? (
        <img src={shot.value} alt={`Screenshot of ${shot.url}`} className="h-full w-full object-cover object-top" />
      ) : shot.state === "loading" ? (
        <span className="flex h-full items-center justify-center text-fg-subtle">
          <Loader2 size={14} className="animate-spin" />
        </span>
      ) : (
        <span className="flex h-full items-center justify-center p-2 text-center text-[11px] text-fg-subtle">{shot.error.slice(0, 90)}</span>
      )}
      <span className="absolute inset-x-0 bottom-0 truncate bg-black/60 px-1.5 py-0.5 text-left font-mono text-[10px] text-white">{shot.url}</span>
    </button>
  );
}

const short = (branch: string) => branch.replace(/^winfer\//, "");

/** "Codex · codex-2" for the agent working in a worktree, else the folder's name. */
function worktreeLabel(path: string, panes: Pane[], providers: Parameters<typeof providerName>[0]): string {
  const pane = panes.find((p) => p.worktree?.path === path);
  if (!pane?.worktree) return path.split(/[\\/]/).pop() ?? path;
  return `${pane.customTitle ?? providerName(providers, pane.provider)} · ${short(pane.worktree.branch)}`;
}

const fileList = (files: string[]) => (files.length > 3 ? `${files.slice(0, 3).join(", ")} +${files.length - 3} more` : files.join(", "));

/** Whether this branch collides with its base now, or with another agent's worktree. */
function ConflictNotes({ pane }: { pane: Pane | undefined }) {
  const report = useConflicts((s) => s.report);
  const providers = useWorkspaces((s) => s.providers);
  const panes = useWorkspaces((s) => s.workspaces.flatMap((w) => w.panes));
  if (!pane?.worktree) return null;
  const { item, others } = conflictsFor(report, pane.worktree.path);
  const base = pane.worktree.base;
  const notes: { tone: "danger" | "warn" | "muted"; text: string; title?: string }[] = [];
  if (item?.baseConflicts.length) notes.push({ tone: "danger", text: `Conflicts with ${base} in ${fileList(item.baseConflicts)}`, title: item.baseConflicts.join("\n") });
  else if (item && item.behind > 0) notes.push({ tone: "muted", text: `${base} moved ${item.behind} commit${item.behind === 1 ? "" : "s"} ahead · merges cleanly` });
  for (const o of others) {
    const who = worktreeLabel(o.other, panes, providers);
    if (o.conflicts.length) notes.push({ tone: "danger", text: `Conflicts with ${who} in ${fileList(o.conflicts)}`, title: o.conflicts.join("\n") });
    else notes.push({ tone: "warn", text: `${who} also edits ${fileList(o.overlap)}`, title: `Both changed:\n${o.overlap.join("\n")}\nNo conflicting lines in what's committed so far.` });
  }
  if (notes.length === 0) return null;
  return (
    <ul className="space-y-0.5">
      {notes.map((n) => (
        <li
          key={n.text}
          title={n.title}
          className={cn("flex items-center gap-1.5 text-[11px]", n.tone === "danger" ? "text-danger" : n.tone === "warn" ? "text-warning" : "text-fg-subtle")}
        >
          {n.tone !== "muted" && <AlertTriangle size={11} className="shrink-0" />}
          <span className="truncate">{n.text}</span>
        </li>
      ))}
    </ul>
  );
}

const STATE_LABEL: Record<QueueItem["state"], string> = {
  queued: "Queued",
  syncing: "Syncing",
  testing: "Testing",
  merging: "Merging",
  merged: "Merged",
  conflict: "Conflict",
  failed: "Stopped",
};

/** Where a branch is in the merge queue, with what can be done about it when it stopped. */
function QueueStatus({ item, position }: { item: QueueItem; position: number | null }) {
  const busy = item.state === "syncing" || item.state === "testing" || item.state === "merging";
  const stopped = item.state === "conflict" || item.state === "failed";
  return (
    <div className={cn("flex items-center gap-2 rounded-md px-2 py-1.5 text-xs", stopped ? "bg-danger/10 text-danger" : "bg-hover text-fg-muted")}>
      {busy ? <Loader2 size={12} className="shrink-0 animate-spin" /> : stopped ? <AlertTriangle size={12} className="shrink-0" /> : <ListOrdered size={12} className="shrink-0" />}
      <span className="min-w-0 flex-1 truncate" title={item.detail ?? undefined}>
        {item.state === "queued" ? `Queued to merge${position ? ` · #${position}` : ""}` : (item.detail ?? STATE_LABEL[item.state])}
        {item.state === "conflict" && item.files.length > 0 && <span className="font-mono"> · {fileList(item.files)}</span>}
      </span>
      {item.state === "conflict" && (
        <button onClick={() => askAgentToResolve(item.reviewId)} className={cn(button, "bg-accent text-on-accent hover:opacity-90")} title={`Ask the agent to merge ${item.base} in and resolve the conflicts`}>
          Ask agent to resolve
        </button>
      )}
      {stopped && (
        <button onClick={() => retry(item.reviewId)} className={cn(button, "text-fg-muted hover:bg-hover hover:text-fg")} title="Try again">
          <RotateCcw size={12} />
        </button>
      )}
      {(item.state === "queued" || stopped) && (
        <button onClick={() => dequeue(item.reviewId)} className={cn(button, "text-fg-muted hover:bg-hover hover:text-fg")} title="Take out of the queue">
          <X size={12} />
        </button>
      )}
    </div>
  );
}

/** The queue across the inbox: order, progress, and whether tests gate each merge. */
function QueuePanel({ items }: { items: QueueItem[] }) {
  const runTestsFirst = useMergeQueue((s) => s.runTests);
  const reviews = useReviews((s) => s.reviews);
  if (items.length === 0) return null;
  const done = items.filter((i) => i.state === "merged").length;
  const pending = items.filter(isPending).length;
  return (
    <section className="space-y-2 rounded-lg border border-border bg-panel-raised p-3">
      <header className="flex items-center gap-2 text-xs">
        <ListOrdered size={13} className="text-accent" />
        <span className="font-medium text-fg">Merge queue</span>
        <span className="text-fg-subtle">
          {done} of {items.length} merged{pending ? ` · ${pending} to go` : ""}
        </span>
        <label className="ml-auto flex cursor-pointer items-center gap-1.5 text-fg-muted" title="Run the project's tests on each branch, with the base's new commits brought in, before merging it">
          <input type="checkbox" checked={runTestsFirst} onChange={(e) => setQueueTests(e.target.checked)} className="accent-accent" />
          Test before each merge
        </label>
        {items.length > pending && (
          <button onClick={clearFinished} className="text-fg-subtle hover:text-fg">
            Clear finished
          </button>
        )}
      </header>
      <ol className="space-y-1">
        {items.map((i, n) => {
          const review = reviews.find((r) => r.id === i.reviewId);
          return (
            <li key={i.reviewId} className="flex items-center gap-2 text-[11px]">
              <span className="w-4 text-right font-mono text-fg-subtle">{n + 1}</span>
              {review && <ProviderIcon provider={review.provider} size={11} />}
              <span className="truncate font-mono text-fg-muted">
                {short(i.branch)} → {i.base}
              </span>
              <span
                className={cn(
                  "ml-auto shrink-0",
                  i.state === "merged" ? "text-success" : i.state === "conflict" || i.state === "failed" ? "text-danger" : isPending(i) && i.state !== "queued" ? "text-accent" : "text-fg-subtle",
                )}
                title={i.detail ?? undefined}
              >
                {STATE_LABEL[i.state]}
              </span>
            </li>
          );
        })}
      </ol>
    </section>
  );
}

function ReviewCard({ review, index, pane, race, workspaceName }: { review: Review; index: number; pane: Pane | undefined; race?: Pane[]; workspaceName?: string }) {
  const providers = useWorkspaces((s) => s.providers);
  const [reply, setReply] = useState("");
  const name = providerName(providers, review.provider);
  const stats = review.stats;
  const open = review.status === "open";
  const queue = useMergeQueue((s) => s.items);
  const queued = queue.find((i) => i.reviewId === review.id);
  const waiting = queue.filter((i) => i.state === "queued");
  const position = queued?.state === "queued" ? waiting.indexOf(queued) + 1 : null;

  const sendBack = () => {
    const text = reply.trim();
    if (!text || !pane) return;
    execute({ type: "pane.sendText", target: { id: pane.id }, text, submit: true });
    logActivity(review.workspaceId, `Sent back to ${name}: ${text.slice(0, 80)}`, review.provider);
    removeReview(review.id); // a fresh card arrives when the agent finishes again
  };

  const cleanUp = async () => {
    if (!pane?.worktree) return removeReview(review.id);
    const ok = await ask(`Close ${name} and delete its worktree and branch ${pane.worktree.branch}? The merged commits stay on ${pane.worktree.base}.`, {
      title: "Clean up",
      okLabel: "Close and delete",
    });
    if (!ok) return;
    closeTerminal(pane.id);
    useWorkspaces.getState().removePane(pane.id);
    await withToast(removeWorktree(pane.worktree, true), {
      loading: `Removing ${pane.worktree.branch}…`,
      success: `Removed ${pane.worktree.branch}`,
      error: "Couldn't remove worktree",
    }).catch(() => {});
    removeReview(review.id);
  };

  return (
    <article className={cn("anim-rise space-y-2.5 rounded-lg border border-border bg-panel-raised p-3 transition-opacity duration-300", !open && "opacity-60")}>
      <header className="flex items-center gap-2 text-xs">
        <span className="font-mono text-[11px] text-fg-subtle">{index}</span>
        {pane && <StatusDot status={pane.status} />}
        <ProviderIcon provider={review.provider} size={13} />
        <span className="font-medium text-fg">{name}</span>
        {workspaceName && (
          <span className="max-w-32 shrink-0 truncate rounded bg-hover px-1.5 py-0.5 text-[10px] font-medium text-fg-muted" title={`Workspace ${workspaceName}`}>
            {workspaceName}
          </span>
        )}
        {review.branch && (
          <span className="flex items-center gap-1 truncate font-mono text-[11px] text-fg-subtle">
            <GitBranch size={11} /> {review.branch.replace(/^winfer\//, "")}
          </span>
        )}
        {stats && (
          <span className="font-mono text-[11px]">
            <span className="text-fg-muted">{stats.files.length} files </span>
            <span className="text-success">+{stats.adds} </span>
            <span className="text-danger">−{stats.dels}</span>
            {stats.dirty && <span className="text-fg-subtle"> · uncommitted</span>}
          </span>
        )}
        <TaskCost dir={review.stats ? review.dir : null} className="ml-auto" />
        <span className={cn("text-[11px] text-fg-subtle", !review.stats && "ml-auto")}>{relativeTime(review.finishedAt)}</span>
        <button title="Remove from inbox" onClick={() => removeReview(review.id)} className="rounded p-0.5 text-fg-subtle hover:bg-hover hover:text-fg">
          <X size={12} />
        </button>
      </header>

      {review.prompt && <p className="truncate text-[11px] text-fg-subtle">Task: {review.prompt}</p>}

      <div className="flex gap-3">
        <div className="min-w-0 flex-1 space-y-2">
          <Summary review={review} />
          {stats && stats.files.length > 0 && (
            <div className="flex flex-wrap gap-1">
              {stats.files.slice(0, 6).map((f) => (
                <span key={f} className="max-w-56 truncate rounded bg-hover px-1.5 py-0.5 font-mono text-[10px] text-fg-muted" title={f}>
                  {f}
                </span>
              ))}
              {stats.files.length > 6 && <span className="px-1 text-[10px] text-fg-subtle">+{stats.files.length - 6} more</span>}
            </div>
          )}
          <div className="flex items-center gap-2">
            <Tests review={review} />
          </div>
          <ConflictNotes pane={pane} />
        </div>
        <Screenshot review={review} />
      </div>

      {review.summary.state === "done" && review.lastOutput && (
        <details className="text-[11px]">
          <summary className="cursor-pointer text-fg-subtle hover:text-fg">Agent's last output</summary>
          <pre className="mt-1 max-h-40 overflow-y-auto font-mono whitespace-pre-wrap text-fg-muted">{review.lastOutput}</pre>
        </details>
      )}

      {queued && open && <QueueStatus item={queued} position={position} />}

      {open ? (
        <footer className="flex items-center gap-1.5">
          <form
            className="flex flex-1 items-center gap-1 rounded-md border border-border bg-panel px-2 focus-within:border-accent"
            onSubmit={(e) => {
              e.preventDefault();
              sendBack();
            }}
          >
            <input
              value={reply}
              onChange={(e) => setReply(e.target.value)}
              disabled={!pane}
              placeholder={pane ? `Send back to ${name}, e.g. “make the header sticky”` : "The agent's pane is closed"}
              className="h-7 flex-1 bg-transparent text-xs text-fg outline-none placeholder:text-fg-subtle"
            />
            <button type="submit" disabled={!reply.trim() || !pane} className="text-fg-subtle hover:text-fg disabled:opacity-40" title="Send back">
              <CornerDownLeft size={12} />
            </button>
          </form>
          <button
            disabled={!pane}
            onClick={() => {
              if (!pane) return;
              useWorkspaces.getState().setOverlay(null);
              execute({ type: "pane.focus", target: { id: pane.id } });
            }}
            className={cn(button, "text-fg-muted hover:bg-hover hover:text-fg")}
          >
            Open
          </button>
          <button
            disabled={!pane?.worktree}
            onClick={() => pane && execute({ type: "pane.showChanges", target: { id: pane.id } })}
            className={cn(button, "text-fg-muted hover:bg-hover hover:text-fg")}
          >
            Diff
          </button>
          {race && race.length > 1 ? (
            <button
              disabled={!pane?.worktree}
              onClick={() => pane && void pickRaceWinner(pane, race).then((won) => won && setReviewStatus(review.id, "merged"))}
              className={cn(button, "bg-accent text-on-accent hover:opacity-90")}
              title="Merge this one and close the other racers"
            >
              <Trophy size={12} /> Pick
            </button>
          ) : (
            <>
            <button
              disabled={!pane?.worktree || isPending(queued)}
              title={pane?.worktree ? `Add to the merge queue: brings in ${pane.worktree.base}'s new commits, runs the tests, then merges. Uncommitted changes are committed.` : "Only agents in their own worktree can be merged"}
              onClick={() => enqueue([review.id])}
              className={cn(button, "border border-border text-fg-muted hover:bg-hover hover:text-fg")}
            >
              <ListPlus size={12} /> Queue
            </button>
            <button
              disabled={!pane?.worktree || isPending(queued)}
              title={pane?.worktree ? `Merge into ${pane.worktree.base}` : "Only agents in their own worktree can be merged"}
              onClick={() => pane?.worktree && void mergeReviewed(pane as Pane & { worktree: NonNullable<Pane["worktree"]> })}
              className={cn(button, "bg-accent text-on-accent hover:opacity-90")}
            >
              <GitMerge size={12} /> Merge
            </button>
            </>
          )}
          <button
            disabled={!pane?.worktree}
            title="Close the agent and delete its worktree"
            onClick={() => pane?.worktree && void discardReviewed(pane as Pane & { worktree: NonNullable<Pane["worktree"]> })}
            className={cn(button, "text-fg-muted hover:bg-hover hover:text-danger")}
          >
            <Trash2 size={12} />
          </button>
        </footer>
      ) : (
        <footer className="flex items-center gap-2 text-xs">
          <span className={cn("flex items-center gap-1", review.status === "merged" ? "text-success" : "text-fg-subtle")}>
            {review.status === "merged" ? (
              <>
                <Check size={12} /> Merged
              </>
            ) : (
              "Discarded"
            )}
          </span>
          {review.status === "merged" && pane && (
            <button onClick={() => void cleanUp()} className={cn(button, "ml-auto border border-border text-fg-muted hover:bg-hover hover:text-fg")}>
              Close agent and delete worktree
            </button>
          )}
        </footer>
      )}
    </article>
  );
}

/** ⌘E: finished agent work, ready to merge, discard or send back. */
export function ReviewInbox() {
  const { overlay, setOverlay, activeWorkspaceId, workspaces, view } = useWorkspaces();
  const all = useReviews((s) => s.reviews);
  // The overview covers every workspace; the workspace view just its own.
  const everywhere = view === "overview";
  const [, tick] = useState(0);
  const queue = useMergeQueue((s) => s.items);
  const queueRunning = useMergeQueue((s) => s.running);

  // Fresh conflict and cost numbers whenever the inbox opens, a card comes or goes, or the queue
  // merged something (the base moved).
  const openKey = all
    .filter((r) => r.status === "open")
    .map((r) => r.id)
    .join();
  const mergedCount = queue.filter((i) => i.state === "merged").length;
  useEffect(() => {
    if (overlay !== "review") return;
    const ws = useWorkspaces.getState().workspaces;
    const shown = useReviews.getState().reviews.filter((r) => everywhere || r.workspaceId === activeWorkspaceId);
    const worktrees = shown.flatMap((r) => ws.flatMap((w) => w.panes).find((p) => p.id === r.paneId)?.worktree ?? []);
    void refreshConflicts(worktrees.map((w) => w.repo));
    void refreshTaskCosts(worktrees.map((w) => w.path));
  }, [overlay, openKey, mergedCount, everywhere, activeWorkspaceId]);

  useEffect(() => {
    if (overlay !== "review") return;
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && setOverlay(null);
    window.addEventListener("keydown", onKey);
    const timer = window.setInterval(() => tick((n) => n + 1), 30_000);
    return () => {
      window.removeEventListener("keydown", onKey);
      window.clearInterval(timer);
    };
  }, [overlay, setOverlay]);

  if (overlay !== "review") return null;
  const workspace = workspaces.find((w) => w.id === activeWorkspaceId);
  const allPanes = workspaces.flatMap((w) => w.panes);
  const panes = new Map(allPanes.map((p) => [p.id, p]));
  const names = new Map(workspaces.map((w) => [w.id, w.name]));
  const reviews = all
    .filter((r) => everywhere || r.workspaceId === activeWorkspaceId)
    .sort((a, b) => Number(b.status === "open") - Number(a.status === "open") || b.finishedAt - a.finishedAt);
  const racers = (raceId: string | null) => (raceId ? allPanes.filter((p) => p.raceId === raceId) : undefined);
  // Race entries are picked, not merged one after another.
  const mergeable = reviews
    .filter((r) => r.status === "open" && !r.raceId && panes.get(r.paneId)?.worktree && !isPending(queue.find((i) => i.reviewId === r.id)))
    .sort((a, b) => a.finishedAt - b.finishedAt);
  const shownQueue = queue.filter((i) => everywhere || i.workspaceId === activeWorkspaceId);

  return (
    <div className="anim-backdrop fixed inset-0 z-40 flex justify-center bg-black/40 pt-[8vh]" onMouseDown={() => setOverlay(null)}>
      <div className="anim-dialog flex h-fit max-h-[84vh] w-[860px] flex-col rounded-xl border border-border-strong bg-panel shadow-2xl" onMouseDown={(e) => e.stopPropagation()}>
        <header className="flex items-center gap-2 border-b border-border px-5 py-3">
          <Inbox size={14} className="text-accent" />
          <h1 className="text-sm font-semibold text-fg">Review</h1>
          <span className="flex-1 truncate text-xs text-fg-subtle">{everywhere ? "All workspaces" : workspace?.name}</span>
          <span className="text-xs text-fg-subtle">Say “what's done?” to hear it · {keysFor({ type: "ui.overlay", overlay: "review" })}</span>
          {mergeable.length > 1 && (
            <button
              onClick={() => enqueue(mergeable.map((r) => r.id))}
              title="Queue every open branch, oldest first: each brings in the base's new commits, runs the tests and merges; one that conflicts or fails stops without holding up the rest"
              className={cn(button, "border border-border text-fg-muted hover:bg-hover hover:text-fg")}
            >
              {queueRunning ? <Loader2 size={12} className="animate-spin" /> : <ListOrdered size={12} />} Merge all ({mergeable.length})
            </button>
          )}
          <button onClick={() => setOverlay(null)} className="rounded p-1 text-fg-subtle hover:bg-hover hover:text-fg">
            <X size={14} />
          </button>
        </header>
        <div className="space-y-3 overflow-y-auto p-4">
          <QueuePanel items={shownQueue} />
          {reviews.length === 0 && (
            <p className="py-10 text-center text-xs text-fg-subtle">
              Nothing to review yet. When an agent finishes work, its changes, a summary, tests and a screenshot of its app show up here.
            </p>
          )}
          {reviews.map((r, i) => (
            <ReviewCard
              key={r.id}
              review={r}
              index={i + 1}
              pane={panes.get(r.paneId)}
              race={racers(r.raceId)}
              workspaceName={everywhere ? names.get(r.workspaceId) : undefined}
            />
          ))}
        </div>
      </div>
    </div>
  );
}
