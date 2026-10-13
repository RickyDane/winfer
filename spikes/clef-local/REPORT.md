# M0.5 spike: local Clef-flash voice routing via llama.cpp `/v1/systemone`

Date 2026-10-13. Apple M1 Pro (16-core GPU, Metal 4), 32 GB RAM, macOS 27.2 (26B5101f).

## Verdict: NO-GO

| Target | Result | Status |
|---|---|---|
| Warm p50 < 600 ms | Floor (142 tok): **678 ms**. Best accurate request (`s_rich`, 1.3k tok): **6.0 s**. Full schema (2.6k tok): **14.4 s** | **Fail** |
| action_1 ≥ 90% | **95.5% (63/66)** with `full` or `s_rich`; the ~1k designs score 80–82% | Pass only at ≥ 1.3k tokens (≥ 6 s) |

Prompt processing is compute-bound at ~200–240 tok/s, fully on the GPU, and scales linearly with input length. Clef has no reusable cache. 600 ms would need ≤ ~125 tokens, but each choice option costs ~14 tokens of template overhead (a 29-way action choice alone is ≥ 473 tokens). Clef-flash 9B cannot be the synchronous voice router on an M1 Pro.

## Versions

- llama.cpp `45451c366d4256f94fbc8845645e31a6b9155054` (`b11589-15-g45451c3`; server reports "0.6.0-dev (build 400)"), Release, Metal, AppleClang 21.0.0. Includes #29818 (`/v1/systemone`) and #29831 (Clef).
- `ggml-org/Clef-Flash-GGUF` @ `6b4fe4c59098479652d94f0fe11a40facbebd1ec`; source `.src_sha` `PRIMARY=8b2e5fd17c09fd49bc2880b805d5323ec7ff4ff3`.
- `Clef-Flash-Q4_K_M.gguf`: 6,486,448,288 bytes (6.03 GiB), SHA256 `3626116623ca8d89641952471fac9ce12af84fb3517520029a2ac6f5146868a2` (matches HF etag).
- 9.08B params; Qwen3.5-9B-style hybrid backbone (32 layers, full attention every 4th) + Clef joint decision head.
- MLX comparison: `mlx-community/clef-flash-4bit` @ `b5adbe2c3a95200d4bf04a0727c8ec457157d5e1`, mlx 0.32.3, mlx-vlm 0.7.6 (venv `~/.cache/winfer-spike/venv-mlx`).

## Best server command (`run_server.sh` defaults)

```
llama-server -m ~/.cache/winfer-spike/models/clef-flash/Clef-Flash-Q4_K_M.gguf --host 127.0.0.1 --port 8899 --api-key <.api_key> -ngl 99 -c 4096 -b 4096 -ub 4096 -np 1 --metrics --no-webui --alias clef-flash
```

- Flash attention on auto (log shows enabled).
- `-ub` must be ≥ the largest prompt: Clef processes the whole prompt in one batch. For prompts ≤ 2k tokens, `-ub 2048` is equally fast and uses ~88 MiB less.

## Task 1: diagnosing the ~13 s

**GPU use is fine:** `offloaded 33/33 layers to GPU`. The only CPU buffer is the token-embedding table (545.64 MiB), which is normal.

**All three warnings are harmless:**
- `pooling_type [-1] vs [0]`: `common.cpp` deliberately forces embedding output with no pooling for decision models.
- `embeddings required … overriding`: printed once during warm-up. Clef needs an output per token by design (one float per token; the head averages over question/option spans). ~4 bytes/token. No flag avoids it, none needed.
- `speculative decoding not supported`: irrelevant; Clef generates no text.

**Config sweep** (warm, n=3, p50 ms; `results/bench_cfg.jsonl`, `results/sweep.log`):

| Config | 142 tok | 575 | 1,115 | 2,195 | 2,452 | 2,644 | Memory after |
|---|---|---|---|---|---|---|---|
| ub 4096, FA auto | 678 | 2,468 | 5,038 | 10,922 | 12,127 | 13,620 | 6.48 GiB |
| ub 2048 | 673 | 2,413 | 4,871 | ERR | ERR | ERR | 6.39 GiB |
| ub 512 (default) | 674 | ERR | ERR | ERR | ERR | ERR | 6.23 GiB |
| ub 8192, FA on | 675 | 2,406 | 4,980 | 11,193 | 12,131 | 13,389 | 6.62 GiB |
| ub 4096, FA off | 679 | 2,424 | 4,871 | 11,000 | 12,531 | 13,626 | 6.68 GiB |
| ub 4096, 4 threads | 672 | 2,400 | 4,894 | 10,858 | 12,212 | 13,384 | 6.65 GiB |

- ERR = `HTTP 500: input (575 tokens) is too large to process. increase the physical batch size (current batch size: 512)`.
- Batch size only caps prompt size. FA, threads and context size are within ±3%.
- Server compute time matches wall-clock within 1–2%; HTTP/JSON overhead negligible.
- Throughput: 209 tok/s @142, 239 @575, 224 @1.1k, 194–201 @2.2–2.6k → ~4.2–5.2 ms/token, no fixed overhead.

**Caching: none, structurally.**
1. Clef's llama.cpp model code runs "without the memory module".
2. Verbose log on a byte-identical repeat: `f_sim_best = 1.000` then `cached n_tokens = 0, memory_seq_rm [0, end)`.
3. `prompt_tokens_cached_total` stayed 0 in every variant (~1,166 tokens each):

| Variant | p50 varying transcript | p50 identical repeat |
|---|---|---|
| Object state | 5,195 ms | 5,368 ms |
| String state, transcript first | 5,236 ms | 5,236 ms |
| String state, transcript last | 5,263 ms | 5,221 ms |
| Transcript last, questions reversed | 5,215 ms | 5,334 ms |

4. Ordering can't help: the template puts state (with transcript) before questions (85–90% of tokens), and the server sorts JSON keys and choice options. "Shared prefix" works only within a single request.

## Task 2: cutting to ~1k tokens

`tokcount.py` reproduces server token counts exactly. Each option costs ~14 tokens of template overhead; 29-way action_1 = 473 tokens bare, 852 with original descriptions; action_2 ~460–480; each count question ~110–130. Shortening descriptions buys little; only removing options does.

Variants:
- **compact** (2,446 tok): short descriptions, small state, reworded action_2/count questions.
- **twostage** (~1k): 1,005-token first call + second call only for needed args (187–1,351 tok).
- **s_rich** (1,338 tok): full original snapshot and descriptions, only action_1 + target_pane.

Dropping context cost accuracy (workspace switches → focus_pane, noise treated as commands, "new browser tab" → open_url). s_rich keeps the context and recovers 95.5%.

Count encoding (14 count cases):

| Encoding | Correct |
|---|---|
| Digits "0".."4" | 4/14 |
| Words "zero".."four" | **8/14** |
| Words, no pane list in state | 2/14 |
| Full 2.6k joint request | 11/14 |

## Task 3: full eval (66 cases: 58 + 8 added)

| Variant | Tokens | action_1 | action_2 raw | multi raw (7) | action_2 gated* | args (37) | p50 | p95 | max |
|---|---|---|---|---|---|---|---|---|---|
| full | 2,636–2,649 | **63/66** | 41/66 | 7/7 | **63/66** (6/7) | 32/37 | 14,427 | 14,527 | 14,545 |
| s_rich | 1,336–1,349 | **63/66** | – | – | – | target 20/20 | **6,015** | 6,123 | 6,258 |
| compact | ~2,450 | 54/66 | 9/66 | 4/7 | 60/66 (2/7) | 21/37 | 13,340 | 13,634 | 13,734 |
| twostage | 994–2,363 (mean 1,177) | 53/66 | 59/66 | **0/7** | – | 19/37 | 4,463 | 8,136 | 11,149 |
| MLX, s_rich | 1,344 | 63/66 | – | – | – | – | 7,297 | – | 7,594 |

\*gated: accept action_2 only if it differs from action_1 (the model mostly repeats action_1; rewording didn't fix it, this rule does).

`full` args: counts 11/14, worktree 14/14, target 20/20, resize 1/3, workspace 3/3.

Same three action_1 failures in full, s_rich and MLX:

| # | Utterance | Wrong answer (full / s_rich confidence) |
|---|---|---|
| 20 | "zoom in on two" | resize_pane (0.31 / 0.38) |
| 37 | "go back" | interrupt_agent (0.22 / 0.29) |
| 66 | "open three codex panes in worktrees and switch to a grid" | set_layout (0.44 / 0.45) |

Case 3 "open cloud code" is correct but at 0.27 / 0.28 (reject band).

Peak memory: 6.65 GiB (previous run's 7.2 GB was ub 8192). MLX peak 7.15 GB.

**action_1 confidence**

| Variant | Correct: min / p10 / median | Wrong |
|---|---|---|
| full | 0.265 / 0.563 / 0.779 | 0.218, 0.306, 0.441 |
| s_rich | 0.279 / 0.625 / 0.829 | 0.294, 0.379, 0.449 |
| compact, twostage | heavy overlap | heavy overlap |

**Suggested thresholds** (full / s_rich prompts only):

| Band | full | s_rich |
|---|---|---|
| Auto-run ≥ 0.60 | 54, all correct | 58, all correct |
| Confirm 0.30–0.60 | 9 (7 correct, 2 wrong) | 6 (4 correct, 2 wrong) |
| Reject < 0.30 | 3 (2 correct, 1 wrong) | 2 (1 correct, 1 wrong) |

- Treat `none` as ignore at any confidence (noise scored 0.27–0.65).
- 0 errors in 54–58 auto-runs still allows a true error rate up to ~5%.
- Thresholds don't transfer between prompt designs (compact had a wrong answer ≥ 0.6). Re-fit per prompt.

## What dominates, and options left

Backbone prompt compute. llama.cpp's Apple Silicon table (discussion #4167): M1 Pro 16-core LLaMA-7B pp512 Q4_0 266 tok/s, Q8_0 270, F16 302 → ~200 tok/s scaled to 9.08B, matching measurements. GPU is saturated.

| Option | Numbers | Assessment |
|---|---|---|
| Q8 instead of Q4 | Not measured; reference shows Q8 ≈ Q4 prompt speed | No gain, +~4 GB RAM |
| Smaller input | 575 tok 2.4 s, 1.0k 4.4 s, 1.3k 6.0 s; 80–82% below 1.3k | Can't reach 600 ms |
| MLX (`clef_mlx.py`) | Floor 876 ms, 1.1k 6,040, 1.3k 7,295, 2.6k 14,273 (162–189 tok/s); 5–29% slower, same 63/66. Card's 0.31 s @1k is M5 Max | Not better here |
| Ollama ≥ 0.35 | Not installed/measured | 9B hits same compute limit |
| Distilled 0.7B (`FluidInference/clef-text-0.6b-coreml`) | Not measured. Vendor: 196 ms @1,024 tok on M5 Pro ANE, 86.4% agreement with clef-flash. Unmeasured estimate here ~0.45 s @1.3k | Only plausible local path < 600 ms |
| Cloud `@cf/cloudflare/clef-flash` | Not measured (no credentials) | Needs measurement |

## Problems hit

1. Orphaned server from the interrupted run (ub 8192) held port 8899 → `couldn't bind HTTP server socket, hostname: 127.0.0.1, port: 8899`; stopped it.
2. Batch smaller than prompt → HTTP 500 above.
3. Sustained load raised latency ~6–10% (possibly thermal, unverified); included in eval p50.
4. `cached n_tokens` only logged with verbose logging; caching evidence from verbose run + metrics counter.
5. First sweep config overlapped a 20 s `shasum` (CPU); numbers agree with others within 3%.
6. Short prompts made action_2 and counts unreliable.
7. One wait loop timed out during the MLX run; the run completed normally.

## Files

`run_server.sh` (best config), `eval.py` (eval/bench/cache; sizes full, s_rich, compact, twostage, s, xs, large), `winfer_request.py` (request builders), `tokcount.py`, `bench_cfg.py`, `sweep.sh`, `run_evals.sh`, `run_extra.sh`, `count_variants.py`, `posthoc.py` (action_2 gate + thresholds), `mlx_bench.py`, `evals.jsonl` (66 cases), `results/` (raw outputs, server logs, `example_s_rich.json`, `example_full.json`).

Large artifacts (GGUF, llama.cpp build, MLX model ~5.8 GB, venv) are under `~/.cache/winfer-spike/`.

Sources: [llama.cpp PR #29831](https://github.com/ggml-org/llama.cpp/pull/29831), [Decision models in llama.cpp](https://huggingface.co/blog/ggml-org/decision-models-in-llamacpp), [llama.cpp Apple Silicon benchmarks #4167](https://github.com/ggml-org/llama.cpp/discussions/4167), [mlx-community/clef-flash-4bit](https://huggingface.co/mlx-community/clef-flash-4bit), [FluidInference/clef-text-0.6b-coreml](https://huggingface.co/FluidInference/clef-text-0.6b-coreml), [Ollama decision models](https://ollama.com/blog/ollama-now-supports-jev-style-decision-models)

---

## Appendix A: real request/response

Request (`results/example_s_rich.json`, sent with `Authorization: Bearer <key>` to `http://127.0.0.1:8899/v1/systemone`):

```json
{
 "model": "clef-flash",
 "state": {
  "transcript": "open two claudes and a codex",
  "app": {
   "workspace": "winfer",
   "workspaces": [
    "winfer",
    "bridgemind-api",
    "bridgemind-web",
    "personal"
   ],
   "focusedPane": "p2",
   "panes": [
    {
     "id": "p1",
     "label": "Claude 1",
     "provider": "claude",
     "status": "waiting_for_approval"
    },
    {
     "id": "p2",
     "label": "Codex 1",
     "provider": "codex",
     "status": "running"
    },
    {
     "id": "p3",
     "label": "Shell",
     "provider": "shell",
     "status": "idle"
    },
    {
     "id": "p4",
     "label": "Grok 1",
     "provider": "grok",
     "status": "idle"
    }
   ],
   "installedProviders": [
    {
     "id": "claude",
     "name": "Claude Code (Anthropic)"
    },
    {
     "id": "codex",
     "name": "Codex CLI (OpenAI)"
    },
    {
     "id": "agy",
     "name": "agy, Antigravity CLI (Google)"
    },
    {
     "id": "grok",
     "name": "Grok CLI (xAI)"
    },
    {
     "id": "shell",
     "name": "plain zsh shell"
    }
   ],
   "browserTabs": [
    {
     "id": "t1",
     "title": "winfer dev - localhost:5173",
     "active": true
    },
    {
     "id": "t2",
     "title": "Pull requests - GitHub",
     "active": false
    }
   ],
   "recentCommands": [
    "open two claudes and a codex",
    "focus pane one"
   ]
  }
 },
 "questions": {
  "action_1": {
   "type": "choice",
   "instructions": "The user spoke a voice command to winfer, a terminal app for coding agents. Which app command does the transcript ask for first? The transcript may contain speech-recognition errors.",
   "criteria": {
    "spawn_panes": "Open new terminal panes running agents like claude, codex, agy, grok or a shell.",
    "close_pane": "Close an existing pane and stop whatever is running inside it.",
    "focus_pane": "Move keyboard focus to a specific existing pane.",
    "zoom_pane": "Maximize one pane to fill the window, or un-zoom it.",
    "resize_pane": "Make a pane bigger or smaller relative to its neighbours.",
    "swap_panes": "Swap the positions of two panes in the layout.",
    "equalize_panes": "Make all panes the same size.",
    "set_layout": "Change the pane arrangement, e.g. grid, columns, rows or stack.",
    "send_to_pane": "Type a message or instruction into one agent pane, e.g. tell claude to do something.",
    "broadcast": "Send the same message or instruction to every agent pane at once.",
    "approve_agent": "Approve or accept a pending permission request from an agent.",
    "interrupt_agent": "Stop or cancel what an agent is currently doing, without closing its pane.",
    "open_url": "Open a specific website, URL or localhost port in the built-in browser.",
    "browser_new_tab": "Open a new empty browser tab.",
    "browser_close_tab": "Close the current browser tab.",
    "browser_switch_tab": "Switch to another already-open browser tab.",
    "browser_back": "Go back to the previous page in the browser.",
    "browser_forward": "Go forward to the next page in the browser.",
    "browser_reload": "Reload or refresh the current browser page.",
    "set_device": "Change the microphone or audio input/output device.",
    "switch_workspace": "Switch to a different existing workspace or project.",
    "create_workspace": "Create a brand new workspace.",
    "toggle_sidebar": "Show or hide the sidebar.",
    "toggle_browser": "Show or hide the built-in browser panel.",
    "load_recipe": "Load a saved recipe, a preset set of panes and layout.",
    "show_shortcuts": "Show the list of keyboard shortcuts and voice commands.",
    "repeat_last": "Repeat the previous voice command again.",
    "undo_last": "Undo the previous voice command.",
    "none": "Not a command for the app: chatter, noise, unclear or unrelated speech."
   }
  },
  "target_pane": {
   "type": "choice",
   "instructions": "Which existing pane does the command refer to? 'it' or 'this' means the focused pane.",
   "criteria": {
    "p1": "Claude 1 (claude, waiting_for_approval)",
    "p2": "Codex 1 (codex, running)",
    "p3": "Shell (shell, idle)",
    "p4": "Grok 1 (grok, idle)",
    "none": "no existing pane is targeted"
   }
  }
 }
}
```

Response (HTTP 200, 6243.2 ms wall clock; floats rounded to 4 dp and action_1 probabilities cut to the top 5 for display, verbatim in the JSON file):

```json
{
 "model": "clef-flash",
 "answers": {
  "action_1": {
   "type": "choice",
   "choice": "spawn_panes",
   "probabilities": {
    "spawn_panes": 0.8247,
    "focus_pane": 0.0202,
    "open_url": 0.0139,
    "load_recipe": 0.0107,
    "set_layout": 0.0094
   },
   "confidence": 0.8184
  },
  "target_pane": {
   "type": "choice",
   "choice": "none",
   "probabilities": {
    "none": 0.6735,
    "p1": 0.0881,
    "p2": 0.1448,
    "p3": 0.0399,
    "p4": 0.0537
   },
   "confidence": 0.5918
  }
 },
 "usage": {
  "input_tokens": 1344,
  "output_tokens": 0
 }
}
```

## Appendix B: per-case results

| # | utterance | expected a1 / a2 | full (2.6k) a1 (conf) | full a2 | full args | s_rich (1.3k) a1 (conf) | compact (2.4k) a1 (conf) | two-stage (~1k) a1 (conf) |
|---|---|---|---|---|---|---|---|---|
| 1 | open 3 new instances with claude, codex and grok | spawn_panes | spawn_panes (0.88) | spawn_panes (0.55) | fail: counts | spawn_panes (0.84) | spawn_panes (0.43) | spawn_panes (0.52) |
| 2 | open two claudes and a codex | spawn_panes | spawn_panes (0.80) | focus_pane (0.30) | ok | spawn_panes (0.82) | spawn_panes (0.29) | spawn_panes (0.38) |
| 3 | open cloud code | spawn_panes | spawn_panes (0.27) | none (0.27) | ok | spawn_panes (0.28) | **XX** focus_pane (0.10) | spawn_panes (0.14) |
| 4 | give me two codecs | spawn_panes | spawn_panes (0.71) | spawn_panes (0.39) | ok | spawn_panes (0.73) | spawn_panes (0.26) | spawn_panes (0.52) |
| 5 | spin up four grok agents | spawn_panes | spawn_panes (0.76) | spawn_panes (0.52) | ok | spawn_panes (0.77) | spawn_panes (0.34) | spawn_panes (0.46) |
| 6 | new shell please | spawn_panes | spawn_panes (0.65) | none (0.37) | ok | spawn_panes (0.63) | spawn_panes (0.28) | spawn_panes (0.33) |
| 7 | open an agy pane | spawn_panes | spawn_panes (0.85) | none (0.46) | ok | spawn_panes (0.79) | spawn_panes (0.43) | spawn_panes (0.54) |
| 8 | open a gravity agent | spawn_panes | spawn_panes (0.70) | none (0.33) | ok | spawn_panes (0.69) | spawn_panes (0.12) | spawn_panes (0.24) |
| 9 | start claude and codex in separate worktrees | spawn_panes | spawn_panes (0.68) | spawn_panes (0.35) | ok | spawn_panes (0.71) | spawn_panes (0.39) | spawn_panes (0.39) |
| 10 | launch three clouds each on its own work tree | spawn_panes | spawn_panes (0.71) | spawn_panes (0.40) | fail: counts | spawn_panes (0.72) | spawn_panes (0.61) | spawn_panes (0.54) |
| 11 | close the codex one | close_pane | close_pane (0.88) | none (0.58) | ok | close_pane (0.92) | close_pane (0.77) | close_pane (0.90) |
| 12 | close this pane | close_pane | close_pane (0.86) | none (0.70) | ok | close_pane (0.91) | close_pane (0.68) | close_pane (0.86) |
| 13 | get rid of the shell | close_pane | close_pane (0.79) | close_pane (0.57) | ok | close_pane (0.83) | close_pane (0.51) | close_pane (0.74) |
| 14 | focus pane three | focus_pane | focus_pane (0.90) | none (0.42) | ok | focus_pane (0.93) | focus_pane (0.28) | focus_pane (0.20) |
| 15 | go to the grok pane | focus_pane | focus_pane (0.87) | focus_pane (0.66) | ok | focus_pane (0.90) | focus_pane (0.81) | focus_pane (0.85) |
| 16 | jump to claude | focus_pane | focus_pane (0.71) | focus_pane (0.57) | ok | focus_pane (0.79) | focus_pane (0.33) | focus_pane (0.26) |
| 17 | make it bigger | resize_pane | resize_pane (0.72) | none (0.35) | fail: resize_amount | resize_pane (0.84) | resize_pane (0.50) | resize_pane (0.65) |
| 18 | make the claude one a lot bigger | resize_pane | resize_pane (0.68) | resize_pane (0.44) | ok | resize_pane (0.85) | resize_pane (0.63) | resize_pane (0.88) |
| 19 | shrink the shell a little | resize_pane | resize_pane (0.83) | none (0.38) | fail: resize_amount | resize_pane (0.93) | resize_pane (0.58) | resize_pane (0.71) |
| 20 | zoom in on two | zoom_pane | **XX** resize_pane (0.31) | zoom_pane (0.32) | ok | **XX** resize_pane (0.38) | zoom_pane (0.19) | zoom_pane (0.23) |
| 21 | maximize grok | zoom_pane | zoom_pane (0.67) | none (0.40) | ok | zoom_pane (0.79) | zoom_pane (0.58) | zoom_pane (0.78) |
| 22 | swap claude and codex | swap_panes | swap_panes (0.82) | swap_panes (0.62) | - | swap_panes (0.88) | swap_panes (0.57) | swap_panes (0.73) |
| 23 | make all the panes the same size | equalize_panes | equalize_panes (0.87) | equalize_panes (0.52) | - | equalize_panes (0.92) | equalize_panes (0.83) | equalize_panes (0.93) |
| 24 | switch to a grid layout | set_layout | set_layout (0.86) | set_layout (0.42) | - | set_layout (0.94) | set_layout (0.59) | set_layout (0.60) |
| 25 | tell claude to run the tests | send_to_pane | send_to_pane (0.84) | none (0.44) | ok | send_to_pane (0.87) | send_to_pane (0.69) | send_to_pane (0.74) |
| 26 | ask codex to fix the lint errors | send_to_pane | send_to_pane (0.62) | send_to_pane (0.49) | ok | send_to_pane (0.62) | send_to_pane (0.30) | send_to_pane (0.51) |
| 27 | tell everyone to commit their work | broadcast | broadcast (0.75) | none (0.39) | - | broadcast (0.81) | broadcast (0.33) | broadcast (0.58) |
| 28 | approve claude | approve_agent | approve_agent (0.74) | none (0.41) | ok | approve_agent (0.71) | approve_agent (0.16) | approve_agent (0.51) |
| 29 | yeah accept that request | approve_agent | approve_agent (0.78) | none (0.51) | ok | approve_agent (0.86) | approve_agent (0.33) | approve_agent (0.45) |
| 30 | stop codex | interrupt_agent | interrupt_agent (0.71) | none (0.55) | ok | interrupt_agent (0.82) | **XX** close_pane (0.33) | **XX** close_pane (0.36) |
| 31 | cancel whatever grok is doing | interrupt_agent | interrupt_agent (0.87) | none (0.56) | ok | interrupt_agent (0.93) | interrupt_agent (0.61) | interrupt_agent (0.69) |
| 32 | open localhost 3000 | open_url | open_url (0.74) | none (0.38) | - | open_url (0.81) | open_url (0.66) | open_url (0.90) |
| 33 | go to github dot com | open_url | open_url (0.68) | open_url (0.45) | - | open_url (0.75) | open_url (0.66) | open_url (0.74) |
| 34 | new browser tab | browser_new_tab | browser_new_tab (0.80) | none (0.38) | - | browser_new_tab (0.89) | browser_new_tab (0.15) | **XX** open_url (0.13) |
| 35 | close the browser tab | browser_close_tab | browser_close_tab (0.78) | none (0.63) | - | browser_close_tab (0.89) | browser_close_tab (0.41) | browser_close_tab (0.65) |
| 36 | switch to the github tab | browser_switch_tab | browser_switch_tab (0.82) | browser_switch_tab (0.37) | - | browser_switch_tab (0.92) | browser_switch_tab (0.36) | browser_switch_tab (0.51) |
| 37 | go back | browser_back | **XX** interrupt_agent (0.22) | none (0.40) | - | **XX** interrupt_agent (0.29) | **XX** focus_pane (0.26) | **XX** focus_pane (0.32) |
| 38 | go forward a page | browser_forward | browser_forward (0.83) | none (0.63) | - | browser_forward (0.87) | browser_forward (0.56) | browser_forward (0.52) |
| 39 | reload the browser | browser_reload | browser_reload (0.78) | none (0.46) | - | browser_reload (0.92) | browser_reload (0.59) | browser_reload (0.86) |
| 40 | use the airpods microphone | set_device | set_device (0.87) | set_device (0.65) | - | set_device (0.96) | set_device (0.78) | set_device (0.93) |
| 41 | switch to bridgemind api | switch_workspace | switch_workspace (0.88) | switch_workspace (0.44) | ok | switch_workspace (0.95) | **XX** focus_pane (0.41) | **XX** focus_pane (0.38) |
| 42 | go to the personal workspace | switch_workspace | switch_workspace (0.88) | switch_workspace (0.47) | ok | switch_workspace (0.96) | **XX** focus_pane (0.34) | **XX** focus_pane (0.31) |
| 43 | create a new workspace | create_workspace | create_workspace (0.90) | none (0.45) | - | create_workspace (0.94) | create_workspace (0.45) | create_workspace (0.61) |
| 44 | hide the sidebar | toggle_sidebar | toggle_sidebar (0.79) | none (0.40) | - | toggle_sidebar (0.96) | toggle_sidebar (0.47) | toggle_sidebar (0.64) |
| 45 | show the browser | toggle_browser | toggle_browser (0.65) | none (0.38) | - | toggle_browser (0.82) | toggle_browser (0.38) | toggle_browser (0.47) |
| 46 | load my full stack recipe | load_recipe | load_recipe (0.81) | load_recipe (0.51) | - | load_recipe (0.87) | load_recipe (0.75) | load_recipe (0.66) |
| 47 | what are the shortcuts | show_shortcuts | show_shortcuts (0.86) | show_shortcuts (0.43) | - | show_shortcuts (0.96) | show_shortcuts (0.74) | show_shortcuts (0.83) |
| 48 | undo that | undo_last | undo_last (0.60) | none (0.46) | - | undo_last (0.74) | **XX** interrupt_agent (0.49) | **XX** interrupt_agent (0.57) |
| 49 | do that again | repeat_last | repeat_last (0.56) | repeat_last (0.42) | - | repeat_last (0.73) | repeat_last (0.33) | repeat_last (0.53) |
| 50 | um yeah so anyway | none | none (0.48) | none (0.57) | - | none (0.65) | none (0.13) | none (0.15) |
| 51 | what's the weather like today | none | none (0.38) | none (0.47) | - | none (0.38) | **XX** send_to_pane (0.22) | **XX** send_to_pane (0.24) |
| 52 | thanks | none | none (0.27) | none (0.69) | - | none (0.32) | **XX** focus_pane (0.11) | none (0.14) |
| 53 | blah blah banana the fish | none | none (0.65) | none (0.69) | - | none (0.75) | none (0.14) | none (0.18) |
| 54 | close codex and open grok | close_pane + spawn_panes | close_pane (0.83) | spawn_panes (0.22) | ok | close_pane (0.85) | close_pane (0.46) | close_pane (0.55) |
| 55 | reload the browser and focus pane one | browser_reload + focus_pane | browser_reload (0.64) | focus_pane (0.84) | ok | browser_reload (0.82) | **XX** focus_pane (0.52) | **XX** focus_pane (0.40) |
| 56 | stop grok and tell codex to push | interrupt_agent + send_to_pane | interrupt_agent (0.57) | send_to_pane (0.45) | - | interrupt_agent (0.62) | interrupt_agent (0.47) | interrupt_agent (0.63) |
| 57 | approve claude then switch to bridgemind web | approve_agent + switch_workspace | approve_agent (0.47) | switch_workspace (0.77) | ok | approve_agent (0.57) | approve_agent (0.23) | approve_agent (0.35) |
| 58 | open two codex panes and make them equal size | spawn_panes + equalize_panes | spawn_panes (0.68) | equalize_panes (0.58) | fail: counts | spawn_panes (0.68) | **XX** resize_pane (0.29) | **XX** equalize_panes (0.74) |
| 59 | open a cloud and a codecs | spawn_panes | spawn_panes (0.80) | spawn_panes (0.33) | ok | spawn_panes (0.79) | spawn_panes (0.26) | spawn_panes (0.58) |
| 60 | focus the claude pane and approve it | focus_pane + approve_agent | focus_pane (0.85) | approve_agent (0.85) | ok | focus_pane (0.85) | focus_pane (0.43) | **XX** approve_agent (0.50) |
| 61 | kill the grok pane | close_pane | close_pane (0.81) | none (0.51) | ok | close_pane (0.88) | close_pane (0.78) | close_pane (0.90) |
| 62 | refresh | browser_reload | browser_reload (0.59) | none (0.32) | - | browser_reload (0.66) | browser_reload (0.23) | **XX** spawn_panes (0.10) |
| 63 | open localhost 5173 in the browser | open_url | open_url (0.83) | none (0.28) | - | open_url (0.92) | open_url (0.72) | open_url (0.90) |
| 64 | hey what time is it | none | none (0.34) | none (0.46) | - | none (0.34) | none (0.22) | none (0.40) |
| 65 | tell all the agents to pull main | broadcast | broadcast (0.86) | broadcast (0.50) | - | broadcast (0.87) | **XX** focus_pane (0.58) | **XX** focus_pane (0.58) |
| 66 | open three codex panes in worktrees and switch to a grid | spawn_panes + set_layout | **XX** set_layout (0.44) | set_layout (0.78) | ok | **XX** set_layout (0.45) | **XX** set_layout (0.62) | **XX** set_layout (0.45) |
