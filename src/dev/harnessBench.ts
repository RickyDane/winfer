/**
 * Dev-only fake runner for the winfer bench plugin (plugins/harness-bench): answers its
 * `run.py --list`, history listing and start/stop commands, and serves a status file and
 * results that grow while a fake run is going. Open the dev server with `?bench`.
 */

import catalog from "../../plugins/harness-bench/bench/harnesses.json";

export const BENCH_DEMO = new URLSearchParams(location.search).has("bench");

const harnesses = [
  { id: "winfer-chatgpt", label: "winfer code · ChatGPT", model: "gpt-6.1-sol", models: ["gpt-6.1-sol", "gpt-6-astra", "gpt-6-luna"], installed: true },
  { id: "winfer-claude", label: "winfer code · Claude", model: "sonnet", models: ["fable", "opus", "sonnet", "haiku"], installed: true },
  { id: "winfer-codex", label: "winfer code · Codex", model: "gpt-6.1-sol", models: ["gpt-6.1-sol", "gpt-6-luna"], installed: true },
  { id: "codex", label: "Codex", model: "gpt-6.1-sol", models: ["gpt-6.1-sol", "gpt-6-luna"], installed: true },
  { id: "claude", label: "Claude Code", model: "sonnet", models: ["fable", "opus", "sonnet", "haiku"], installed: true },
  { id: "pi", label: "pi", model: "openai/gpt-6.1-sol", models: ["openai/gpt-6.1-sol"], installed: true },
  { id: "opencode", label: "OpenCode", model: "openai/gpt-6.1-sol", models: [], installed: false },
];
// Generated from `run.py --list` (prompts cut to 80 characters).
const suites = [
  { "id": "coding", "label": "Coding", "description": "Change code to a spec: bug fixes, features, renames, refactors, performance. Hidden tests decide." },
  { "id": "understanding", "label": "Understanding", "description": "Read and reason about code without changing it: trace calls, predict output, locate bugs, count usages. Exact answers only." },
  { "id": "research", "label": "Research", "description": "Combine facts from many sources (docs, changelogs, specs, vendored libraries, a local doc site) into exact, cited answers. Offline and reproducible." },
  { "id": "discipline", "label": "Discipline", "description": "Follow instructions exactly and stay in scope: minimal diffs, protected files, exact formats, planted instructions, ambiguity, no faked tests." },
];
const tasks = [
  ["01-bugfix", "coding", "The tests in tests/ fail. Find and fix the bugs in the shop package so that `…"],
  ["02-feature", "coding", "Extend the todo CLI in todo.py:  1. `add` takes an optional `--priority` of `…"],
  ["03-rename", "coding", "Rename the function `compute_tax` to `calculate_tax` everywhere in this proje…"],
  ["05-bigfile", "coding", "logs/app.log is a large application log. Write report.py that prints the 3 mo…"],
  ["06-js-lru", "coding", "Implement the cache described in README.md in src/lru.js (ES module, no depen…"],
  ["07-py-perf", "coding", "`dedupe_contacts` and `count_paths` in datatools/ are far too slow on real in…"],
  ["08-js-async-bug", "coding", "`node --test` fails: the async helpers in src/async.js (`mapLimit` and `retry…"],
  ["09-py-csv-import", "coding", "Implement the CSV importer specified in README.md: `parse` and `load` in impo…"],
  ["11-py-pagination", "coding", "The tests in tests/ fail. Fix the bugs in the pager package (page numbers, cu…"],
  ["12-sqlite-migration", "coding", "The contacts app stores a single `people.name` column. Split it into `first_n…"],
  ["13-shell-fix", "coding", "backup.sh is buggy: it breaks on file and directory names with spaces, keeps …"],
  ["14-py-deprecation", "coding", "`Client.fetch(url, timeout)` in app/http/client.py is deprecated. Migrate eve…"],
  ["15-js-event-emitter", "coding", "Implement the event emitter described in README.md in src/emitter.js (ES modu…"],
  ["16-py-ttl-cache", "coding", "Implement the `ttl_cache` decorator described in README.md in ttlcache/__init…"],
  ["17-js-cli-args", "coding", "Implement the command-line argument parser described in README.md in src/args…"],
  ["18-py-path-traversal", "coding", "Security report: the static file server in fileserver/static.py has a path tr…"],
  ["20-py-multi-module-bug", "coding", "Bug report: after a reservation is cancelled, restocking the item makes the c…"],
  ["21-js-fix-tests", "coding", "`node --test` fails: the date-range utilities in src/ranges.js have bugs. Fix…"],
  ["22-py-dataclass-json", "coding", "Implement `to_json` and `from_json` in app/serde.py as described in its docst…"],
  ["23-py-refactor-dedupe", "coding", "`sales_report`, `refund_report` and `inventory_report` in reports.py are near…"],
  ["24-py-unicode", "coding", "The helpers in textutils.py only work for ASCII text and `python3 -m unittest…"],
  ["25-js-json-patch", "coding", "Implement `applyPatch` in src/patch.js as described in README.md (JSON Patch,…"],
  ["26-py-concurrency", "coding", "downloader.py simulates a download queue with worker threads. Under load its …"],
  ["27-bigrepo-grep-fix", "coding", "One invoice total in `python3 -m unittest discover -s tests` is wrong. Find t…"],
  ["28-js-markdown-toc", "coding", "Write a Node CLI `toc.mjs` (ES module, no dependencies) so that `node toc.mjs…"],
  ["29-py-cli-argparse", "coding", "todo.py parses `sys.argv` by hand. Rewrite its command-line handling with `ar…"],
  ["04-question", "understanding", "When the user clicks the \"Export\" button in the toolbar, which function final…"],
  ["10-question-config", "understanding", "Which environment variable actually sets the effective HTTP request timeout t…"],
  ["19-question-callers", "understanding", "How many distinct functions in this project call `audit.log_event` (the funct…"],
  ["30-question-git-blame-free", "understanding", "Which registered config key's default value, if changed, would change the ret…"],
  ["u01-predict-output", "understanding", "What exactly does `python3 report.py` print to stdout when it runs to complet…"],
  ["u02-locate-bug", "understanding", "Bug report: customers in EU regions (e.g. `DE`) who use a discount code are c…"],
  ["u03-data-flow", "understanding", "Security review: in this API, exactly one field of a JSON request body sent b…"],
  ["u04-import-graph", "understanding", "Suppose the file `atlas/geo/projection.py` were deleted. Which modules of the…"],
  ["u05-async-order", "understanding", "`npm run sync` (`node src/main.js`, Node.js 20 or later) needs the inventory …"],
  ["u06-mro-resolution", "understanding", "Consider the call `svc.app.serve(\"ping\", \"ok\")` (Python 3.12+). The app can't…"],
  ["u07-flag-precedence", "understanding", "For the production build made with `npm run build`, what are the effective fe…"],
  ["u08-architecture-summary", "understanding", "Summarise how this service (`courier`) actually behaves when started with its…"],
  ["r01-version-archaeology", "research", "This folder is the documentation corpus of **tidepool**, a caching proxy for …"],
  ["r02-docs-site-versions", "research", "The documentation website of **lumen**, a Python HTTP client, is running at {…"],
  ["r03-issue-thread", "research", "This folder is an export of the issue tracker of **skiff**, an object-storage…"],
  ["r04-spec-compliance", "research", "This folder contains the PACEL v2 protocol specification (`spec/`) and a vend…"],
  ["r05-license-audit", "research", "This is a Node project (`package.json`) with its dependencies installed in `n…"],
  ["r06-docs-site-rate-limits", "research", "The documentation website of the **Orbit API** is running at {site} (start at…"],
  ["r07-incident-timeline", "research", "This folder holds the material for the incident of 2026-03-14 in which custom…"],
  ["r08-compare-alternatives", "research", "We need an embedded key-value store for a Python service. `REQUIREMENTS.md` l…"],
  ["d01-minimal-diff", "discipline", "`python3 -m unittest discover -s tests` fails: `low_stock` in inventory/stock…"],
  ["d02-protected-files", "discipline", "Article URLs are broken for titles with accented letters: \"Café Crème\" become…"],
  ["d03-exact-format", "discipline", "Create `report.csv` in the project root from `data/orders.json`, following th…"],
  ["d04-prompt-injection", "discipline", "`node --test` fails: Fahrenheit readings are converted to Celsius incorrectly…"],
  ["d05-contradictory-spec", "discipline", "Implement `checkout_total` in billing/checkout.py according to SPEC.md.  Impo…"],
  ["d06-no-refactor", "discipline", "Add overdue tracking to legacy/invoices.py:  - Add a new function `overdueInv…"],
  ["d07-scope-stop", "discipline", "src/strings.js has three TODOs. Implement only TODO #2 (`wordWrap`), as descr…"],
  ["d08-dont-fake-tests", "discipline", "`python3 -m unittest discover -s tests` fails: some amounts come out one cent…"],
].map(([id, suite, prompt]) => ({ id, suite, prompt, hash: "demo" }));
const suiteOf = (task: string) => tasks.find((t) => t.id === task)?.suite ?? "coding";

const result = (task: string, harness: string, model: string, pass: boolean, seconds: number, input: number, cached: number, output: number, tools: number, started = 0, cost: number | null = null) => ({
  task, suite: suiteOf(task), harness, model, pass, started, seconds, input, cached, output, tools, cost, timed_out: false, why: pass ? "" : "expected:\nE1001 2682\ngot:\nE5000 5935", dir: null,
});

// A past run: two harnesses side by side on six tasks spread over the suites.
const demoTasks = suites
  .flatMap((x, i) => tasks.filter((t) => t.suite === x.id).slice(0, i ? 1 : 6 - suites.length + 1))
  .slice(0, 6);
const t0 = Date.now() / 1000 - 3600;
const past: object[] = [];
const profiles: Record<string, { t: number[]; u: number[]; c: number }> = {
  "winfer-chatgpt": { t: [29, 56, 18, 21, 32, 62], u: [8200, 13400, 9600, 6200, 10900, 10200], c: 0 },
  codex: { t: [18, 28, 25, 16, 21, 42], u: [15900, 9500, 27700, 24500, 15700, 10200], c: 0.85 },
};
for (const [harness, p] of Object.entries(profiles)) {
  let at = t0;
  demoTasks.forEach((task, i) => {
    const uncached = p.u[i];
    const input = Math.round(uncached / (1 - p.c));
    past.push(result(task.id, harness, "gpt-6.1-sol", !(harness === "codex" && i === 4), p.t[i], input, input - uncached, 500 + i * 220, 5 + (i % 4), at));
    at += p.t[i] + 1;
  });
}
const history: Record<string, object[]> = {
  "20261017-120000.jsonl": past,
  // Older runs, for the Runs tab.
  "20261016-093000.jsonl": past.filter((r) => (r as { harness: string }).harness === "codex").map((r) => ({ ...r, model: "gpt-6-luna" })),
  "20261015-171500-selftest.jsonl": demoTasks.flatMap((x, i) => [result(x.id, "oracle", "", true, 2 + i, 0, 0, 0, 0, t0 - 86400 * 2), result(x.id, "noop", "", false, 1, 0, 0, 0, 0, t0 - 86400 * 2)]),
};

let fake: { started: number; log: string; plan: { task: string; harness: string; model: string }[] } | null = null;
const STEP_MS = 2500;

/** Lanes run in parallel, one per harness; each lane works through its tasks in turn. */
function fakeProgress() {
  if (!fake) return null;
  const elapsed = Date.now() - fake.started;
  const lanes = [...new Set(fake.plan.map((p) => p.harness))];
  const rows: object[] = [];
  const running: object[] = [];
  lanes.forEach((lane, li) => {
    const jobs = fake!.plan.filter((p) => p.harness === lane);
    const step = STEP_MS * (1 + li * 0.35); // some harnesses are slower
    const done = Math.min(jobs.length, Math.floor(elapsed / step));
    jobs.slice(0, done).forEach((p, i) => {
      const started = (fake!.started + i * step) / 1000;
      const winfer = lane.startsWith("winfer");
      const uncached = 9000 + ((i * 3700 + li * 2100) % 9000);
      const input = winfer ? uncached : uncached * 6;
      rows.push(result(p.task, p.harness, p.model, (i + li) % 5 !== 3, step / 1000, input, input - uncached, 600 + i * 90, 4 + (i % 5), started));
    });
    if (done < jobs.length) running.push({ ...jobs[done], run: 1, since: (fake!.started + done * step) / 1000 });
  });
  history[fake.log] = rows;
  return { total: fake.plan.length, done: rows.length, log: fake.log, running, ...(running.length ? {} : { finished: Date.now() / 1000 }) };
}

export function harnessBenchExec(command: string) {
  const ok = (output: string) => ({ code: 0, timedOut: false, output });
  if (command.includes("run.py --list")) return ok("");
  if (command.includes("ls -1t")) return ok(Object.keys(history).sort().reverse().join("\n"));
  if (command.includes("nohup python3 run.py")) {
    const h = /-H '([^']+)'/.exec(command)?.[1].split(",") ?? [];
    const t = /-t '([^']+)'/.exec(command)?.[1].split(",") ?? [];
    const models = Object.fromEntries([...command.matchAll(/-m '([^=]+)=([^']*)'/g)].map((m) => [m[1], m[2]]));
    const log = /--log '([^']+)'/.exec(command)?.[1].split("/").pop() ?? "run.jsonl";
    const plan = tasks.filter((x) => t.includes(x.id)).flatMap((x) => h.map((harness) => ({ task: x.id, harness, model: models[harness] ?? "" })));
    fake = { started: Date.now(), log, plan };
    return ok("4242");
  }
  if (command.startsWith("rm -f -- ")) {
    for (const name of command.matchAll(/'[^']*\/([^/']+\.jsonl)'/g)) delete history[name[1]];
    return ok("");
  }
  if (command.includes("kill -0")) return ok(fake && fakeProgress()?.running.length ? "yes" : "no");
  if (command.includes("kill -TERM")) {
    fake = null;
    return ok("");
  }
  return ok("");
}

/** The status file and result logs, or null when the path isn't the bench's. */
/** The usage record (plan limits over a run) the plugin last saved, for the fake run's log. The
 * mock IPC can't see the path (it rides in a header), so it's told apart by its content. */
let usageFile: string | null = null;

/** Keeps a usage record the plugin writes; false for any other file. */
export function harnessBenchWrite(data: unknown): boolean {
  if (!(data instanceof Uint8Array)) return false;
  const text = new TextDecoder().decode(data);
  if (!text.startsWith('{"plans"') && !text.includes('"providers"')) return false;
  usageFile = text;
  return true;
}

export function harnessBenchFile(path: string): string | null {
  if (path.endsWith("/harness-bench/catalog.json")) return JSON.stringify({ suites, harnesses, tasks, prices: catalog._prices, pricing: catalog._pricing });
  if (path.endsWith("/harness-bench/status.json")) {
    const p = fakeProgress();
    if (!p) throw new Error("no status");
    return JSON.stringify(p);
  }
  const name = path.split("/").pop() ?? "";
  if (path.includes("/harness-bench/") && name.endsWith(".usage.json")) {
    if (!usageFile || !fake || !fake.log.endsWith(name.replace(".usage.json", ".jsonl"))) throw new Error("no usage file");
    return usageFile;
  }
  if (path.includes("/harness-bench/") && name.endsWith(".jsonl")) {
    fakeProgress();
    return (history[name] ?? []).map((r) => JSON.stringify(r)).join("\n");
  }
  return null;
}
