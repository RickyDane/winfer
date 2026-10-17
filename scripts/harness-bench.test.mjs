import { describe, expect, it } from "vitest";
import { PRESETS, parseJsonl, runningJobs, scoreRows, summarize } from "../plugins/harness-bench/index.js";

const run = (harness, model, pass, seconds, input, cached, cost = null) => ({ task: "01", harness, model, pass, seconds, input, cached, output: 100, tools: 4, cost });

describe("harness bench", () => {
  it("parses runner output, skipping junk lines", () => {
    const rows = parseJsonl('{"task":"01"}\nnot json\n\n{"task":"02"}\n{broken');
    expect(rows.map((r) => r.task)).toEqual(["01", "02"]);
  });

  it("reads running jobs from new (list) and old (object) status files", () => {
    expect(runningJobs({ running: [{ task: "01" }, { task: "02" }] })).toHaveLength(2);
    expect(runningJobs({ running: { task: "01" } })).toEqual([{ task: "01" }]);
    expect(runningJobs({ running: null })).toEqual([]);
    expect(runningJobs(null)).toEqual([]);
  });

  it("leaves harness errors out of medians and pass counts", () => {
    const [row] = summarize([run("codex", "m", true, 20, 9000, 0), { ...run("codex", "m", false, 2, 0, 0), error: "429 rate limited" }]);
    expect(row).toMatchObject({ runs: 1, passed: 1, errors: 1, seconds: 20, input: 9000 });
  });

  it("scores harnesses on all metrics and ranks them", () => {
    const rows = summarize([
      run("a", "m", true, 10, 10_000, 0), // fast, cheap, passes
      run("b", "m", true, 40, 80_000, 20_000), // slow and expensive, passes
      run("c", "m", false, 5, 5_000, 0), // fastest and cheapest, but fails
    ]);
    const ranked = scoreRows(rows, PRESETS.balanced.weights);
    expect(ranked.map((r) => r.harness)).toEqual(["a", "b", "c"]);
    expect(ranked[0].score).toBeGreaterThan(ranked[1].score);
    expect(ranked[0].score).toBeLessThanOrEqual(100);
    // Quality only: both passers tie at 100, the failure scores 0.
    const q = scoreRows(rows, PRESETS.quality.weights);
    expect(q.filter((r) => Math.round(r.score) === 100)).toHaveLength(2);
    expect(q.at(-1).score).toBe(0);
    // Cheapest weighs tokens: the failing but cheapest harness climbs.
    const c = scoreRows(rows, PRESETS.cheap.weights);
    expect(c.find((r) => r.harness === "c").score).toBeGreaterThan(ranked.find((r) => r.harness === "c").score);
  });

  it("gives no token points to a harness without token counts", () => {
    const rows = summarize([run("a", "m", true, 10, 10_000, 0), run("x", "m", true, 10, 0, 0)]);
    const [a, x] = scoreRows(rows, PRESETS.balanced.weights);
    expect(a.harness).toBe("a");
    expect(x.parts.uncached).toBe(0);
  });

  it("summarizes medians per harness and model", () => {
    const rows = summarize([
      run("codex", "gpt-6.1-sol", true, 10, 80_000, 70_000),
      run("codex", "gpt-6.1-sol", false, 30, 60_000, 50_000),
      run("codex", "gpt-6-luna", true, 5, 40_000, 0),
      run("winfer-chatgpt", "gpt-6.1-sol", true, 20, 20_000, 0, 0.5),
    ]);
    const sol = rows.find((r) => r.harness === "codex" && r.model === "gpt-6.1-sol");
    expect(sol).toMatchObject({ runs: 2, passed: 1, seconds: 20, input: 70_000, uncached: 10_000, cost: null });
    expect(rows.find((r) => r.model === "gpt-6-luna").runs).toBe(1);
    expect(rows.find((r) => r.harness === "winfer-chatgpt").cost).toBe(0.5);
  });
});
