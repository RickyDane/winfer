import { describe, expect, it } from "vitest";
import { followAfterScroll, isAwayFromBottom } from "./scrollFollow";

describe("transcript jump button", () => {
  it("stays hidden after pausing when the transcript fits or is at the bottom", () => {
    expect(isAwayFromBottom(false, 0)).toBe(false);
    expect(isAwayFromBottom(false, -1)).toBe(false);
    expect(isAwayFromBottom(false, 0.5)).toBe(false);
    expect(isAwayFromBottom(false, 2)).toBe(false);
  });

  it("only shows when paused with content below the viewport", () => {
    expect(isAwayFromBottom(false, 3)).toBe(true);
    expect(isAwayFromBottom(false, 100)).toBe(true);
    expect(isAwayFromBottom(true, 100)).toBe(false);
  });

  it("hides when a resize or collapsed output removes the remaining distance", () => {
    expect([100, 20, 0].map((distance) => isAwayFromBottom(false, distance)))
      .toEqual([true, true, false]);
  });
});

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

  it("keeps keyboard/scrollbar input paused before its scroll event arrives", () => {
    expect(followAfterScroll(false, 1000, 900, 100, -1)).toBe(false);
    expect(followAfterScroll(false, 1000, 900, 100, 0)).toBe(false);
  });

  it("keeps following when layout moves upward, even if new output has grown below it", () => {
    expect(followAfterScroll(true, 1000, 900, 0, 0)).toBe(true);
    // A collapse or history trim clamps scrollTop; output then grows before the
    // queued scroll event, so distance is no longer zero when it is handled.
    expect(followAfterScroll(true, 4013, 335, 702, 0)).toBe(true);
    expect(followAfterScroll(true, 1000, 900, 100, 1)).toBe(true);
  });

  it("continues through repeated history trims and streaming growth", () => {
    let following = true;
    for (let trim = 0; trim < 3; trim++) {
      following = followAfterScroll(following, 4000, 300, 700, 0);
      expect(following).toBe(true);
      expect(isAwayFromBottom(following, 700)).toBe(false);
      following = followAfterScroll(following, 300, 1000, 0, 0);
      expect(following).toBe(true);
    }
  });

  it("keeps following during normal streaming and programmatic downward jumps", () => {
    expect(followAfterScroll(true, 1000, 1000, 100, 0)).toBe(true);
    expect(followAfterScroll(true, 1000, 1100, 0, 0)).toBe(true);
  });
});
