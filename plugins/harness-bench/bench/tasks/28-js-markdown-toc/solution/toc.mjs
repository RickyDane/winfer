#!/usr/bin/env node
// Inserts or updates a table of contents between <!-- toc --> and <!-- tocstop -->.
import { readFileSync, writeFileSync } from "node:fs";

const START = "<!-- toc -->";
const END = "<!-- tocstop -->";

function headings(lines) {
  const out = [];
  let fence = null; // { ch, len }
  for (const line of lines) {
    if (fence) {
      const m = line.match(/^ {0,3}(`{3,}|~{3,})\s*$/);
      if (m && m[1][0] === fence.ch && m[1].length >= fence.len) fence = null;
      continue;
    }
    const f = line.match(/^ {0,3}(`{3,}|~{3,})/);
    if (f) {
      fence = { ch: f[1][0], len: f[1].length };
      continue;
    }
    const h = line.match(/^(#{1,6})[ \t](.*)$/);
    if (!h) continue;
    let text = h[2].trim().replace(/(^|[ \t]+)#+$/, "").trim();
    out.push({ level: h[1].length, text });
  }
  return out;
}

function slugger() {
  const seen = new Map();
  return (text) => {
    const base = text.toLowerCase().replace(/[^\p{L}\p{N}\p{M} _-]/gu, "").replace(/ /g, "-");
    const n = seen.get(base) ?? 0;
    seen.set(base, n + 1);
    return n ? `${base}-${n}` : base;
  };
}

const file = process.argv[2];
if (!file) {
  console.error("usage: node toc.mjs FILE.md");
  process.exit(1);
}
const src = readFileSync(file, "utf8");
const lines = src.split("\n");
const start = lines.indexOf(START);
const end = start < 0 ? -1 : lines.indexOf(END, start + 1);
if (start < 0 || end < 0) {
  console.error(`${file}: no ${START} ... ${END} markers`);
  process.exit(1);
}
const slug = slugger();
const toc = [];
for (const h of headings([...lines.slice(0, start), ...lines.slice(end + 1)])) {
  const s = slug(h.text);
  if (h.level >= 2) toc.push(`${"  ".repeat(h.level - 2)}- [${h.text}](#${s})`);
}
const out = [...lines.slice(0, start + 1), ...toc, ...lines.slice(end)].join("\n");
if (out !== src) writeFileSync(file, out);
