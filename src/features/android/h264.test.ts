import { describe, expect, it } from "vitest";
import { codecFromSps, startCodes } from "./h264";

describe("H.264 Annex B parsing", () => {
  it("finds 3- and 4-byte start codes", () => {
    const buf = new Uint8Array([0, 0, 0, 1, 0x67, 0x42, 0, 0, 1, 0x68, 0xce, 0, 0, 1, 0x65, 0x88]);
    expect(startCodes(buf)).toEqual([0, 6, 11]);
  });

  it("builds the codec string from the SPS profile and level", () => {
    // 00 00 00 01 | 67 (SPS) | profile 0x42 (baseline) | constraints 0xC0 | level 0x1F (3.1)
    expect(codecFromSps(new Uint8Array([0, 0, 0, 1, 0x67, 0x42, 0xc0, 0x1f, 0xda]))).toBe("avc1.42c01f");
    expect(codecFromSps(new Uint8Array([0, 0, 1, 0x67, 0x64, 0x00, 0x28]))).toBe("avc1.640028");
  });
});
