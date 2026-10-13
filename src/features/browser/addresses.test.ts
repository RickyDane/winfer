import { describe, expect, it } from "vitest";
import { findAddresses } from "./addresses";

describe("findAddresses", () => {
  it("finds files and URLs, newest first, resolved against the pane folder", () => {
    const out = [
      "  ➜  Local:   http://localhost:5173/",
      "Ich habe /Users/ricky/Coding/index.html erstellt.",
      "Öffnen kannst du sie mit open /Users/ricky/Coding/index.html.",
      "Also wrote about.html next to it.",
    ].join("\n");
    expect(findAddresses(out, "~/Coding", "/Users/ricky")).toEqual([
      "file:///Users/ricky/Coding/about.html",
      "file:///Users/ricky/Coding/index.html",
      "http://localhost:5173/",
    ]);
  });

  it("expands ~ and ignores paths inside URLs", () => {
    expect(findAddresses("see ~/site/page.html and https://example.com/docs/index.html", "/tmp", "/Users/r")).toEqual([
      "https://example.com/docs/index.html",
      "file:///Users/r/site/page.html",
    ]);
  });
});
