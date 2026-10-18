import { describe, expect, it } from "vitest";
import catalog from "./bench/harnesses.json";
import { clock, isLimitError, planOf, quoteArg, runLabel, trackUsage, usageRows, runOverview, suiteMatrix, suiteOf, suitesIn, summarize, taskName, withEstimatedCost } from "./index.js";

const run = (patch = {}) => ({
  harness: "codex", model: "gpt-6.1-sol", task: "01-bugfix", pass: true,
  seconds: 10, input: 1_000_000, cached: 500_000, output: 100_000, tools: 3,
  cost: null, ...patch,
});

describe("API cost estimates", () => {
  it("prices cached tokens separately, including provider-prefixed models", () => {
    const saved = run({ model: "openai/gpt-6.1-sol" });
    expect(withEstimatedCost(saved, catalog._prices)).toMatchObject({ cost: 2.05, cost_estimated: true });
    expect(saved.cost).toBeNull();
  });

  it("preserves reported costs, including zero and stored estimates", () => {
    for (const cost of [0, 0.123]) {
      const reported = run({ cost });
      expect(withEstimatedCost(reported, catalog._prices)).toBe(reported);
    }
    const estimated = run({ cost: 7, cost_estimated: true });
    expect(withEstimatedCost(estimated, catalog._prices)).toBe(estimated);
  });

  it("leaves unknown models, floating aliases and absent usage unpriced", () => {
    for (const saved of [run({ model: "unknown" }), run({ model: "sonnet" }), run({ input: 0, cached: 0, output: 0 })]) {
      expect(withEstimatedCost(saved, catalog._prices)).toBe(saved);
    }
    expect(withEstimatedCost(run()).cost).toBeNull();
  });

  it("marks mixed totals as estimated and excludes harness errors", () => {
    const results = [run(), run({ cost: 1 }), run({ error: "rate limited", cost: 100 })];
    expect(summarize(results, catalog._prices)[0]).toMatchObject({
      cost: 3.05, costEstimated: true, costPartial: false, runs: 2, errors: 1,
    });
  });

  it("flags partial coverage without treating unknown usage as free", () => {
    const results = [run({ cost: 1 }), run({ input: 0, cached: 0, output: 0 })];
    expect(summarize(results, catalog._prices)[0]).toMatchObject({ cost: 1, costEstimated: false, costPartial: true });
    expect(summarize([run({ model: "unknown" })], catalog._prices)[0].cost).toBeNull();
  });
});

describe("runs overview", () => {
  it("labels result files by date and kind", () => {
    expect(runLabel("20261017-120500.jsonl")).toBe("17.10. 12:05");
    expect(runLabel("20261015-171500-selftest.jsonl")).toBe("15.10. 17:15 · self-test");
    expect(runLabel("odd.jsonl")).toBe("odd");
  });

  it("summarizes a run without counting harness errors", () => {
    const results = [
      run({ started: 100, seconds: 10, cost: 1 }),
      run({ harness: "pi", task: "02-feature", pass: false, started: 105, seconds: 20, cost: 2 }),
      run({ harness: "pi", error: "rate limited", started: 130, seconds: 1 }),
    ];
    expect(runOverview(results, catalog._prices)).toMatchObject({
      runs: 2, passed: 1, errors: 1, harnesses: ["codex", "pi"], models: ["gpt-6.1-sol"], tasks: 2, wall: 31, cost: 3, costEstimated: false, costPartial: false,
    });
  });

  it("handles an empty or unpriced run", () => {
    expect(runOverview([])).toMatchObject({ runs: 0, passed: 0, wall: 0, cost: null });
    expect(runOverview([run({ model: "unknown" })], catalog._prices).cost).toBeNull();
  });
});

describe("suites", () => {
  const catalog = {
    suites: [{ id: "coding" }, { id: "understanding" }, { id: "research" }, { id: "discipline" }],
    tasks: [{ id: "01-bugfix", suite: "coding" }, { id: "04-question", suite: "understanding" }, { id: "r01-docs", suite: "research" }],
  };

  it("strips number prefixes from task ids", () => {
    expect(taskName("01-bugfix")).toBe("bugfix");
    expect(taskName("u03-data-flow")).toBe("data-flow");
    expect(taskName("plain")).toBe("plain");
  });

  it("takes a result's suite from the result, then the catalog, else coding", () => {
    expect(suiteOf({ task: "01-bugfix", suite: "discipline" }, catalog.tasks)).toBe("discipline");
    // Runs saved before suites have no suite field; a moved task follows the catalog.
    expect(suiteOf({ task: "04-question" }, catalog.tasks)).toBe("understanding");
    expect(suiteOf({ task: "99-gone" }, catalog.tasks)).toBe("coding");
  });

  it("orders suites like the catalog", () => {
    const rs = [run({ task: "r01-docs" }), run({ task: "04-question" }), run({ suite: "zeta" }), run()];
    expect(suitesIn(rs, catalog)).toEqual(["coding", "understanding", "research", "zeta"]);
  });

  it("counts passes per harness and suite without harness errors", () => {
    const rs = [
      run(), run({ pass: false }), run({ task: "04-question" }),
      run({ task: "04-question", error: "rate limited" }),
      run({ harness: "pi", task: "r01-docs", pass: false }),
    ];
    const m = suiteMatrix(rs, catalog);
    expect(m.suites).toEqual(["coding", "understanding", "research"]);
    const codex = m.rows.find((r) => r.harness === "codex");
    expect(codex.cells).toEqual({ coding: { passed: 1, runs: 2 }, understanding: { passed: 1, runs: 1 } });
    expect(m.rows.find((r) => r.harness === "pi").cells).toEqual({ research: { passed: 0, runs: 1 } });
  });

  it("lists a run's suites in its overview", () => {
    expect(runOverview([run(), run({ task: "04-question" })], {}, catalog.tasks).suites).toEqual(["coding", "understanding"]);
  });
});

describe("plan usage over a run", () => {
  const snap = (at, used, resetsAt = 10_000) => [{ provider: "codex", at, windows: [{ kind: "session", label: "5-hour", usedPercent: used, resetsAt }] }];

  it("maps harnesses to the plans whose limits they use", () => {
    expect(["codex", "winfer-codex", "winfer-chatgpt"].map(planOf)).toEqual(["codex", "codex", "codex"]);
    expect(["claude", "winfer-claude"].map(planOf)).toEqual(["claude", "claude"]);
    expect(planOf("pi")).toBeNull();
  });

  it("keeps the first and latest reading and when the limit ran out", () => {
    let rec = trackUsage(null, snap(1000, 40));
    rec = trackUsage(rec, snap(2000, 90));
    rec = trackUsage(rec, snap(3000, 100));
    rec = trackUsage(rec, snap(4000, 100));
    rec = trackUsage(rec, snap(3500, 10)); // stale cached reading: ignored
    expect(rec.plans.codex.session).toMatchObject({ start: 40, startAt: 1000, end: 100, endAt: 4000, outAt: 3000, okAt: 2000, outAtStart: false, reset: false });
  });

  it("flags limits already out at the start and windows that reset", () => {
    expect(trackUsage(null, snap(1000, 100)).plans.codex.session).toMatchObject({ outAt: 1000, outAtStart: true });
    const rec = trackUsage(trackUsage(null, snap(1000, 70, 5000)), snap(6000, 5, 9000));
    expect(rec.plans.codex.session).toMatchObject({ start: 70, end: 5, reset: true, outAt: null });
  });

  it("adds the first limit error of a harness, ignoring guessed ones", () => {
    const rec = trackUsage(null, snap(1000, 50));
    const results = [
      run({ harness: "codex", started: 20, seconds: 5, error: "You've hit your usage limit." }),
      run({ harness: "winfer-chatgpt", started: 10, seconds: 2, error: "429 Too Many Requests" }),
      run({ harness: "claude", started: 5, seconds: 1, error: "The harness exited within seconds without doing anything (likely a rate limit or crash)." }),
    ];
    expect(isLimitError(results[2].error)).toBe(false);
    expect(usageRows(rec, results)).toEqual([expect.objectContaining({ provider: "codex", kind: "session", limitErrorAt: 12_000 })]);
    expect(usageRows(null, results.slice(0, 1))).toEqual([{ provider: "codex", kind: null, label: null, limitErrorAt: 25_000 }]);
  });

  it("formats times with the date only when not today", () => {
    const now = new Date(2026, 9, 17, 18, 0).getTime();
    expect(clock(new Date(2026, 9, 17, 9, 5).getTime(), now)).toBe("09:05");
    expect(clock(new Date(2026, 9, 16, 23, 59).getTime(), now)).toBe("16.10. 23:59");
  });
});

describe("shell quoting for run.py arguments", () => {
  it("single-quotes for sh, escaping quotes", () => {
    expect(quoteArg("/a b/it's.jsonl", false)).toBe(`'/a b/it'\\''s.jsonl'`);
  });

  it("double-quotes for cmd.exe, dropping quotes and keeping a trailing backslash literal", () => {
    expect(quoteArg("C:\\Users\\me\\harness bench", true)).toBe(`"C:\\Users\\me\\harness bench"`);
    expect(quoteArg('C:\\dir\\', true)).toBe(`"C:\\dir\\\\"`);
    expect(quoteArg('codex="x"', true)).toBe(`"codex=x"`);
  });
});
