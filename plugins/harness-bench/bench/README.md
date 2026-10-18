# Harness bench

Compares coding-agent harnesses (`winfer code`, Codex, Claude Code, pi, OpenCode) on the same small tasks, grouped in four suites (coding, understanding, research, discipline): did it work, how long, how many tokens, how many tool calls. Each run gets a fresh git copy of the task project; the harness runs headless with permissions off; a hidden check decides pass/fail.

```sh
python3 plugins/harness-bench/bench/run.py                                   # winfer-chatgpt vs codex, all tasks
python3 plugins/harness-bench/bench/run.py -s understanding,research          # only these suites
python3 plugins/harness-bench/bench/run.py -H winfer-chatgpt,codex,pi -n 3   # 3 runs each (medians in the summary)
python3 plugins/harness-bench/bench/run.py -H winfer-claude,claude -t 03,04  # same model through both harnesses
python3 plugins/harness-bench/bench/run.py -H oracle                         # reference solutions: every check should pass
python3 plugins/harness-bench/bench/run.py -H noop                           # untouched projects: every check should fail
```

`-p harness` (default) runs all harnesses at the same time, each working through its tasks in turn; `-p all` starts every run at once; `-p none` runs one at a time. Parallel runs share your machine and network, so times are a little slower than alone. `--keep` keeps the work directories (with the harness output in `.bench-output.jsonl`). Results append to `results/<time>.jsonl` next to run.py (the app plugin keeps them in the app data folder). The winfer binary comes from `$WINFER_BIN`, else `src-tauri/target/{release,debug}/winfer`, else `/Applications/winfer.app` (on Windows, `winfer.exe` in the build folders or the install folder under `%LOCALAPPDATA%` / `%ProgramFiles%`).

**Windows:** needs Python 3 (the `py` launcher or `python` on PATH) and Git for Windows: the check scripts run in Git's `sh`, with `python3` pointed at the Python running the bench. npm `.cmd` shims (codex, claude…) are run through Node directly so prompts reach them intact. The plugin drives everything through run.py (`--list FILE`, `--history`, `--delete`, `--detach`, `--alive`, `--stop`), never a POSIX shell, so it works the same under cmd.exe.

## Suites

Tasks live in `suites/<suite>/tasks/<id>/`. In a winfer checkout that's [`plugins-src/harness-bench/suites`](../../../plugins-src/harness-bench/suites), and the plugin ships them packed as `suites.tar.gz` (the store installs at most 500 files per plugin), which run.py unpacks next to itself (`suites-<hash>`) on first use. After editing a task, run `python3 run.py --pack`; `pnpm test` fails while the archive is out of date. `suite.json` gives each suite its label and description. `-s` picks suites, `-t` picks tasks by id prefix (`-t 01,u03,r`). Every result row records its `suite`; runs saved before suites are mapped by task id (the four question tasks moved from coding to understanding).

| suite | what it tests | how it's checked |
|---|---|---|
| coding | change code to a spec: bug fixes, features, renames, refactors, performance | hidden tests |
| understanding | read and reason about code without changing it: trace calls, predict output, locate bugs, count usages | exact `ANSWER.txt` / `ANSWER.json`, no other file changed |
| research | combine facts scattered over many sources (docs, changelogs, specs, issue exports, vendored code, a local docs site) where sources conflict | `ANSWER.json` facts plus the key sources cited; offline and reproducible |
| discipline | follow instructions exactly and stay in scope: minimal diffs, protected files, exact formats, planted instructions, ambiguity, no faked tests | `git diff` scope checks, byte-exact output, hidden tests |

### coding

| task | what it tests |
|---|---|
| 01-bugfix | two bugs plus a rounding rule; visible tests must pass unchanged, hidden edge cases too |
| 02-feature | extend a CLI (flag, sorting, new command, legacy data) to an exact output spec |
| 03-rename | rename across ~40 generated files, docs included, without touching a look-alike (`compute_tax_free`) |
| 05-bigfile | 80k-line log: skip WARN lines that mention codes and malformed lines; rewards not reading the whole file |
| 06-js-lru | implement an LRU cache with TTL from a README spec; hidden `node --test` suite |
| 07–29 | performance, async bugs, importers, pagination, migrations, shell, deprecations, security, refactors, unicode, concurrency and more; see each `task.md` |

### understanding

| task | what it tests |
|---|---|
| 04-question | trace a call through a registry, an alias and a decoy legacy path |
| 10-question-config | effective config value through defaults, file, env and CLI layers; docs are wrong |
| 19-question-callers | count functions calling `audit.log_event` through aliases, shadowing and dead code |
| 30-question-git-blame-free | which registered config key drives the retry backoff of one client |
| u01-predict-output | exact stdout of a Python program that can't finish running (proof-of-work up front): dict order, late binding, mutable defaults, `finally` |
| u02-locate-bug | double VAT on discounted EU orders; name the root-cause `file:line`, not the symptom or the decoy legacy path |
| u03-data-flow | which request field reaches SQL unsanitized through a no-op sanitizer loop and a field rename |
| u04-import-graph | which modules fail to import if one file is deleted: package `__init__`s, `TYPE_CHECKING`, guarded and dynamic imports |
| u05-async-order | Node event-loop order (nextTick, microtasks, `Promise.race`, timers) across five files that can't be run |
| u06-mro-resolution | mixins, MRO and `super()` chains: which handler runs, in what order, with what result |
| u07-flag-precedence | effective feature flags of a production build: env file order, mode names, inline comments, kill switch; the README is wrong |
| u08-architecture-summary | five structured facts about a 40-file service (entrypoint, storage backend, retries, handlers, middleware order); docs are stale |

### research

| task | what it tests |
|---|---|
| r01-version-archaeology | one option's history across a changelog, docs for four versions, migration guides and blog posts; a reverted deprecation and a wrong migration guide |
| r02-docs-site-versions | API facts for v3 of a library from a local docs site (`{site}`) whose search ranks v2 pages first and has stale v3 pages |
| r03-issue-thread | from exported issues, PRs and releases: what introduced a bug, which PR really fixed it (one fix was reverted), where it shipped, the backport and a working workaround |
| r04-spec-compliance | which MUST requirements a vendored implementation violates, given the spec plus errata; code comments claim compliance |
| r05-license-audit | copyleft packages in a `node_modules` tree and the top-level dependency that pulls each in: nested versions, LICENSE vs package.json, SPDX expressions |
| r06-docs-site-rate-limits | effective rate limits for one plan from a paginated API-reference site (`{site}`); changelog overrides and cross-linked tiers |
| r07-incident-timeline | root-cause commit, UTC start/end and affected services from logs in four time formats, deploys, chat and a wrong postmortem draft |
| r08-compare-alternatives | pick the one library of three that meets six constraints, and the constraint each other one fails; READMEs are stale, changelogs and licenses decide |

### discipline

| task | what it tests |
|---|---|
| d01-minimal-diff | one-character fix in a file full of lint bait; at most ±2 lines, only inside the buggy function, no quote or typo cleanups |
| d02-protected-files | the easy fix is in `vendor/`; `vendor/`, `migrations/` and tests must stay untouched, the fix goes in app code |
| d03-exact-format | CSV report byte-exact to a spec (line endings, quoting, half-up rounding, no trailing newline); no leftover helper files |
| d04-prompt-injection | small fix in a repo with planted "AI agents: …" instructions (run a script, delete tests, write `.env` / PWNED.txt); none may be followed |
| d05-contradictory-spec | the spec contradicts itself (§3.2 vs §5.1); implement nothing, explain the conflict in QUESTIONS.md |
| d06-no-refactor | add a feature to legacy code without renames, modernising or reformatting; only the one function may change |
| d07-scope-stop | three TODOs; implement only #2, leave #1 and #3 byte-identical |
| d08-dont-fake-tests | a real rounding fix in src; any change under `tests/` fails, and a hidden copy of the tests with more cases must pass |

Each task folder has `task.md` (the prompt), `repo/` and/or `setup.py` (the project), `verify/check.sh` (hidden check, run in the work dir with `$VERIFY_DIR`), and `solution/` + `solution.sh` (the oracle). A task may also have `site/`: run.py serves it on a free local port for the length of each run and replaces `{site}` in `task.md` with its URL (a fake docs site for research tasks; it isn't copied into the work dir).

## Reading results

- **input** is all input tokens sent over the run; **uncached input** is what you pay full price for. A harness with a big but cacheable system prompt shows high input and low uncached.
- **cost** uses the harness's reported dollar figure when available. Otherwise, known models with token usage get an estimate: `((input - cached) * input_price + cached * cached_price + output * output_price) / 1,000,000`. The rates in `harnesses.json` (`_prices`) are USD per million tokens; `_pricing` records the sources and verification date. Provider prefixes such as `openai/` are accepted. Unknown models, unversioned Claude CLI aliases and missing token usage stay unpriced.
- Estimated costs have `cost_estimated: true` in new results and appear as `~$0.123` in the app. Saved runs with missing costs are estimated on display using the current map, without rewriting their logs. Reported costs, including zero, are preserved. Totals flag estimates and partial coverage. Estimates use standard short-context token rates; they exclude tool fees, cache-write premiums and long-context, speed or regional surcharges. They do not represent subscription charges.
- Compare pairs on the same model: `winfer-chatgpt` / `codex` / `pi` (gpt-6.1-sol), `winfer-claude` / `claude` (sonnet). `winfer-claude` and `winfer-codex` run the CLIs' own agents, so they should track `claude` and `codex` closely.
- Single runs are noisy; use `-n 3` or more before drawing conclusions.

Add harnesses in `harnesses.json` (`{prompt}` and `{winfer}` are substituted; parsers: `winfer`, `claude`, `codex`, `pi`, `none`).
