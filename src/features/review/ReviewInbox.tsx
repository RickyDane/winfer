import { ask } from "@tauri-apps/plugin-dialog";
import { Check, CornerDownLeft, FlaskConical, GitBranch, GitMerge, Inbox, Loader2, Trash2, Trophy, X } from "lucide-react";
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
import { discardReviewed, mergeReviewed, removeReview, runTests, setReviewStatus, useReviews, type Review } from "./reviews";

const button = "flex items-center gap-1 rounded-md px-2 py-1 text-xs disabled:opacity-40";

function Summary({ review }: { review: Review }) {
  const { summary } = review;
  if (summary.state === "loading" || summary.state === "idle")
    return (
      <p className="flex items-center gap-1.5 text-xs text-fg-subtle">
        <Loader2 size={12} className="animate-spin" /> Summarizing…
      </p>
    );
  if (summary.state === "done") return <p className="text-[13px] leading-relaxed text-fg">{summary.value}</p>;
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

function ReviewCard({ review, index, pane, race }: { review: Review; index: number; pane: Pane | undefined; race?: Pane[] }) {
  const providers = useWorkspaces((s) => s.providers);
  const [reply, setReply] = useState("");
  const name = providerName(providers, review.provider);
  const stats = review.stats;
  const open = review.status === "open";

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
    await removeWorktree(pane.worktree, true).catch(() => {});
    removeReview(review.id);
  };

  return (
    <article className={cn("space-y-2.5 rounded-lg border border-border bg-panel-raised p-3", !open && "opacity-60")}>
      <header className="flex items-center gap-2 text-xs">
        <span className="font-mono text-[11px] text-fg-subtle">{index}</span>
        {pane && <StatusDot status={pane.status} />}
        <ProviderIcon provider={review.provider} size={13} />
        <span className="font-medium text-fg">{name}</span>
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
        <span className="ml-auto text-[11px] text-fg-subtle">{relativeTime(review.finishedAt)}</span>
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
        </div>
        <Screenshot review={review} />
      </div>

      {review.summary.state === "done" && review.lastOutput && (
        <details className="text-[11px]">
          <summary className="cursor-pointer text-fg-subtle hover:text-fg">Agent's last output</summary>
          <pre className="mt-1 max-h-40 overflow-y-auto font-mono whitespace-pre-wrap text-fg-muted">{review.lastOutput}</pre>
        </details>
      )}

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
              className={cn(button, "bg-accent text-white hover:opacity-90")}
              title="Merge this one and close the other racers"
            >
              <Trophy size={12} /> Pick
            </button>
          ) : (
            <button
              disabled={!pane?.worktree}
              title={pane?.worktree ? `Merge into ${pane.worktree.base}` : "Only agents in their own worktree can be merged"}
              onClick={() => pane?.worktree && void mergeReviewed(pane as Pane & { worktree: NonNullable<Pane["worktree"]> })}
              className={cn(button, "bg-accent text-white hover:opacity-90")}
            >
              <GitMerge size={12} /> Merge
            </button>
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
  const { overlay, setOverlay, activeWorkspaceId, workspaces } = useWorkspaces();
  const all = useReviews((s) => s.reviews);
  const [, tick] = useState(0);

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
  const panes = new Map((workspace?.panes ?? []).map((p) => [p.id, p]));
  const reviews = all
    .filter((r) => r.workspaceId === activeWorkspaceId)
    .sort((a, b) => Number(b.status === "open") - Number(a.status === "open") || b.finishedAt - a.finishedAt);
  const racers = (raceId: string | null) => (raceId ? (workspace?.panes ?? []).filter((p) => p.raceId === raceId) : undefined);

  return (
    <div className="fixed inset-0 z-40 flex justify-center bg-black/40 pt-[8vh]" onMouseDown={() => setOverlay(null)}>
      <div className="flex h-fit max-h-[84vh] w-[860px] flex-col rounded-xl border border-border-strong bg-panel shadow-2xl" onMouseDown={(e) => e.stopPropagation()}>
        <header className="flex items-center gap-2 border-b border-border px-5 py-3">
          <Inbox size={14} className="text-accent" />
          <h1 className="flex-1 text-sm font-semibold text-fg">Review</h1>
          <span className="text-xs text-fg-subtle">Say “what's done?” to hear it · {keysFor({ type: "ui.overlay", overlay: "review" })}</span>
          <button onClick={() => setOverlay(null)} className="rounded p-1 text-fg-subtle hover:bg-hover hover:text-fg">
            <X size={14} />
          </button>
        </header>
        <div className="space-y-3 overflow-y-auto p-4">
          {reviews.length === 0 && (
            <p className="py-10 text-center text-xs text-fg-subtle">
              Nothing to review yet. When an agent finishes work, its changes, a summary, tests and a screenshot of its app show up here.
            </p>
          )}
          {reviews.map((r, i) => (
            <ReviewCard key={r.id} review={r} index={i + 1} pane={panes.get(r.paneId)} race={racers(r.raceId)} />
          ))}
        </div>
      </div>
    </div>
  );
}
