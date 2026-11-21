import { Activity, RefreshCw, X } from "lucide-react";
import { Spinner } from "@/components/Spinner";
import { llmEnabled } from "@/features/llm/llm";
import { useEffect, useState } from "react";
import { execute } from "@/commands/bus";
import { keysFor } from "@/commands/keymap";
import { cn } from "@/lib/cn";
import { AgentTile, PaneName } from "@/features/workspaces/Sidebar";
import { relativeTime } from "@/stores/activity";
import { useWorkspaces, useWorkspacesPick, type Pane } from "@/stores/workspaces";
import { estimate, progressTitle, useProgress, type Progress } from "./progress";

/** Thin progress bar; empty track while there's no estimate yet. */
export function ProgressBar({ progress, status, className }: { progress: Progress | undefined; status: Pane["status"]; className?: string }) {
  const percent = progress?.percent;
  const done = status === "done" || percent === 100;
  return (
    <span className={cn("block h-1 overflow-hidden rounded-full bg-hover", className)}>
      {percent != null && (
        <span
          className={cn("block h-full rounded-full transition-[width,background-color] duration-700 ease-out", done ? "bg-success" : status === "needs-input" ? "bg-warning" : "bg-accent")}
          style={{ width: `${Math.max(percent, 2)}%` }}
        />
      )}
    </span>
  );
}

/** "62% · Writing the auth guard" for tooltips. */
export function progressLabel(progress: Progress | undefined): string {
  if (!progress || progress.percent === null) return progress?.task ? "Estimating progress…" : "";
  return [`${progress.percent}%`, progress.step, progress.title && `Task: ${progress.title}`].filter(Boolean).join(" · ");
}

/** Sessions that have or had a task: working agents plus finished ones with an estimate. */
export function trackedSessions(panes: Pane[], byPane: Record<string, Progress>) {
  return panes.filter((p) => p.provider !== "shell" && (p.status === "working" || p.status === "needs-input" || byPane[p.id]?.percent != null));
}

type Filter = "all" | "working" | "needs-input" | "done";

/** Mod+O: how far each agent is with its task, across workspaces, in a panel on the right. */
export function SessionsOverview() {
  const { overlay, setOverlay, workspaces } = useWorkspacesPick("overlay", "setOverlay", "workspaces");
  const byPane = useProgress((s) => s.byPane);
  const [, tick] = useState(0);
  const [refreshing, setRefreshing] = useState(false);
  const [filter, setFilter] = useState<Filter>("all");

  useEffect(() => {
    if (overlay !== "progress") return;
    const timer = window.setInterval(() => tick((n) => n + 1), 15_000);
    return () => window.clearInterval(timer);
  }, [overlay]);

  if (overlay !== "progress") return null;
  const all = workspaces.flatMap((w) => trackedSessions(w.panes.filter((p) => !p.archived), byPane).concat(w.panes.filter((p) => !p.archived && p.provider !== "shell" && p.status === "done" && byPane[p.id]?.percent == null)));
  const count = (status: Pane["status"]) => all.filter((p) => p.status === status).length;
  const [working, waiting, done] = [count("working"), count("needs-input"), count("done")];
  const known = all.filter((p) => p.status === "working").map((p) => byPane[p.id]?.percent).filter((n): n is number => n != null);
  const average = known.length ? Math.round(known.reduce((a, b) => a + b, 0) / known.length) : null;
  const rank = (p: Pane) => (p.status === "needs-input" ? 0 : p.status === "working" ? 1 : 2);
  const shown = all.filter((p) => filter === "all" || p.status === filter).sort((a, b) => rank(a) - rank(b) || (byPane[b.id]?.percent ?? 0) - (byPane[a.id]?.percent ?? 0));
  const writerOff = !llmEnabled();
  const workspaceName = (id: string) => workspaces.find((w) => w.id === id)?.name ?? "";

  const focus = (pane: Pane) => {
    setOverlay(null);
    if (pane.workspaceId !== useWorkspaces.getState().activeWorkspaceId) useWorkspaces.getState().setActiveWorkspace(pane.workspaceId);
    execute({ type: "pane.focus", target: { id: pane.id } });
  };
  const stat = (label: string, value: string, tone?: string) => (
    <div className="flex-1 rounded-lg bg-panel p-2.5 shadow-sm">
      <div className={cn("text-lg font-semibold text-fg", tone)}>{value}</div>
      <div className="text-[11px] text-fg-subtle">{label}</div>
    </div>
  );
  const tabs: [Filter, string, number][] = [["all", "All", all.length], ["working", "Working", working], ["needs-input", "Needs you", waiting], ["done", "Done", done]];

  return (
    <div className="anim-backdrop fixed inset-0 z-40 flex justify-end bg-black/25 p-2" onMouseDown={() => setOverlay(null)}>
      <div className="anim-slide flex h-full w-[440px] max-w-[90vw] flex-col overflow-hidden rounded-xl border border-border-strong bg-panel-raised shadow-2xl" onMouseDown={(e) => e.stopPropagation()}>
        <header className="space-y-3 px-4 pt-4 pb-3">
          <div className="flex items-center gap-2">
            <Activity size={15} className="text-accent" />
            <h1 className="flex-1 text-[15px] font-semibold text-fg">Sessions</h1>
            <kbd className="rounded px-1 font-mono text-[10.5px] text-fg-subtle shadow-sm">{keysFor({ type: "ui.overlay", overlay: "progress" })}</kbd>
            <button
              title="Estimate again now"
              disabled={refreshing}
              onClick={() => {
                setRefreshing(true);
                void Promise.allSettled(all.filter((p) => p.status === "working").map((p) => estimate(p))).finally(() => setRefreshing(false));
              }}
              className="rounded p-1 text-fg-subtle hover:bg-hover hover:text-fg"
            >
              <RefreshCw size={13} className={cn(refreshing && "animate-spin")} />
            </button>
            <button onClick={() => setOverlay(null)} aria-label="Close" className="rounded p-1 text-fg-subtle hover:bg-hover hover:text-fg">
              <X size={14} />
            </button>
          </div>
          <div className="flex gap-2">
            {stat("working", String(working))}
            {stat("need you", String(waiting), waiting > 0 ? "text-warning" : undefined)}
            {stat("done", String(done), done > 0 ? "text-success" : undefined)}
            {stat("avg progress", average != null ? `${average}%` : "—")}
          </div>
          <div role="tablist" className="flex gap-0.5 rounded-lg bg-app p-0.5 shadow-sm">
            {tabs.map(([id, label, n]) => (
              <button key={id} role="tab" aria-selected={filter === id} onClick={() => setFilter(id)} className={cn("h-[26px] flex-1 rounded-md text-xs", filter === id ? "bg-active text-fg" : "text-fg-muted hover:text-fg")}>
                {label} {n}
              </button>
            ))}
          </div>
        </header>
        <div className="flex-1 space-y-1 overflow-y-auto p-2">
          {writerOff && <p className="px-1 pb-2 text-xs text-warning">Progress estimates use the prompt writer LLM, which is off in Settings.</p>}
          {all.length === 0 && (
            <div className="flex flex-col items-center gap-1 py-10 text-fg-subtle">
              <Activity size={20} />
              <div className="text-[13px] text-fg">No agent is working right now</div>
              <div className="text-xs">Started agents show up here with how far they are.</div>
            </div>
          )}
          {shown.map((pane) => {
            const progress = byPane[pane.id];
            const task = progressTitle(progress) ?? pane.prompt;
            const waitingHere = pane.status === "needs-input";
            const detail = waitingHere ? (progress?.step ?? "Waiting for you") : pane.status === "done" ? "Finished — ready to review" : (progress?.step ?? (progress?.error ? "Couldn't estimate" : "Estimating…"));
            return (
              <div
                key={pane.id}
                role="button"
                tabIndex={0}
                onClick={() => focus(pane)}
                className={cn("flex cursor-pointer gap-3 rounded-lg p-2.5 hover:bg-hover", waitingHere && "bg-warning/[0.06]")}
              >
                <AgentTile pane={pane} ring="var(--color-panel-raised)" />
                <div className="min-w-0 flex-1 space-y-1">
                  <div className="flex items-center gap-2">
                    <PaneName pane={pane} className="flex-1 text-[13px] font-medium text-fg" />
                    {progress?.estimating && <Spinner size={11} className="text-fg-subtle" />}
                    <span className="font-mono text-[11px] text-fg-muted">{progress?.percent != null ? `${progress.percent}%` : "—"}</span>
                  </div>
                  {task && <div className="truncate text-[11.5px] text-fg-subtle">{workspaceName(pane.workspaceId)} · {task}</div>}
                  <ProgressBar progress={progress} status={pane.status} className="h-[3px]" />
                  <div className="flex items-center gap-2 text-[11.5px]">
                    <span className={cn("min-w-0 flex-1 truncate", waitingHere ? "text-warning" : "text-fg-muted")}>{detail}</span>
                    {progress?.updatedAt && <span className="shrink-0 text-fg-subtle">{relativeTime(progress.updatedAt)}</span>}
                    {waitingHere && (
                      <button
                        onClick={(e) => {
                          e.stopPropagation();
                          execute({ type: "pane.approve", target: { id: pane.id } });
                        }}
                        className="h-6 shrink-0 rounded-md bg-accent px-2.5 text-[11.5px] font-medium text-on-accent hover:opacity-90"
                      >
                        Approve
                      </button>
                    )}
                    {pane.status === "done" && (
                      <button
                        onClick={(e) => {
                          e.stopPropagation();
                          execute({ type: "ui.overlay", overlay: "review" });
                        }}
                        className="h-6 shrink-0 rounded-md px-2.5 text-[11.5px] text-fg hover:bg-hover shadow-sm"
                      >
                        Review
                      </button>
                    )}
                  </div>
                </div>
              </div>
            );
          })}
        </div>
      </div>
    </div>
  );
}
