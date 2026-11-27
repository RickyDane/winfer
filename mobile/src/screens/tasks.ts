import { qualify } from "../multi";
import type { HomeTask, Pane, TaskStage } from "../protocol";

export const STAGE_LABEL: Record<TaskStage, string> = {
  planning: "Planning",
  running: "Running",
  checking: "Checking",
  "needs-you": "Needs you",
  ready: "Ready",
  merged: "Merged",
  failed: "Failed",
  cancelled: "Cancelled",
};

/** Still being worked on or waiting for the user. */
export const taskOpen = (t: HomeTask) => t.stage !== "merged" && t.stage !== "cancelled";

/** The agents of a task of `home`'s that this phone can open. A remote task's is on whichever other paired machine lists it. */
export function taskAgents(task: HomeTask, home: Pane, panes: Pane[]): Pane[] {
  if (task.mode !== "remote") return task.panes.flatMap((id) => panes.find((p) => p.id === id) ?? []);
  const bare = task.remotePane;
  if (!bare) return [];
  return panes.filter((p) => p.machine && p.machine !== home.machine && p.id === qualify(p.machine, bare));
}

export function taskHow(task: HomeTask, agents: number): string {
  if (task.mode === "remote") return "on another machine";
  if (task.mode === "fork") return agents > 1 ? `orchestrated · ${agents} agents` : "orchestrated";
  return agents > 1 ? `${agents} agents` : "one agent";
}

/** Why a task has nothing to open. */
export function noAgentsNote(task: HomeTask): string {
  if (task.mode === "remote") {
    const machine = task.machine ?? "another machine";
    if (!task.remotePane) return `Its orchestrator on ${machine} hasn't shown up yet.`;
    return `Its orchestrator runs on ${machine}. It opens here while that machine is paired with this phone, connected, and still running it.`;
  }
  if (!taskOpen(task)) return "Its agents have closed.";
  if (task.mode === "fork") return "No agents on it yet. Its orchestrator's chat is in winfer on your computer.";
  return "No agent is on it right now.";
}
