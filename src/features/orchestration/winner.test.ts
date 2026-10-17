import { describe, expect, it } from "vitest";
import { judge, type Contender } from "./winner";

const c = (o: Partial<Contender>): Contender => ({ timeMs: 60_000, cost: 1, tokens: 1000, changed: 10, errored: false, finished: true, ...o });

describe("judge", () => {
  it("picks the agent that is cheaper and faster", () => {
    const v = judge([c({}), c({ cost: 0.5, timeMs: 30_000 })]);
    expect(v.winner).toBe(1);
    expect(v.scores[1]).toBe(1);
    expect(v.reason).toContain("cost, time");
  });

  it("weighs the metrics against each other", () => {
    // A is twice as expensive, B three times as slow: time and cost weigh the same, so A wins.
    const v = judge([c({ cost: 2, timeMs: 10_000 }), c({ cost: 1, timeMs: 30_000 })]);
    expect(v.winner).toBe(0);
  });

  it("rules out agents that errored or changed nothing", () => {
    const v = judge([c({ cost: 0.1, errored: true }), c({ cost: 0.1, changed: 0 }), c({})]);
    expect(v.out).toEqual(["errored", "changed nothing", null]);
    expect(v.winner).toBe(2);
  });

  it("doesn't rule out no-change agents when nobody changed anything", () => {
    const v = judge([c({ changed: 0 }), c({ changed: 0, cost: 0.5 })]);
    expect(v.winner).toBe(1);
  });

  it("calls near ties and skips unknown metrics", () => {
    expect(judge([c({}), c({ cost: 1.01 })]).winner).toBeNull();
    const v = judge([c({ cost: null, timeMs: 10_000 }), c({ cost: 0.1 })]);
    expect(v.winner).toBe(0);
    expect(judge([c({ cost: null, timeMs: null, tokens: null }), c({})]).winner).toBeNull();
  });

  it("is provisional until everyone finished", () => {
    expect(judge([c({}), c({ finished: false, cost: 0.2 })]).final).toBe(false);
  });
});
