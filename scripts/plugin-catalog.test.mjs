import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { buildCatalog } from "./plugin-catalog.mjs";

describe("plugin store catalog", () => {
  it("matches the plugins/ folder (run node scripts/plugin-catalog.mjs after changing a plugin)", () => {
    expect(readFileSync(new URL("../plugins/catalog.json", import.meta.url), "utf8")).toBe(buildCatalog());
  });
});
