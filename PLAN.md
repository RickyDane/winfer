# winfer — Plan

Cross-platform (macOS + Windows) desktop app for orchestrating coding agents (Claude Code, Codex, Antigravity CLI, Grok Build, …) in auto-tiling terminal panes, with voice-first control, an embedded agent-drivable browser, and a workspace sidebar.

## Decisions

| Topic | Decision |
|---|---|
| Platforms | macOS + Windows. No Linux yet. |
| Product scope | Local-only. No accounts, no billing. |
| Shell | Tauri v2 (Rust) + React 19 / TypeScript / Vite |
| Terminals | `portable-pty` (ConPTY on Windows) + xterm.js (WebGL) |
| Browser | Managed Chromium (Chrome for Testing, ~150 MB, downloaded on first use) driven via CDP screencast; agents attach to the same browser |
| Voice STT | Local whisper.cpp (`whisper-rs`) |
| Voice intent | **OpenAI Decisions API** (`POST /v1/decisions`, GPT-6 Luna) for short commands; long or complex ones are planned by an LLM (ChatGPT via the Responses API, or Claude Code headless). No grammar parser. Replaced the Cloudflare Clef model in Oct 2026. |
| Why not local Clef | M0.5 spike (`spikes/clef-local/REPORT.md`): 95.5% accurate but ~6 s per command on an M1 Pro (needs < 0.6 s). Compute-bound; no caching possible. |
| Voice requirements | Network + Sign in with ChatGPT (the user's ChatGPT plan; tokens in the OS keychain). Without it, voice commands are off unless Claude Code is installed for the planner; keyboard and palette still work. |
| Git worktree per agent | On by default |
| Voice auto-run | Confident, safe commands run immediately, with undo |
| Plugins | Folders with a `plugin.json` in the app data dir: agent CLIs, recipes, palette/voice tools, and a JS module (`activate(api)`) that can add title bar pages, tools for agents (via winfer's MCP server) and keychain secrets. The store is the repo's `plugins/` folder (`catalog.json` with SHA-256 per file), installed from Settings → Plugins. See `docs/plugins/README.md`. |

## Architecture

```
┌──────────────────────── React (WebView) ──────────────────────────┐
│ Sidebar │ TileLayout (xterm panes) │ BrowserPanel (canvas + tabs) │
│ CommandPalette · VoiceHUD · Toasts · Settings · Keymap overlay    │
│   Zustand stores ◄──► TOOL REGISTRY ──► COMMAND BUS (typed)       │
└───────────────┬───────────────────────────────────────────────────┘
     invoke() · ipc::Channel (binary streams) · events
┌───────────────┴──────────── Rust (Tauri core) ────────────────────┐
│ PtyManager (portable-pty, ring-buffer scrollback)                 │
│ ProviderRegistry (detect, spawn, resume, hook injection)          │
│ AgentStatus (hook socket, OSC 9/777, idle heuristics)             │
│ BrowserManager (Chromium + CDP, screencast, input)                │
│ VoiceEngine (cpal → Silero VAD → whisper.cpp)                     │
│ Decision (SystemOneClient → Workers AI Clef API)                  │
│ Store (SQLite) · Worktrees (git) · local MCP server (later)       │
└──────┬────────────────────┬─────────────────────┬─────────────────┘
       │ spawns PTYs        │ launches + controls │ HTTPS
 claude / codex / agy / grok   Chromium (CDP)       Workers AI (@cf/cloudflare/clef-flash)
                                ▲
              agents attach: browser-use, Playwright MCP, chrome-devtools-mcp
```

**Core rule:** every input (keys, voice, palette, sidebar, later MCP) becomes a typed `Command` on one bus. The **Tool Registry** describes every user-facing action once; voice, the ⌘K palette, keymap labels and the future MCP server all read it.

```ts
type Command =
  | { type: "pane.spawn"; provider: ProviderId; count?: number; workspaceId?: string; cwd?: string; prompt?: string; worktree?: boolean }
  | { type: "pane.focus" | "pane.close" | "pane.zoom"; target: PaneRef }
  | { type: "pane.resize"; target: PaneRef; dir: "left" | "right" | "up" | "down"; step?: number }
  | { type: "pane.sendText"; target: PaneRef | "all"; text: string; submit?: boolean }
  | { type: "layout.preset"; name: "auto" | "grid" | "main-stack" | "columns" | "rows" }
  | { type: "browser.open"; url: string; newTab?: boolean }
  | { type: "workspace.switch" | "workspace.create"; id?: string; name?: string };
```

## Subsystems

### Terminals / PTY
- Reader thread per pane; output batched (~8 ms / 64 KB) and streamed as raw bytes over `tauri::ipc::Channel`; xterm write callback for backpressure.
- Rust keeps a 2–5 MB ring buffer per pane so a reloaded webview or re-attached pane replays scrollback; agents keep running while their workspace is hidden.
- macOS GUI apps don't inherit the shell PATH: resolve env once via `$SHELL -ilc env`.
- Env injected into every PTY: `WINFER_PANE_ID`, `WINFER_WORKSPACE`, `WINFER_CDP_URL`, `WINFER_SOCK`.
- Closing a pane kills the whole process group.

### Tiling
```ts
type LayoutNode =
  | { kind: "leaf"; paneId: string }
  | { kind: "split"; dir: "row" | "col"; sizes: number[]; children: LayoutNode[] };
```
- Default auto-tile is "dwindle" BSP: new pane splits the focused/largest leaf along its longer axis. Presets: grid, main+stack, columns, rows.
- Resize by gutter drag, keyboard (5% steps), zoom/maximize, equalize; min 40 cols × 8 rows. Drag header to swap/move. Pop a pane out to its own window.
- xterm instances never unmount on layout change: DOM nodes live in a registry outside React and are re-parented into leaf slots.
- WebGL context limit (~16/page): only visible panes use the WebGL renderer.

### Providers
Data-driven, user-editable JSON:
```jsonc
{ "id": "claude", "name": "Claude Code", "command": "claude", "args": [],
  "resumeArgs": ["--continue"], "aliases": ["claude", "cloud code"],
  "status": "claude-hooks", "icon": "claude.svg", "env": {} }
```
- Presets: Claude Code `claude`, Codex `codex`, Antigravity CLI `agy`, Grok Build `grok`, OpenCode, Cursor CLI, Copilot CLI, Aider, Amp, plain shell. Detect installed CLIs at startup.
- Status (idle / working / needs input / done / error): Claude Code hooks via per-session `--settings` file calling `winfer-notify $WINFER_PANE_ID`; Codex `-c notify=[...]`; generic OSC 9/777, BEL, idle timer, prompt regex.

### Git worktrees (on by default)
- Spawning an agent into a git repo creates a worktree + branch `winfer/<provider>-<n>` under the app data dir; the pane's cwd is the worktree.
- Per-pane diff view; merge / discard / open PR actions. Toggle off per spawn ("no worktree") or per workspace.

### Browser
- WKWebView has no CDP, so the browser is a managed headless Chromium (`--remote-debugging-port=<random>` on 127.0.0.1, per-workspace `--user-data-dir`).
- Each UI tab = CDP target. `Page.startScreencast` frames → Rust → Channel → `createImageBitmap` → canvas. Input via `Input.dispatch*` / `Input.insertText`. Device toggle via `Emulation.setDeviceMetricsOverride`.
- Agents get `WINFER_CDP_URL`; MCP auto-config for Playwright MCP (`--cdp-endpoint`), chrome-devtools-mcp (`--browser-url`), browser-use (`cdp_url`). Agent-opened tabs appear with an agent badge; tabs have user/agent ownership, "follow agent" and "lock tab".
- Later: native child webview for preview-only tabs (smoother scrolling, no agent control).

### Voice → decision → action
```
hold ⌥Space → cpal → VAD → whisper.cpp → transcript
  → state = transcript + compact app snapshot + last 3 commands
  → ONE Workers AI call (@cf/cloudflare/clef-flash)
  → policy gate (confidence + risk)
  → tool.toCommands(args) → Command Bus → HUD (+ optional TTS)
```

**Tool registry**
```ts
interface ToolDef {
  id: string;            // "spawn_panes"
  description: string;   // option text the model sees — this IS the prompt (keep ~10–15 words)
  risk: "safe" | "confirm";
  params: Record<string, ParamDef>;
  toCommands(args: Record<string, unknown>, ctx: AppSnapshot): Command[];
}

type ParamDef =
  | { kind: "enum"; ask: string; options: (ctx: AppSnapshot) => Record<string, string | null> } // choice
  | { kind: "count"; ask: string; max: number }                                                  // choice "0".."max"
  | { kind: "perOption"; ask: string; options: (ctx: AppSnapshot) => Record<string, string>; max: number } // one count question per option
  | { kind: "bool"; ask: string }                                                                // noul
  | { kind: "level"; ask: string; levels: string[] }                                             // score
  | { kind: "span"; ask: string };                                                               // choice over transcript spans
```
- Options are built from live state each call (installed providers, open panes, workspaces, tabs, dev-server URLs, layout presets).
- Multi-provider spawns use one count question per provider ("claude, codex and grok" → 1/1/1; "two claudes and a codex" → 2/1). Spawn order = order of first mention.
- Up to two actions per utterance (`action_1`, `action_2`).
- Free text without a parser: dictation is its own mode (hold ⌥⇧Space; whole transcript is the text; model picks only `target_pane` + `submit`). URLs / workspace names use `span` params (contiguous transcript spans ≤12 words as choice options) plus a URL normalizer.
- Initial tools: spawn_panes, close_pane, focus_pane, zoom_pane, resize_pane, swap_panes, equalize_panes, set_layout, send_to_pane, broadcast, approve_agent, interrupt_agent, open_url, browser tab new/close/switch, back/forward/reload, set_device, switch_workspace, create_workspace, toggle_sidebar, toggle_browser, load_recipe, show_shortcuts, repeat_last, undo_last, none.
- Use the full joint request (~2.6k tokens): in the spike it gave the best accuracy (action_1 63/66, args 32/37). Datacenter GPUs make input length cheap; re-measure latency once credentials exist.
- Accept `action_2` only if it differs from `action_1` (the model tends to repeat action_1). Encode counts as words ("zero".."four"), not digits.

**Policy gate**

| Condition | Result |
|---|---|
| `none`, or confidence < 0.30 | "Didn't catch that"; nothing runs |
| 0.30–0.60 | HUD shows top 2; ↩ or "yes" confirms |
| ≥ 0.60 and `safe` | Runs immediately; toast with undo (⌘Z / "undo") |
| `confirm` tools (close all, approve agent, kill, delete workspace) | Always confirm |

Thresholds are calibrated by the eval harness (starting values from the M0.5 spike).

**Decision client**
- Rust `decision/client.rs`: `POST https://api.cloudflare.com/client/v4/accounts/{id}/ai/run/@cf/cloudflare/clef-flash`, model `clef-flash`, unwrap `result`. Optional routing through Cloudflare AI Gateway (logs, caching, retries).
- Account ID + API token in the OS keychain (`keyring`), never in the webview. Settings has a "Test connection" button.
- Model selectable: `clef-flash` (default, fast) or `clef` (27B, more precise, slower, $0.24/M).
- Cost: ~2.6k input tokens/command at $0.09/M ≈ $0.00024; the Workers AI free 10k neurons/day ≈ 1.2M tokens ≈ 450 commands/day.
- Privacy: only the transcript + minimal snapshot (pane labels, providers, statuses, workspace names, tab titles, recent commands) are sent; never terminal output, file paths or env.
- Timeout 3 s; on error/offline the HUD says so and nothing runs. No local fallback.
- Target: < 1 s from key release to action (whisper local ~300 ms + API round trip).

**Eval harness**
- `voice-evals/*.jsonl`: utterance + snapshot → expected commands (start ~200, include misheard names).
- `pnpm eval:voice --model clef-flash|clef` → accuracy, p50/p95 latency, calibration → thresholds. Reuse the 66 cases from `spikes/clef-local/evals.jsonl`.
- Starting thresholds from the spike (re-fit on the API's full-precision model): auto-run ≥ 0.60, confirm 0.30–0.60, reject < 0.30; `none` = ignore at any confidence.
- Tool descriptions are the prompt; tune descriptions, not code.

### Workspaces
- Workspace = root dir(s), color, layout tree, panes, browser tabs, env, default provider, browser profile.
- Sidebar tree (workspace → sessions) with status dots and unread badges, drag reorder, sections.
- Switching hides the view; PTYs keep running. Everything persists to SQLite; restore on launch with provider resume flags.

### Keyboard (macOS; Windows maps ⌘ → Ctrl+Shift)

| Action | Key | Action | Key |
|---|---|---|---|
| New pane (default provider) | ⌘T | Command palette | ⌘K |
| New pane, pick provider | ⌘⇧T | Push-to-talk | hold ⌥Space (global) |
| Spawn provider N | ⌘⌥1–9 | Dictate to pane | hold ⌥⇧Space |
| Focus pane N | ⌘1–9 | Voice on/off | ⌘⇧V |
| Move focus | ⌘⌥←↑→↓ | Next agent needing input | ⌘J |
| Resize pane | ⌘⌃←↑→↓ | Swap pane | ⌘⇧⌥←↑→↓ |
| Zoom pane | ⌘↩ | Split right / down | ⌘D / ⌘⇧D |
| Equalize / cycle layout | ⌘⌃= / ⌘⌃L | Close pane | ⌘W |
| Toggle sidebar / browser | ⌘B / ⌘⇧B | Broadcast input | ⌘⇧I |
| Switch workspace | ⌃⌘1–9 | Browser URL bar | ⌘L |
| Search scrollback | ⌘F | Cheat sheet | ⌘/ |

All rebindable (keymap JSON). xterm `attachCustomKeyEventHandler` lets app shortcuts through.

## Repo layout
```
winfer/
  src-tauri/
    src/{pty,providers,status,browser,voice,decision,worktree,store,mcp}/
    resources/providers.json
  src/
    commands/      bus.ts, keymap.ts
    tools/         registry.ts, tool definitions
    features/
      terminal/    TerminalHost, xtermRegistry
      layout/      tree.ts (pure), TileView, presets
      browser/     BrowserPanel, ScreencastCanvas, TabStrip
      voice/       VoiceHUD, decision client glue
      workspaces/  Sidebar
      palette/
    stores/  lib/ipc.ts  styles/
  voice-evals/
  spikes/
```

## Milestones
1. **M0 Shell** — Tauri + React scaffold, theme tokens from the reference, three-column shell, CI builds for macOS + Windows.
2. **M0.5 Clef spike** — done: local Clef-flash is no-go (~6 s/command on M1 Pro). Follow-up: measure the Workers AI API on the 66 evals once credentials exist.
3. **M1 Terminals** — PtyManager, Channel streaming, xterm WebGL, provider registry + presets, PATH fix.
4. **M2 Tiling, keys, tools** — layout tree, auto-tile, resize/zoom/swap, DOM re-parenting, Command Bus, Tool Registry, keymap, palette.
5. **M3 Workspaces + worktrees** — SQLite, sidebar, restore/resume, status hooks, notifications, ⌘J, worktree per agent + diff view.
6. **M4 Browser** — Chromium manager, screencast tabs, navigation, device emulation, agent CDP + MCP auto-config, dev-server detection.
7. **M5 Voice** — capture, VAD, whisper, push-to-talk, Workers AI Clef client + credentials UI, policy gate, HUD, dictation, eval harness.
8. **M6 Extras** — race mode, element picker → agent, console/network capture, app MCP server, recipes, voice readback, activity feed.
9. **M7 Ship** — signing/notarization, auto-update, perf with 12+ panes, accessibility.

## Risks
- Embedded browser via screencast: latency, scroll smoothness, IME. Mitigation: native preview tabs later.
- Voice depends on network + Cloudflare: offline means no voice commands. Clef is weeks old; run the eval set when Cloudflare updates the model.
- Cloud latency from the user's region is unmeasured (others report ~200 ms from Europe).
- CDP port is unauthenticated (localhost, random port); app socket 0600 + token; agents get isolated browser profiles.
- Provider CLIs change flags often → data-driven registry.
