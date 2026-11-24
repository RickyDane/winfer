import { describe, expect, it } from "vitest";
import { arcs, fmt, ticks } from "./vizMath";

describe("vizMath", () => {
  it("formats numbers compactly", () => {
    expect(fmt(0)).toBe("0");
    expect(fmt(12.345)).toBe("12.3");
    expect(fmt(1234)).toBe("1234");
    expect(fmt(12_500)).toBe("12.5k");
    expect(fmt(3_400_000)).toBe("3.4M");
    expect(fmt(45, "%")).toBe("45%");
    expect(fmt(2, "s")).toBe("2 s");
  });

  it("picks round ticks that cover the data", () => {
    expect(ticks(0, 87)).toEqual([0, 20, 40, 60, 80, 100]);
    expect(ticks(-5, 10)[0]).toBeLessThanOrEqual(-5);
    expect(ticks(0, 0)).toEqual([0, 1]);
  });

  it("splits a pie into consecutive arcs", () => {
    const a = arcs([1, 1, 2]);
    expect(a[0]).toEqual([0, 0.25]);
    expect(a[2]).toEqual([0.5, 1]);
  });
});
