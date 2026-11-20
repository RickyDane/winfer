#!/usr/bin/env node
/**
 * Builds src/features/settings/searchIndex.generated.json: every setting's label and the text
 * that explains it (hints, card descriptions), read from the settings components, so the search
 * finds what's on the pages ("local", "host", "API key") and not only a hand-picked list.
 * The Tauri search (search.ts) and the native one (ui/settings.rs) both read it.
 *
 *   node scripts/gen-settings-index.mjs          write the file
 *   node scripts/gen-settings-index.mjs --check  fail if the file is out of date
 */
import { readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const SETTINGS = "src/features/settings/SettingsModal.tsx";
const FILES = [SETTINGS, "src/features/accounts/AccountsSettings.tsx", "src/features/usage/UsagePage.tsx"];

/** The page each top-level settings component is on (see GROUPS in SettingsModal.tsx). */
const PAGES = {
  general: ["TerminalSettings", "EditorSettings", "ShortcutSettings"],
  theme: ["AppearanceSettings"],
  agents: ["AgentSettings", "ReviewSettings", "ProgressSettings"],
  "winfer-code": ["HarnessSettings"],
  accounts: ["AccountsSettings"],
  ai: ["AiSettings"],
  voice: ["VoiceSettings"],
  plugins: ["PluginSettings"],
  skills: ["SkillSettings"],
  usage: ["UsageSettings", "PaneCostSettings"],
  mobile: ["MobileSettings"],
  data: ["DataSettings"],
  updates: ["UpdateSettings"],
  diagnostics: ["DiagnosticsSettings"],
};

/** Components rendered through the `<Row>`/`<Card>`/… helpers; their props name a setting. */
const TAGS = ["Row", "Card", "Toggle", "Field", "Segmented", "Select", "Checkbox", "NumberField", "TextField"];
const LABEL_PROPS = ["label", "title"];
const TEXT_PROPS = ["hint", "description", "placeholder", "aria-label", "tooltip"];

const sources = FILES.map((f) => readFileSync(join(root, f), "utf8"));

/** Every top-level `function Name(` in the files, with its source. */
const functions = new Map();
for (const src of sources) {
  const starts = [...src.matchAll(/^(?:export )?function ([A-Z][A-Za-z0-9]*)\(/gm)];
  starts.forEach((m, i) => functions.set(m[1], src.slice(m.index, starts[i + 1]?.index ?? src.length)));
}

/** The text inside a JSX attribute value or element: string literals and plain words, no code. */
function plainText(code) {
  const parts = [];
  for (const m of code.matchAll(/"((?:[^"\\]|\\.)*)"|`((?:[^`\\$]|\\.)*)`|'((?:[^'\\]|\\.)*)'|>([^<>{}]+)</g)) {
    const t = (m[1] ?? m[2] ?? m[3] ?? m[4] ?? "").replace(/\s+/g, " ").trim();
    if (t.length > 1 && /[A-Za-z]/.test(t)) parts.push(t);
  }
  return parts.join(" ");
}

/** The end of the opening tag that starts at `from` (after `<Name`), skipping `{…}` and strings. */
function tagEnd(src, from) {
  let depth = 0;
  for (let i = from; i < src.length; i++) {
    const c = src[i];
    if (c === '"' || c === "'" || c === "`") {
      for (i++; i < src.length && src[i] !== c; i++) if (src[i] === "\\") i++;
    } else if (c === "{") depth++;
    else if (c === "}") depth--;
    else if (c === ">" && depth === 0 && src[i - 1] !== "=") return i;
  }
  return src.length;
}

/** The value of prop `name` in an opening tag: a string, or the text inside `{…}`. */
function prop(tag, name) {
  const m = new RegExp(`(?:^|\\s)${name.replace("-", "\\-")}=`).exec(tag);
  if (!m) return null;
  const at = m.index + m[0].length;
  if (tag[at] === '"') return tag.slice(at + 1, tag.indexOf('"', at + 1));
  if (tag[at] !== "{") return null;
  let depth = 0;
  for (let i = at; i < tag.length; i++) {
    if (tag[i] === "{") depth++;
    else if (tag[i] === "}" && --depth === 0) return plainText(tag.slice(at + 1, i));
  }
  return null;
}

/** The settings found in one component: its Row/Card/Toggle/… tags. */
function entriesOf(source) {
  const out = [];
  for (const m of source.matchAll(new RegExp(`<(${TAGS.join("|")})(?=[\\s>])`, "g"))) {
    const end = tagEnd(source, m.index + m[0].length);
    const tag = source.slice(m.index + m[0].length, end);
    const label = LABEL_PROPS.map((p) => prop(tag, p)).find((v) => v && v.length > 1 && /[A-Za-z]/.test(v));
    if (!label) continue;
    const words = TEXT_PROPS.map((p) => prop(tag, p)).filter(Boolean).join(" ");
    out.push({ label: label.replace(/\s+/g, " ").trim(), words });
  }
  return out;
}

/** The component and the ones it renders (`<Name`), found in the same files. */
function closure(name, seen = new Set()) {
  if (seen.has(name) || !functions.has(name)) return seen;
  seen.add(name);
  for (const m of functions.get(name).matchAll(/<([A-Z][A-Za-z0-9]*)[\s/>]/g)) closure(m[1], seen);
  return seen;
}

const entries = [];
const claimed = new Set();
for (const [page, tops] of Object.entries(PAGES)) {
  const names = new Set(tops.flatMap((t) => [...closure(t)]));
  for (const name of names) {
    if (claimed.has(name)) continue;
    claimed.add(name);
    for (const e of entriesOf(functions.get(name))) entries.push({ page, ...e });
  }
}

// Same label on the same page twice: keep one, with the words of both.
const merged = new Map();
for (const e of entries) {
  const key = `${e.page}\0${e.label}`;
  const have = merged.get(key);
  if (have) have.words = [...new Set(`${have.words} ${e.words}`.trim().split(" "))].join(" ");
  else merged.set(key, { ...e });
}
const json = JSON.stringify([...merged.values()], null, 1) + "\n";
const target = join(root, "src/features/settings/searchIndex.generated.json");

if (process.argv.includes("--check")) {
  if (readFileSync(target, "utf8") !== json) {
    console.error("searchIndex.generated.json is out of date: run `node scripts/gen-settings-index.mjs`");
    process.exit(1);
  }
} else {
  writeFileSync(target, json);
  console.log(`${merged.size} settings indexed`);
}
