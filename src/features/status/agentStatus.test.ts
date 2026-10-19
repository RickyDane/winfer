import { beforeEach, describe, expect, it, vi } from "vitest";

const h = vi.hoisted(() => ({ outputAge: 0 }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => () => {}) }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(async () => null) }));
vi.mock("@tauri-apps/api/window", () => ({ getCurrentWindow: () => ({}) }));
vi.mock("@tauri-apps/plugin-notification", () => ({ isPermissionGranted: vi.fn(), requestPermission: vi.fn(), sendNotification: vi.fn() }));
vi.mock("@/features/terminal/xtermRegistry", () => ({ outputAge: () => h.outputAge, readTerminal: () => "" }));
vi.mock("@/commands/bus", () => ({ execute: vi.fn() }));
vi.mock("@/stores/activity", () => ({ logActivity: vi.fn() }));
vi.mock("@/features/voice/readback", () => ({ speak: vi.fn() }));
vi.mock("@/features/review/reviews", () => ({ agentFinished: vi.fn() }));
vi.mock("@/features/progress/progress", () => ({ estimateNow: vi.fn(), markFinished: vi.fn() }));
vi.mock("@/features/mobile/mobile", () => ({ noteStatusMessage: vi.fn() }));

import { useWorkspaces, type AgentStatus, type Pane } from "@/stores/workspaces";
import { reportInput, reportStatus, settleGuesses } from "./agentStatus";

function setPane(status: AgentStatus, provider = "winfer") {
  useWorkspaces.setState({
    providers: [{ id: "winfer", statusHooks: true }, { id: "aider", statusHooks: false }] as never,
    workspaces: [{ id: "w1", name: "w", rootDir: "/", panes: [{ id: "p1", workspaceId: "w1", provider, status } as Pane], layout: null, zoomedPaneId: null, presetIndex: 0, useWorktrees: false }] as never,
  });
}
const status = () => useWorkspaces.getState().workspaces[0].panes[0].status;

describe("input-guessed working", () => {
  beforeEach(() => {
    h.outputAge = 0;
  });

  it("goes back to idle when no hook confirms it and the screen stays quiet (/clear, /undo)", () => {
    setPane("done");
    const t0 = Date.now();
    reportInput("p1");
    expect(status()).toBe("working");
    settleGuesses(t0 + 2000);
    expect(status()).toBe("working");
    h.outputAge = 5000;
    settleGuesses(t0 + 5000);
    expect(status()).toBe("idle");
  });

  it("stays working while the agent's screen keeps changing", () => {
    setPane("idle");
    const t0 = Date.now();
    reportInput("p1");
    h.outputAge = 300;
    settleGuesses(t0 + 10_000);
    expect(status()).toBe("working");
  });

  it("trusts the agent once a hook reports", () => {
    setPane("idle");
    const t0 = Date.now();
    reportInput("p1");
    reportStatus("p1", "idle"); // the hook: the agent is back at its prompt
    reportStatus("p1", "working"); // a later turn the hook started
    h.outputAge = 9000;
    settleGuesses(t0 + 10_000);
    expect(status()).toBe("working");
  });

  it("leaves agents without hooks to the quiet-output rule", () => {
    setPane("idle", "aider");
    const t0 = Date.now();
    reportInput("p1");
    h.outputAge = 9000;
    settleGuesses(t0 + 10_000);
    expect(status()).toBe("working");
  });
});
