// Harness Bench: runs bench/run.py (shipped in this plugin) on the harnesses, models and tasks
// picked on the page, in the background, and shows live progress, charts and results.

const STORE_KEY = "winfer.harness-bench";
const POLL_MS = 1500;
const CHECKS = ["oracle", "noop"]; // self-tests of the checks, not harnesses

const esc = (s) => String(s ?? "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]);
const q = (s) => `'${String(s).replace(/'/g, `'\\''`)}'`; // POSIX shell quoting
const tokens = (n) => (n >= 1e6 ? `${(n / 1e6).toFixed(2)}M` : n >= 1000 ? `${(n / 1000).toFixed(1)}k` : String(Math.round(n || 0)));
const secs = (s) => (s >= 60 ? `${Math.floor(s / 60)}m ${String(Math.round(s % 60)).padStart(2, "0")}s` : `${Math.round(s)}s`);
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

/** Per-run results → summary per harness+model. */
export function summarize(results) {
  const groups = new Map();
  for (const r of results) {
    const key = `${r.harness}|${r.model ?? ""}`;
    if (!groups.has(key)) groups.set(key, []);
    groups.get(key).push(r);
  }
  return [...groups.entries()].map(([key, all]) => {
    const [harness, model] = key.split("|");
    // Runs where the harness itself errored (rate limit, crash) say nothing about the task.
    const rs = all.filter((r) => !r.error);
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
.hb-chips { display: flex; flex-wrap: wrap; gap: 6px; }
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
.hb-cell.fail { cursor: pointer; border-color: color-mix(in srgb, var(--color-danger) 45%, var(--color-border)); }
.hb-cell.fail .st { color: var(--color-danger); }
.hb-cell.busy { border-style: dashed; }
.hb-cell.error { cursor: pointer; border-style: dashed; border-color: color-mix(in srgb, var(--color-warning) 55%, var(--color-border)); }
.hb-cell.error .st { color: var(--color-warning); }
.hb-cell.has-error { cursor: pointer; }
.hb-cell.queued { opacity: .5; }
.hb-why { margin-top: 12px; white-space: pre-wrap; font-family: var(--font-mono, ui-monospace, monospace); font-size: 11px; background: var(--color-panel); border: 1px solid var(--color-border); border-radius: 8px; padding: 10px 12px; max-height: 200px; overflow: auto; }
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
`;

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
    why: null,
    starting: false,
  };
  let root = null;
  let tip = null;
  let timer = null;

  const sh = async (command, timeoutSec = 60) => {
    const r = await api.vault.exec({ command, cwd: bench, keys: [], timeoutSec });
    if (r.code !== 0) throw new Error(r.output.trim() || `exit ${r.code}`);
    return r.output;
  };

  const prefs = () => state.prefs;
  const setPrefs = (patch) => {
    state.prefs = { ...state.prefs, ...patch };
    savePrefs(state.prefs);
  };
  const harnessPref = (h) => ({ on: ["winfer-chatgpt", "codex"].includes(h.id), model: h.model, ...(prefs().harnesses?.[h.id] ?? {}) });
  const taskOn = (t) => prefs().tasks?.[t.id] ?? true;
  const harnesses = () => (state.catalog?.harnesses ?? []).filter((h) => !CHECKS.includes(h.id));
  const labelOf = (id) => state.catalog?.harnesses.find((h) => h.id === id)?.label ?? id;
  /** The label for chart axes, where space is short. */
  const shortLabel = (id) => labelOf(id).replace(/^winfer code · /, "winfer · ");
  const colorOf = (id) => {
    const i = harnesses().findIndex((h) => h.id === id);
    const set = isLightTheme() ? SERIES.light : SERIES.dark;
    return i >= 0 && i < set.length ? set[i] : "var(--color-fg-subtle)";
  };

  async function loadCatalog() {
    try {
      state.catalog = JSON.parse(await sh(`python3 run.py --list`));
      state.error = null;
    } catch (e) {
      state.error = `Couldn't read the bench: ${e.message}. It needs python3.`;
    }
    render();
  }

  async function loadHistory() {
    try {
      const out = await sh(`mkdir -p ${q(data)} && ls -1t ${q(data)} | grep '\\.jsonl$' | head -30 || true`);
      state.history = out.split("\n").map((l) => l.trim()).filter(Boolean);
    } catch {
      state.history = [];
    }
    if (!state.viewing && state.history[0]) await view(`${data}/${state.history[0]}`);
    render();
  }

  async function view(log) {
    state.viewing = log;
    state.why = null;
    try {
      state.results = parseJsonl(await api.readFile(log)).map(withLegacyErrors);
    } catch {
      state.results = [];
    }
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
      `-H ${q(hs.map((h) => h.id).join(","))} -t ${q(ts.map((t) => t.id.slice(0, 2)).join(","))} -n ${runs} --timeout ${timeout} -p ${q(parallel)} ${models} ` +
      `${prefs().keep && !selfTest ? "--keep " : ""}--log ${q(log)} --status ${q(status)} > ${q(log.replace(/\.jsonl$/, ".out"))} 2>&1 < /dev/null & } ; echo $!`;
    state.starting = true;
    render();
    try {
      const pid = Number((await sh(cmd, 20)).trim().split("\n").pop());
      if (!pid) throw new Error("the runner didn't start");
      state.running = { pid, log, started: Date.now() / 1000, plan: { cols: hs.map((h) => `${h.id}|${h.model ? model(h) : ""}`), tasks: ts.map((t) => t.id) } };
      state.progress = { total: hs.length * ts.length * runs, done: 0 };
      state.results = [];
      state.viewing = log;
      state.why = null;
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
      const passed = state.results.filter((r) => r.pass).length;
      api.toast(`Bench finished: ${passed}/${state.results.length} passed`, "success");
      state.running = null;
      render();
      loadHistory();
      return;
    }
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
        if (w > 64) svg += `<text x="${x0 + 6}" y="${y + 11}" style="font-size:10px;fill:#fff;opacity:.92">${esc(r.task.slice(3))}</text>`;
        svg += `<rect class="hit" x="${x0}" y="${y - 6}" width="${w}" height="26" data-tip="${esc(tipText)}"/>`;
      }
    });
    return svg + `</svg>`;
  }

  // ---- page sections ----

  function header() {
    const running = !!state.running;
    const runLabel = (f) => {
      const m = /^(\d{4})(\d{2})(\d{2})-(\d{2})(\d{2})(\d{2})(-selftest)?/.exec(f);
      return m ? `${m[3]}.${m[2]}. ${m[4]}:${m[5]}${m[7] ? " · self-test" : ""}` : f.replace(".jsonl", "");
    };
    const current = state.viewing && !state.history.some((f) => state.viewing.endsWith(f));
    return `<div class="hb-head">
      <div class="hb-title"><div class="hb-logo">${ICON}</div><div><h1>Harness Bench</h1><p>Same tasks, same models, different harnesses: what passes, how fast, how many tokens.</p></div></div>
      <div class="hb-actions">
        <select class="wf-select" data-action="history" aria-label="Results to show" ${running ? "disabled" : ""}>
          ${current ? `<option selected>${esc(runLabel(state.viewing.split(/[\\/]/).pop()))}${running ? " · running" : ""}</option>` : ""}
          ${state.history.length || current ? "" : `<option>No runs yet</option>`}
          ${state.history.map((f) => `<option value="${esc(f)}" ${state.viewing?.endsWith(f) ? "selected" : ""}>${esc(runLabel(f))}</option>`).join("")}
        </select>
        <button class="wf-button wf-ghost" data-action="reveal" ${state.viewing ? "" : "disabled"} title="Show the results file">Open file</button>
        <button class="wf-button wf-ghost" data-action="selftest" ${running || state.starting ? "disabled" : ""} title="Run the reference solutions (every task should pass) and a no-op (every task should fail) to confirm the checks are trustworthy. No AI, no tokens.">Check the checks</button>
        ${
          running
            ? `<button class="wf-button wf-danger" data-action="stop">Stop</button>`
            : `<button class="wf-button wf-primary" data-action="start" ${windows || state.starting ? "disabled" : ""} ${windows ? "title='macOS and Linux only for now'" : ""}>${state.starting ? "Starting…" : "Run bench"}</button>`
        }
      </div>
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
    const tasks = c.tasks
      .map((t) => `<button class="hb-chip" data-task="${esc(t.id)}" aria-pressed="${taskOn(t)}" title="${esc(t.prompt)}">${esc(t.id.replace(/^\d+-/, ""))}</button>`)
      .join("");
    const par = p.parallel ?? "harness";
    const seg = (v, label, title) => `<button data-parallel="${v}" aria-pressed="${par === v}" title="${esc(title)}">${label}</button>`;
    return `<aside class="hb-aside">
      <div class="hb-panel"><div class="hb-label">Harnesses & models</div>${hs}
        <p class="hb-sub" style="margin:10px 0 0">Compare on the same model for a fair read. Colors stay with each harness in every chart.</p></div>
      <div class="hb-panel"><div class="hb-label">Tasks</div><div class="hb-chips">${tasks}</div></div>
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
        <div class="hb-sub" style="margin:2px 0 0">${jobs.length} running · ${secs(elapsed)} elapsed${jobs.length ? ` · ${esc(jobs.map((j) => `${labelOf(j.harness)} on ${j.task.replace(/^\d+-/, "")}`).join(", "))}` : ""}</div>
      </div>
    </div>`;
  }

  function kpis(rows) {
    const errors = state.results.filter((r) => r.error).length;
    const runs = state.results.length - errors;
    const passed = state.results.filter((r) => r.pass).length;
    const ok = rows.filter((r) => r.passed > 0);
    const fastest = [...ok].sort((a, b) => a.seconds - b.seconds)[0];
    const leanest = [...ok].filter((r) => r.hasTokens).sort((a, b) => a.uncached - b.uncached)[0];
    const timed = state.results.filter((r) => r.started);
    const wall = timed.length ? Math.max(...timed.map((r) => r.started + r.seconds)) - Math.min(...timed.map((r) => r.started)) : 0;
    const cost = rows.some((r) => r.cost != null) ? rows.reduce((a, r) => a + (r.cost || 0), 0) : null;
    const who = (r) => (r ? `<span class="hb-dot" style="background:${colorOf(r.harness)}"></span>${esc(labelOf(r.harness))}` : "—");
    const rate = runs ? passed / runs : 0;
    return `<div class="hb-kpis">
      <div class="hb-kpi"><div class="k">Passed</div><div class="v">${passed}<span style="font-size:14px;color:var(--color-fg-muted);font-weight:500"> / ${runs}</span></div>
        <div class="hb-meter"><div style="width:${Math.round(rate * 100)}%;background:var(--color-success)"></div></div>
        ${errors ? `<div class="s" style="color:var(--color-warning)" title="Runs where the harness itself failed (rate limit, crash, not signed in); not counted">⚠ ${errors} harness error${errors === 1 ? "" : "s"} not counted</div>` : ""}</div>
      <div class="hb-kpi"><div class="k">Fastest (median per task)</div><div class="v">${fastest ? secs(fastest.seconds) : "—"}</div><div class="s">${who(fastest)}</div></div>
      <div class="hb-kpi"><div class="k">Leanest (uncached tokens)</div><div class="v">${leanest ? tokens(leanest.uncached) : "—"}</div><div class="s">${who(leanest)}</div></div>
      <div class="hb-kpi"><div class="k">Wall time</div><div class="v">${wall ? secs(wall) : "—"}</div><div class="s">${rows.length} harness${rows.length === 1 ? "" : "es"} · ${new Set(state.results.map((r) => r.task)).size} tasks</div></div>
      ${cost != null ? `<div class="hb-kpi"><div class="k">Cost at API prices</div><div class="v">$${cost.toFixed(2)}</div><div class="s">where the harness reports it</div></div>` : ""}
    </div>`;
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
    const tl = timeline(state.results, jobs, plan, full);
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
    if (!state.results.length && !plan) return "";
    const cols = [...new Set([...(plan?.cols ?? []), ...state.results.map((r) => `${r.harness}|${r.model ?? ""}`)])];
    const tasks = [...new Set([...(plan?.tasks ?? []), ...state.results.map((r) => r.task)])].sort();
    const busy = new Set(state.running ? runningJobs(state.progress).map((j) => `${j.task}|${j.harness}`) : []);
    const cell = (task, col) => {
      const rs = state.results.filter((r) => r.task === task && `${r.harness}|${r.model ?? ""}` === col);
      if (!rs.length) {
        if (busy.has(`${task}|${col.split("|")[0]}`)) return `<td><div class="hb-cell busy hb-pulse"><div class="st">● running</div><div class="m">&nbsp;</div></div></td>`;
        return `<td><div class="hb-cell queued"><div class="st" style="color:var(--color-fg-subtle)">${state.running ? "queued" : "—"}</div><div class="m">&nbsp;</div></div></td>`;
      }
      const errored = rs.filter((r) => r.error);
      const counted = rs.filter((r) => !r.error);
      const name = `${task} · ${labelOf(col.split("|")[0])}`;
      if (!counted.length) {
        const why = `${name}\nThe harness errored before working on the task (not counted):\n${errored.map((r) => r.error).join("\n")}`;
        return `<td><div class="hb-cell error" data-why="${esc(why)}" title="Click for the harness error"><div class="st">⚠ error</div><div class="m">${errored.length} run${errored.length === 1 ? "" : "s"} not counted</div></div></td>`;
      }
      const passed = counted.filter((r) => r.pass).length;
      const failed = counted.find((r) => !r.pass);
      const all = passed === counted.length;
      const notes = [failed ? `${failed.timed_out ? "timed out\n" : ""}${failed.why}` : "", errored.length ? `Harness errors (not counted):\n${errored.map((r) => r.error).join("\n")}` : ""].filter(Boolean);
      const why = notes.length ? `${name}\n${notes.join("\n\n")}` : "";
      return `<td><div class="hb-cell ${all ? "pass" : "fail"}${errored.length ? " has-error" : ""}" ${why ? `data-why="${esc(why)}" title="Click for details"` : ""}>
        <div class="st">${all ? "✓ passed" : `✗ ${counted.length > 1 ? `${passed}/${counted.length}` : "failed"}`}${errored.length ? ` <span style="color:var(--color-warning)" title="harness errors">⚠${errored.length}</span>` : ""}</div>
        <div class="m">${secs(median(counted.map((r) => r.seconds)))}${counted.some((r) => r.input > 0) ? ` · ${tokens(median(counted.map((r) => r.input - r.cached)))} tok` : ""}</div></div></td>`;
    };
    return `<div class="hb-panel"><h2>Per task</h2><p class="hb-sub">Pass or fail with median time and uncached tokens. Click a failed cell for the reason.</p>
      <div style="overflow-x:auto"><table class="hb-grid-table">
        <tr><th></th>${cols.map((c) => `<th><span class="hb-dot" style="display:inline-block;margin-right:6px;background:${colorOf(c.split("|")[0])}"></span>${esc(labelOf(c.split("|")[0]))}</th>`).join("")}</tr>
        ${tasks.map((t) => `<tr><td class="task">${esc(t)}</td>${cols.map((c) => cell(t, c)).join("")}</tr>`).join("")}
      </table></div>
      ${state.why ? `<div class="hb-why">${esc(state.why)}</div>` : ""}
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
            <td>${r.cost == null ? "—" : `$${r.cost.toFixed(3)}`}</td></tr>`,
          )
          .join("")}
      </table></div></details></div>`;
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
    const rows = summarize(state.results.filter((r) => !CHECKS.includes(r.harness) || state.viewing?.includes("selftest")));
    const order = (r) => {
      const i = state.catalog.harnesses.findIndex((h) => h.id === r.harness);
      return i < 0 ? 99 : i;
    };
    rows.sort((a, b) => order(a) - order(b));
    const hasResults = state.results.length > 0;
    root.innerHTML = `<style>${STYLE}</style>
    <div class="hb">
      ${header()}
      <div class="hb-body">
        ${setup()}
        <main class="hb-main">
          ${live()}
          ${hasResults ? winner(rows) : ""}
          ${hasResults ? kpis(rows) : ""}
          ${hasResults || state.running ? charts(rows) : ""}
          ${grid()}
          ${table(rows)}
          ${!hasResults && !state.running ? empty() : ""}
        </main>
      </div>
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
      if (bd.action === "selftest") return void start(true);
      if (bd.action === "stop") return void stop();
      if (bd.action === "reveal" && state.viewing) return void api.revealFile(state.viewing);
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
      const why = t.closest?.("[data-why]");
      if (why) {
        state.why = state.why === why.dataset.why ? null : why.dataset.why;
        return render();
      }
    }
    if (e.type === "toggle" && t.dataset?.prefDetails) {
      setPrefs({ [t.dataset.prefDetails]: t.open });
      return;
    }
    if (e.type === "change") {
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

  function onHover(e) {
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
    title: "Bench",
    icon: "star",
    mount(el) {
      root = el;
      tip = document.createElement("div");
      tip.className = "hb-tip";
      tip.setAttribute("role", "tooltip");
      document.body.appendChild(tip);
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
                state.running = { pid: Number(pid.trim()), log: s.log, started: Math.min(...jobs.map((j) => j.since)) };
                state.viewing = s.log;
                poll();
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
        root = null;
      };
    },
  });
}
