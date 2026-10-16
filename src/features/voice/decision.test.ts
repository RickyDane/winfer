import { describe, expect, it, vi } from "vitest";
import type { ToolContext } from "@/tools/registry";
import type { Provider } from "@/features/providers/providers";
import type { Workspace } from "@/stores/workspaces";
import { orderByMention } from "./decision";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@/features/terminal/xtermRegistry", () => ({ readTerminal: vi.fn(() => "") }));
const { parsePlan } = await import("./planner");

const provider = (id: string, name: string, aliases: string[] = []): Provider => ({
  id, name, command: id, args: [], resumeArgs: [], aliases, statusHooks: false, promptArg: false, approveInput: null, modelArg: id === "shell" ? null : "--model", modelCommand: null, installed: true,
});

const pane = (id: string, provider: string) => ({
  id, workspaceId: "ws", provider, title: "api", cwd: "~", status: "idle" as const, worktree: null,
  wantsWorktree: false, resume: false, sessionId: null, unseen: false, devUrl: null, raceId: null, plain: false, prompt: null, model: null, agentTitle: null, customTitle: null,
});

const workspace: Workspace = {
  id: "ws", name: "bridgemind", rootDir: "~", zoomedPaneId: null, presetIndex: 0, useWorktrees: true,
  panes: [pane("p-a", "claude"), pane("p-b", "codex")],
  layout: { kind: "split", dir: "row", sizes: [0.5, 0.5], children: [{ kind: "leaf", paneId: "p-a" }, { kind: "leaf", paneId: "p-b" }] },
};

const ctx: ToolContext = {
  providers: [provider("claude", "Claude Code", ["cloud code"]), provider("codex", "Codex", ["codecs"]), provider("grok", "Grok Build"), provider("shell", "Shell")],
  workspaces: [workspace],
  active: workspace,
  focusedPaneId: "p-a",
  browserTabs: [],
  lastCommands: [],
  models: { claude: [{ id: "opus", label: "Opus (latest)" }, { id: "sonnet", label: "Sonnet (latest)" }] },
  recentAddresses: { "file:///Users/r/Coding/index.html": "/Users/r/Coding/index.html — mentioned in pane 1 (Claude Code)" },
};

/** The LLM planner's answer for these tool calls, parsed like a real one. */
const plan = (...actions: [string, Record<string, unknown>][]) => parsePlan(JSON.stringify({ actions: actions.map(([tool, args]) => ({ tool, args })) }), ctx)!;
const commandsOf = (...actions: [string, Record<string, unknown>][]) => plan(...actions).map((d) => d.tool.toCommands(d.args, ctx));

describe("planned tool calls", () => {
  it("spawns agents in the order they were said", () => {
    const [d] = plan(["spawn_panes", { counts: { claude: 1, codex: 1, grok: 1 } }]);
    const commands = orderByMention(d.tool.toCommands(d.args, ctx), "open grok, then claude and codex", ctx);
    expect(commands.map((c) => c.type === "pane.spawn" && c.provider)).toEqual(["grok", "claude", "codex"]);
  });

  it("resolves pane targets and layout presets", () => {
    expect(commandsOf(["close_pane", { target: "p-b" }], ["set_layout", { preset: "grid" }])).toEqual([
      [{ type: "pane.close", target: { id: "p-b" } }],
      [{ type: "layout.preset", name: "grid" }],
    ]);
  });

  it("opens more than four, and the default agent when none is named", () => {
    expect(commandsOf(["spawn_panes", { counts: { codex: 6 } }])[0]).toMatchObject([{ type: "pane.spawn", provider: "codex", count: 6 }]);
    expect(commandsOf(["spawn_panes", { total: 5 }])[0]).toMatchObject([{ type: "pane.spawn", provider: "default", count: 5 }]);
  });

  it("gives a new agent its prompt", () => {
    expect(commandsOf(["spawn_panes", { counts: { codex: 1 }, prompt: "Add dark mode to the settings page." }])[0]).toEqual([
      { type: "pane.spawn", provider: "codex", count: 1, prompt: "Add dark mode to the settings page." },
    ]);
  });

  it("sends text to an open pane", () => {
    expect(commandsOf(["send_to_pane", { target: "p-a", text: "Run the tests.", submit: true }])[0]).toEqual([
      { type: "pane.sendText", target: { id: "p-a" }, text: "Run the tests.", submit: true },
    ]);
  });
});

describe("models", () => {
  it("opens an agent with a spoken model only if that agent offers it", () => {
    const [commands] = commandsOf(["spawn_panes", { counts: { claude: 1, codex: 1 }, model: "opus" }]);
    // Codex can't list models here, so it gets the name as-is; Claude offers "opus".
    expect(commands.map((c) => c.type === "pane.spawn" && [c.provider, c.model])).toEqual([
      ["claude", "opus"],
      ["codex", "opus"],
    ]);
  });

  it("changes the model of every pane that offers it", () => {
    // p-a is Claude (offers opus); p-b is Codex, which can't list models, so it gets the name as-is.
    expect(commandsOf(["change_model", { target: "all", model: "opus" }])[0]).toEqual([
      { type: "pane.setModel", target: { id: "p-a" }, model: "opus" },
      { type: "pane.setModel", target: { id: "p-b" }, model: "opus" },
    ]);
  });

  it("switches the model of a pane", () => {
    expect(commandsOf(["change_model", { target: "p-a", model: "sonnet" }])[0]).toEqual([{ type: "pane.setModel", target: { id: "p-a" }, model: "sonnet" }]);
  });
});

describe("open_url", () => {
  it("opens a page an agent mentioned", () => {
    expect(commandsOf(["open_url", { recent: "file:///Users/r/Coding/index.html" }])[0]).toEqual([
      { type: "browser.open", url: "file:///Users/r/Coding/index.html", newTab: false },
    ]);
  });

  it("prefers a spoken address", () => {
    expect(commandsOf(["open_url", { recent: "none", url: "localhost 3000" }])[0]).toEqual([{ type: "browser.open", url: "localhost 3000", newTab: false }]);
  });
});
