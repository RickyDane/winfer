import { describe, expect, it } from "vitest";
import { mentionAt, rankFiles } from "./mentions";

describe("mentionAt", () => {
  it("finds the @word under the caret", () => {
    expect(mentionAt("look at @src/ap", 15)).toEqual({ start: 8, end: 15, query: "src/ap" });
    expect(mentionAt("@foo bar", 2)).toEqual({ start: 0, end: 4, query: "f" });
    expect(mentionAt("@", 1)).toEqual({ start: 0, end: 1, query: "" });
  });
  it("ignores words that aren't mentions", () => {
    expect(mentionAt("mail me@x.com", 13)).toBeNull();
    expect(mentionAt("@foo bar", 8)).toBeNull();
    expect(mentionAt("", 0)).toBeNull();
  });
});

describe("rankFiles", () => {
  const files = ["src/App.tsx", "src/features/code/Composer.tsx", "src/features/code/composer.css", "docs/apple.md"];
  it("prefers filename matches and drops non-matches", () => {
    expect(rankFiles(files, "composer")[0]).toMatch(/omposer/);
    expect(rankFiles(files, "zzz")).toEqual([]);
    expect(rankFiles(files, "app")[0]).toBe("src/App.tsx");
  });
  it("lists short paths first for an empty query", () => {
    expect(rankFiles(files, "", 2)).toEqual(["src/App.tsx", "docs/apple.md"]);
  });
});
