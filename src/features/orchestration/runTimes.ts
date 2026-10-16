/**
 * How long each race and A/B test agent has worked, from its status changes. Kept in memory
 * only: after a restart the usage logs' first and last model call stand in (see RaceResults).
 */
import { create } from "zustand";
import { providerName, useWorkspaces, type AgentStatus, type Pane } from "@/stores/workspaces";
import { logActivity } from "@/stores/activity";
import { toast } from "@/stores/toasts";
import { openRaceResults } from "./resultsStore";

export interface RunTime {
  /** First time the agent started working (unix ms). */
  startedAt: number;
  /** Last time it stopped working; null while it hasn't yet. */
  finishedAt: number | null;
  /** Time spent working in finished stretches. */
  workMs: number;
  /** Start of the current working stretch, or null when not working. */
  since: number | null;
}

export const useRunTimes = create<{ byPane: Record<string, RunTime> }>(() => ({ byPane: {} }));

/** Time spent working so far, counting the current stretch. */
export const activeMs = (t: RunTime, now = Date.now()) => t.workMs + (t.since !== null ? now - t.since : 0);

/** Applies a status change to a pane's timing. */
export function nextRunTime(t: RunTime | undefined, was: AgentStatus | undefined, status: AgentStatus, now: number): RunTime | undefined {
  if (status === "working") {
    if (!t) return { startedAt: now, finishedAt: null, workMs: 0, since: now };
    return t.since !== null ? t : { ...t, since: now };
  }
  if (was !== "working" || !t || t.since === null) return t;
  return { ...t, workMs: t.workMs + (now - t.since), since: null, finishedAt: now };
}

/** Done with its task: hook agents say so; the others go quiet (Claude also goes idle at start). */
export function isFinished(pane: Pane): boolean {
  if (pane.status === "done" || pane.status === "error") return true;
  const hooks = useWorkspaces.getState().providers.find((p) => p.id === pane.provider)?.statusHooks ?? false;
  return pane.status === "idle" && !hooks && !!useRunTimes.getState().byPane[pane.id];
}

let started = false;

export function startRunTimes() {
  if (started) return;
  started = true;
  useWorkspaces.subscribe((s, prev) => {
    if (s.workspaces === prev.workspaces) return;
    const before = new Map(prev.workspaces.flatMap((w) => w.panes.map((p) => [p.id, p] as const)));
    const now = Date.now();
    let byPane = useRunTimes.getState().byPane;
    const done = new Set<string>();
    for (const pane of s.workspaces.flatMap((w) => w.panes)) {
      const old = before.get(pane.id);
      if (!pane.raceId || old?.status === pane.status) continue;
      const next = nextRunTime(byPane[pane.id], old?.status, pane.status, now);
      if (next !== byPane[pane.id]) byPane = { ...byPane, [pane.id]: next! };
      if (old?.status === "working") done.add(pane.raceId);
    }
    if (byPane !== useRunTimes.getState().byPane) useRunTimes.setState({ byPane });
    // Tell the user once every agent of an A/B test has stopped.
    for (const raceId of done) {
      const panes = s.workspaces.flatMap((w) => w.panes).filter((p) => p.raceId === raceId);
      if (!raceId.startsWith("ab-") || !panes.every(isFinished)) continue;
      const name = providerName(s.providers, panes[0].provider);
      logActivity(panes[0].workspaceId, `A/B test with ${name} finished`, panes[0].provider);
      toast(`A/B test with ${name} finished`, "success", undefined, {
        label: "Results",
        run: () => openRaceResults(raceId),
      });
    }
  });
}
