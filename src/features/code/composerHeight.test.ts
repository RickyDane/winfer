import { describe, expect, it } from "vitest";
import { COMPOSER_MAX_HEIGHT, composerHeight } from "./composerHeight";

describe("composer textarea height", () => {
  it("never sizes an empty composer from scrollHeight (wrapped placeholder / narrow mount)", () => {
    // A clean session has no text; the placeholder measured at a hidden or
    // narrow mount can report many lines. Empty must stay one line (null =
    // no inline height, CSS rows/min-height govern).
    expect(composerHeight("", 0)).toBeNull();
    expect(composerHeight("", 48)).toBeNull();
    expect(composerHeight("", COMPOSER_MAX_HEIGHT)).toBeNull();
    expect(composerHeight("", 9999)).toBeNull();
  });

  it("sizes non-empty text from scrollHeight, capped", () => {
    expect(composerHeight("hi", 48)).toBe(48);
    expect(composerHeight("line\nline\nline", 120)).toBe(120);
    expect(composerHeight("x".repeat(5000), 9999)).toBe(COMPOSER_MAX_HEIGHT);
    expect(composerHeight("x", COMPOSER_MAX_HEIGHT)).toBe(COMPOSER_MAX_HEIGHT);
  });
});
