#!/usr/bin/env node
// Builds plugins/catalog.json, the plugin store's index for app versions up to 0.1.64: every
// plugin folder's manifest fields plus each file's SHA-256, which those versions check when
// installing. Newer versions list the plugins/ folder itself and ignore this file.
//
// A plugin's catalog version needn't match its plugin.json: the catalog keeps its own and takes
// plugin.json's only when that one is higher, so either can be bumped to publish an update.
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
    .filter((name) => !name.startsWith(".") && name !== "__pycache__") // Python's bytecode cache isn't shipped
    .sort()
    .flatMap((name) => {
      const path = join(dir, name);
      return statSync(path).isDirectory() ? files(path) : [path];
    });
}

/** Compares dotted versions numerically ("1.10.0" > "1.9.2"); non-numeric parts compare as text. */
export function compareVersions(a, b) {
  const pa = String(a).split(/[.+-]/);
  const pb = String(b).split(/[.+-]/);
  for (let i = 0; i < Math.max(pa.length, pb.length); i++) {
    const x = pa[i] ?? "0";
    const y = pb[i] ?? "0";
    const nx = Number(x);
    const ny = Number(y);
    const diff = Number.isNaN(nx) || Number.isNaN(ny) ? x.localeCompare(y) : nx - ny;
    if (diff) return Math.sign(diff);
  }
  return 0;
}

function previousVersions(root) {
  try {
    const catalog = JSON.parse(readFileSync(join(root, "catalog.json"), "utf8"));
    return new Map(catalog.plugins.map((p) => [p.id, p.version]));
  } catch {
    return new Map();
  }
}

export function buildCatalog(root = PLUGINS) {
  const previous = previousVersions(root);
  const plugins = readdirSync(root)
    .filter((name) => !name.startsWith(".") && statSync(join(root, name)).isDirectory())
    .sort()
    .map((id) => {
      const dir = join(root, id);
      const manifest = JSON.parse(readFileSync(join(dir, "plugin.json"), "utf8"));
      if (manifest.id !== id) throw new Error(`plugins/${id}/plugin.json has id “${manifest.id}”`);
      const versions = [previous.get(id), manifest.version].filter(Boolean);
      if (!versions.length) throw new Error(`plugins/${id} needs a version in plugin.json or catalog.json`);
      return {
        id,
        name: manifest.name,
        version: versions.reduce((a, b) => (compareVersions(b, a) > 0 ? b : a)),
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
