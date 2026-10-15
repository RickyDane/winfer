import { describe, expect, it, vi } from "vitest";
import type { ToolContext } from "@/tools/registry";
import type { Provider } from "@/features/providers/providers";
import type { Workspace } from "@/stores/workspaces";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@/features/terminal/xtermRegistry", () => ({ readTerminal: vi.fn(() => "") }));

const { parsePlan, plannerPrompt } = await import("./planner");

const provider = (id: string, name: string, aliases: string[] = []): Provider => ({
  id, name, command: id, args: [], resumeArgs: [], aliases, statusHooks: false, promptArg: false, approveInput: null, modelArg: null, modelCommand: null, installed: true,
});

const pane = (id: string, provider: string) => ({
  id, workspaceId: "ws", provider, title: "api", cwd: "~", status: "idle" as const, worktree: null,
  wantsWorktree: false, resume: false, sessionId: null, unseen: false, devUrl: null, raceId: null, prompt: null, model: null, agentTitle: null, customTitle: null,
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

describe("parsePlan", () => {
  it("keeps only known tools and allowed values", () => {
    const plan = JSON.stringify({
      actions: [
        { tool: "spawn_panes", args: { counts: { claude: 1, nope: 3 }, prompt: " Build a plugin system. ", worktree: true, model: "gpt-9" } },
        { tool: "rm_rf", args: {} },
        { tool: "send_to_pane", args: { target: "p-b", text: "Run the tests.", submit: true } },
        { tool: "send_to_pane", args: { target: "p-zzz", text: "Hi" } },
        { tool: "none", args: {} },
      ],
    });
    const decisions = parsePlan(`Sure:\n${plan}`, ctx)!;
    expect(decisions.map((d) => d.tool.id)).toEqual(["spawn_panes", "send_to_pane", "send_to_pane"]);
    expect(decisions[0].args).toMatchObject({ counts: { claude: 1, codex: 0 }, prompt: "Build a plugin system.", worktree: true });
    expect(decisions[0].args.model).toBe("default"); // unknown model → the only allowed value
    expect(decisions[0].args.counts).not.toHaveProperty("nope");
    expect(decisions[1].args).toMatchObject({ target: "p-b", text: "Run the tests.", submit: true });
    expect(decisions[2].args.target).toBeUndefined();
  });

  it("clamps counts and rejects answers that aren't a plan", () => {
    const [d] = parsePlan('{"actions":[{"tool":"spawn_panes","args":{"counts":{"codex":40}}}]}', ctx)!;
    expect((d.args.counts as Record<string, number>).codex).toBe(9);
    expect(parsePlan("I can't help with that", ctx)).toBeNull();
    expect(parsePlan('{"actions": "spawn"}', ctx)).toBeNull();
    expect(parsePlan('{"actions": []}', ctx)).toEqual([]);
  });
});

describe("plannerPrompt", () => {
  it("lists the panes, the tools with their values and the command", () => {
    const prompt = plannerPrompt("we need two features", ctx, [], "error: build failed");
    expect(prompt).toContain('pane 1 (id "p-a"): Claude Code, idle, focused');
    expect(prompt).toContain("- spawn_panes:");
    expect(prompt).toContain('target: one of "focused" (The pane that currently has focus)');
    expect(prompt).toContain("worktree: true or false");
    expect(prompt).not.toContain("- none:");
    expect(prompt).toContain("error: build failed");
    expect(prompt.endsWith('Voice command: "we need two features"')).toBe(true);
  });
});
