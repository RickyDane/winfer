# Harness bench

Compares coding-agent harnesses (`winfer code`, Codex, Claude Code, pi, OpenCode) on the same small tasks: did it work, how long, how many tokens, how many tool calls. Each run gets a fresh git copy of the task project; the harness runs headless with permissions off; a hidden check decides pass/fail.

```sh
python3 plugins/harness-bench/bench/run.py                                   # winfer-chatgpt vs codex, all tasks
python3 plugins/harness-bench/bench/run.py -H winfer-chatgpt,codex,pi -n 3   # 3 runs each (medians in the summary)
python3 plugins/harness-bench/bench/run.py -H winfer-claude,claude -t 03,04  # same model through both harnesses
python3 plugins/harness-bench/bench/run.py -H oracle                         # reference solutions: every check should pass
python3 plugins/harness-bench/bench/run.py -H noop                           # untouched projects: every check should fail
```

`-p harness` (default) runs all harnesses at the same time, each working through its tasks in turn; `-p all` starts every run at once; `-p none` runs one at a time. Parallel runs share your machine and network, so times are a little slower than alone. `--keep` keeps the work directories (with the harness output in `.bench-output.jsonl`). Results append to `results/<time>.jsonl` next to run.py (the app plugin keeps them in the app data folder). The winfer binary comes from `$WINFER_BIN`, else `src-tauri/target/{release,debug}/winfer`, else `/Applications/winfer.app`.

## Tasks

| task | what it tests |
|---|---|
| 01-bugfix | two bugs plus a rounding rule; visible tests must pass unchanged, hidden edge cases too |
| 02-feature | extend a CLI (flag, sorting, new command, legacy data) to an exact output spec |
| 03-rename | rename across ~40 generated files, docs included, without touching a look-alike (`compute_tax_free`) |
| 04-question | trace a call through a registry, an alias and a decoy legacy path; answer only, no other edits |
| 05-bigfile | 80k-line log: skip WARN lines that mention codes and malformed lines; rewards not reading the whole file |
| 06-js-lru | implement an LRU cache with TTL from a README spec; hidden `node --test` suite |

Each task folder has `task.md` (the prompt), `repo/` and/or `setup.py` (the project), `verify/check.sh` (hidden check, run in the work dir with `$VERIFY_DIR`), and `solution/` + `solution.sh` (the oracle).

## Reading results

- **input** is all input tokens sent over the run; **uncached input** is what you pay full price for. A harness with a big but cacheable system prompt shows high input and low uncached.
- Compare pairs on the same model: `winfer-chatgpt` / `codex` / `pi` (gpt-6.1-sol), `winfer-claude` / `claude` (sonnet). `winfer-claude` and `winfer-codex` run the CLIs' own agents, so they should track `claude` and `codex` closely.
- Single runs are noisy; use `-n 3` or more before drawing conclusions.

Add harnesses in `harnesses.json` (`{prompt}` and `{winfer}` are substituted; parsers: `winfer`, `claude`, `codex`, `pi`, `none`).
