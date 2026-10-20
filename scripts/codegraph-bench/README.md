# codegraph A/B benchmark

Headless `claude -p` runs that reproduce winfer code's lean Claude setup (`--tools LEAN_TOOLS`,
`--system-prompt LEAN_PROMPT…`, `--append-system-prompt DEFAULT_SYSTEM_PROMPT`, `--strict-mcp-config`)
with and without codegraph's MCP server. The `codegraph` CLI is blocked in every arm (`block.sh`,
as in codegraph's own benchmark). Prompts are read from this checkout's source.

```sh
export CGBENCH_DIR=/tmp/cgbench              # results/<task>/<arm>-<n>.jsonl (+ .diff for edit tasks)
python3 -I run.py 4 V2:plain:1 V2:cg:1 …    # TASK:ARM:N, 4 in parallel; finished runs are skipped
python3 -I summary.py                        # median cost/time/tool calls per task and arm
python3 -I cites.py                          # file:line citations checked against the repo
JUDGE_ARMS=plain,cg JUDGE_TAG=x python3 -I judge.py V2   # blind Opus judge with read-only repo access
```

Tasks and arms are in `run.py` / `arms.py` (repos under `~/Coding/bench`: vscode, deepseek-harness
and grok-build, the last two created after the model's training cutoff). Edit tasks run in a git
worktree with a clone of the codegraph index; they need a few GB of free disk.

## Results (2026-10-20, Claude Opus 5.5, codegraph 1.6.2, 3 runs per arm, medians)

| Tasks | Arm | Cost (sum) | Judge (corr+compl /20) |
|---|---|---|---|
| Familiar code, 5 read tasks (VS Code, winfer) | plain | $1.05 | 17.1 |
| | codegraph direct | $1.49 | **17.7** |
| | Haiku scout subagent | **$0.78** | 17.0 |
| Unfamiliar code, 6 read tasks (deepseek-harness, grok-build) | plain | $4.00 | **17.4** |
| | codegraph direct | $3.71 | 16.0 |
| | Haiku scout | **$1.95** | 14.8 |
| | Sonnet scout, hops marked seen/inferred | $2.58 | 14.7 |
| Unfamiliar code, 2 edit tasks | plain | $2.19 | 14.3 |
| | codegraph direct | $1.91 | 12.7 |
| | plain + "grep every sibling reference" | $3.06 | **15.4** |

Also measured, none better than `cg` (codegraph direct): `CODEGRAPH_EXPLORE_DEDUP=1`, the `prompt-hook`
UserPromptSubmit hook, `CODEGRAPH_MCP_TOOLS=explore,node`, and the old "search and reads" prompt.
Full non-lean Claude Code (upstream's setup) answered upstream's VS Code question in 2 tool calls
for $0.14, not the 28 calls codegraph's README reports for Opus 4.8.

Takeaways: current Opus with batched grep/sed is already cheap, so codegraph's fixed ~25k-char
explore payload (sized by repo, not by `maxFiles`) rarely pays for itself. It helps answer quality
a little on familiar code and cost a little on large unfamiliar code. Delegating exploration to a
scout subagent halves the cost but loses accuracy (inferred or wrong hops, uncalled code). On edits,
explore's sample of the code makes agents miss peripheral registration sites that grep finds.

## Codex (2026-10-20, codex-cli 0.163.0, gpt-6-luna medium, 3 runs per arm)

`codex_run.py` runs a stock Codex pane as winfer starts it (winfer's default prompt as
`developer_instructions`, no other MCP servers, codegraph CLI kept off PATH); `codex_summary.py`
reports a cost proxy (uncached input + 0.1 × cached + 8 × output tokens). Judge:
`JUDGE_SRC=codex JUDGE_ARMS=plain,cg_hint python3 -I judge.py <task>`.

| Tasks | Arm | Cost proxy (sum) | Judge (/20) |
|---|---|---|---|
| 11 read tasks | plain | 1.05M | 16.4 |
| | codegraph attached, no hint | 1.03M | — (never called it: 0 explores in 33 runs) |
| | codegraph + "use codegraph_explore first" hint | 1.03M | 13.6 |
| 2 edit tasks | plain | 0.50M | 12.5 |
| | codegraph + hint | 0.61M | 11.2 |

Codex ignores codegraph unless told to use it, and when told to, it trusts explore's slice of the code:
quality drops on familiar (16.5 → 14.2) and unfamiliar code (16.3 → 13.0) at the same cost, and it's
slower on VS Code (~54s vs ~28s). Don't steer Codex to codegraph.
