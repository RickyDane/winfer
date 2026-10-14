#!/usr/bin/env node
// Builds plugins/catalog.json, the plugin store's index: every plugin folder's manifest
// fields plus each file's SHA-256, which the app checks when installing.
//
//   node scripts/plugin-catalog.mjs          write plugins/catalog.json
//   node scripts/plugin-catalog.mjs --check  exit 1 if it's out of date
import { createHash } from "node:crypto";
import { readdirSync, readFileSync, statSync, writeFileSync } from "node:fs";
import { join, relative, sep } from "node:path";
import { fileURLToPath } from "node:url";

const PLUGINS = fileURLToPath(new URL("../plugins/", import.meta.url));
const CATALOG = join(PLUGINS, "catalog.json");

function files(dir) {
  return readdirSync(dir)
    .filter((name) => !name.startsWith("."))
    .sort()
    .flatMap((name) => {
      const path = join(dir, name);
      return statSync(path).isDirectory() ? files(path) : [path];
    });
}

export function buildCatalog(root = PLUGINS) {
  const plugins = readdirSync(root)
    .filter((name) => !name.startsWith(".") && statSync(join(root, name)).isDirectory())
    .sort()
    .map((id) => {
      const dir = join(root, id);
      const manifest = JSON.parse(readFileSync(join(dir, "plugin.json"), "utf8"));
      if (manifest.id !== id) throw new Error(`plugins/${id}/plugin.json has id “${manifest.id}”`);
      if (!manifest.version) throw new Error(`plugins/${id}/plugin.json needs a version`);
      return {
        id,
        name: manifest.name,
        version: manifest.version,
        description: manifest.description ?? "",
        author: manifest.author ?? null,
        tags: manifest.tags ?? [],
        files: files(dir).map((path) => ({
          path: relative(dir, path).split(sep).join("/"),
          sha256: createHash("sha256").update(readFileSync(path)).digest("hex"),
        })),
      };
    });
  return `${JSON.stringify({ version: 1, plugins }, null, 2)}\n`;
}

if (process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1]) {
  const catalog = buildCatalog();
  if (process.argv.includes("--check")) {
    const current = (() => {
      try {
        return readFileSync(CATALOG, "utf8");
      } catch {
        return "";
      }
    })();
    if (current !== catalog) {
      console.error("plugins/catalog.json is out of date: run node scripts/plugin-catalog.mjs");
      process.exit(1);
    }
  } else {
    writeFileSync(CATALOG, catalog);
    console.log(`Wrote ${relative(process.cwd(), CATALOG)}`);
  }
}
