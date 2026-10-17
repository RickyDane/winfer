import { describe, expect, it } from "vitest";
import {
  DEFAULT_POLICY,
  buildClaudeAgents,
  classifyRules,
  claudeRank,
  delegationPrompt,
  normalizePolicy,
  parseLlmVerdict,
  pickTier,
  resolveTier,
  routePlan,
  normalizeTasks,
  nextRunnable,
  blocked,
  settled,
  plannerPrompt,
  workerPrompt,
  reviewerPrompt,
} from "../plugins/model-router/index.js";

const plan = (over = {}) => ({
  paneId: "p1",
  workspaceId: "w1",
  provider: "claude",
  cwd: "/repo",
  model: "opus",
  effort: null,
  prompt: null,
  systemPrompt: null,
  agents: null,
  route: null,
  resume: false,
  plain: false,
  ...over,
});

describe("classifyRules", () => {
  it.each([
    ["grep for TODO comments in src", "cheap"],
    ["Find where the auth token is refreshed", "cheap"],
    ["where is useWorkspaces defined?", "cheap"],
    ["List the files that import llm.ts", "cheap"],
    ["rename fooBar to fooBaz everywhere", "standard"],
    ["Fix the lint errors in the settings page", "standard"],
    ["write tests for parseModels", "standard"],
    ["Redesign the auth flow to support SSO", "frontier"],
    ["Track down the race condition in the pty reader", "frontier"],
    ["review the diff for bugs", "frontier"],
  ])("%s → %s", (task, tier) => {
    expect(classifyRules(task)?.tier).toBe(tier);
  });

  it("keeps tasks that edit out of the cheap tier", () => {
    expect(classifyRules("find the bug in the parser and fix it")?.tier).not.toBe("cheap");
  });

  it("returns null when nothing matches", () => {
    expect(classifyRules("make the login page nicer")).toBeNull();
    expect(classifyRules("")).toBeNull();
  });

  it("bumps big tasks a tier", () => {
    const steps = "find these:\n- a\n- b\n- c\n- d";
    expect(classifyRules(steps)?.tier).toBe("standard");
  });
});

describe("parseLlmVerdict", () => {
  it("reads JSON, fenced or in prose", () => {
    expect(parseLlmVerdict('{"tier":"cheap","reason":"just a search"}')).toEqual({ tier: "cheap", reason: "just a search" });
    expect(parseLlmVerdict('Sure:\n```json\n{"tier": "Frontier", "reason": "design"}\n```')).toEqual({ tier: "frontier", reason: "design" });
  });

  it("falls back to a bare word and rejects garbage", () => {
    expect(parseLlmVerdict("standard")).toEqual({ tier: "standard", reason: "" });
    expect(parseLlmVerdict('{"tier":"huge"}')).toBeNull();
    expect(parseLlmVerdict("no idea")).toBeNull();
  });
});

describe("policy", () => {
  it("fills defaults and drops broken rules", () => {
    const p = normalizePolicy({ delegation: false, tiers: { cheap: { codex: { model: "gpt-mini", effort: "minimal" } } }, rules: [{ tier: "cheap", pattern: "(" }, { tier: "nope", pattern: "x" }, { tier: "cheap", pattern: "x" }] });
    expect(p.delegation).toBe(false);
    expect(p.routeSpawns).toBe(true);
    expect(p.tiers.cheap.codex).toEqual({ model: "gpt-mini", effort: "minimal" });
    expect(p.tiers.cheap.claude).toEqual(DEFAULT_POLICY.tiers.cheap.claude);
    expect(p.rules).toHaveLength(1);
    expect(normalizePolicy(null).rules).toEqual(DEFAULT_POLICY.rules);
  });

  it("resolves tiers per provider, empty parts as the CLI's default", () => {
    expect(resolveTier(DEFAULT_POLICY, "claude", "cheap")).toEqual({ model: "haiku", effort: "low" });
    expect(resolveTier(DEFAULT_POLICY, "codex", "cheap")).toEqual({ model: null, effort: "low" });
    expect(resolveTier(DEFAULT_POLICY, "grok", "cheap")).toBeNull();
  });
});

describe("delegation", () => {
  it("adds only subagents cheaper than the session", () => {
    expect(claudeRank(null)).toBeGreaterThan(claudeRank("opus"));
    expect(Object.keys(buildClaudeAgents(DEFAULT_POLICY, "opus"))).toEqual(["scout", "worker"]);
    expect(Object.keys(buildClaudeAgents(DEFAULT_POLICY, "claude-sonnet-5-5"))).toEqual(["scout"]);
    expect(buildClaudeAgents(DEFAULT_POLICY, "haiku")).toEqual({});
  });

  it("matches the --agents schema", () => {
    const { scout, worker } = buildClaudeAgents(DEFAULT_POLICY, null);
    expect(scout).toMatchObject({ model: "haiku", effort: "low", tools: ["Read", "Grep", "Glob", "Bash"] });
    expect(typeof scout.description).toBe("string");
    expect(typeof scout.prompt).toBe("string");
    expect(worker).toMatchObject({ model: "sonnet", effort: "medium" });
    expect(worker.tools).toBeUndefined();
    expect(delegationPrompt({ scout })).toContain("scout (haiku)");
    expect(delegationPrompt({})).toBe("");
  });
});

describe("routePlan", () => {
  const never = () => {
    throw new Error("LLM not expected");
  };

  it("gives user panes subagents but keeps their model", async () => {
    const { patch, entry } = await routePlan(plan({ route: { origin: "user" } }), DEFAULT_POLICY, never);
    expect(patch.model).toBeUndefined();
    expect(Object.keys(patch.agents)).toEqual(["scout", "worker"]);
    expect(patch.systemPrompt).toContain("scout");
    expect(entry).toBeNull();
  });

  it("routes agent spawns by rule, and the subagents follow the routed model", async () => {
    const { patch, entry } = await routePlan(plan({ route: { origin: "agent", task: "grep for unused exports" } }), DEFAULT_POLICY, never);
    expect(patch).toMatchObject({ model: "haiku", effort: "low" });
    expect(patch.agents).toBeUndefined();
    expect(entry).toMatchObject({ tier: "cheap", via: "rule", from: "opus" });
  });

  it("keeps explicit models, resumed and plain panes", async () => {
    for (const p of [
      plan({ route: { origin: "agent", task: "grep x", explicitModel: true } }),
      plan({ route: { origin: "agent", task: "grep x" }, resume: true }),
      plan({ route: { origin: "agent", task: "grep x" }, plain: true }),
    ]) {
      const { patch, entry } = await routePlan(p, DEFAULT_POLICY, never);
      expect(patch.model).toBeUndefined();
      expect(entry).toBeNull();
    }
    expect((await routePlan(plan({ plain: true }), DEFAULT_POLICY, never)).patch).toEqual({});
  });

  it("only sets effort on codex", async () => {
    const { patch } = await routePlan(plan({ provider: "codex", model: null, route: { origin: "workflow", tier: "frontier" } }), DEFAULT_POLICY, never);
    expect(patch).toEqual({ effort: "high" });
  });
});

describe("pickTier", () => {
  const p = (route) => plan({ route: { origin: "agent", ...route } });

  it("prefers the hint, then rules, then the LLM, then standard", async () => {
    expect((await pickTier(p({ tier: "frontier", task: "grep x" }), DEFAULT_POLICY)).via).toBe("hint");
    expect((await pickTier(p({ tier: "auto", task: "grep x" }), DEFAULT_POLICY)).via).toBe("rule");
    const llm = await pickTier(p({ task: "make the login page nicer" }), DEFAULT_POLICY, async () => '{"tier":"cheap","reason":"css"}');
    expect(llm).toEqual({ tier: "cheap", via: "llm", reason: "css" });
    const failed = await pickTier(p({ task: "make the login page nicer" }), DEFAULT_POLICY, async () => {
      throw new Error("offline");
    });
    expect(failed).toMatchObject({ tier: "standard", via: "default" });
    const off = await pickTier(p({ task: "make it nicer" }), { ...DEFAULT_POLICY, llmFallback: false }, async () => "cheap");
    expect(off.via).toBe("default");
  });
});

describe("workflows", () => {
  const wf = (tasks, over = {}) => ({ id: "wf1", goal: "g", status: "running", maxParallel: 2, tasks, ...over });

  it("normalizes planner tasks", () => {
    const tasks = normalizeTasks([{ title: " A ", details: "x", tier: "Cheap" }, { title: "B", tier: "huge", depends_on: [1, 2, 9, 1] }], 3);
    expect(tasks.map((t) => [t.n, t.title, t.tier, t.deps])).toEqual([
      [4, "A", "cheap", []],
      [5, "B", "", [1, 2]],
    ]);
    expect(() => normalizeTasks([])).toThrow();
    expect(() => normalizeTasks([{ details: "no title" }])).toThrow(/title/);
  });

  it("starts ready tasks up to the parallel limit", () => {
    const tasks = normalizeTasks([{ title: "a" }, { title: "b" }, { title: "c", depends_on: [1] }, { title: "d" }]);
    expect(nextRunnable(wf(tasks)).map((t) => t.n)).toEqual([1, 2]);
    tasks[0].status = "done";
    tasks[1].status = "running";
    expect(nextRunnable(wf(tasks)).map((t) => t.n)).toEqual([3]);
    expect(nextRunnable(wf(tasks, { status: "planning" }))).toEqual([]);
  });

  it("blocks tasks behind failed ones and settles", () => {
    const tasks = normalizeTasks([{ title: "a" }, { title: "b", depends_on: [1] }, { title: "c", depends_on: [2] }, { title: "d" }]);
    tasks[0].status = "failed";
    tasks[3].status = "done";
    expect(blocked(wf(tasks)).map((t) => t.n)).toEqual([2, 3]);
    expect(settled(wf(tasks))).toBe(true);
    tasks[3].status = "running";
    expect(settled(wf(tasks))).toBe(false);
  });

  it("writes prompts that name the workflow and its tools", () => {
    const w = wf(normalizeTasks([{ title: "a", details: "do a" }]));
    expect(plannerPrompt(w)).toContain('workflow_plan with workflow "wf1"');
    expect(workerPrompt(w, w.tasks[0])).toContain("workflow_done");
    expect(reviewerPrompt(w, { 1: "winfer/a" })).toContain("branch winfer/a");
  });
});
