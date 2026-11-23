import { describe, expect, it } from "vitest";
import { bgNoteTitle, parseBgNote } from "./bgNote";

describe("parseBgNote", () => {
  it("parses an exit note with output", () => {
    const n = parseBgNote('Background task bg11 "Verify SKU" (`az x`) exited with code 0 after 9s. Its output since your last read:\n{\n  "a": 1\n}');
    expect(n).toMatchObject({ id: "bg11", name: "Verify SKU", cmd: "az x", code: 0, secs: 9 });
    expect(n?.output).toBe('{\n  "a": 1\n}');
    expect(bgNoteTitle(n!)).toBe("Verify SKU · done · 9s");
  });
  it("parses unnamed tasks without output and failures", () => {
    const n = parseBgNote("Background task bg1 (`npm run dev`) exited with code 1 after 4s. No new output.");
    expect(n).toMatchObject({ name: "", cmd: "npm run dev", code: 1, output: "" });
    expect(bgNoteTitle(n!)).toBe("npm run dev · exit 1 · 4s");
  });
  it("parses a user stop and ignores other notices", () => {
    expect(parseBgNote('The user stopped background task bg2 "dev" (`pnpm dev`).')).toMatchObject({ code: null, name: "dev" });
    expect(parseBgNote("Resumed session")).toBeNull();
  });
});
