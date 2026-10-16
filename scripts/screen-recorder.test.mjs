import { describe, expect, it, vi } from "vitest";
import { duration, fileName, fit, outputSize, pickFormat } from "../plugins/screen-recorder/index.js";

describe("format", () => {
  it("prefers MP4, then WebM, else nothing", () => {
    vi.stubGlobal("MediaRecorder", class {});
    expect(pickFormat((t) => t.startsWith("video/mp4"))).toEqual({ mime: "video/mp4;codecs=avc1.640028", ext: "mp4" });
    expect(pickFormat((t) => t === "video/webm;codecs=vp8")).toEqual({ mime: "video/webm;codecs=vp8", ext: "webm" });
    expect(pickFormat(() => false)).toBeNull();
    vi.unstubAllGlobals();
    expect(pickFormat(() => true)).toBeNull();
  });
});

describe("geometry", () => {
  it("caps the long side and keeps even sizes", () => {
    expect(outputSize(1170, 2532)).toEqual({ width: 888, height: 1920 });
    expect(outputSize(801, 601)).toEqual({ width: 802, height: 602 });
  });

  it("fits and centers a picture", () => {
    expect(fit(100, 200, 400, 400)).toEqual({ x: 100, y: 0, w: 200, h: 400 });
  });
});

describe("names", () => {
  it("names files after the source and start time", () => {
    expect(fileName("android", "mp4", new Date(2026, 9, 6, 9, 3, 7))).toBe("android-2026-10-06_09-03-07.mp4");
  });

  it("formats durations", () => {
    expect(duration(65_000)).toBe("1:05");
    expect(duration(3_723_000)).toBe("1:02:03");
  });
});
