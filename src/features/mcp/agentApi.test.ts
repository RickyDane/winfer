import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(async () => null) }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => () => {}) }));
vi.mock("@/features/browser/browser", () => ({ openUrl: vi.fn(async () => {}) }));
vi.mock("@/features/status/agentStatus", () => ({ reportStatus: vi.fn() }));
vi.mock("@/features/terminal/xtermRegistry", () => ({
  pasteText: vi.fn(),
  sendText: vi.fn(),
  startDetached: vi.fn(),
  readTerminal: vi.fn((id: string) => (id === "p-2" ? "$ npm test\n12 passing" : null)),
}));

const storage = new Map<string, string>();
vi.stubGlobal("localStorage", { getItem: (k: string) => storage.get(k) ?? null, setItem: (k: string, v: string) => storage.set(k, v) });
vi.mock("@tauri-apps/plugin-notification", () => ({ isPermissionGranted: vi.fn(async () => false), sendNotification: vi.fn() }));

const { runTool } = await import("./agentApi");
const { useWorkspaces } = await import("@/stores/workspaces");
const terminal = await import("@/features/terminal/xtermRegistry");

const pane = (id: string, provider: string) => ({
  id, workspaceId: "ws", provider, title: "api", cwd: "~", status: "idle" as const, worktree: null,
  wantsWorktree: false, resume: false, sessionId: null, unseen: false, devUrl: null, raceId: null, plain: false, prompt: null, model: null, agentTitle: null, customTitle: null, archived: false,
});

beforeEach(() => {
  vi.clearAllMocks();
  useWorkspaces.setState({
    activeWorkspaceId: "ws",
    focusedPaneId: "p-1",
    providers: [
      { id: "claude", name: "Claude Code", command: "claude", args: [], resumeArgs: [], aliases: [], statusHooks: true, promptArg: true, approveInput: "\r", modelArg: null, modelCommand: null, installed: true },
      { id: "codex", name: "Codex", command: "codex", args: [], resumeArgs: [], aliases: [], statusHooks: true, promptArg: true, approveInput: "y", modelArg: null, modelCommand: null, installed: true },
    ],
    workspaces: [
      {
        id: "ws", name: "api", rootDir: "~/api", zoomedPaneId: null, presetIndex: 0, useWorktrees: true,
        panes: [pane("p-1", "claude"), pane("p-2", "codex")],
        layout: { kind: "split", dir: "row", sizes: [0.5, 0.5], children: [{ kind: "leaf", paneId: "p-1" }, { kind: "leaf", paneId: "p-2" }] },
      },
    ],
  });
});

const call = (tool: string, args: Record<string, unknown> = {}) => runTool({ id: "1", tool, args, paneId: "p-1" });

describe("agent MCP tools", () => {
  it("lists panes and marks the caller", async () => {
    expect(await call("list_panes")).toBe("Workspace “api” (~/api):\n1. Claude Code · idle · (you)\n2. Codex · idle");
  });

  it("reads another pane by number", async () => {
    expect(await call("read_pane", { pane: 2 })).toBe("$ npm test\n12 passing");
    await expect(call("read_pane", { pane: 7 })).rejects.toThrow("No pane 7");
  });

  it("sends messages but not to itself", async () => {
    await call("send_to_pane", { pane: "2", text: "run the tests" });
    expect(terminal.sendText).toHaveBeenCalledWith("p-2", "run the tests");
    expect(terminal.sendText).toHaveBeenCalledWith("p-2", "\r");
    await call("send_to_pane", { pane: 2, text: "line 1\nline 2", submit: false });
    expect(terminal.pasteText).toHaveBeenCalledWith("p-2", "line 1\nline 2");
    await expect(call("send_to_pane", { pane: 1, text: "hi" })).rejects.toThrow("your own pane");
  });

  it("spawns installed agents next to the caller", async () => {
    expect(await call("spawn_agent", { provider: "codex", prompt: "write tests" })).toBe("Started Codex as pane 2.");
    const ws = useWorkspaces.getState().workspaces[0];
    const spawned = ws.panes.at(-1)!;
    expect(spawned).toMatchObject({ provider: "codex", prompt: "write tests", wantsWorktree: true });
    await expect(call("spawn_agent", { provider: "grok" })).rejects.toThrow("Available: claude, codex");
  });
});

describe("usage tools", async () => {
  const { useUsage } = await import("@/features/usage/usage");
  const { deliverWakes, pendingWake } = await import("@/features/usage/agentUsage");
  const now = Date.now();

  beforeEach(() => {
    useUsage.setState({
      samples: {},
      snapshots: {
        claude: {
          provider: "claude",
          at: now,
          windows: [
            { kind: "session", label: "5-hour", usedPercent: 92, resetsAt: now + 90 * 60_000 },
            { kind: "week", label: "Weekly", usedPercent: 40, resetsAt: now + 3 * 86_400_000 },
          ],
        },
      },
    });
  });

  it("reports what is left of each window", async () => {
    const text = await call("usage_status");
    expect(text).toContain("5-hour: 92% used, 8% left, resets");
    expect(text).toContain("(in 1h 30m)");
    expect(text).toContain("Weekly: 40% used, 60% left");
    expect(text).toContain("Least left: 5-hour, 8%.");
    expect(await runTool({ id: "1", tool: "usage_status", args: {}, paneId: "p-2" })).toContain("no usage numbers for Codex yet");
  });

  it("wakes the agent after the low window resets", async () => {
    expect(await call("wait_for_usage_reset", { message: "Continue building xyz" })).toContain("when the 5-hour limit resets");
    const wake = pendingWake("p-1")!;
    expect(wake.at).toBe(now + 91 * 60_000);
    deliverWakes(wake.at - 1000);
    expect(terminal.sendText).not.toHaveBeenCalled();
    deliverWakes(wake.at);
    expect(terminal.sendText).toHaveBeenCalledWith("p-1", "Continue building xyz");
    expect(terminal.sendText).toHaveBeenCalledWith("p-1", "\r");
    expect(pendingWake("p-1")).toBeNull();
  });

  it("gives OpenCode the limits of the plan its model is signed in to", async () => {
    const { invoke } = await import("@tauri-apps/api/core");
    const ws = useWorkspaces.getState().workspaces[0];
    useWorkspaces.setState({ workspaces: [{ ...ws, panes: [...ws.panes, pane("p-3", "opencode")] }] });
    useUsage.setState((s) => ({ snapshots: { ...s.snapshots, codex: { ...s.snapshots.claude, provider: "codex" } } }));
    const asOpencode = () => runTool({ id: "1", tool: "usage_status", args: {}, paneId: "p-3" });
    vi.mocked(invoke).mockImplementation(async (cmd) => (cmd === "usage_opencode_model" ? { provider: "openai", model: "gpt-5.5", plan: true } : null));
    const text = await asOpencode();
    expect(text).toContain("through your ChatGPT sign-in, so Codex's plan limits apply");
    expect(text).toContain("5-hour: 92% used");
    vi.mocked(invoke).mockImplementation(async (cmd) => (cmd === "usage_opencode_model" ? { provider: "openai", model: "gpt-5.5", plan: false } : null));
    expect(await asOpencode()).toContain("with an API key");
    vi.mocked(invoke).mockImplementation(async () => null);
  });

  it("can wait a fixed time, or be cancelled", async () => {
    await call("wait_for_usage_reset", { minutes: 10 });
    expect(pendingWake("p-1")!.at).toBeGreaterThanOrEqual(now + 10 * 60_000);
    expect(await call("wait_for_usage_reset", { cancel: true })).toContain("Cancelled");
    expect(pendingWake("p-1")).toBeNull();
    await expect(runTool({ id: "1", tool: "wait_for_usage_reset", args: {}, paneId: "p-2" })).rejects.toThrow("doesn't know when");
  });
});

describe("open_file", () => {
  it("shows the file in the side panel's viewer", async () => {
    const { invoke } = await import("@tauri-apps/api/core");
    vi.mocked(invoke).mockImplementation(async (cmd, args) =>
      cmd === "files_resolve" ? (args as { paths: string[] }).paths.map((p) => (p === "docs/PLAN.md" ? "/home/api/docs/PLAN.md" : null)) : null,
    );
    useWorkspaces.setState({ view: "workspace", sidePanel: "browser", viewer: { docs: [], active: null } });
    expect(await call("open_file", { path: "docs/PLAN.md", line: 12 })).toContain("Showing /home/api/docs/PLAN.md");
    expect(invoke).toHaveBeenCalledWith("files_resolve", { cwd: "~", paths: ["docs/PLAN.md"] });
    const s = useWorkspaces.getState();
    expect(s.viewer).toEqual({ docs: ["/home/api/docs/PLAN.md"], active: "/home/api/docs/PLAN.md" });
    expect(s.sidePanel).toBe("viewer");
    await expect(call("open_file", { path: "nope.md" })).rejects.toThrow("No file at nope.md");
  });
});

describe("bg exit notices", async () => {
  const { deliverBgNotes, pendingBgNotes, queueBgNote } = await import("./bgNotices");

  it("types notes into the pane once its agent is idle", () => {
    useWorkspaces.getState().updatePane("p-1", { status: "working" });
    queueBgNote("p-1", "[winfer] Background task bg1 (`npm run dev`) exited with code 1 after 4s.");
    queueBgNote("gone", "dropped");
    deliverBgNotes();
    expect(terminal.sendText).not.toHaveBeenCalled();
    expect(pendingBgNotes("p-1")).toHaveLength(1);
    expect(pendingBgNotes("gone")).toHaveLength(0);
    useWorkspaces.getState().updatePane("p-1", { status: "done" });
    deliverBgNotes();
    expect(terminal.sendText).toHaveBeenCalledWith("p-1", "[winfer] Background task bg1 (`npm run dev`) exited with code 1 after 4s.");
    expect(terminal.sendText).toHaveBeenCalledWith("p-1", "\r");
    expect(pendingBgNotes("p-1")).toHaveLength(0);
  });

  it("pastes several notes as one message", () => {
    queueBgNote("p-2", "a");
    queueBgNote("p-2", "b");
    deliverBgNotes();
    expect(terminal.pasteText).toHaveBeenCalledWith("p-2", "a\n\nb");
  });
});
