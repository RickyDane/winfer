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

const { runTool } = await import("./agentApi");
const { useWorkspaces } = await import("@/stores/workspaces");
const terminal = await import("@/features/terminal/xtermRegistry");

const pane = (id: string, provider: string) => ({
  id, workspaceId: "ws", provider, title: "api", cwd: "~", status: "idle" as const, worktree: null,
  wantsWorktree: false, resume: false, unseen: false, devUrl: null, raceId: null, prompt: null, model: null, agentTitle: null,
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
