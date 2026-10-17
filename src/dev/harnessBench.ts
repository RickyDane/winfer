/**
 * Dev-only fake runner for the Harness Bench plugin (plugins/harness-bench): answers its
 * `run.py --list`, history listing and start/stop commands, and serves a status file and
 * results that grow while a fake run is going. Open the dev server with `?bench`.
 */

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
const tasks = [
  ["01-bugfix", "The tests in tests/ fail. Find and fix the bugs in the shop package so that `…"],
  ["02-feature", "Extend the todo CLI in todo.py:"],
  ["03-rename", "Rename the function `compute_tax` to `calculate_tax` everywhere in this proje…"],
  ["04-question", "When the user clicks the \"Export\" button in the toolbar, which function final…"],
  ["05-bigfile", "logs/app.log is a large application log. Write report.py that prints the 3 mo…"],
  ["06-js-lru", "Implement the cache described in README.md in src/lru.js (ES module, no depen…"],
  ["07-py-perf", "`dedupe_contacts` and `count_paths` in datatools/ are far too slow on real in…"],
  ["08-js-async-bug", "`node --test` fails: the async helpers in src/async.js (`mapLimit` and `retry…"],
  ["09-py-csv-import", "Implement the CSV importer specified in README.md: `parse` and `load` in impo…"],
  ["10-question-config", "Which environment variable actually sets the effective HTTP request timeout t…"],
  ["11-py-pagination", "The tests in tests/ fail. Fix the bugs in the pager package (page numbers, cu…"],
  ["12-sqlite-migration", "The contacts app stores a single `people.name` column. Split it into `first_n…"],
  ["13-shell-fix", "backup.sh is buggy: it breaks on file and directory names with spaces, keeps …"],
  ["14-py-deprecation", "`Client.fetch(url, timeout)` in app/http/client.py is deprecated. Migrate eve…"],
  ["15-js-event-emitter", "Implement the event emitter described in README.md in src/emitter.js (ES modu…"],
  ["16-py-ttl-cache", "Implement the `ttl_cache` decorator described in README.md in ttlcache/__init…"],
  ["17-js-cli-args", "Implement the command-line argument parser described in README.md in src/args…"],
  ["18-py-path-traversal", "Security report: the static file server in fileserver/static.py has a path tr…"],
  ["19-question-callers", "How many distinct functions in this project call `audit.log_event` (the funct…"],
  ["20-py-multi-module-bug", "Bug report: after a reservation is cancelled, restocking the item makes the c…"],
  ["21-js-fix-tests", "`node --test` fails: the date-range utilities in src/ranges.js have bugs. Fix…"],
  ["22-py-dataclass-json", "Implement `to_json` and `from_json` in app/serde.py as described in its docst…"],
  ["23-py-refactor-dedupe", "`sales_report`, `refund_report` and `inventory_report` in reports.py are near…"],
  ["24-py-unicode", "The helpers in textutils.py only work for ASCII text and `python3 -m unittest…"],
  ["25-js-json-patch", "Implement `applyPatch` in src/patch.js as described in README.md (JSON Patch,…"],
  ["26-py-concurrency", "downloader.py simulates a download queue with worker threads. Under load its …"],
  ["27-bigrepo-grep-fix", "One invoice total in `python3 -m unittest discover -s tests` is wrong. Find t…"],
  ["28-js-markdown-toc", "Write a Node CLI `toc.mjs` (ES module, no dependencies) so that `node toc.mjs…"],
  ["29-py-cli-argparse", "todo.py parses `sys.argv` by hand. Rewrite its command-line handling with `ar…"],
  ["30-question-git-blame-free", "Which registered config key's default value, if changed, would change the ret…"],
].map(([id, prompt]) => ({ id, prompt, hash: "demo" }));

const result = (task: string, harness: string, model: string, pass: boolean, seconds: number, input: number, cached: number, output: number, tools: number, started = 0, cost: number | null = null) => ({
  task, harness, model, pass, started, seconds, input, cached, output, tools, cost, timed_out: false, why: pass ? "" : "expected:\nE1001 2682\ngot:\nE5000 5935", dir: null,
});

// A past run: two harnesses side by side on the first six tasks.
const t0 = Date.now() / 1000 - 3600;
const past: object[] = [];
const profiles: Record<string, { t: number[]; u: number[]; c: number }> = {
  "winfer-chatgpt": { t: [29, 56, 18, 21, 32, 62], u: [8200, 13400, 9600, 6200, 10900, 10200], c: 0 },
  codex: { t: [18, 28, 25, 16, 21, 42], u: [15900, 9500, 27700, 24500, 15700, 10200], c: 0.85 },
};
for (const [harness, p] of Object.entries(profiles)) {
  let at = t0;
  tasks.slice(0, 6).forEach((task, i) => {
    const uncached = p.u[i];
    const input = Math.round(uncached / (1 - p.c));
    past.push(result(task.id, harness, "gpt-6.1-sol", !(harness === "codex" && i === 4), p.t[i], input, input - uncached, 500 + i * 220, 5 + (i % 4), at));
    at += p.t[i] + 1;
  });
}
const history: Record<string, object[]> = { "20261017-120000.jsonl": past };

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
  if (command.includes("run.py --list")) return ok(JSON.stringify({ harnesses, tasks }));
  if (command.includes("ls -1t")) return ok(Object.keys(history).sort().reverse().join("\n"));
  if (command.includes("nohup python3 run.py")) {
    const h = /-H '([^']+)'/.exec(command)?.[1].split(",") ?? [];
    const t = /-t '([^']+)'/.exec(command)?.[1].split(",") ?? [];
    const models = Object.fromEntries([...command.matchAll(/-m '([^=]+)=([^']*)'/g)].map((m) => [m[1], m[2]]));
    const log = /--log '([^']+)'/.exec(command)?.[1].split("/").pop() ?? "run.jsonl";
    const plan = tasks.filter((x) => t.includes(x.id.slice(0, 2))).flatMap((x) => h.map((harness) => ({ task: x.id, harness, model: models[harness] ?? "" })));
    fake = { started: Date.now(), log, plan };
    return ok("4242");
  }
  if (command.includes("kill -0")) return ok(fake && fakeProgress()?.running.length ? "yes" : "no");
  if (command.includes("kill -TERM")) {
    fake = null;
    return ok("");
  }
  return ok("");
}

/** The status file and result logs, or null when the path isn't the bench's. */
export function harnessBenchFile(path: string): string | null {
  if (path.endsWith("/harness-bench/status.json")) {
    const p = fakeProgress();
    if (!p) throw new Error("no status");
    return JSON.stringify(p);
  }
  const name = path.split("/").pop() ?? "";
  if (path.includes("/harness-bench/") && name.endsWith(".jsonl")) {
    fakeProgress();
    return (history[name] ?? []).map((r) => JSON.stringify(r)).join("\n");
  }
  return null;
}
