// winfer bench: runs bench/run.py (shipped in this plugin) on the harnesses, models and tasks
// picked on the page, in the background, and shows live progress, charts and results.

const STORE_KEY = "winfer.harness-bench";
const POLL_MS = 1500;
const USAGE_MS = 60_000; // how often a run reads the plan limits
const CHECKS = ["oracle", "noop"]; // self-tests of the checks, not harnesses

const esc = (s) => String(s ?? "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]);
const q = (s) => `'${String(s).replace(/'/g, `'\\''`)}'`; // POSIX shell quoting
const tokens = (n) => (n >= 1e6 ? `${(n / 1e6).toFixed(2)}M` : n >= 1000 ? `${(n / 1000).toFixed(1)}k` : String(Math.round(n || 0)));
const secs = (s) => (s >= 60 ? `${Math.floor(s / 60)}m ${String(Math.round(s % 60)).padStart(2, "0")}s` : `${Math.round(s)}s`);
/** Unix ms → "14:05", with the date ("17.10. 14:05") when it isn't today. */
export function clock(ms, now = Date.now()) {
  const d = new Date(ms);
  const pad = (n) => String(n).padStart(2, "0");
  const time = `${pad(d.getHours())}:${pad(d.getMinutes())}`;
  return d.toDateString() === new Date(now).toDateString() ? time : `${pad(d.getDate())}.${pad(d.getMonth() + 1)}. ${time}`;
}
const median = (xs) => {
  if (!xs.length) return 0;
  const a = [...xs].sort((x, y) => x - y);
  const m = a.length >> 1;
  return a.length % 2 ? a[m] : (a[m - 1] + a[m]) / 2;
};

function loadPrefs() {
  try {
    return JSON.parse(localStorage.getItem(STORE_KEY)) ?? {};
  } catch {
    return {};
  }
}

function savePrefs(prefs) {
  try {
    localStorage.setItem(STORE_KEY, JSON.stringify(prefs));
  } catch {}
}

/** Fill missing costs in saved runs without rewriting their logs or replacing reported costs. */
export function withEstimatedCost(r, prices = {}) {
  if (r.cost != null) return r;
  const model = r.model ?? "";
  const price = prices[model] ?? prices[model.split("/").pop()];
  if (!price || !(r.input > 0 || r.output > 0)) return r;
  const cached = Math.min(r.input || 0, Math.max(0, r.cached || 0));
  const cost = (((r.input || 0) - cached) * price.input + cached * price.cached + (r.output || 0) * price.output) / 1e6;
  return { ...r, cost, cost_estimated: true };
}

function costNote(estimated, partial) {
  const note = estimated
    ? "Includes costs estimated from standard API list prices (short context). Excludes tool fees, cache-write premiums and long-context, speed or regional surcharges. These estimates are not subscription charges."
    : "Cost reported by the harness.";
  return note + (partial ? " Some runs have no known cost; only available costs are summed." : "");
}

/** Per-run results → summary per harness+model. */
export function summarize(results, prices = {}) {
  const groups = new Map();
  for (const r of results) {
    const key = `${r.harness}|${r.model ?? ""}`;
    if (!groups.has(key)) groups.set(key, []);
    groups.get(key).push(r);
  }
  return [...groups.entries()].map(([key, all]) => {
    const [harness, model] = key.split("|");
    // Runs where the harness itself errored (rate limit, crash) say nothing about the task.
    const rs = all.filter((r) => !r.error).map((r) => withEstimatedCost(r, prices));
    return {
      errors: all.length - rs.length,
      key,
      harness,
      model,
      runs: rs.length,
      passed: rs.filter((r) => r.pass).length,
      // Harnesses whose output carries no usage report 0 tokens: that's "unknown", not "none".
      hasTokens: rs.some((r) => r.input > 0),
      seconds: median(rs.map((r) => r.seconds)),
      input: median(rs.map((r) => r.input)),
      cached: median(rs.map((r) => r.cached)),
      uncached: median(rs.map((r) => r.input - r.cached)),
      output: median(rs.map((r) => r.output)),
      tools: median(rs.map((r) => r.tools)),
      cost: rs.some((r) => r.cost != null) ? rs.reduce((a, r) => a + (r.cost || 0), 0) : null,
      costEstimated: rs.some((r) => r.cost != null && r.cost_estimated),
      costPartial: rs.some((r) => r.cost == null),
    };
  });
}

/** What the runner is doing now: a list (parallel runs) or, from older runners, one object. */
export function runningJobs(progress) {
  const r = progress?.running;
  return Array.isArray(r) ? r : r ? [r] : [];
}

/** Results from runners before error detection: a run that did nothing (no tokens, no tool
 * calls) and ended within seconds was the harness failing to start, not the task failing. */
export function withLegacyErrors(r) {
  if (r.error !== undefined || CHECKS.includes(r.harness)) return r;
  const idle = !r.pass && r.input === 0 && r.tools === 0 && r.seconds < 10 && !r.timed_out;
  return idle ? { ...r, error: "The harness exited within seconds without doing anything (likely a rate limit or crash)." } : r;
}

export function parseJsonl(text) {
  return text
    .split("\n")
    .filter((l) => l.trim().startsWith("{"))
    .map((l) => {
      try {
        return JSON.parse(l);
      } catch {
        return null;
      }
    })
    .filter(Boolean);
}

/** A results file name (`20261017-120000[-selftest].jsonl`) → "17.10. 12:00[ · self-test]". */
export function runLabel(file) {
  const m = /^(\d{4})(\d{2})(\d{2})-(\d{2})(\d{2})(\d{2})(-selftest)?/.exec(file);
  return m ? `${m[3]}.${m[2]}. ${m[4]}:${m[5]}${m[7] ? " · self-test" : ""}` : file.replace(".jsonl", "");
}

/** A task id without its number prefix: "u03-data-flow" → "data-flow", "04-question" → "question". */
export const taskName = (id) => String(id).replace(/^[a-z]?\d+-/, "");

/** The suite a result belongs to: its own field (newer runners), else its task's suite in the
 * catalog, else coding (every task was a coding task before suites). */
export function suiteOf(r, tasks = []) {
  return r.suite ?? tasks.find((t) => t.id === r.task)?.suite ?? "coding";
}

/** Suite ids that occur in `results`, in the catalog's suite order (unknown ones last). */
export function suitesIn(results, catalog) {
  const order = (catalog?.suites ?? []).map((s) => s.id);
  const found = [...new Set(results.map((r) => suiteOf(r, catalog?.tasks)))];
  const rank = (s) => (order.includes(s) ? order.indexOf(s) : order.length);
  return found.sort((a, b) => rank(a) - rank(b) || a.localeCompare(b));
}

/** Pass counts per harness+model and suite; harness errors aren't counted. */
export function suiteMatrix(results, catalog) {
  const suites = suitesIn(results, catalog);
  const rows = new Map();
  for (const r of results) {
    const key = `${r.harness}|${r.model ?? ""}`;
    if (!rows.has(key)) rows.set(key, { key, harness: r.harness, model: r.model ?? "", cells: {} });
    if (r.error) continue;
    const s = suiteOf(r, catalog?.tasks);
    const cell = (rows.get(key).cells[s] ??= { passed: 0, runs: 0 });
    cell.runs += 1;
    if (r.pass) cell.passed += 1;
  }
  return { suites, rows: [...rows.values()] };
}

/** One saved run's results → a line in the runs overview. */
export function runOverview(results, prices = {}, tasks = []) {
  const counted = results.filter((r) => !r.error).map((r) => withEstimatedCost(r, prices));
  const timed = results.filter((r) => r.started);
  const priced = counted.filter((r) => r.cost != null);
  return {
    runs: counted.length,
    passed: counted.filter((r) => r.pass).length,
    errors: results.length - counted.length,
    harnesses: [...new Set(results.map((r) => r.harness))],
    models: [...new Set(results.map((r) => r.model).filter(Boolean))],
    tasks: new Set(results.map((r) => r.task)).size,
    suites: [...new Set(results.map((r) => suiteOf(r, tasks)))],
    wall: timed.length ? Math.max(...timed.map((r) => r.started + r.seconds)) - Math.min(...timed.map((r) => r.started)) : 0,
    cost: priced.length ? priced.reduce((a, r) => a + r.cost, 0) : null,
    costEstimated: priced.some((r) => r.cost_estimated),
    costPartial: priced.length > 0 && priced.length < counted.length,
  };
}

// ---- plan usage limits over a run ----

/** Whose plan limits a harness uses (a ChatGPT sign-in shares Codex's); null when winfer doesn't track them. */
export function planOf(harness) {
  if (["codex", "winfer-codex", "winfer-chatgpt"].includes(harness)) return "codex";
  if (["claude", "winfer-claude"].includes(harness)) return "claude";
  return null;
}

const LIMIT_ERROR = /usage limit|rate.?limit|limit (?:reached|exceeded)|quota|too many requests|\b429\b|out of (?:credits|usage)/i;

/** A harness error that says the plan ran out (not the old runners' "likely a rate limit or crash" guess). */
export const isLimitError = (error) => !!error && LIMIT_ERROR.test(error) && !/likely a rate limit or crash/.test(error);

/**
 * Adds one `api.usage()` reading to a run's usage record (`{ plans: { [provider]: { [kind]: window } } }`).
 * Per window: % used at the first and latest reading, when it was first seen at 100% (`outAt`) and the
 * last reading before that (`okAt`, so it ran out in between), and whether it reset during the run.
 * Readings older than the latest kept are ignored (a failed refresh returns the cached one).
 */
export function trackUsage(record, snapshots) {
  const plans = { ...(record?.plans ?? {}) };
  for (const s of snapshots ?? []) {
    const windows = { ...(plans[s.provider] ?? {}) };
    for (const w of s.windows ?? []) {
      const prev = windows[w.kind];
      const used = w.usedPercent;
      const out = used >= 100;
      if (!prev) {
        windows[w.kind] = { label: w.label, start: used, end: used, startAt: s.at, endAt: s.at, resetsAt: w.resetsAt, outAt: out ? s.at : null, okAt: null, outAtStart: out, reset: false };
        continue;
      }
      if (s.at <= prev.endAt) continue;
      const reset = prev.reset || (prev.resetsAt != null && s.at >= prev.resetsAt);
      const outAt = prev.outAt ?? (out ? s.at : null);
      windows[w.kind] = { ...prev, label: w.label, end: used, endAt: s.at, resetsAt: w.resetsAt, outAt, okAt: prev.outAt == null && out ? prev.endAt : prev.okAt, reset };
    }
    plans[s.provider] = windows;
  }
  return { ...(record ?? {}), plans };
}

/**
 * Rows for the usage panel: one per plan window, plus when a harness first failed on a limit
 * (from the results, ms), which can be earlier than the next usage reading.
 */
export function usageRows(record, results = []) {
  const firstLimitError = {};
  for (const r of results) {
    const plan = planOf(r.harness);
    if (!plan || !isLimitError(r.error) || !r.started) continue;
    const at = (r.started + (r.seconds || 0)) * 1000;
    if (!(firstLimitError[plan] <= at)) firstLimitError[plan] = at;
  }
  const rows = [];
  for (const [provider, windows] of Object.entries(record?.plans ?? {})) {
    for (const [kind, w] of Object.entries(windows)) rows.push({ provider, kind, ...w, limitErrorAt: firstLimitError[provider] ?? null });
  }
  for (const [provider, at] of Object.entries(firstLimitError)) {
    if (!record?.plans?.[provider]) rows.push({ provider, kind: null, label: null, limitErrorAt: at });
  }
  return rows;
}

/** What the overall score weighs. Lower is better for all but the pass rate. */
export const METRICS = [
  { id: "pass", label: "Pass rate", short: "pass" },
  { id: "uncached", label: "Full-price tokens", short: "cost" },
  { id: "seconds", label: "Time", short: "speed" },
  { id: "input", label: "Total input tokens", short: "volume" },
  { id: "tools", label: "Tool calls", short: "steps" },
];

/** Weight presets (relative; they're normalized). */
export const PRESETS = {
  balanced: { label: "Balanced", weights: { pass: 50, uncached: 20, seconds: 20, input: 5, tools: 5 } },
  cheap: { label: "Cheapest", weights: { pass: 40, uncached: 40, seconds: 10, input: 5, tools: 5 } },
  fast: { label: "Fastest", weights: { pass: 40, uncached: 10, seconds: 45, input: 0, tools: 5 } },
  quality: { label: "Quality only", weights: { pass: 100, uncached: 0, seconds: 0, input: 0, tools: 0 } },
};

/**
 * One overall score (0-100) per harness from its summary row. Each metric scores 0..1: the pass
 * rate as is, the others relative to the best harness (best / value, so half as good scores 0.5).
 * A harness without token counts scores 0 on token metrics (unknown can't win). Ranked best first;
 * `parts` is each metric's weighted contribution, for the breakdown.
 */
export function scoreRows(rows, weights) {
  const counted = rows.filter((r) => r.runs > 0);
  const total = METRICS.reduce((a, m) => a + Math.max(0, weights[m.id] ?? 0), 0) || 1;
  const tokenMetric = (id) => id === "uncached" || id === "input";
  const best = {};
  for (const m of METRICS) {
    if (m.id === "pass") continue;
    const vals = counted.filter((r) => !tokenMetric(m.id) || r.hasTokens).map((r) => r[m.id]);
    best[m.id] = vals.length ? Math.min(...vals) : 0;
  }
  return counted
    .map((r) => {
      const parts = {};
      for (const m of METRICS) {
        const w = Math.max(0, weights[m.id] ?? 0) / total;
        let v;
        if (m.id === "pass") v = r.runs ? r.passed / r.runs : 0;
        else if (tokenMetric(m.id) && !r.hasTokens) v = 0;
        else v = (best[m.id] + 1) / (r[m.id] + 1); // +1 keeps a 0 from dividing
        parts[m.id] = w * Math.min(1, v) * 100;
      }
      return { ...r, parts, score: Object.values(parts).reduce((a, b) => a + b, 0) };
    })
    .sort((a, b) => b.score - a.score || b.passed / b.runs - a.passed / a.runs);
}

/** A "nice" axis maximum and tick step for values up to `max`. */
export function niceScale(max, ticks = 4) {
  if (!(max > 0)) return { max: 1, step: 1 };
  const raw = max / ticks;
  const mag = 10 ** Math.floor(Math.log10(raw));
  const step = [1, 2, 2.5, 5, 10].map((m) => m * mag).find((s) => s >= raw);
  return { max: Math.ceil(max / step) * step, step };
}

// Categorical series colors (validated for CVD separation, normal-vision distance and 3:1
// contrast on winfer's dark surface; the light steps for the light theme). Assigned to
// harnesses in their fixed catalog order, so a color always means the same harness.
const SERIES = {
  dark: ["#3987e5", "#d95926", "#199e70", "#c98500", "#d55181", "#008300", "#9085e9", "#e66767"],
  light: ["#2a78d6", "#eb6834", "#1baf7a", "#eda100", "#e87ba4", "#008300", "#6250d6", "#e34948"],
};

function isLightTheme() {
  const hex = getComputedStyle(document.documentElement).getPropertyValue("--color-panel-raised").trim();
  const m = /^#?([0-9a-f]{2})([0-9a-f]{2})([0-9a-f]{2})/i.exec(hex);
  if (!m) return false;
  const [r, g, b] = m.slice(1).map((h) => parseInt(h, 16) / 255);
  return 0.2126 * r + 0.7152 * g + 0.0722 * b > 0.5;
}

const STYLE = `
.hb { --hb-surface: var(--color-panel-raised); --hb-grid: var(--color-border); --hb-axis: var(--color-border-strong);
  max-width: 1280px; margin: 0 auto; padding: 24px 28px 48px; display: flex; flex-direction: column; gap: 20px; color: var(--color-fg); }
.hb * { box-sizing: border-box; }
.hb-head { display: flex; align-items: center; gap: 12px; flex-wrap: wrap; }
.hb-title { display: flex; align-items: center; gap: 12px; flex: 1; min-width: 240px; }
.hb-logo { width: 34px; height: 34px; border-radius: 9px; display: grid; place-items: center; background: var(--color-accent-soft); color: var(--color-accent); flex: none; }
.hb-title h1 { margin: 0; font-size: 17px; font-weight: 600; }
.hb-title p { margin: 2px 0 0; font-size: 12px; color: var(--color-fg-muted); }
.hb-actions { display: flex; align-items: center; gap: 8px; flex-wrap: wrap; }
.hb-body { display: grid; grid-template-columns: 300px minmax(0, 1fr); gap: 20px; align-items: start; }
@media (max-width: 980px) { .hb-body { grid-template-columns: 1fr; } }
.hb-aside { display: flex; flex-direction: column; gap: 14px; position: sticky; top: 0; }
.hb-main { display: flex; flex-direction: column; gap: 16px; min-width: 0; }
.hb-panel { border: 1px solid var(--color-border); border-radius: 10px; background: var(--hb-surface); padding: 14px 16px; }
.hb-panel h2 { margin: 0 0 2px; font-size: 13px; font-weight: 600; display: flex; align-items: center; gap: 8px; }
.hb-panel .hb-sub { margin: 0 0 12px; font-size: 11.5px; color: var(--color-fg-muted); line-height: 1.45; }
.hb-label { font-size: 11px; font-weight: 600; letter-spacing: .04em; text-transform: uppercase; color: var(--color-fg-subtle); margin: 0 0 8px; }
.hb-harness { display: grid; grid-template-columns: 10px minmax(0, 1fr) auto; gap: 4px 10px; align-items: center; padding: 8px 0; border-top: 1px solid var(--color-border); }
.hb-harness:first-of-type { border-top: none; }
.hb-harness.off .hb-swatch { opacity: .25; }
.hb-harness.missing { opacity: .45; }
.hb-swatch { width: 10px; height: 10px; border-radius: 3px; }
.hb-hname { font-size: 12.5px; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.hb-hmodel { grid-column: 2 / 4; display: flex; gap: 6px; align-items: center; }
.hb-hmodel input { width: 100%; height: 26px; font-size: 11.5px; }
.hb-chips { display: flex; flex-wrap: wrap; gap: 6px; max-height: 400px; overflow-y: auto; }
.hb-tasks-head { display: flex; align-items: baseline; justify-content: space-between; gap: 8px; }
.hb-tasks-all { display: flex; gap: 2px; margin-bottom: 8px; }
.hb-suite + .hb-suite { margin-top: 12px; padding-top: 10px; border-top: 1px solid var(--color-border); }
.hb-suite-head { display: flex; align-items: center; gap: 6px; margin-bottom: 6px; }
.hb-suite-head .hb-tasks-all { margin: 0 0 0 auto; }
.hb-suite-name { font-size: 12px; font-weight: 600; color: var(--color-fg); cursor: help; }
.hb-suite-count { font-size: 11px; color: var(--color-fg-muted); font-variant-numeric: tabular-nums; }
.hb-suitebar { margin-bottom: 14px; flex-wrap: wrap; }
.hb-grid-table tr.hb-grid-suite td { font-size: 11px; font-weight: 600; text-transform: uppercase; letter-spacing: .04em; color: var(--color-fg-muted); padding: 12px 0 4px; }
.hb-suite-table td .hb-pass { justify-content: flex-start; }
.hb-tasks-all button { border: 0; background: transparent; color: var(--color-fg-muted); font: inherit; font-size: 11px; padding: 2px 6px; border-radius: 5px; cursor: pointer; }
.hb-tasks-all button:hover { background: var(--color-active); color: var(--color-fg); }
.hb-chip { border: 1px solid var(--color-border-strong); background: transparent; color: var(--color-fg-muted); border-radius: 999px; padding: 4px 10px; font-size: 11.5px; cursor: pointer; font-family: inherit; }
.hb-chip[aria-pressed="true"] { background: var(--color-accent-soft); border-color: var(--color-accent); color: var(--color-fg); }
.hb-seg { display: flex; border: 1px solid var(--color-border-strong); border-radius: 7px; padding: 2px; gap: 2px; }
.hb-seg button { flex: 1; border: 0; background: transparent; color: var(--color-fg-muted); font: inherit; font-size: 11.5px; padding: 5px 4px; border-radius: 5px; cursor: pointer; }
.hb-seg button[aria-pressed="true"] { background: var(--color-active); color: var(--color-fg); }
.hb-opts { display: grid; grid-template-columns: 1fr 1fr; gap: 10px; }
.hb-opts label { display: flex; flex-direction: column; gap: 4px; font-size: 11.5px; color: var(--color-fg-muted); }
.hb-opts input[type=number] { width: 100%; }
.hb-kpis { display: grid; grid-template-columns: repeat(auto-fit, minmax(150px, 1fr)); gap: 12px; }
.hb-kpi { border: 1px solid var(--color-border); border-radius: 10px; background: var(--hb-surface); padding: 12px 14px; display: flex; flex-direction: column; gap: 4px; min-width: 0; }
.hb-kpi .k { font-size: 11.5px; color: var(--color-fg-muted); }
.hb-kpi .v { font-size: 22px; font-weight: 600; letter-spacing: -.01em; }
.hb-kpi .s { font-size: 11.5px; color: var(--color-fg-muted); display: flex; align-items: center; gap: 6px; min-width: 0; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
.hb-dot { width: 8px; height: 8px; border-radius: 50%; flex: none; }
.hb-meter { height: 4px; border-radius: 2px; background: var(--color-border); overflow: hidden; margin-top: 4px; }
.hb-meter > div { height: 100%; border-radius: 2px; }
.hb-charts { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 16px; }
.hb-charts.one { grid-template-columns: 1fr; }
.hb-wide { grid-column: 1 / -1; }
.hb-legend { display: flex; flex-wrap: wrap; gap: 6px 14px; font-size: 11.5px; color: var(--color-fg-muted); margin: 0 0 10px; }
.hb-legend span { display: inline-flex; align-items: center; gap: 6px; }
.hb-key { width: 12px; height: 8px; border-radius: 2px; display: inline-block; }
.hb svg { display: block; max-width: 100%; height: auto; overflow: visible; font-family: inherit; }
.hb svg text { fill: var(--color-fg-muted); font-size: 11px; }
.hb svg .t-strong { fill: var(--color-fg); }
.hb svg .grid { stroke: var(--hb-grid); stroke-width: 1; }
.hb svg .axis { stroke: var(--hb-axis); stroke-width: 1; }
.hb svg [data-tip] { cursor: default; }
.hb svg .hit { fill: transparent; }
.hb-grid-table { width: 100%; border-collapse: separate; border-spacing: 4px; font-size: 12px; }
.hb-grid-table th { text-align: left; font-weight: 500; color: var(--color-fg-muted); padding: 2px 6px; font-size: 11.5px; white-space: nowrap; }
.hb-grid-table td.task { font-family: var(--font-mono, ui-monospace, monospace); font-size: 11.5px; color: var(--color-fg-muted); white-space: nowrap; padding-right: 10px; }
.hb-cell { border-radius: 6px; padding: 7px 9px; background: var(--color-panel); border: 1px solid var(--color-border); white-space: nowrap; }
.hb-cell .st { display: inline-flex; align-items: center; gap: 5px; font-weight: 600; font-size: 11.5px; }
.hb-cell .m { color: var(--color-fg-muted); font-size: 11px; margin-top: 2px; }
.hb-cell.pass .st { color: var(--color-success); }
.hb-cell.fail { cursor: help; border-color: color-mix(in srgb, var(--color-danger) 45%, var(--color-border)); }
.hb-cell.fail .st { color: var(--color-danger); }
.hb-cell.busy { border-style: dashed; }
.hb-cell.error { cursor: help; border-style: dashed; border-color: color-mix(in srgb, var(--color-warning) 55%, var(--color-border)); }
.hb-cell.error .st { color: var(--color-warning); }
.hb-cell.has-error { cursor: help; }
.hb-cell.queued { opacity: .5; }
.hb-why { position: fixed; z-index: 1000; display: none; max-width: min(520px, calc(100vw - 16px)); max-height: min(320px, calc(100vh - 16px)); color: var(--color-fg); background: var(--color-panel-raised); border-color: var(--color-border-strong); box-shadow: 0 8px 24px rgba(0,0,0,.35); white-space: pre-wrap; word-break: break-word; font-family: var(--font-mono, ui-monospace, monospace); font-size: 11px; background: var(--color-panel); border: 1px solid var(--color-border); border-radius: 8px; padding: 10px 12px; overflow: auto; }
.hb-why.on { display: block; }
.hb-table { width: 100%; border-collapse: collapse; font-size: 12px; font-variant-numeric: tabular-nums; }
.hb-table th { text-align: right; font-weight: 500; color: var(--color-fg-muted); padding: 6px 8px; border-bottom: 1px solid var(--color-border); white-space: nowrap; }
.hb-table td { text-align: right; padding: 7px 8px; border-bottom: 1px solid var(--color-border); white-space: nowrap; }
.hb-table th:first-child, .hb-table td:first-child, .hb-table th:nth-child(2), .hb-table td:nth-child(2) { text-align: left; }
.hb-table tr:last-child td { border-bottom: none; }
.hb-best { color: var(--color-success); font-weight: 600; }
.hb-live { display: flex; align-items: center; gap: 14px; }
.hb-ring { flex: none; }
.hb-ring circle { fill: none; stroke-width: 5; }
.hb-pulse { animation: hb-pulse 1.4s ease-in-out infinite; }
@keyframes hb-pulse { 50% { opacity: .45; } }
.hb-empty { border: 1px dashed var(--color-border-strong); border-radius: 12px; padding: 36px 24px; text-align: center; color: var(--color-fg-muted); display: flex; flex-direction: column; align-items: center; gap: 10px; }
.hb-empty h3 { margin: 0; font-size: 14px; color: var(--color-fg); font-weight: 600; }
.hb-empty p { margin: 0; font-size: 12px; max-width: 420px; line-height: 1.5; }
.hb-winner { display: grid; grid-template-columns: minmax(0, 1fr); gap: 14px; }
.hb-crown { display: flex; align-items: center; gap: 14px; }
.hb-trophy { width: 44px; height: 44px; border-radius: 12px; display: grid; place-items: center; flex: none; background: color-mix(in srgb, var(--color-warning) 18%, transparent); color: var(--color-warning); }
.hb-crown .name { font-size: 16px; font-weight: 600; display: flex; align-items: center; gap: 8px; }
.hb-crown .why { font-size: 12px; color: var(--color-fg-muted); margin-top: 2px; }
.hb-score { margin-left: auto; text-align: right; }
.hb-score .n { font-size: 28px; font-weight: 600; letter-spacing: -.02em; line-height: 1; }
.hb-score .of { font-size: 11px; color: var(--color-fg-muted); }
.hb-rank { display: grid; grid-template-columns: 22px minmax(90px, 160px) minmax(0, 1fr) 44px; gap: 8px 10px; align-items: center; font-size: 12px; }
.hb-rank .pos { color: var(--color-fg-subtle); font-variant-numeric: tabular-nums; text-align: right; }
.hb-rank .who { display: flex; align-items: center; gap: 6px; min-width: 0; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
.hb-rank .val { text-align: right; font-variant-numeric: tabular-nums; font-weight: 600; }
.hb-stack { display: flex; gap: 2px; height: 12px; border-radius: 4px; overflow: hidden; background: var(--color-border); }
.hb-stack > span { height: 100%; }
.hb-weights { display: grid; grid-template-columns: repeat(auto-fit, minmax(150px, 1fr)); gap: 10px 16px; margin-top: 4px; }
.hb-weights label { display: flex; flex-direction: column; gap: 4px; font-size: 11.5px; color: var(--color-fg-muted); }
.hb-weights label span { display: flex; justify-content: space-between; }
.hb-weights input[type=range] { width: 100%; accent-color: var(--color-accent); }
.hb-tip { position: fixed; z-index: 1000; pointer-events: none; background: var(--color-panel-raised); border: 1px solid var(--color-border-strong); border-radius: 8px; padding: 8px 10px; font-size: 11.5px; line-height: 1.5; color: var(--color-fg); box-shadow: 0 8px 24px rgba(0,0,0,.35); max-width: 280px; white-space: pre-line; opacity: 0; transition: opacity .08s; }
.hb-tip.on { opacity: 1; }
.hb details summary { cursor: pointer; font-size: 12px; color: var(--color-fg-muted); }
.hb-tabs { display: flex; gap: 2px; border-bottom: 1px solid var(--color-border); margin-top: -6px; }
.hb-tabs button { border: 0; background: transparent; color: var(--color-fg-muted); font: inherit; font-size: 12.5px; padding: 8px 12px; cursor: pointer; border-bottom: 2px solid transparent; margin-bottom: -1px; display: inline-flex; align-items: center; gap: 6px; }
.hb-tabs button:hover { color: var(--color-fg); }
.hb-tabs button[aria-selected="true"] { color: var(--color-fg); border-bottom-color: var(--color-accent); font-weight: 600; }
.hb-count { font-size: 10.5px; font-weight: 500; padding: 1px 6px; border-radius: 999px; background: var(--color-active); color: var(--color-fg-muted); }
.hb-runs-bar { display: flex; align-items: center; gap: 10px; margin-bottom: 10px; flex-wrap: wrap; }
.hb-runs-bar .hb-sub { margin: 0; flex: 1; }
.hb-runs { width: 100%; border-collapse: separate; border-spacing: 0 2px; font-size: 12px; font-variant-numeric: tabular-nums; }
.hb-runs th { text-align: left; font-weight: 500; color: var(--color-fg-muted); padding: 6px 8px; border-bottom: 1px solid var(--color-border); white-space: nowrap; font-size: 11.5px; }
.hb-runs td { padding: 8px; vertical-align: middle; transition: background .1s; }
.hb-runs tbody td:first-child { border-radius: 8px 0 0 8px; }
.hb-runs tbody td:last-child { border-radius: 0 8px 8px 0; }
.hb-runs tbody tr { cursor: pointer; }
.hb-runs tbody tr:hover td { background: var(--color-active); }
.hb-runs tr.viewing td { background: color-mix(in srgb, var(--color-accent) 10%, transparent); }
.hb-runs .num { text-align: right; white-space: nowrap; }
.hb-runs .when { white-space: nowrap; font-weight: 500; }
.hb-runs .who { display: flex; flex-wrap: wrap; gap: 4px 10px; color: var(--color-fg-muted); }
.hb-runs .who span { display: inline-flex; align-items: center; gap: 5px; white-space: nowrap; }
.hb-runs .act { text-align: right; white-space: nowrap; width: 40px; }
.hb .hb-icon-btn svg { width: 14px; height: 14px; max-width: none; }
.hb-runs input[type=checkbox] { cursor: pointer; }
.hb-badge { font-size: 10.5px; padding: 1px 7px; border-radius: 999px; border: 1px solid var(--color-border-strong); color: var(--color-fg-muted); margin-left: 6px; font-weight: 500; }
.hb-badge.live { border-color: var(--color-accent); color: var(--color-accent); }
.hb-pass { display: flex; align-items: center; gap: 8px; justify-content: flex-end; }
.hb-pass .hb-meter { width: 56px; margin: 0; }
.hb-icon-btn { border: 0; background: transparent; color: var(--color-fg-muted); padding: 4px; border-radius: 6px; cursor: pointer; display: inline-grid; place-items: center; }
.hb-icon-btn:hover:not(:disabled) { background: color-mix(in srgb, var(--color-danger) 15%, transparent); color: var(--color-danger); }
.hb-icon-btn:disabled { opacity: .35; cursor: default; }
`;

const TRASH = `<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M3 6h18M8 6V4h8v2M19 6l-1 14H6L5 6M10 11v6M14 11v6"/></svg>`;

const ICON = `<svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M3 3v18h18"/><rect x="7" y="12" width="3" height="6" rx="1"/><rect x="12" y="8" width="3" height="10" rx="1"/><rect x="17" y="5" width="3" height="13" rx="1"/></svg>`;

/** The plugin's folder. winfer before 0.1.73 has no `api.dir`: use its plugins folder (the app
 * data folder from Rust's `dirs::data_dir`). */
async function pluginDir(api) {
  if (api.dir) return api.dir;
  const r = await api.vault.exec({ command: 'printf %s "$HOME"', cwd: "", keys: [], timeoutSec: 10 });
  const home = r.output.trim();
  const base = /mac/i.test(navigator.platform) ? `${home}/Library/Application Support` : `${home}/.local/share`;
  return `${base}/dev.winfer.app/plugins/${api.id}`;
}

export async function activate(api) {
  const dir = await pluginDir(api);
  const bench = `${dir}/bench`;
  // Results live outside the plugin folder so updates don't delete them.
  const data = `${dir.replace(/[\\/]plugins[\\/][^\\/]+[\\/]?$/, "")}/harness-bench`;
  const status = `${data}/status.json`;
  const windows = /win/i.test(navigator.platform);

  let state = {
    catalog: null, // { harnesses, tasks }
    error: null,
    prefs: loadPrefs(),
    running: null, // { pid, log, plan }
    progress: null,
    results: [],
    history: [],
    viewing: null, // log path shown
    starting: false,
    tab: "runs", // or "results"
    saved: {}, // file name → its results, for the runs tab
    selected: new Set(), // file names ticked in the runs tab
    suite: "all", // suite the results tab is narrowed to
    usage: null, // plan limits over the viewed run (see trackUsage)
  };
  let root = null;
  let tip = null;
  let pop = null; // hover popup with a task cell's failure reason or harness error
  let popFor = null; // the cell it's showing
  let popHide = null;
  let timer = null;
  let usageAt = 0; // last usage reading during a run (ms)
  let usageBusy = null;

  const sh = async (command, timeoutSec = 60) => {
    const r = await api.vault.exec({ command, cwd: bench, keys: [], timeoutSec });
    if (r.code !== 0) throw new Error(r.output.trim() || `exit ${r.code}`);
    return r.output;
  };

  const usageFile = (log) => log.replace(/\.jsonl$/, ".usage.json");
  const withTimeout = (p, ms) => Promise.race([p, new Promise((_, no) => setTimeout(() => no(new Error("timeout")), ms))]);

  /** Reads the plan limits the run's harnesses use (none for winfer before `api.usage`). */
  async function readUsage(providers) {
    if (!api.usage || !providers.length) return [];
    const all = await withTimeout(api.usage(), 15000).catch(() => []);
    return all.filter((u) => providers.includes(u.provider));
  }

  /** Adds a usage reading to the running run's record and saves it next to its results. */
  function sampleUsage(final = false) {
    const run = state.running;
    if (!run?.usage?.providers?.length) return Promise.resolve();
    if (usageBusy && !final) return usageBusy;
    usageAt = Date.now();
    const go = (usageBusy ?? Promise.resolve()).then(async () => {
      const rec = trackUsage(run.usage, await readUsage(run.usage.providers));
      if (final) rec.endedAt = Date.now();
      run.usage = rec;
      if (state.viewing === run.log) state.usage = rec;
      await api.writeFile(usageFile(run.log), JSON.stringify(rec)).catch(() => {});
    });
    usageBusy = go.finally(() => {
      if (usageBusy === go) usageBusy = null;
    });
    return go;
  }

  const prefs = () => state.prefs;
  const setPrefs = (patch) => {
    state.prefs = { ...state.prefs, ...patch };
    savePrefs(state.prefs);
  };
  const harnessPref = (h) => ({ on: ["winfer-chatgpt", "codex"].includes(h.id), model: h.model, ...(prefs().harnesses?.[h.id] ?? {}) });
  const taskOn = (t) => prefs().tasks?.[t.id] ?? true;
  const harnesses = () => (state.catalog?.harnesses ?? []).filter((h) => !CHECKS.includes(h.id));
  const labelOf = (id) => state.catalog?.harnesses.find((h) => h.id === id)?.label ?? id;
  const suiteLabel = (id) => state.catalog?.suites?.find((s) => s.id === id)?.label ?? id.charAt(0).toUpperCase() + id.slice(1);
  const catalogSuites = () => state.catalog?.suites ?? [{ id: "coding", label: "Coding", description: "" }];
  const taskSuite = (t) => t.suite ?? "coding";
  /** The results the results tab shows: every run, or one suite's. */
  const shown = () => (state.suite === "all" ? state.results : state.results.filter((r) => suiteOf(r, state.catalog?.tasks) === state.suite));
  /** The label for chart axes, where space is short. */
  const shortLabel = (id) => labelOf(id).replace(/^winfer code · /, "winfer · ");
  const colorOf = (id) => {
    const i = harnesses().findIndex((h) => h.id === id);
    const set = isLightTheme() ? SERIES.light : SERIES.dark;
    return i >= 0 && i < set.length ? set[i] : "var(--color-fg-subtle)";
  };

  async function loadCatalog() {
    try {
      // Through a file: exec output keeps only its last 20,000 characters, and the task
      // prompts alone are more than that.
      const file = `${data}/catalog.json`;
      await sh(`mkdir -p ${q(data)} && python3 run.py --list > ${q(file)}`);
      state.catalog = JSON.parse(await api.readFile(file));
      state.error = null;
    } catch (e) {
      state.error = `Couldn't read the bench: ${e.message}. It needs python3.`;
    }
    render();
  }

  async function loadHistory() {
    try {
      const out = await sh(`mkdir -p ${q(data)} && ls -1t ${q(data)} | grep '\\.jsonl$' || true`);
      state.history = out.split("\n").map((l) => l.trim()).filter(Boolean);
    } catch {
      state.history = [];
    }
    for (const f of state.selected) if (!state.history.includes(f)) state.selected.delete(f);
    if (!state.viewing && state.history[0]) await view(`${data}/${state.history[0]}`);
    render();
    loadOverviews();
  }

  /** Summaries for the runs tab; finished runs don't change, so each file is read once. */
  async function loadOverviews() {
    const live = state.running?.log.split(/[\\/]/).pop();
    const todo = state.history.filter((f) => !(f in state.saved) && f !== live);
    if (!todo.length) return;
    await Promise.all(
      todo.map(async (f) => {
        try {
          state.saved[f] = parseJsonl(await api.readFile(`${data}/${f}`)).map(withLegacyErrors);
        } catch {
          state.saved[f] = null;
        }
      }),
    );
    render();
  }

  /** Deletes saved runs (results and runner output) after asking; never the one still running. */
  async function deleteRuns(files) {
    const live = state.running?.log.split(/[\\/]/).pop();
    files = files.filter((f) => f !== live && state.history.includes(f));
    if (!files.length) return;
    const message =
      files.length === 1
        ? `Delete the run from ${runLabel(files[0])}? Its results can't be restored.`
        : `Delete ${files.length} runs? Their results can't be restored.`;
    const ask = api.confirm ? api.confirm(message, { title: "Delete runs", okLabel: "Delete", kind: "warning" }) : Promise.resolve(window.confirm(message));
    if (!(await ask)) return;
    const paths = files.flatMap((f) => [`${data}/${f}`, `${data}/${f.replace(/\.jsonl$/, ".out")}`, usageFile(`${data}/${f}`)]);
    try {
      await sh(`rm -f -- ${paths.map(q).join(" ")}`);
    } catch (e) {
      return void api.toast(`Couldn't delete: ${e.message}`, "error");
    }
    for (const f of files) {
      delete state.saved[f];
      state.selected.delete(f);
    }
    if (state.viewing && files.some((f) => state.viewing.endsWith(f))) {
      state.viewing = null;
      state.results = [];
      state.usage = null;
    }
    api.toast(files.length === 1 ? "Run deleted" : `${files.length} runs deleted`, "success");
    await loadHistory();
  }

  async function view(log) {
    state.viewing = log;
    state.suite = "all";
    try {
      state.results = parseJsonl(await api.readFile(log)).map(withLegacyErrors);
    } catch {
      state.results = [];
    }
    state.usage = state.running?.log === log ? state.running.usage : await api.readFile(usageFile(log)).then(JSON.parse).catch(() => null);
    render();
  }

  /** Starts a run; `selfTest` runs the reference solutions and the no-op on every task instead. */
  async function start(selfTest = false) {
    const c = state.catalog;
    if (!c || state.starting || state.running) return;
    const hs = selfTest ? c.harnesses.filter((h) => CHECKS.includes(h.id)) : harnesses().filter((h) => harnessPref(h).on && h.installed);
    const ts = selfTest ? c.tasks : c.tasks.filter(taskOn);
    if (!hs.length || !ts.length) return api.toast("Pick at least one installed harness and one task.", "error");
    const runs = selfTest ? 1 : Math.max(1, Math.min(20, Number(prefs().runs) || 1));
    const timeout = Math.max(60, Number(prefs().timeout) || 900);
    const parallel = selfTest ? "all" : ["harness", "all", "none"].includes(prefs().parallel) ? prefs().parallel : "harness";
    const stamp = new Date().toISOString().replace(/[-:]/g, "").replace("T", "-").slice(0, 15);
    const log = `${data}/${stamp}${selfTest ? "-selftest" : ""}.jsonl`;
    // Without `api.appExe` (winfer before 0.1.73) the runner finds winfer itself.
    const exe = api.appExe ? await api.appExe().catch(() => "") : "";
    const model = (h) => harnessPref(h).model || h.model;
    const models = hs.filter((h) => h.model).map((h) => `-m ${q(`${h.id}=${model(h)}`)}`).join(" ");
    // Only the runner goes to the background, with its own output: a background job holding
    // vault.exec's output pipe would keep the call (and the button) waiting for the whole run.
    const cmd =
      `mkdir -p ${q(data)} && rm -f ${q(status)} && { ${exe ? `WINFER_BIN=${q(exe)} ` : ""}nohup python3 run.py ` +
      `-H ${q(hs.map((h) => h.id).join(","))} -t ${q(ts.map((t) => t.id).join(","))} -n ${runs} --timeout ${timeout} -p ${q(parallel)} ${models} ` +
      `${prefs().keep && !selfTest ? "--keep " : ""}--log ${q(log)} --status ${q(status)} > ${q(log.replace(/\.jsonl$/, ".out"))} 2>&1 < /dev/null & } ; echo $!`;
    state.starting = true;
    render();
    // The limits before the first run starts, for the harnesses' plans.
    const providers = [...new Set(hs.map((h) => planOf(h.id)).filter(Boolean))];
    const usage = providers.length ? { ...trackUsage(null, await readUsage(providers)), providers, startedAt: Date.now() } : null;
    try {
      const pid = Number((await sh(cmd, 20)).trim().split("\n").pop());
      if (!pid) throw new Error("the runner didn't start");
      state.running = { pid, log, usage, started: Date.now() / 1000, plan: { cols: hs.map((h) => `${h.id}|${h.model ? model(h) : ""}`), tasks: ts.map((t) => t.id) } };
      state.progress = { total: hs.length * ts.length * runs, done: 0 };
      state.results = [];
      state.usage = usage;
      usageAt = Date.now();
      if (usage) api.writeFile(usageFile(log), JSON.stringify(usage)).catch(() => {});
      state.viewing = log;
      state.suite = "all";
      state.tab = "results"; // show the live progress
      poll();
      api.toast(selfTest ? "Checking the checks…" : `Bench started: ${state.progress.total} runs`, "info");
    } catch (e) {
      api.toast(`Couldn't start the bench: ${e.message}`, "error");
    } finally {
      state.starting = false;
    }
    render();
  }

  async function stop() {
    const run = state.running;
    if (!run) return;
    // The runner and the agents it started share a process group.
    await sh(`pg=$(ps -o pgid= -p ${run.pid} | tr -d ' '); [ -n "$pg" ] && kill -TERM -"$pg" 2>/dev/null; kill ${run.pid} 2>/dev/null; true`).catch(() => {});
    await sampleUsage(true);
    state.running = null;
    state.progress = null;
    api.toast("Bench stopped", "info");
    loadHistory();
  }

  async function poll() {
    clearTimeout(timer);
    if (!state.running) return;
    try {
      state.progress = JSON.parse(await api.readFile(status));
    } catch {}
    try {
      state.results = parseJsonl(await api.readFile(state.running.log)).map(withLegacyErrors);
    } catch {}
    const finished = state.progress?.finished || state.progress?.done >= state.progress?.total;
    let alive = true;
    if (!finished) {
      alive = await sh(`kill -0 ${state.running.pid} 2>/dev/null && echo yes || echo no`).then((o) => o.includes("yes")).catch(() => true);
    }
    if (finished || !alive) {
      await sampleUsage(true);
      const passed = state.results.filter((r) => r.pass).length;
      api.toast(`Bench finished: ${passed}/${state.results.length} passed`, "success");
      state.running = null;
      render();
      loadHistory();
      return;
    }
    if (Date.now() - usageAt > USAGE_MS) sampleUsage().then(render);
    render();
    timer = setTimeout(poll, POLL_MS);
  }

  // ---- charts (hand-built SVG: thin marks, one axis, hover tooltips via data-tip) ----

  // Charts are drawn at their real pixel width (measured), so text stays legible at any size.
  let mainWidth = 720;

  /** Horizontal bars, one per row; `parts` stack (e.g. uncached + cached). */
  function barChart(rows, { parts, fmt, max: forcedMax, width: W }) {
    const left = Math.min(132, W * 0.32);
    const right = 56;
    const band = 30;
    const bar = 14;
    const top = 6;
    const total = (r) => parts.reduce((a, p) => a + p.value(r), 0);
    const { max, step } = niceScale(forcedMax ?? Math.max(...rows.map(total)));
    const x = (v) => left + ((W - left - right) * v) / max;
    const h = top + rows.length * band + 22;
    let svg = `<svg viewBox="0 0 ${W} ${h}" width="${W}" height="${h}" role="img">`;
    for (let v = 0; v <= max + 1e-9; v += step) {
      svg += `<line class="grid" x1="${x(v)}" x2="${x(v)}" y1="${top}" y2="${top + rows.length * band}"/>`;
      svg += `<text x="${x(v)}" y="${h - 4}" text-anchor="middle">${esc(fmt(v))}</text>`;
    }
    rows.forEach((r, i) => {
      const y = top + i * band + (band - bar) / 2;
      svg += `<text x="${left - 10}" y="${y + bar / 2 + 4}" text-anchor="end" class="t-strong">${esc(r.label)}</text>`;
      let at = 0;
      const segs = parts.filter((p) => p.value(r) > 0);
      segs.forEach((p, k) => {
        const v = p.value(r);
        const x0 = x(at) + (k ? 1 : 0); // 2px surface gap between segments
        const x1 = x(at + v) - (k < segs.length - 1 ? 1 : 0);
        const w = Math.max(1, x1 - x0);
        const last = k === segs.length - 1;
        const rad = Math.min(4, w / 2);
        // Rounded data-end, square at the baseline.
        const path = last
          ? `M${x0},${y} H${x0 + w - rad} Q${x0 + w},${y} ${x0 + w},${y + rad} V${y + bar - rad} Q${x0 + w},${y + bar} ${x0 + w - rad},${y + bar} H${x0} Z`
          : `M${x0},${y} H${x0 + w} V${y + bar} H${x0} Z`;
        svg += `<path d="${path}" fill="${r.color}" opacity="${p.opacity ?? 1}"/>`;
        at += v;
      });
      svg += `<text x="${x(at) + 6}" y="${y + bar / 2 + 4}" class="t-strong">${esc(fmt(total(r)))}</text>`;
      svg += `<rect class="hit" x="0" y="${top + i * band}" width="${W}" height="${band}" data-tip="${esc(r.tip)}"/>`;
    });
    svg += `<line class="axis" x1="${x(0)}" x2="${x(0)}" y1="${top}" y2="${top + rows.length * band}"/></svg>`;
    return svg;
  }

  /** Speed vs tokens: one dot per harness, both axes from zero, "better" is bottom-left. */
  function scatter(rows, W) {
    const left = 46;
    const right = 18;
    const top = 12;
    const bottom = 34;
    const H = 260;
    const xs = niceScale(Math.max(...rows.map((r) => r.uncached)));
    const ys = niceScale(Math.max(...rows.map((r) => r.seconds)));
    const x = (v) => left + ((W - left - right) * v) / xs.max;
    const y = (v) => H - bottom - ((H - top - bottom) * v) / ys.max;
    let svg = `<svg viewBox="0 0 ${W} ${H}" width="${W}" height="${H}" role="img">`;
    for (let v = 0; v <= ys.max + 1e-9; v += ys.step) {
      svg += `<line class="grid" x1="${left}" x2="${W - right}" y1="${y(v)}" y2="${y(v)}"/><text x="${left - 8}" y="${y(v) + 4}" text-anchor="end">${esc(secs(v))}</text>`;
    }
    for (let v = 0; v <= xs.max + 1e-9; v += xs.step) {
      svg += `<text x="${x(v)}" y="${H - bottom + 16}" text-anchor="middle">${esc(tokens(v))}</text>`;
    }
    svg += `<line class="axis" x1="${left}" x2="${W - right}" y1="${y(0)}" y2="${y(0)}"/><line class="axis" x1="${left}" x2="${left}" y1="${top}" y2="${y(0)}"/>`;
    svg += `<text x="${W - right}" y="${H - 2}" text-anchor="end">uncached input tokens (median) →</text>`;
    svg += `<text x="${left + 6}" y="${top + 4}" >↑ time (median)</text>`;
    svg += `<text x="${left + 8}" y="${y(0) - 8}" opacity=".7">↙ faster and leaner</text>`;
    // Direct labels, nudged apart vertically when they'd collide.
    const placed = [];
    rows.forEach((r) => {
      const cx = x(r.uncached);
      const cy = y(r.seconds);
      let ly = cy + 4;
      while (placed.some((p) => Math.abs(p.y - ly) < 13 && Math.abs(p.x - cx) < 140)) ly += 13;
      placed.push({ x: cx, y: ly });
      const anchor = cx > W - 150 ? "end" : "start";
      const lx = anchor === "end" ? cx - 10 : cx + 10;
      svg += `<circle cx="${cx}" cy="${cy}" r="6" fill="${r.color}" stroke="var(--hb-surface)" stroke-width="2"/>`;
      svg += `<text x="${lx}" y="${ly}" text-anchor="${anchor}" class="t-strong">${esc(r.label)}</text>`;
      svg += `<circle class="hit" cx="${cx}" cy="${cy}" r="14" data-tip="${esc(r.tip)}"/>`;
    });
    return svg + `</svg>`;
  }

  /** Swimlanes over wall-clock time: each run is a bar from its start to its end. */
  function timeline(results, jobs, plan, W) {
    const now = Date.now() / 1000;
    const runs = [
      ...results.filter((r) => r.started).map((r) => ({ ...r, end: r.started + r.seconds, live: false })),
      ...jobs.map((j) => ({ task: j.task, harness: j.harness, started: j.since, end: now, live: true })),
    ];
    if (!runs.length) return "";
    const lanes = [...new Set([...(plan?.cols ?? []).map((c) => c.split("|")[0]), ...runs.map((r) => r.harness)])];
    const t0 = Math.min(...runs.map((r) => r.started));
    const span = Math.max(10, Math.max(...runs.map((r) => r.end)) - t0);
    const { max, step } = niceScale(span, 6);
    const left = Math.min(132, W * 0.3);
    const right = 16;
    const band = 28;
    const top = 4;
    const x = (t) => left + ((W - left - right) * (t - t0)) / max;
    const h = top + lanes.length * band + 22;
    let svg = `<svg viewBox="0 0 ${W} ${h}" width="${W}" height="${h}" role="img"><defs>
      <pattern id="hb-hatch" width="6" height="6" patternUnits="userSpaceOnUse" patternTransform="rotate(45)"><rect width="6" height="6" fill="var(--hb-surface)"/><line x1="0" y1="0" x2="0" y2="6" stroke="var(--color-danger)" stroke-width="2.5"/></pattern></defs>`;
    for (let v = 0; v <= max + 1e-9; v += step) {
      svg += `<line class="grid" x1="${x(t0 + v)}" x2="${x(t0 + v)}" y1="${top}" y2="${top + lanes.length * band}"/><text x="${x(t0 + v)}" y="${h - 4}" text-anchor="middle">${esc(secs(v))}</text>`;
    }
    lanes.forEach((lane, i) => {
      const y = top + i * band + 7;
      svg += `<text x="${left - 10}" y="${y + 11}" text-anchor="end" class="t-strong">${esc(shortLabel(lane))}</text>`;
      for (const r of runs.filter((r) => r.harness === lane)) {
        const x0 = x(r.started) + 1;
        const w = Math.max(3, x(r.end) - x(r.started) - 2); // 2px gap between back-to-back runs
        const fill = r.live || r.pass ? colorOf(lane) : r.error ? "transparent" : "url(#hb-hatch)";
        const tipText = r.live
          ? `${labelOf(lane)} · ${r.task}\nrunning for ${secs(now - r.started)}`
          : r.error
            ? `${labelOf(lane)} · ${r.task}\n⚠ harness error after ${secs(r.seconds)} (not counted)\n${r.error}`
            : `${labelOf(lane)} · ${r.task}\n${r.pass ? "✓ passed" : "✗ failed"} in ${secs(r.seconds)}\n${tokens(r.input - r.cached)} uncached · ${tokens(r.input)} input · ${r.tools} tool calls`;
        svg += `<rect x="${x0}" y="${y}" width="${w}" height="14" rx="4" fill="${fill}" ${r.live ? `class="hb-pulse" opacity=".7"` : ""} ${!r.live && !r.pass ? `stroke="var(--color-${r.error ? "warning" : "danger"})" stroke-width="1" ${r.error ? `stroke-dasharray="3 2"` : ""}` : ""}/>`;
        if (w > 64) svg += `<text x="${x0 + 6}" y="${y + 11}" style="font-size:10px;fill:#fff;opacity:.92">${esc(taskName(r.task))}</text>`;
        svg += `<rect class="hit" x="${x0}" y="${y - 6}" width="${w}" height="26" data-tip="${esc(tipText)}"/>`;
      }
    });
    return svg + `</svg>`;
  }

  // ---- page sections ----

  function header() {
    const running = !!state.running;
    const current = state.viewing && !state.history.some((f) => state.viewing.endsWith(f));
    return `<div class="hb-head">
      <div class="hb-title"><div class="hb-logo">${ICON}</div><div><h1>winfer bench</h1><p>Same tasks, same models, different harnesses: what passes, how fast, how many tokens.</p></div></div>
      <div class="hb-actions">
        <select class="wf-select" data-action="history" aria-label="Results to show" ${running ? "disabled" : ""}>
          ${current ? `<option selected>${esc(runLabel(state.viewing.split(/[\\/]/).pop()))}${running ? " · running" : ""}</option>` : ""}
          ${state.history.length || current ? "" : `<option>No runs yet</option>`}
          ${state.history.slice(0, 30).map((f) => `<option value="${esc(f)}" ${state.viewing?.endsWith(f) ? "selected" : ""}>${esc(runLabel(f))}</option>`).join("")}
          ${state.history.length > 30 && state.viewing && !state.history.slice(0, 30).some((f) => state.viewing.endsWith(f)) && !current ? `<option selected>${esc(runLabel(state.viewing.split(/[\\/]/).pop()))}</option>` : ""}
        </select>
        <button class="wf-button wf-ghost" data-action="reveal" ${state.viewing ? "" : "disabled"} title="Show the results file">Open file</button>
        <button class="wf-button wf-ghost" data-action="selftest" ${running || state.starting ? "disabled" : ""} title="Run the reference solutions (every task should pass) and a no-op (every task should fail) to confirm the checks are trustworthy. No AI, no tokens.">Check the checks</button>
        ${
          running
            ? `<button class="wf-button wf-danger" data-action="stop">Stop</button>`
            : state.tab === "runs"
              ? `<button class="wf-button wf-primary" data-action="configure" title="Pick the harnesses, models and tasks to bench">New run…</button>`
              : `<button class="wf-button wf-primary" data-action="start" ${windows || state.starting ? "disabled" : ""} ${windows ? "title='macOS and Linux only for now'" : ""}>${state.starting ? "Starting…" : "Run bench"}</button>`
        }
      </div>
    </div>`;
  }

  function tabs() {
    const tab = (id, label, count) =>
      `<button role="tab" data-tab="${id}" aria-selected="${state.tab === id}">${label}${count != null ? ` <span class="hb-count">${count}</span>` : ""}</button>`;
    return `<div class="hb-tabs" role="tablist">${tab("runs", "Runs", state.history.length + (state.running && !state.history.some((f) => state.running.log.endsWith(f)) ? 1 : 0))}${tab("results", "Results")}</div>`;
  }

  /** Every saved run in one table: open one, or tick several and delete them. */
  function runsView() {
    const live = state.running?.log.split(/[\\/]/).pop();
    const files = live && !state.history.includes(live) ? [live, ...state.history] : state.history;
    if (!files.length) {
      return `<div class="hb-empty"><h3>No runs yet</h3><p>Runs you start appear here, newest first, with their pass rate, time and cost.</p></div>`;
    }
    const deletable = state.history.filter((f) => f !== live);
    const picked = deletable.filter((f) => state.selected.has(f));
    const all = picked.length > 0 && picked.length === deletable.length;
    const rows = files
      .map((f) => {
        const isLive = f === live;
        const saved = isLive ? state.results : state.saved[f];
        const o = saved ? runOverview(saved, state.catalog.prices, state.catalog.tasks) : null;
        const viewing = state.viewing?.endsWith(f);
        const self = f.includes("-selftest");
        const who = o
          ? o.harnesses
              .filter((h) => self || !CHECKS.includes(h))
              .map((h) => `<span><i class="hb-dot" style="background:${colorOf(h)}"></i>${esc(shortLabel(h))}</span>`)
              .join("")
          : `<span class="wf-subtle">…</span>`;
        // A self-test is right when the reference solution passes and the no-op fails.
        if (o && self) o.passed = saved.filter((r) => !r.error && r.pass === (r.harness !== "noop")).length;
        const rate = o?.runs ? o.passed / o.runs : 0;
        const cost = o?.cost != null ? `${o.costEstimated ? "~" : ""}$${o.cost.toFixed(2)}` : "—";
        return `<tr data-open-run="${esc(f)}" class="${viewing ? "viewing" : ""}" title="Show these results">
          <td style="width:1%"><input type="checkbox" data-select-run="${esc(f)}" aria-label="Select run ${esc(runLabel(f))}" ${state.selected.has(f) && !isLive ? "checked" : ""} ${isLive ? "disabled" : ""} /></td>
          <td class="when">${esc(runLabel(f).replace(" · self-test", ""))}${self ? `<span class="hb-badge">self-test</span>` : ""}${isLive ? `<span class="hb-badge live hb-pulse">running</span>` : ""}</td>
          <td><div class="who">${who}</div>${o?.models.length ? `<div class="wf-subtle wf-mono" style="font-size:11px;margin-top:3px">${esc(o.models.join(", "))}</div>` : ""}</td>
          <td class="num" ${o ? `title="${esc(o.suites.map(suiteLabel).join(", "))}"` : ""}>${o ? o.tasks : ""}${o && (o.suites.length > 1 || o.suites[0] !== "coding") ? `<div class="wf-subtle" style="font-size:11px;margin-top:3px">${esc(o.suites.length > 1 ? `${o.suites.length} suites` : suiteLabel(o.suites[0]))}</div>` : ""}</td>
          <td class="num">${o ? `<div class="hb-pass"><span ${self ? `title="Checks that behaved as expected"` : ""}>${o.passed}/${o.runs}${self ? " ok" : ""}</span><div class="hb-meter"><div style="width:${Math.round(rate * 100)}%;background:var(--color-success)"></div></div></div>` : ""}</td>
          <td class="num">${o?.errors ? `<span style="color:var(--color-warning)" title="Harness errors, not counted">⚠ ${o.errors}</span>` : o ? `<span class="wf-subtle">0</span>` : ""}</td>
          <td class="num">${o?.wall ? secs(o.wall) : o ? "—" : ""}</td>
          <td class="num" ${o ? `title="${esc(o.cost != null ? costNote(o.costEstimated, o.costPartial) : "No known cost")}"` : ""}>${o ? cost : ""}</td>
          <td class="act"><button class="hb-icon-btn" data-delete-run="${esc(f)}" ${isLive ? "disabled title='Stop the run first'" : `title="Delete this run"`} aria-label="Delete run ${esc(runLabel(f))}">${TRASH}</button></td>
        </tr>`;
      })
      .join("");
    return `<div class="hb-panel">
      <div class="hb-runs-bar">
        <p class="hb-sub">${files.length} run${files.length === 1 ? "" : "s"}, newest first. Click a run to see its results.</p>
        ${picked.length ? `<button class="wf-button wf-ghost" data-action="clear-selection">Clear selection</button>` : ""}
        <button class="wf-button wf-danger" data-action="delete-selected" ${picked.length ? "" : "disabled"}>Delete${picked.length ? ` ${picked.length}` : ""} selected</button>
      </div>
      <div style="overflow-x:auto"><table class="hb-runs">
        <thead><tr><th><input type="checkbox" data-select-all aria-label="Select all runs" ${all ? "checked" : ""} ${deletable.length ? "" : "disabled"} /></th><th>When</th><th>Harnesses</th><th class="num">Tasks</th><th class="num">Passed</th><th class="num">Errors</th><th class="num">Wall time</th><th class="num">Cost</th><th></th></tr></thead>
        <tbody>${rows}</tbody>
      </table></div>
    </div>`;
  }

  function setup() {
    const c = state.catalog;
    const p = prefs();
    const hs = harnesses()
      .map((h) => {
        const hp = harnessPref(h);
        const on = hp.on && h.installed;
        const options = [...new Set([hp.model, h.model, ...h.models].filter(Boolean))];
        return `<div class="hb-harness ${on ? "" : "off"} ${h.installed ? "" : "missing"}">
          <span class="hb-swatch" style="background:${colorOf(h.id)}"></span>
          <span class="hb-hname" title="${esc(h.id)}">${esc(h.label)}${h.installed ? "" : ` <span class="wf-subtle">· not installed</span>`}</span>
          <input type="checkbox" class="wf-switch" data-harness="${esc(h.id)}" aria-label="Run ${esc(h.label)}" ${on ? "checked" : ""} ${h.installed ? "" : "disabled"} />
          ${
            h.model || h.models.length
              ? `<div class="hb-hmodel"><input class="wf-input wf-mono" list="hb-models-${esc(h.id)}" data-model="${esc(h.id)}" value="${esc(hp.model)}" placeholder="${esc(h.model)}" aria-label="${esc(h.label)} model" ${h.installed ? "" : "disabled"} />
                 <datalist id="hb-models-${esc(h.id)}">${options.map((m) => `<option value="${esc(m)}"></option>`).join("")}</datalist></div>`
              : ""
          }
        </div>`;
      })
      .join("");
    const chip = (t) => `<button class="hb-chip" data-task="${esc(t.id)}" aria-pressed="${taskOn(t)}" title="${esc(t.prompt)}">${esc(taskName(t.id))}</button>`;
    // One group per suite, each with its own count and All/None.
    const tasks = catalogSuites()
      .map((s) => {
        const ts = c.tasks.filter((t) => taskSuite(t) === s.id);
        if (!ts.length) return "";
        return `<div class="hb-suite">
          <div class="hb-suite-head"><span class="hb-suite-name" title="${esc(s.description)}">${esc(s.label)}</span><span class="hb-suite-count">${ts.filter(taskOn).length}/${ts.length}</span>
            <span class="hb-tasks-all"><button data-suite-all="${esc(s.id)}:on" title="Select every ${esc(s.label.toLowerCase())} task">All</button><button data-suite-all="${esc(s.id)}:off" title="Deselect every ${esc(s.label.toLowerCase())} task">None</button></span></div>
          <div class="hb-chips">${ts.map(chip).join("")}</div></div>`;
      })
      .join("");
    const par = p.parallel ?? "harness";
    const seg = (v, label, title) => `<button data-parallel="${v}" aria-pressed="${par === v}" title="${esc(title)}">${label}</button>`;
    return `<aside class="hb-aside">
      <div class="hb-panel"><div class="hb-label">Harnesses & models</div>${hs}
        <p class="hb-sub" style="margin:10px 0 0">Compare on the same model for a fair read. Colors stay with each harness in every chart.</p></div>
      <div class="hb-panel"><div class="hb-tasks-head"><div class="hb-label">Tasks · ${c.tasks.filter(taskOn).length}/${c.tasks.length}</div>
        <div class="hb-tasks-all"><button data-tasks-all="on" title="Select every task">All</button><button data-tasks-all="off" title="Deselect every task">None</button></div></div>
        ${tasks}</div>
      <div class="hb-panel"><div class="hb-label">Run</div>
        <div class="hb-seg" role="group" aria-label="Parallel">${seg("harness", "Per harness", "All harnesses at once, each working through its tasks in turn")}${seg("all", "Everything", "Every run at once")}${seg("none", "Sequential", "One run at a time")}</div>
        <div class="hb-opts" style="margin-top:12px">
          <label>Runs per task<input class="wf-input" type="number" min="1" max="20" data-pref="runs" value="${esc(p.runs ?? 1)}" /></label>
          <label>Timeout (s)<input class="wf-input" type="number" min="60" data-pref="timeout" value="${esc(p.timeout ?? 900)}" /></label>
        </div>
        <label class="wf-inline" style="gap:8px;margin-top:12px;font-size:12px"><input type="checkbox" class="wf-switch" data-pref="keep" ${p.keep ? "checked" : ""} /> Keep work folders</label>
      </div>
    </aside>`;
  }

  function live() {
    if (!state.running) return "";
    const p = state.progress ?? { done: 0, total: 1 };
    const jobs = runningJobs(p);
    const pct = p.total ? p.done / p.total : 0;
    const r = 22;
    const circ = 2 * Math.PI * r;
    const elapsed = Date.now() / 1000 - (state.running.started ?? Date.now() / 1000);
    return `<div class="hb-panel hb-live">
      <svg class="hb-ring" width="56" height="56" viewBox="0 0 56 56" aria-hidden="true">
        <circle cx="28" cy="28" r="${r}" stroke="var(--color-border)"/>
        <circle cx="28" cy="28" r="${r}" stroke="var(--color-accent)" stroke-linecap="round" stroke-dasharray="${(circ * pct).toFixed(1)} ${circ.toFixed(1)}" transform="rotate(-90 28 28)" style="transition: stroke-dasharray .4s ease"/>
        <text x="28" y="32" text-anchor="middle" class="t-strong" style="font-size:12px;font-weight:600">${Math.round(pct * 100)}%</text>
      </svg>
      <div style="flex:1;min-width:0">
        <div style="font-size:13px;font-weight:600">${p.done} of ${p.total} runs done</div>
        ${usageRows(state.running.usage, state.results)
          .filter((u) => u.outAt || u.outAtStart || u.limitErrorAt)
          .map((u) => `<div class="hb-sub" style="margin:2px 0 0;color:var(--color-danger)">⚠ ${esc(u.label ? `${u.provider === "codex" ? "Codex · ChatGPT" : "Claude"} ${u.label.toLowerCase()} limit` : `${u.provider} limit`)} ran out ${u.outAtStart ? "before the start" : `at ${clock(u.outAt ?? u.limitErrorAt)}`}: later runs may error</div>`)
          .join("")}
        <div class="hb-sub" style="margin:2px 0 0">${jobs.length} running · ${secs(elapsed)} elapsed${jobs.length ? ` · ${esc(jobs.map((j) => `${labelOf(j.harness)} on ${taskName(j.task)}`).join(", "))}` : ""}</div>
      </div>
    </div>`;
  }

  function kpis(rows) {
    const errors = shown().filter((r) => r.error).length;
    const runs = shown().length - errors;
    const passed = shown().filter((r) => r.pass).length;
    const ok = rows.filter((r) => r.passed > 0);
    const fastest = [...ok].sort((a, b) => a.seconds - b.seconds)[0];
    const leanest = [...ok].filter((r) => r.hasTokens).sort((a, b) => a.uncached - b.uncached)[0];
    const timed = shown().filter((r) => r.started);
    const wall = timed.length ? Math.max(...timed.map((r) => r.started + r.seconds)) - Math.min(...timed.map((r) => r.started)) : 0;
    const cost = rows.some((r) => r.cost != null) ? rows.reduce((a, r) => a + (r.cost || 0), 0) : null;
    const estimated = rows.some((r) => r.costEstimated);
    const partial = rows.some((r) => r.costPartial);
    const who = (r) => (r ? `<span class="hb-dot" style="background:${colorOf(r.harness)}"></span>${esc(labelOf(r.harness))}` : "—");
    const rate = runs ? passed / runs : 0;
    return `<div class="hb-kpis">
      <div class="hb-kpi"><div class="k">Passed</div><div class="v">${passed}<span style="font-size:14px;color:var(--color-fg-muted);font-weight:500"> / ${runs}</span></div>
        <div class="hb-meter"><div style="width:${Math.round(rate * 100)}%;background:var(--color-success)"></div></div>
        ${errors ? `<div class="s" style="color:var(--color-warning)" title="Runs where the harness itself failed (rate limit, crash, not signed in); not counted">⚠ ${errors} harness error${errors === 1 ? "" : "s"} not counted</div>` : ""}</div>
      <div class="hb-kpi"><div class="k">Fastest (median per task)</div><div class="v">${fastest ? secs(fastest.seconds) : "—"}</div><div class="s">${who(fastest)}</div></div>
      <div class="hb-kpi"><div class="k">Leanest (uncached tokens)</div><div class="v">${leanest ? tokens(leanest.uncached) : "—"}</div><div class="s">${who(leanest)}</div></div>
      <div class="hb-kpi"><div class="k">Wall time</div><div class="v">${wall ? secs(wall) : "—"}</div><div class="s">${rows.length} harness${rows.length === 1 ? "" : "es"} · ${new Set(shown().map((r) => r.task)).size} tasks</div></div>
      ${cost != null ? `<div class="hb-kpi" title="${esc(costNote(estimated, partial))}"><div class="k">Cost at API prices</div><div class="v">${estimated ? "~" : ""}$${cost.toFixed(2)}</div><div class="s">${estimated ? "includes token estimates" : "reported by harnesses"}${partial ? " · available costs only" : ""}</div></div>` : ""}
    </div>`;
  }

  /** Plan usage limits at the start and end of the run, and when one ran out during it. */
  function usagePanel() {
    const rows = usageRows(state.usage, state.results);
    if (!rows.length) return "";
    const live = state.running && state.running.log === state.viewing;
    const plan = (p) => (p === "codex" ? "Codex · ChatGPT" : p === "claude" ? "Claude" : p);
    const pct = (v) => `${Math.round(v * 10) / 10}%`;
    const reading = (used, at) => `<b>${pct(used)}</b> used <span class="wf-subtle">· ${pct(Math.max(0, 100 - used))} left · ${clock(at)}</span>`;
    const ranOut = (r) => {
      if (r.outAtStart) return `<span style="color:var(--color-danger)" title="This limit was already used up when the bench started">⚠ already out at start</span>`;
      if (r.outAt) {
        const tip = r.okAt ? `Below the limit at ${clock(r.okAt)}, at 100% at ${clock(r.outAt)} (read every ${USAGE_MS / 60000} min)` : "";
        return `<span style="color:var(--color-danger)" ${tip ? `title="${esc(tip)}"` : ""}>⚠ ran out ${r.okAt ? `between ${clock(r.okAt)} and ` : "at "}${clock(r.outAt)}</span>`;
      }
      return `<span style="color:var(--color-success)">no</span>`;
    };
    const limitError = (r) => (r.limitErrorAt ? `<div class="wf-subtle" style="color:var(--color-warning)">first limit error from a harness: ${clock(r.limitErrorAt)}</div>` : "");
    const body = rows
      .map((r) =>
        r.kind === null
          ? `<tr><td>${esc(plan(r.provider))}</td><td class="wf-subtle" colspan="3">no usage numbers</td><td>${limitError(r)}</td></tr>`
          : `<tr><td>${esc(plan(r.provider))}</td><td>${esc(r.label)}</td><td>${reading(r.start, r.startAt)}</td>
          <td>${reading(r.end, r.endAt)}${r.reset ? ` <span class="wf-badge" title="The window reset during the run, so the end reading counts from that reset">reset</span>` : ""}</td>
          <td>${ranOut(r)}${limitError(r)}</td></tr>`,
      )
      .join("");
    return `<div class="hb-panel"><h2>Plan usage limits</h2>
      <p class="hb-sub">Read from your Claude and ChatGPT plans when the run started, every minute while it ran and when it ended. Account-wide: anything else on the same plan counts too.</p>
      <div style="overflow-x:auto"><table class="hb-table">
        <tr><th>Plan</th><th>Limit</th><th>At start</th><th>${live ? "Now" : "At end"}</th><th>Ran out</th></tr>${body}
      </table></div></div>`;
  }

  /** Metric shades for the score breakdown: one hue (the accent), stepped, so they never look
   * like harness colors. Pass rate is the strongest. */
  const METRIC_SHADE = { pass: 1, uncached: 0.72, seconds: 0.5, input: 0.32, tools: 0.2 };

  function weights() {
    const w = prefs().weights;
    return w && typeof w === "object" ? { ...PRESETS.balanced.weights, ...w } : PRESETS.balanced.weights;
  }

  function winner(rows) {
    if (rows.length < 2) return "";
    const w = weights();
    const ranked = scoreRows(rows, w);
    const top = ranked[0];
    if (!top) return "";
    const tie = ranked[1] && Math.abs(ranked[1].score - top.score) < 0.5;
    // Say why: the metrics it wins outright, else its strongest contributions.
    const wins = METRICS.filter((m) => (w[m.id] ?? 0) > 0 && ranked.every((r) => r === top || top.parts[m.id] > r.parts[m.id] + 1e-9))
      .sort((a, b) => (w[b.id] ?? 0) - (w[a.id] ?? 0))
      .slice(0, 2)
      .map((m) => m.label.toLowerCase());
    const why = wins.length ? `Leads on ${wins.join(" and ")}` : "Strongest all-round";
    const presetId = Object.entries(PRESETS).find(([, p]) => METRICS.every((m) => (p.weights[m.id] ?? 0) === (w[m.id] ?? 0)))?.[0];
    const usedMetrics = METRICS.filter((m) => (w[m.id] ?? 0) > 0);
    const legendRow = `<div class="hb-legend" style="margin:0">${usedMetrics.map((m) => `<span><i class="hb-key" style="background:var(--color-accent);opacity:${METRIC_SHADE[m.id]}"></i>${esc(m.label)}</span>`).join("")}</div>`;
    const rowsHtml = ranked
      .map((r, i) => {
        const tip = `${labelOf(r.harness)}${r.model ? ` · ${r.model}` : ""}: ${r.score.toFixed(0)} / 100\n${METRICS.filter((m) => (w[m.id] ?? 0) > 0)
          .map((m) => `${m.label}: +${r.parts[m.id].toFixed(1)}`)
          .join("\n")}${r.hasTokens ? "" : "\nNo token counts: token metrics score 0"}`;
        const segs = usedMetrics
          .filter((m) => r.parts[m.id] > 0.05)
          .map((m) => `<span style="width:${r.parts[m.id]}%;background:var(--color-accent);opacity:${METRIC_SHADE[m.id]}"></span>`)
          .join("");
        return `<div class="pos">${i + 1}</div>
          <div class="who"><span class="hb-dot" style="background:${colorOf(r.harness)}"></span>${esc(labelOf(r.harness))}${r.hasTokens ? "" : ` <span class="wf-subtle" title="No token counts">*</span>`}</div>
          <div class="hb-stack" data-tip="${esc(tip)}">${segs}</div>
          <div class="val">${r.score.toFixed(0)}</div>`;
      })
      .join("");
    const sliders = METRICS.map(
      (m) => `<label><span>${esc(m.label)}<b>${Math.round(w[m.id] ?? 0)}</b></span><input type="range" min="0" max="100" step="5" value="${w[m.id] ?? 0}" data-weight="${m.id}" /></label>`,
    ).join("");
    return `<div class="hb-panel hb-winner">
      <div class="hb-crown">
        <div class="hb-trophy" aria-hidden="true"><svg width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M8 21h8M12 17v4M7 4h10v5a5 5 0 0 1-10 0V4Z"/><path d="M17 5h3v2a3 3 0 0 1-3 3M7 5H4v2a3 3 0 0 0 3 3"/></svg></div>
        <div style="min-width:0">
          <div class="hb-label" style="margin:0 0 2px">${tie ? "Overall: a tie" : "Overall winner"}</div>
          <div class="name"><span class="hb-dot" style="width:10px;height:10px;background:${colorOf(top.harness)}"></span>${esc(labelOf(top.harness))}${tie ? ` <span class="wf-subtle" style="font-weight:400">and ${esc(labelOf(ranked[1].harness))}</span>` : ""}</div>
          <div class="why">${esc(why)} · ${top.passed}/${top.runs} passed · ${secs(top.seconds)} · ${top.hasTokens ? `${tokens(top.uncached)} full-price tokens` : "no token counts"}</div>
        </div>
        <div class="hb-score"><div class="n">${top.score.toFixed(0)}</div><div class="of">of 100</div></div>
      </div>
      <div class="hb-rank">${rowsHtml}</div>
      ${legendRow}
      <details ${prefs().weightsOpen ? "open" : ""} data-pref-details="weightsOpen"><summary>How it's scored${presetId ? ` · ${esc(PRESETS[presetId].label)}` : " · custom"}</summary>
        <p class="hb-sub" style="margin:8px 0 10px">Each metric scores up to its weight: pass rate as is, the others relative to the best harness (half as good, half the points). Weights are relative. Harness errors aren't counted; a harness without token counts gets no token points.</p>
        <div class="hb-seg" role="group" aria-label="Scoring preset" style="margin-bottom:10px">${Object.entries(PRESETS).map(([id, p]) => `<button data-preset="${id}" aria-pressed="${presetId === id}">${esc(p.label)}</button>`).join("")}</div>
        <div class="hb-weights">${sliders}</div>
      </details>
    </div>`;
  }

  function legend(rows) {
    return `<div class="hb-legend">${rows.map((r) => `<span><i class="hb-key" style="background:${colorOf(r.harness)}"></i>${esc(labelOf(r.harness))}${r.model ? ` <span class="wf-subtle">${esc(r.model)}</span>` : ""}</span>`).join("")}</div>`;
  }

  function charts(rows) {
    const named = rows.map((r) => ({
      ...r,
      label: shortLabel(r.harness),
      color: colorOf(r.harness),
      tip: `${labelOf(r.harness)}${r.model ? ` · ${r.model}` : ""}\n${r.passed}/${r.runs} passed · median ${secs(r.seconds)}\n${tokens(r.uncached)} uncached of ${tokens(r.input)} input\n${tokens(r.output)} output · ${Math.round(r.tools)} tool calls`,
    }));
    const withTokens = named.filter((r) => r.hasTokens);
    const noTokens = named.filter((r) => !r.hasTokens);
    const byTokens = [...withTokens].sort((a, b) => a.uncached - b.uncached);
    const missing = noTokens.length
      ? `<p class="hb-sub" style="margin:8px 0 0">No token counts from ${esc(noTokens.map((r) => labelOf(r.harness)).join(", "))}, so ${noTokens.length === 1 ? "it's" : "they're"} not shown.</p>`
      : "";
    const byTime = [...named].sort((a, b) => a.seconds - b.seconds);
    const plan = state.running?.plan;
    const jobs = state.running ? runningJobs(state.progress) : [];
    const two = mainWidth >= 760;
    const full = mainWidth - 34; // panel padding and border
    const half = two ? (mainWidth - 16) / 2 - 34 : full;
    const tl = timeline(shown(), jobs, plan, full);
    return `<div class="hb-charts ${two ? "" : "one"}">
      ${tl ? `<div class="hb-panel hb-wide"><h2>Timeline</h2><p class="hb-sub">Each bar is one run, from start to finish. Hatched bars failed their check; dashed outlines are harness errors (rate limit, crash), not counted.${jobs.length ? " Pulsing bars are still running." : ""}</p>${tl}</div>` : ""}
      ${
        named.length
          ? `<div class="hb-panel"><h2>Input tokens per task</h2><p class="hb-sub">Median per run. Solid is paid at full price; the lighter part was served from the prompt cache.</p>
              <div class="hb-legend"><span><i class="hb-key" style="background:var(--color-fg-muted)"></i>uncached</span><span><i class="hb-key" style="background:var(--color-fg-muted);opacity:.3"></i>cached</span></div>
              ${byTokens.length ? barChart(byTokens, { width: half, fmt: tokens, parts: [{ value: (r) => r.uncached }, { value: (r) => r.cached, opacity: 0.3 }] }) : ""}${missing}</div>
            <div class="hb-panel"><h2>Time per task</h2><p class="hb-sub">Median wall time of one run, start to finish.</p>
              ${barChart(byTime, { width: half, fmt: secs, parts: [{ value: (r) => r.seconds }] })}</div>
            <div class="hb-panel hb-wide"><h2>Speed vs. tokens</h2><p class="hb-sub">Bottom-left is best: fast and lean. Uses medians over every run.</p>${byTokens.length ? `${legend(withTokens)}${scatter(withTokens, full)}` : ""}${missing}</div>`
          : ""
      }
    </div>`;
  }

  function grid() {
    const plan = state.running?.plan;
    if (!shown().length && !plan) return "";
    const cols = [...new Set([...(plan?.cols ?? []), ...shown().map((r) => `${r.harness}|${r.model ?? ""}`)])];
    const suiteOfTask = (task) => suiteOf({ task, suite: shown().find((r) => r.task === task)?.suite }, state.catalog?.tasks);
    const planned = (plan?.tasks ?? []).filter((t) => state.suite === "all" || suiteOfTask(t) === state.suite);
    const tasks = [...new Set([...planned, ...shown().map((r) => r.task)])].sort();
    // Grouped by suite (catalog order) when the run spans more than one.
    const groups = suitesIn(tasks.map((task) => ({ task, suite: suiteOfTask(task) })), state.catalog).map((s) => [s, tasks.filter((t) => suiteOfTask(t) === s)]);
    const busy = new Set(state.running ? runningJobs(state.progress).map((j) => `${j.task}|${j.harness}`) : []);
    const cell = (task, col) => {
      const rs = shown().filter((r) => r.task === task && `${r.harness}|${r.model ?? ""}` === col);
      if (!rs.length) {
        if (busy.has(`${task}|${col.split("|")[0]}`)) return `<td><div class="hb-cell busy hb-pulse"><div class="st">● running</div><div class="m">&nbsp;</div></div></td>`;
        return `<td><div class="hb-cell queued"><div class="st" style="color:var(--color-fg-subtle)">${state.running ? "queued" : "—"}</div><div class="m">&nbsp;</div></div></td>`;
      }
      const errored = rs.filter((r) => r.error);
      const counted = rs.filter((r) => !r.error);
      const name = `${task} · ${labelOf(col.split("|")[0])}`;
      if (!counted.length) {
        const why = `${name}\nThe harness errored before working on the task (not counted):\n${errored.map((r) => r.error).join("\n")}`;
        return `<td><div class="hb-cell error" data-why="${esc(why)}"><div class="st">⚠ error</div><div class="m">${errored.length} run${errored.length === 1 ? "" : "s"} not counted</div></div></td>`;
      }
      const passed = counted.filter((r) => r.pass).length;
      const failed = counted.find((r) => !r.pass);
      const all = passed === counted.length;
      const notes = [failed ? `${failed.timed_out ? "timed out\n" : ""}${failed.why}` : "", errored.length ? `Harness errors (not counted):\n${errored.map((r) => r.error).join("\n")}` : ""].filter(Boolean);
      const why = notes.length ? `${name}\n${notes.join("\n\n")}` : "";
      return `<td><div class="hb-cell ${all ? "pass" : "fail"}${errored.length ? " has-error" : ""}" ${why ? `data-why="${esc(why)}"` : ""}>
        <div class="st">${all ? "✓ passed" : `✗ ${counted.length > 1 ? `${passed}/${counted.length}` : "failed"}`}${errored.length ? ` <span style="color:var(--color-warning)" title="harness errors">⚠${errored.length}</span>` : ""}</div>
        <div class="m">${secs(median(counted.map((r) => r.seconds)))}${counted.some((r) => r.input > 0) ? ` · ${tokens(median(counted.map((r) => r.input - r.cached)))} tok` : ""}</div></div></td>`;
    };
    return `<div class="hb-panel"><h2>Per task</h2><p class="hb-sub">Pass or fail with median time and uncached tokens. Hover a failed cell for the reason.</p>
      <div style="overflow-x:auto"><table class="hb-grid-table">
        <tr><th></th>${cols.map((c) => `<th><span class="hb-dot" style="display:inline-block;margin-right:6px;background:${colorOf(c.split("|")[0])}"></span>${esc(labelOf(c.split("|")[0]))}</th>`).join("")}</tr>
        ${groups
          .map(
            ([s, ts]) =>
              (groups.length > 1 ? `<tr class="hb-grid-suite"><td colspan="${cols.length + 1}">${esc(suiteLabel(s))}</td></tr>` : "") +
              ts.map((t) => `<tr><td class="task">${esc(t)}</td>${cols.map((c) => cell(t, c)).join("")}</tr>`).join(""),
          )
          .join("")}
      </table></div>
    </div>`;
  }

  function table(rows) {
    if (!rows.length) return "";
    const tokenKeys = ["input", "uncached", "output"];
    const known = (r, k) => r.hasTokens || !tokenKeys.includes(k);
    const best = (k) => Math.min(...rows.filter((r) => r.passed && known(r, k)).map((r) => r[k]));
    const td = (r, k, f) => (known(r, k) ? `<td class="${r.passed && r[k] === best(k) ? "hb-best" : ""}">${f(r[k])}</td>` : `<td class="wf-subtle">—</td>`);
    return `<div class="hb-panel"><details ${prefs().tableOpen ? "open" : ""} data-pref-details="tableOpen"><summary>Table view</summary>
      <div style="overflow-x:auto;margin-top:10px"><table class="hb-table">
        <tr><th>Harness</th><th>Model</th><th>Passed</th><th>Time</th><th>Input</th><th>Uncached</th><th>Output</th><th>Tool calls</th><th>Cost</th></tr>
        ${rows
          .map(
            (r) => `<tr><td><span class="hb-dot" style="display:inline-block;margin-right:6px;background:${colorOf(r.harness)}"></span>${esc(labelOf(r.harness))}</td><td class="wf-mono">${esc(r.model)}</td>
            <td>${r.passed}/${r.runs}</td>${td(r, "seconds", secs)}${td(r, "input", tokens)}${td(r, "uncached", tokens)}${td(r, "output", tokens)}${td(r, "tools", (v) => String(Math.round(v)))}
            <td title="${esc(costNote(r.costEstimated, r.costPartial))}">${r.cost == null ? "—" : `${r.costEstimated ? "~" : ""}$${r.cost.toFixed(3)}${r.costPartial ? " (partial)" : ""}`}</td></tr>`,
          )
          .join("")}
      </table></div></details></div>`;
  }

  const counted = (rs) => rs.filter((r) => !CHECKS.includes(r.harness) || state.viewing?.includes("selftest"));

  /** Narrows the results tab to one suite; only shown when the run spans several. */
  function suiteBar() {
    const suites = suitesIn(state.results, state.catalog);
    if (suites.length < 2) return "";
    const stat = (rs) => {
      const ok = counted(rs).filter((r) => !r.error);
      return ok.length ? ` <span class="hb-count">${ok.filter((r) => r.pass).length}/${ok.length}</span>` : "";
    };
    const btn = (id, label, rs, title = "") =>
      `<button data-suite="${esc(id)}" aria-pressed="${state.suite === id}" ${title ? `title="${esc(title)}"` : ""}>${esc(label)}${stat(rs)}</button>`;
    return `<div class="hb-seg hb-suitebar" role="group" aria-label="Suite">${btn("all", "All suites", state.results)}${suites
      .map((s) => btn(s, suiteLabel(s), state.results.filter((r) => suiteOf(r, state.catalog.tasks) === s), catalogSuites().find((x) => x.id === s)?.description))
      .join("")}</div>`;
  }

  /** Pass rate per harness and suite: where each harness is strong or weak. */
  function suitesPanel() {
    const { suites, rows } = suiteMatrix(counted(state.results), state.catalog);
    if (suites.length < 2 || !rows.length) return "";
    const order = (r) => {
      const i = state.catalog.harnesses.findIndex((h) => h.id === r.harness);
      return i < 0 ? 99 : i;
    };
    rows.sort((a, b) => order(a) - order(b));
    const rate = (c) => (c?.runs ? c.passed / c.runs : -1);
    const best = Object.fromEntries(suites.map((s) => [s, Math.max(...rows.map((r) => rate(r.cells[s])))]));
    const cell = (r, s) => {
      const c = r.cells[s];
      if (!c?.runs) return `<td class="wf-subtle">—</td>`;
      const v = c.passed / c.runs;
      return `<td class="${v > 0 && v === best[s] && rows.length > 1 ? "hb-best" : ""}"><div class="hb-pass"><span>${c.passed}/${c.runs}</span><div class="hb-meter"><div style="width:${Math.round(v * 100)}%;background:var(--color-success)"></div></div></div></td>`;
    };
    return `<div class="hb-panel"><h2>By suite</h2><p class="hb-sub">Passed runs per suite. Harness errors aren't counted. Pick a suite above to see only its tasks.</p>
      <div style="overflow-x:auto"><table class="hb-table hb-suite-table">
        <tr><th>Harness</th>${suites.map((s) => `<th>${esc(suiteLabel(s))}</th>`).join("")}</tr>
        ${rows
          .map(
            (r) =>
              `<tr><td><span class="hb-dot" style="display:inline-block;margin-right:6px;background:${colorOf(r.harness)}"></span>${esc(labelOf(r.harness))}${r.model ? ` <span class="wf-subtle wf-mono">${esc(r.model)}</span>` : ""}</td>${suites.map((s) => cell(r, s)).join("")}</tr>`,
          )
          .join("")}
      </table></div></div>`;
  }

  function empty() {
    return `<div class="hb-empty">
      <svg width="64" height="48" viewBox="0 0 64 48" aria-hidden="true"><rect x="4" y="26" width="10" height="18" rx="3" fill="${SERIES.dark[0]}" opacity=".85"/><rect x="20" y="14" width="10" height="30" rx="3" fill="${SERIES.dark[1]}" opacity=".85"/><rect x="36" y="20" width="10" height="24" rx="3" fill="${SERIES.dark[2]}" opacity=".85"/><rect x="52" y="6" width="10" height="38" rx="3" fill="${SERIES.dark[3]}" opacity=".85"/></svg>
      <h3>No results yet</h3>
      <p>Pick harnesses and models on the left, choose the tasks, then Run bench. Harnesses run side by side; results appear here as each run finishes.</p>
    </div>`;
  }

  function render() {
    if (!root) return;
    if (state.error) {
      root.innerHTML = `<style>${STYLE}</style><div class="hb"><div class="wf-error">${esc(state.error)}</div></div>`;
      return;
    }
    if (!state.catalog) {
      root.innerHTML = `<style>${STYLE}</style><div class="hb"><div class="hb-empty"><h3>Loading the bench…</h3></div></div>`;
      return;
    }
    // Keep the scroll position of whatever element scrolls the page across re-renders.
    const scrollers = () => [root, root.parentElement, root.parentElement?.parentElement].filter(Boolean);
    const saved = scrollers().map((e) => e.scrollTop);
    const focused = document.activeElement?.dataset?.model;
    const rows = summarize(shown().filter((r) => !CHECKS.includes(r.harness) || state.viewing?.includes("selftest")), state.catalog.prices);
    const order = (r) => {
      const i = state.catalog.harnesses.findIndex((h) => h.id === r.harness);
      return i < 0 ? 99 : i;
    };
    rows.sort((a, b) => order(a) - order(b));
    const hasResults = state.results.length > 0;
    root.innerHTML = `<style>${STYLE}</style>
    <div class="hb">
      ${header()}
      ${tabs()}
      ${state.tab === "runs" ? runsView() : `<div class="hb-body">
        ${setup()}
        <main class="hb-main">
          ${live()}
          ${hasResults ? suiteBar() : ""}
          ${hasResults ? winner(rows) : ""}
          ${hasResults ? kpis(rows) : ""}
          ${usagePanel()}
          ${hasResults && state.suite === "all" ? suitesPanel() : ""}
          ${hasResults || state.running ? charts(rows) : ""}
          ${grid()}
          ${table(rows)}
          ${!hasResults && !state.running ? empty() : ""}
        </main>
      </div>`}
    </div>`;
    scrollers().forEach((e, i) => (e.scrollTop = saved[i]));
    // Charts need the main column's width; redraw once it's known or when it changed.
    const measured = root.querySelector(".hb-main")?.clientWidth;
    if (measured && Math.abs(measured - mainWidth) > 4) {
      mainWidth = measured;
      render();
    }
    if (focused) root.querySelector(`[data-model="${CSS.escape(focused)}"]`)?.focus();
  }

  // ---- events ----

  function onEvent(e) {
    const t = e.target;
    if (!(t instanceof HTMLElement || t instanceof SVGElement)) return;
    const d = t.dataset ?? {};
    if (e.type === "click") {
      const button = t.closest?.("button") ?? t;
      const bd = button.dataset ?? {};
      if (bd.action === "start") return void start();
      if (bd.action === "configure") {
        state.tab = "results";
        render();
        return void root?.querySelector(".hb-aside")?.scrollIntoView({ block: "start", behavior: "smooth" });
      }
      if (bd.action === "selftest") return void start(true);
      if (bd.action === "stop") return void stop();
      if (bd.action === "reveal" && state.viewing) return void api.revealFile(state.viewing);
      if (bd.tab) {
        state.tab = bd.tab;
        if (bd.tab === "runs") loadOverviews();
        return render();
      }
      if (bd.deleteRun) return void deleteRuns([bd.deleteRun]);
      if (bd.action === "delete-selected") return void deleteRuns([...state.selected]);
      if (bd.action === "clear-selection") {
        state.selected.clear();
        return render();
      }
      const open = t.closest?.("[data-open-run]");
      // A row's checkbox and delete button act on their own, they don't open the run.
      if (open && t.closest?.("input, button, a")) return;
      if (open && !state.running) {
        state.tab = "results";
        return void view(`${data}/${open.dataset.openRun}`);
      }
      if (open) {
        state.tab = "results";
        return render();
      }
      if (bd.tasksAll) {
        const on = bd.tasksAll === "on";
        setPrefs({ tasks: Object.fromEntries((state.catalog?.tasks ?? []).map((x) => [x.id, on])) });
        return render();
      }
      if (bd.suiteAll) {
        const [suite, v] = bd.suiteAll.split(":");
        const picks = (state.catalog?.tasks ?? []).filter((x) => taskSuite(x) === suite).map((x) => [x.id, v === "on"]);
        setPrefs({ tasks: { ...(prefs().tasks ?? {}), ...Object.fromEntries(picks) } });
        return render();
      }
      if (bd.suite) {
        state.suite = bd.suite;
        return render();
      }
      if (bd.task) {
        setPrefs({ tasks: { ...(prefs().tasks ?? {}), [bd.task]: bd.task && !(prefs().tasks?.[bd.task] ?? true) } });
        return render();
      }
      if (bd.preset && PRESETS[bd.preset]) {
        setPrefs({ weights: { ...PRESETS[bd.preset].weights } });
        return render();
      }
      if (bd.parallel) {
        setPrefs({ parallel: bd.parallel });
        return render();
      }
    }
    if (e.type === "toggle" && t.dataset?.prefDetails) {
      setPrefs({ [t.dataset.prefDetails]: t.open });
      return;
    }
    if (e.type === "change") {
      if (d.selectRun) {
        if (t.checked) state.selected.add(d.selectRun);
        else state.selected.delete(d.selectRun);
        return render();
      }
      if (d.selectAll !== undefined) {
        const live = state.running?.log.split(/[\\/]/).pop();
        state.selected = new Set(t.checked ? state.history.filter((f) => f !== live) : []);
        return render();
      }
      if (d.weight) {
        setPrefs({ weights: { ...weights(), [d.weight]: Number(t.value) } });
        return render();
      }
      if (d.harness) {
        const hs = { ...(prefs().harnesses ?? {}) };
        hs[d.harness] = { ...(hs[d.harness] ?? {}), on: t.checked };
        setPrefs({ harnesses: hs });
      } else if (d.model) {
        const hs = { ...(prefs().harnesses ?? {}) };
        hs[d.model] = { ...(hs[d.model] ?? {}), model: t.value.trim() };
        setPrefs({ harnesses: hs });
      } else if (d.pref) {
        setPrefs({ [d.pref]: t.type === "checkbox" ? t.checked : t.value });
      } else if (d.action === "history") {
        return void view(`${data}/${t.value}`);
      }
      render();
    }
  }

  function showWhy(cell) {
    clearTimeout(popHide);
    if (popFor === cell) return;
    popFor = cell;
    pop.textContent = cell.dataset.why;
    pop.classList.add("on");
    pop.scrollTop = 0;
    // Beside the cell (right, else left), kept on screen.
    const r = cell.getBoundingClientRect();
    let x = r.right + 6;
    if (x + pop.offsetWidth > window.innerWidth - 8) x = Math.max(8, r.left - pop.offsetWidth - 6);
    pop.style.left = `${x}px`;
    pop.style.top = `${Math.max(8, Math.min(r.top, window.innerHeight - pop.offsetHeight - 8))}px`;
  }

  // Delayed so the pointer can move into the popup to scroll or copy it.
  function hideWhy() {
    clearTimeout(popHide);
    popHide = setTimeout(() => {
      pop?.classList.remove("on");
      popFor = null;
    }, 200);
  }

  function onHover(e) {
    if (pop) {
      const cell = e.type === "mouseleave" ? null : e.target?.closest?.("[data-why]");
      if (cell) showWhy(cell);
      else if (popFor) hideWhy();
    }
    if (!tip) return;
    const target = e.target?.closest?.("[data-tip]");
    if (!target || e.type === "mouseleave") {
      tip.classList.remove("on");
      return;
    }
    tip.textContent = target.getAttribute("data-tip");
    tip.classList.add("on");
    const x = Math.min(e.clientX + 14, window.innerWidth - tip.offsetWidth - 8);
    const y = Math.min(e.clientY + 14, window.innerHeight - tip.offsetHeight - 8);
    tip.style.left = `${x}px`;
    tip.style.top = `${y}px`;
  }

  return api.registerPage({
    id: "bench",
    title: "winfer bench",
    icon: "gauge",
    mount(el) {
      root = el;
      tip = document.createElement("div");
      tip.className = "hb-tip";
      tip.setAttribute("role", "tooltip");
      document.body.appendChild(tip);
      pop = document.createElement("div");
      pop.className = "hb-why";
      pop.setAttribute("role", "tooltip");
      pop.addEventListener("mouseenter", () => clearTimeout(popHide));
      pop.addEventListener("mouseleave", hideWhy);
      document.body.appendChild(pop);
      el.addEventListener("click", onEvent);
      el.addEventListener("change", onEvent);
      el.addEventListener("toggle", onEvent, true);
      el.addEventListener("mousemove", onHover);
      el.addEventListener("mouseleave", onHover);
      const resize = new ResizeObserver(() => {
        const w = root?.querySelector(".hb-main")?.clientWidth;
        if (w && Math.abs(w - mainWidth) > 4) render();
      });
      resize.observe(el);
      render();
      loadCatalog();
      loadHistory();
      // Re-attach to a run still going from before (the page was closed or winfer reloaded).
      api
        .readFile(status)
        .then((text) => {
          const s = JSON.parse(text);
          const jobs = runningJobs(s);
          if (jobs.length && !s.finished && Date.now() / 1000 - Math.min(...jobs.map((j) => j.since)) < 3600) {
            const name = s.log.split(/[\\/]/).pop();
            return sh(`pgrep -f ${q(`run\\.py.*${name}`)} | head -1 || true`).then((pid) => {
              if (pid.trim()) {
                return api
                  .readFile(usageFile(s.log))
                  .then(JSON.parse)
                  .catch(() => null)
                  .then((usage) => {
                    state.running = { pid: Number(pid.trim()), log: s.log, usage, started: Math.min(...jobs.map((j) => j.since)) };
                    state.viewing = s.log;
                    state.usage = usage;
                    poll();
                  });
              }
            });
          }
        })
        .catch(() => {});
      return () => {
        clearTimeout(timer);
        resize.disconnect();
        el.removeEventListener("click", onEvent);
        el.removeEventListener("change", onEvent);
        el.removeEventListener("toggle", onEvent, true);
        el.removeEventListener("mousemove", onHover);
        el.removeEventListener("mouseleave", onHover);
        tip?.remove();
        tip = null;
        clearTimeout(popHide);
        pop?.remove();
        pop = popFor = null;
        root = null;
      };
    },
  });
}
