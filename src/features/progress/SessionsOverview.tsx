import { Activity, Loader2, RefreshCw, X } from "lucide-react";
import { llmEnabled } from "@/features/llm/llm";
import { useEffect, useState } from "react";
import { execute } from "@/commands/bus";
import { keysFor } from "@/commands/keymap";
import { cn } from "@/lib/cn";
import { ProviderIcon } from "@/features/providers/ProviderIcon";
import { StatusDot } from "@/features/terminal/StatusDot";
import { relativeTime } from "@/stores/activity";
import { providerName, useWorkspaces, type Pane } from "@/stores/workspaces";
import { estimate, useProgress, type Progress } from "./progress";

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

/** Mod+O: how far each running agent is with its task, across workspaces. */
export function SessionsOverview() {
  const { overlay, setOverlay, workspaces, providers } = useWorkspaces();
  const byPane = useProgress((s) => s.byPane);
  const [, tick] = useState(0);
  const [refreshing, setRefreshing] = useState(false);

  useEffect(() => {
    if (overlay !== "progress") return;
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && setOverlay(null);
    window.addEventListener("keydown", onKey);
    const timer = window.setInterval(() => tick((n) => n + 1), 15_000);
    return () => {
      window.removeEventListener("keydown", onKey);
      window.clearInterval(timer);
    };
  }, [overlay, setOverlay]);

  if (overlay !== "progress") return null;
  const groups = workspaces.map((w) => ({ workspace: w, panes: trackedSessions(w.panes, byPane) })).filter((g) => g.panes.length > 0);
  const writerOff = !llmEnabled();

  const focus = (pane: Pane) => {
    setOverlay(null);
    if (pane.workspaceId !== useWorkspaces.getState().activeWorkspaceId) useWorkspaces.getState().setActiveWorkspace(pane.workspaceId);
    execute({ type: "pane.focus", target: { id: pane.id } });
  };

  return (
    <div className="anim-backdrop fixed inset-0 z-40 flex justify-center bg-black/40 pt-[10vh]" onMouseDown={() => setOverlay(null)}>
      <div className="anim-dialog flex h-fit max-h-[80vh] w-[760px] flex-col rounded-xl border border-border-strong bg-panel shadow-2xl" onMouseDown={(e) => e.stopPropagation()}>
        <header className="flex items-center gap-2 border-b border-border px-5 py-3">
          <Activity size={14} className="text-accent" />
          <h1 className="flex-1 text-sm font-semibold text-fg">Sessions</h1>
          <span className="text-xs text-fg-subtle">Say “how far are the agents?” · {keysFor({ type: "ui.overlay", overlay: "progress" })}</span>
          <button
            title="Estimate again now"
            disabled={refreshing}
            onClick={() => {
              const working = groups.flatMap((g) => g.panes).filter((p) => p.status === "working");
              setRefreshing(true);
              void Promise.allSettled(working.map((p) => estimate(p))).finally(() => setRefreshing(false));
            }}
            className="rounded p-1 text-fg-subtle hover:bg-hover hover:text-fg"
          >
            <RefreshCw size={13} className={cn(refreshing && "animate-spin")} />
          </button>
          <button onClick={() => setOverlay(null)} className="rounded p-1 text-fg-subtle hover:bg-hover hover:text-fg">
            <X size={14} />
          </button>
        </header>
        <div className="space-y-4 overflow-y-auto p-4">
          {writerOff && <p className="text-xs text-warning">Progress estimates use the prompt writer LLM, which is off in Settings.</p>}
          {groups.length === 0 && <p className="py-8 text-center text-xs text-fg-subtle">No agent is working right now.</p>}
          {groups.map(({ workspace, panes }) => (
            <section key={workspace.id} className="space-y-1">
              <h2 className="px-1 text-xs font-medium tracking-wide text-fg-subtle uppercase">{workspace.name}</h2>
              {panes.map((pane) => {
                const progress = byPane[pane.id];
                const task = progress?.title ?? progress?.task ?? pane.prompt;
                return (
                  <button key={pane.id} onClick={() => focus(pane)} className="block w-full space-y-1.5 rounded-lg px-3 py-2 text-left hover:bg-hover">
                    <div className="flex items-center gap-2 text-xs">
                      <StatusDot status={pane.status} />
                      <ProviderIcon provider={pane.provider} size={13} />
                      <span className="shrink-0 font-medium text-fg">{providerName(providers, pane.provider)}</span>
                      <span className="min-w-0 flex-1 truncate text-fg-muted" title={progress?.task ?? undefined}>
                        {task ?? "Task unknown"}
                      </span>
                      {progress?.estimating && <Loader2 size={11} className="animate-spin text-fg-subtle" />}
                      <span className="w-10 text-right font-mono text-[12px] text-fg">{progress?.percent != null ? `${progress.percent}%` : "—"}</span>
                    </div>
                    <ProgressBar progress={progress} status={pane.status} />
                    <div className="flex items-center gap-2 text-[11px] text-fg-subtle">
                      <span className="flex-1 truncate">
                        {pane.status === "needs-input" ? "Waiting for you" : pane.status === "done" ? "Finished" : (progress?.step ?? (progress?.error ? "Couldn't estimate" : "Estimating…"))}
                      </span>
                      {progress?.updatedAt && <span>updated {relativeTime(progress.updatedAt)}</span>}
                    </div>
                  </button>
                );
              })}
            </section>
          ))}
        </div>
      </div>
    </div>
  );
}
