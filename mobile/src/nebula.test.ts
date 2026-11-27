import { describe, expect, it } from "vitest";
import { paintNebulaRgba, type NebulaKind } from "./nebula";

const SIZE = 96;

function meanLum(buf: Uint8ClampedArray): number {
  let sum = 0;
  for (let i = 0; i < buf.length; i += 4) sum += buf[i] + buf[i + 1] + buf[i + 2];
  return sum / (SIZE * SIZE * 3);
}

describe("nebula", () => {
  it("paints a dark rim and a lit middle in dark mode", () => {
    const p = paintNebulaRgba("ready", false, 0, SIZE);
    expect(p.length).toBe(SIZE * SIZE * 4);
    const lum = (x: number, y: number) => {
      const i = (y * SIZE + x) * 4;
      return p[i] + p[i + 1] + p[i + 2];
    };
    const rim = lum(1, 1) + lum(SIZE - 2, 1) + lum(1, SIZE - 2) + lum(SIZE - 2, SIZE - 2);
    let midSum = 0;
    for (let y = 40; y < 56; y++) {
      for (let x = 40; x < 56; x++) midSum += lum(x, y);
    }
    const mid = (midSum / 256) * 4;
    expect(mid).toBeGreaterThan(rim);
  });

  it("is visibly brighter in light mode for every state", () => {
    for (const kind of ["idle", "ready", "stop"] as NebulaKind[]) {
      const dark = meanLum(paintNebulaRgba(kind, false, 0, SIZE));
      const light = meanLum(paintNebulaRgba(kind, true, 0, SIZE));
      expect(light).toBeGreaterThan(dark + 40);
    }
    expect(meanLum(paintNebulaRgba("idle", true, 0, SIZE))).toBeGreaterThan(190);
  });
});
