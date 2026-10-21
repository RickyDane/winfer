import { describe, expect, it } from "vitest";
import { blendsIn, luminance } from "./iconContrast";

/** A 2x2 icon: `fill` pixels of `rgb`, the rest transparent. */
const icon = (rgb: [number, number, number], fill = 4) =>
  new Uint8ClampedArray([0, 1, 2, 3].flatMap((i) => (i < fill ? [...rgb, 255] : [0, 0, 0, 0])));

const dark = luminance([26, 26, 26]);
const light = luminance([250, 250, 250]);

describe("blendsIn", () => {
  it("flags a dark icon on a dark bar", () => expect(blendsIn(icon([0, 0, 0]), dark)).toBe(true));
  it("passes a dark icon on a light bar", () => expect(blendsIn(icon([0, 0, 0]), light)).toBe(false));
  it("flags a white icon on a light bar", () => expect(blendsIn(icon([255, 255, 255]), light)).toBe(true));
  it("passes a colourful icon either way", () => {
    expect(blendsIn(icon([230, 80, 30]), dark)).toBe(false);
    expect(blendsIn(icon([230, 80, 30]), light)).toBe(false);
  });
  it("ignores transparent pixels and empty icons", () => {
    expect(blendsIn(icon([255, 255, 255], 1), dark)).toBe(false);
    expect(blendsIn(icon([0, 0, 0], 0), dark)).toBe(false);
  });
});
