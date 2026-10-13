import { describe, expect, it } from "vitest";
import type { ToolContext } from "@/tools/registry";
import type { Provider } from "@/features/providers/providers";
import type { Workspace } from "@/stores/workspaces";
import { buildRequest, orderByMention, parseDecision, transcriptSpans, verdict, type DecisionResult } from "./decision";

const provider = (id: string, name: string, aliases: string[] = []): Provider => ({
  id, name, command: id, args: [], resumeArgs: [], aliases, statusHooks: false, installed: true,
});

const pane = (id: string, provider: string) => ({
  id, workspaceId: "ws", provider, title: "api", cwd: "~", status: "idle" as const, worktree: null,
  wantsWorktree: false, resume: false, unseen: false, devUrl: null,
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
};

describe("buildRequest", () => {
  const req = buildRequest("open three new instances with claude, codex and grok", ctx);

  it("stays within System One limits", () => {
    expect(Object.keys(req.questions).length).toBeLessThanOrEqual(64);
    for (const [key, q] of Object.entries(req.questions)) {
      if (q.type === "choice") {
        const n = Object.keys(q.criteria).length;
        expect(n, key).toBeGreaterThanOrEqual(2);
        expect(n, key).toBeLessThanOrEqual(255);
      }
    }
  });

  it("asks one count question per installed provider, in words", () => {
    expect(req.questions["spawn_panes.counts.grok"]).toMatchObject({ type: "choice", criteria: { zero: null, four: null } });
  });

  it("asks the pane target once for all tools", () => {
    expect(Object.keys(req.questions).filter((k) => k.endsWith("target"))).toEqual(["target"]);
  });

  it("offers transcript spans for free text", () => {
    expect(transcriptSpans("tell claude to run the tests.")).toContain("run the tests");
  });
});

const choice = (c: string, confidence = 0.9, probabilities?: Record<string, number>) => ({ type: "choice" as const, choice: c, confidence, probabilities });

describe("parseDecision", () => {
  it("turns counts into spawn commands in the order they were said", () => {
    const result: DecisionResult = {
      answers: {
        action_1: choice("spawn_panes"),
        action_2: choice("spawn_panes", 0.55), // repeats action_1 → dropped
        "spawn_panes.counts.claude": choice("one"),
        "spawn_panes.counts.codex": choice("one"),
        "spawn_panes.counts.grok": choice("one"),
        "spawn_panes.counts.shell": choice("zero"),
      },
    };
    const transcript = "open grok, then claude and codex";
    const decisions = parseDecision(result, ctx);
    expect(decisions).toHaveLength(1);
    const commands = orderByMention(decisions[0].tool.toCommands(decisions[0].args, ctx), transcript, ctx);
    expect(commands.map((c) => c.type === "pane.spawn" && c.provider)).toEqual(["grok", "claude", "codex"]);
    expect(verdict(decisions)).toBe("run");
  });

  it("keeps a different second action and resolves the shared pane target", () => {
    const decisions = parseDecision(
      { answers: { action_1: choice("close_pane", 0.8), action_2: choice("set_layout", 0.7), target: choice("p-b"), "set_layout.preset": choice("grid") } },
      ctx,
    );
    expect(decisions.map((d) => d.tool.toCommands(d.args, ctx))).toEqual([
      [{ type: "pane.close", target: { id: "p-b" } }],
      [{ type: "layout.preset", name: "grid" }],
    ]);
  });

  it("picks free text from a transcript span", () => {
    const [d] = parseDecision(
      { answers: { action_1: choice("send_to_pane"), target: choice("p-a"), "send_to_pane.text": choice("run the tests"), "send_to_pane.submit": { type: "noul", noul: 0.9 } } },
      ctx,
    );
    expect(d.tool.toCommands(d.args, ctx)).toEqual([{ type: "pane.sendText", target: { id: "p-a" }, text: "run the tests", submit: true }]);
  });

  it("applies the confidence policy", () => {
    const at = (c: number, tool = "zoom_pane") => parseDecision({ answers: { action_1: choice(tool, c) } }, ctx);
    expect(verdict(at(0.75))).toBe("run");
    expect(verdict(at(0.45))).toBe("confirm");
    expect(verdict(at(0.2))).toBe("reject");
    expect(verdict(at(0.95, "merge_worktree"))).toBe("confirm"); // risky tools always confirm
    expect(verdict(parseDecision({ answers: { action_1: choice("none") } }, ctx))).toBe("reject");
  });
});
