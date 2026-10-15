import { mkdirSync, mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { buildCatalog, compareVersions } from "./plugin-catalog.mjs";

describe("plugin store catalog", () => {
  it("matches the plugins/ folder (run node scripts/plugin-catalog.mjs after changing a plugin)", () => {
    expect(readFileSync(new URL("../plugins/catalog.json", import.meta.url), "utf8")).toBe(buildCatalog());
  });

  it("doesn't require the catalog version to match plugin.json", () => {
    const root = mkdtempSync(join(tmpdir(), "winfer-catalog-"));
    const plugin = (id, manifest) => {
      mkdirSync(join(root, id));
      writeFileSync(join(root, id, "plugin.json"), JSON.stringify({ id, name: id, ...manifest }));
    };
    plugin("ahead", { version: "1.0.0" });
    plugin("behind", { version: "2.0.0" });
    plugin("unversioned", {});
    const old = { ahead: "1.2.0", behind: "1.9.0", unversioned: "0.3.0" };
    writeFileSync(join(root, "catalog.json"), JSON.stringify({ plugins: Object.entries(old).map(([id, version]) => ({ id, version })) }));
    const versions = Object.fromEntries(JSON.parse(buildCatalog(root)).plugins.map((p) => [p.id, p.version]));
    expect(versions).toEqual({ ahead: "1.2.0", behind: "2.0.0", unversioned: "0.3.0" });
  });

  it("compares versions numerically", () => {
    expect(compareVersions("1.10.0", "1.9.2")).toBe(1);
    expect(compareVersions("1.0", "1.0.0")).toBe(0);
    expect(compareVersions("0.9", "1.0.0")).toBe(-1);
  });
});
