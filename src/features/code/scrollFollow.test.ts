import { describe, expect, it } from "vitest";
import { followAfterScroll } from "./scrollFollow";

describe("transcript scroll following", () => {
  it("keeps an upward wheel/trackpad gesture paused, including subpixel moves", () => {
    expect(followAfterScroll(false, 1000, 999.5, 0.5, -1)).toBe(false);
    expect(followAfterScroll(false, 999.5, 999.5, 0.5, -1)).toBe(false);
    expect(followAfterScroll(false, 999.5, 1000, 0, -1)).toBe(false);
  });

  it("does not resume on duplicate or layout-generated near-bottom scroll events", () => {
    expect(followAfterScroll(false, 960, 960, 40, 0)).toBe(false);
    expect(followAfterScroll(false, 960, 975, 25, 0)).toBe(false);
    expect(followAfterScroll(false, 960, 960, 0, 1)).toBe(false);
  });

  it("resumes when a downward gesture or scrollbar drag reaches the bottom", () => {
    expect(followAfterScroll(false, 960, 980, 20, 1)).toBe(false);
    expect(followAfterScroll(false, 980, 1000, 0, 1)).toBe(true);
    expect(followAfterScroll(false, 980, 999, 1, 0)).toBe(true);
  });

  it("pauses when a keyboard/scrollbar move goes up, but not when content shrinks", () => {
    expect(followAfterScroll(true, 1000, 900, 100, 0)).toBe(false);
    expect(followAfterScroll(true, 1000, 900, 0, 0)).toBe(true);
  });

  it("keeps following during normal streaming and programmatic downward jumps", () => {
    expect(followAfterScroll(true, 1000, 1000, 100, 0)).toBe(true);
    expect(followAfterScroll(true, 1000, 1100, 0, 0)).toBe(true);
  });
});
