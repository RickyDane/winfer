import { describe, expect, it } from "vitest";
import type { HomeTask, Pane } from "../protocol";
import { noAgentsNote, taskAgents, taskHow, taskOpen } from "./tasks";

const pane = (id: string, machine?: string): Pane => ({ id, machine, title: id, workspace: "repo", provider: null, status: "idle", message: null, updatedAt: 0, request: null, reply: null });
const task = (more: Partial<HomeTask>): HomeTask => ({ id: "t", number: 1, goal: "g", stage: "running", workspace: "repo", summary: "", mode: "light", panes: [], machine: null, remotePane: null, updatedAt: 0, ...more });

describe("taskAgents", () => {
  it("lists a task's own panes in its order, without those that are gone", () => {
    const panes = [pane("p1"), pane("p2")];
    expect(taskAgents(task({ panes: ["p2", "gone", "p1"] }), pane("home"), panes).map((p) => p.id)).toEqual(["p2", "p1"]);
  });

  it("finds a remote task's orchestrator on another paired machine only", () => {
    const home = pane("A~home", "A");
    const panes = [pane("A~r", "A"), pane("B~r", "B"), pane("B~xr", "B")];
    expect(taskAgents(task({ mode: "remote", remotePane: "r" }), home, panes).map((p) => p.id)).toEqual(["B~r"]);
    expect(taskAgents(task({ mode: "remote" }), home, panes)).toEqual([]);
    // Paired with the desktop alone: its own pane of that id isn't the remote one.
    expect(taskAgents(task({ mode: "remote", remotePane: "r" }), pane("home"), [pane("r")])).toEqual([]);
  });
});

describe("task text", () => {
  it("counts merged and cancelled tasks as done", () => {
    expect(["failed", "ready", "merged", "cancelled"].map((stage) => taskOpen(task({ stage: stage as HomeTask["stage"] })))).toEqual([true, true, false, false]);
  });

  it("says how a task runs and why there's nothing to open", () => {
    expect(taskHow(task({}), 1)).toBe("one agent");
    expect(taskHow(task({ mode: "fork" }), 3)).toBe("orchestrated · 3 agents");
    expect(noAgentsNote(task({ mode: "remote", machine: "mini" }))).toContain("on mini hasn't shown up");
    expect(noAgentsNote(task({ mode: "remote", machine: "mini", remotePane: "r" }))).toContain("runs on mini");
    expect(noAgentsNote(task({ mode: "fork" }))).toContain("on your computer");
    expect(noAgentsNote(task({ stage: "merged" }))).toBe("Its agents have closed.");
  });
});
