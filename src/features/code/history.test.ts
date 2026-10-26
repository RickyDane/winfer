import { describe, expect, it } from "vitest";
import { HISTORY_PAGE, historyStart } from "./history";

describe("transcript history window", () => {
  it("bounds initial rendering even for very large restores", () => {
    expect(historyStart(0)).toBe(0);
    expect(historyStart(12)).toBe(0);
    expect(historyStart(100_000)).toBe(100_000 - HISTORY_PAGE);
  });

  it("keeps the same first row while tokens and new messages arrive", () => {
    const start = historyStart(600);
    expect(historyStart(600, start)).toBe(start);
    expect(historyStart(650, start)).toBe(start);
    expect(historyStart(650, start - HISTORY_PAGE)).toBe(start - HISTORY_PAGE);
    expect(historyStart(650, 0)).toBe(0);
  });

  it("resets a window removed by undo or clear", () => {
    expect(historyStart(20, 560)).toBe(0);
    expect(historyStart(100, 560)).toBe(100 - HISTORY_PAGE);
    expect(historyStart(0, 560)).toBe(0);
  });

  it("also bounds long-running sessions, but only when following the latest output", () => {
    expect(historyStart(80, 0, true)).toBe(0);
    expect(historyStart(81, 0, true)).toBe(81 - HISTORY_PAGE);
    expect(historyStart(1000, 0, true)).toBe(1000 - HISTORY_PAGE);
    expect(historyStart(1000, 0, false)).toBe(0);
  });
});
