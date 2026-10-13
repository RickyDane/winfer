import { describe, expect, it } from "vitest";
import type { ToolContext } from "@/tools/registry";
import type { Provider } from "@/features/providers/providers";
import type { Workspace } from "@/stores/workspaces";
import { buildRequest, orderByMention, parseDecision, transcriptSpans, verdict, withoutLeadIn, type DecisionResult } from "./decision";

const provider = (id: string, name: string, aliases: string[] = []): Provider => ({
  id, name, command: id, args: [], resumeArgs: [], aliases, statusHooks: false, promptArg: false, approveInput: null, modelArg: null, modelCommand: null, installed: true,
});

const pane = (id: string, provider: string) => ({
  id, workspaceId: "ws", provider, title: "api", cwd: "~", status: "idle" as const, worktree: null,
  wantsWorktree: false, resume: false, unseen: false, devUrl: null, raceId: null, prompt: null, model: null,
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

  it("offers short phrases and sentence endings as free text", () => {
    const spans = transcriptSpans("create a new window for each provider and tell them to create a new Flappy Bird clone.");
    expect(spans).toContain("create a new Flappy Bird clone");
    expect(spans).toContain("Flappy Bird");
    expect(spans).not.toContain("create a new window for each provider and tell"); // long, not an ending
    expect(spans.length).toBeLessThan(120);
  });

  it("asks free text once per slot", () => {
    expect(Object.keys(req.questions).filter((k) => k === "instruction" || k === "address")).toEqual(["instruction", "address"]);
    expect(Object.keys(req.questions).some((k) => k.endsWith(".prompt") || k.endsWith(".text") || k.endsWith(".url"))).toBe(false);
  });
});

/** A choice answer whose winner beats the runner-up by `ratio`. */
const choice = (c: string, ratio = 10) => ({ type: "choice" as const, choice: c, confidence: 0.1, probabilities: { [c]: 0.5, other: 0.5 / ratio } });

describe("parseDecision", () => {
  it("turns counts into spawn commands in the order they were said", () => {
    const result: DecisionResult = {
      answers: {
        action_1: choice("spawn_panes"),
        action_2: choice("spawn_panes", 1.5), // repeats action_1 → dropped
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
      { answers: { action_1: choice("close_pane", 4), action_2: choice("set_layout", 3), target: choice("p-b"), "set_layout.preset": choice("grid") } },
      ctx,
    );
    expect(decisions.map((d) => d.tool.toCommands(d.args, ctx))).toEqual([
      [{ type: "pane.close", target: { id: "p-b" } }],
      [{ type: "layout.preset", name: "grid" }],
    ]);
  });

  it("gives a new agent its instruction instead of typing it elsewhere", () => {
    const decisions = parseDecision(
      {
        answers: {
          action_1: choice("spawn_panes"),
          action_2: choice("send_to_pane", 3),
          "spawn_panes.counts.codex": choice("one"),
          instruction: choice("add dark mode to the settings page"),
          instruction_given: { type: "noul", noul: 0.9 },
        },
      },
      ctx,
    );
    expect(decisions.map((d) => d.tool.id)).toEqual(["spawn_panes"]);
    expect(decisions[0].tool.toCommands(decisions[0].args, ctx)).toEqual([
      { type: "pane.spawn", provider: "codex", count: 1, prompt: "add dark mode to the settings page" },
    ]);
  });

  it("ignores a picked span when the user gave no instruction", () => {
    const [d] = parseDecision(
      { answers: { action_1: choice("spawn_panes"), "spawn_panes.counts.claude": choice("one"), instruction: choice("open cloud code"), instruction_given: { type: "noul", noul: 0.1 } } },
      ctx,
    );
    expect(d.tool.toCommands(d.args, ctx)).toEqual([{ type: "pane.spawn", provider: "claude", count: 1, prompt: undefined }]);
  });

  it("picks free text from a transcript span", () => {
    const [d] = parseDecision(
      { answers: { action_1: choice("send_to_pane"), target: choice("p-a"), instruction: choice("run the tests"), instruction_given: { type: "noul", noul: 0.95 }, "send_to_pane.submit": { type: "noul", noul: 0.9 } } },
      ctx,
    );
    expect(d.tool.toCommands(d.args, ctx)).toEqual([{ type: "pane.sendText", target: { id: "p-a" }, text: "run the tests", submit: true }]);
  });

  it("applies the confidence policy", () => {
    const at = (c: number, tool = "zoom_pane") => parseDecision({ answers: { action_1: choice(tool, c) } }, ctx);
    expect(verdict(at(2.5))).toBe("run");
    expect(verdict(at(1.4))).toBe("confirm");
    expect(verdict(at(1.01))).toBe("reject");
    expect(verdict(at(20, "merge_worktree"))).toBe("confirm"); // risky tools always confirm
    expect(verdict(parseDecision({ answers: { action_1: choice("none") } }, ctx))).toBe("reject");
  });
});

describe("models", () => {
  it("opens an agent with a spoken model only if that agent offers it", () => {
    const [d] = parseDecision(
      { answers: { action_1: choice("spawn_panes"), "spawn_panes.counts.claude": choice("one"), "spawn_panes.counts.codex": choice("one"), model: choice("opus") } },
      ctx,
    );
    // Codex can't list models here, so it gets the name as-is; Claude offers "opus".
    expect(d.tool.toCommands(d.args, ctx).map((c) => c.type === "pane.spawn" && [c.provider, c.model])).toEqual([
      ["claude", "opus"],
      ["codex", "opus"],
    ]);
  });

  it("changes the model of every pane that offers it", () => {
    const [d] = parseDecision({ answers: { action_1: choice("change_model"), target: choice("all"), model: choice("opus") } }, ctx);
    // p-a is Claude (offers opus); p-b is Codex, which can't list models, so it gets the name as-is.
    expect(d.tool.toCommands(d.args, ctx)).toEqual([
      { type: "pane.setModel", target: { id: "p-a" }, model: "opus" },
      { type: "pane.setModel", target: { id: "p-b" }, model: "opus" },
    ]);
  });

  it("switches the model of a pane", () => {
    const [d] = parseDecision({ answers: { action_1: choice("change_model"), target: choice("p-a"), model: choice("sonnet") } }, ctx);
    expect(d.tool.toCommands(d.args, ctx)).toEqual([{ type: "pane.setModel", target: { id: "p-a" }, model: "sonnet" }]);
  });
});

describe("withoutLeadIn", () => {
  it.each([
    ["ask it to add dark mode to the settings page", "add dark mode to the settings page"],
    ["claude to run the tests", "run the tests"],
    ["ask codex to fix the lint errors", "fix the lint errors"],
    ["ask both to write a snake game in python", "write a snake game in python"],
    ["and tell them to create a new Flappy Bird clone", "create a new Flappy Bird clone"],
    ["insert fix the failing test into pane two", "fix the failing test"],
    ["create a Flappy Bird clone", "create a Flappy Bird clone"],
    ["hello world", "hello world"],
    ["er soll einen Flappy Bird Klon bauen", "einen Flappy Bird Klon bauen"],
    ["Claude er soll die Tests ausführen", "die Tests ausführen"],
    ["sag ihm, er soll die Tests ausführen", "die Tests ausführen"],
  ])("%s → %s", (input, expected) => expect(withoutLeadIn(input)).toBe(expected));
});
