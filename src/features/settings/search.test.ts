import { describe, expect, it } from "vitest";
import INDEX from "./searchIndex.json";
import { searchSettings } from "./search";

const PAGES = ["general", "theme", "agents", "winfer-code", "accounts", "ai", "voice", "plugins", "skills", "usage", "mobile", "data", "updates", "diagnostics"].map((id) => ({ id, label: id, description: "" }));

describe("searchSettings", () => {
  it("points every index entry at a page", () => {
    for (const e of INDEX) expect(PAGES.map((p) => p.id)).toContain(e.page);
  });
  it("finds settings by label and words, best first, without duplicates", () => {
    expect(searchSettings("approve", PAGES).some((h) => h.page === "agents")).toBe(true);
    expect(searchSettings("nebula", PAGES).some((h) => h.page === "updates")).toBe(true);
    expect(searchSettings("text size", PAGES)[0].label).toBe("Text size");
    expect(searchSettings("auto-approve", PAGES).filter((h) => h.page === "agents")).toHaveLength(1);
    expect(searchSettings("  ", PAGES)).toEqual([]);
    expect(searchSettings("zzz-nothing", PAGES)).toEqual([]);
  });
});
