# Performance audit — winfer & winfer code (2026-10-24)

## Done (same day; measured with the same dev-build benchmarks)
| Commit | Change | Before → after |
| --- | --- | --- |
| `5381445` | winfer code: memoized transcript rows and Linkify, lazily mounted collapsed bodies, markdown rendered per block (`mdBlocks.ts`), code highlighted once complete | per token in the 241-item session: 215 → 11 ms (DOM 47k → 16k nodes); 20 KB reply: 25 → 8 ms average |
| `f0346ef` | Startup orphan sweep: one `ps` for all profiles, on a background thread that browser launches wait for | ~1.7 s on the main thread → ~50 ms off it |
| `3709139` | Screencast acked once the view drew a frame (with a 1 s fallback); detached while the window is hidden | frame rate follows what the view can draw; nothing streams while minimized |
| `27091e3` | `useWorkspacesPick` instead of whole-store `useWorkspaces()`; no-op `focusPane`/`updatePane`; memoized `PaneFrame` | click in the focused pane: 166 → 0.04 ms; unrelated store change: 19.5 → 0.06 ms |

Still open: items 4–8 below, the harness-backend and Rust items, and the rest of the frontend list.

How it was measured: the `?codeweb` demo in a Vite dev server, driven and profiled over CDP in winfer's browser panel
(Chromium, React dev build), plus Node micro-benchmarks of the hot functions (`/tmp/winfer-bench/`). Absolute numbers
will differ in the production WKWebView build. Roughly 35% of the dev-build render time is React dev-only overhead,
but the scaling (what grows with session length or reply length) is the same. Items marked *(audit)* come from a
code read and were spot-checked, not measured.

## winfer code — web UI

### 1. Every streamed token re-renders the whole transcript — **high**
- `Transcript.tsx:368-432` maps every item on every store change. `ToolCard` (`:158`), `Reasoning` (`:256`), and the
  user and notice rows aren't memoized.
- Each render re-runs `Linkify` → `split()` (`Links.tsx:96`, `:59`) over every tool output. That is a URL regex plus
  `findPaths`' three Unicode regexes (`viewer/paths.ts:34`).
- Collapsed tool bodies stay in the DOM (`Collapse`, `Transcript.tsx:53-65`, the grid-rows trick). Tool outputs are
  kept in full: whole `Read` results and whole `Bash` outputs (`claude.rs:693-706`).
- **Measured:** a 241-item session (40 rounds of Read/Bash/Edit plus a reply, 635 KB) had 47k DOM nodes.
  - Each streamed delta took **~215 ms** of main thread, against ~12 ms in a fresh session.
  - Mounting the session (resume or remount) took ~1 s.
  - In the CPU profile, Linkify/`findPaths` accounted for 23%, and React element creation and reconciliation for
    most of the rest.
- During streaming the window drops to about 4–5 fps. That includes every terminal pane, because they share the
  main thread.
- **Fix:**
  - `memo` the row components on the `item` prop. Items are immutable and only the changed one gets a new object.
  - `useMemo` the `split()` result by text.
  - Mount a collapsed body only once it has been opened for the first time.
  - Add `content-visibility: auto` with `contain-intrinsic-size` on rows, or virtualize.
  - With these, a token should cost what it costs in a fresh session.

### 2. Streaming markdown is O(n²) — **high**
- `Markdown.tsx:105-122` re-parses the whole reply (remark + gfm → React) on every delta.
- **Node:** for a 20 KB reply, a delta costs 15 ms by the end, 6.5 s in total.
- **In the app (dev):** a 20 KB reply averaged 25 ms per delta and reached 55 ms per delta by the end.
- **Fix:**
  - Split the text into top-level blocks (blank lines outside code fences, or `marked.lexer`).
  - Memoize the finished blocks and re-render only the last, growing one.
  - Optionally apply store updates once per animation frame.

### 3. Shiki re-highlights a streaming code block on every delta — **high**
- `Markdown.tsx:20` runs the effect on `[code, lang, dark]`, so every delta inside a code fence runs `codeToHtml` again
  on the whole block. That work is synchronous on the main thread and can't be cancelled once it starts.
- **Node:** one highlight of a 4 KB TS block takes 35 ms, which adds up to **~3 s** of main thread for one streamed
  4 KB block. For a 12 KB block it is ~13 s.
- **Fix:** show a plain `<pre>` while the block is still open (fence not closed, or the item is still streaming), then
  highlight once. Alternatively debounce by ~300 ms, or move highlighting to a worker.

### 4. The "Working"/"Thinking" shimmer repaints every frame — **medium**
- `.cw-shimmer` (`code.css:417-424`) animates `background-position` on a `background-clip: text` span. It's used at
  `Composer.tsx:299-300`, `Transcript.tsx:264` and `Sheets.tsx:467`.
- **Measured A/B on a busy pane:** 96 ms/s of main thread with the shimmer and orb, 9 ms/s without the shimmer, and
  0.6 ms/s with neither. So that's **~9% of a core per working pane**, for as long as it works.
- **Fix:** use an opacity pulse (there's already `cw-breathe`), or a gradient overlay moved with `transform` under a
  mask. As a stopgap, `steps(12)`.

### 5. The welcome night sky uses CPU while idle — **medium**
- It has 159 stars with infinite `cw-twinkle` animations (`Welcome.tsx:27-30`, `code.css:2585`), plus nebula, drift,
  2 shimmers, float and orbit.
- **Measured:** 129 ms/s of main thread while idle, 59 ms/s without the stars, 5 ms/s with no animations. Every fresh
  pane pays this until its first message. The TUI's welcome screen also redraws at 25 fps forever (`tui/app.rs:490`)
  *(audit)*.
- **Fix:**
  - Draw the stars as one canvas or SVG layer, or fewer of them.
  - Stop after the intro or when the pane isn't focused.
  - Honor `prefers-reduced-motion` in the CSS, not only for shooting stars.

### 6. Title tracking does O(items) work per delta and can loop LLM calls — **medium**
- `CodePane.tsx:388-400`: an in-place item update isn't "appended", so `added = st.items`, and it filters every item
  on every delta.
- For a resumed session with no saved title, it calls `nameFromHistory` on every store change. If naming fails or the
  title is rejected (`progress.ts:139`), `naming` is cleared (`:145`) and the next delta fires another `llmCompose`.
  With the Codex provider, each of those calls is a new CLI process (`llm.rs:164`).
- **Fix:** handle restored items only on the `restore` event, and remember per pane that naming was attempted.

### 7. Task-log polling resends up to 256 KB every 400 ms — **medium**
- `Sheets.tsx:376` polls `task_log`. Each time, `serve.rs:394-402` re-reads and cleans a tail of up to 256 KB and sends
  all of it as JSON through the PTY. The store update then re-renders even when nothing changed.
- **Fix:** reply only when the size or mtime changed, or send only the new bytes, with the frontend appending.

### 8. Smaller items — **low**
- **Whole-store `useCode()`:** `Composer.tsx:160`, `CodePane.tsx:208` and `:241`, and `Sheets.tsx:90`, `:256`, `:282`,
  `:494`, `:609`, `:669` re-render on every event. **Fix:** per-field selectors or `useShallow`.
- **`@` mentions:**
  - Every new mention re-walks up to 50k files (`serve.rs:328-331`, `tui/app.rs:2703`) and ships a multi-MB JSON line.
  - `rankFiles` (`mentions.ts:38-48`) lowercases every path on every keystroke: 16–20 ms per key with 50k files.
  - **Fix:** lowercase the list once when it arrives, and cache it per session with a short TTL.
- **Transport:** `transport.ts:25` re-scans the whole buffer from the start on each chunk. A 2.4 MB line in 16 KB
  chunks takes 38 ms, against 3 ms with an incremental scan.
- **`DiffView`:** re-splits the diff and renders every row of every diff on each render, each row with its own fade
  animation (`Transcript.tsx:67-111`, `code.css:640-644`). Item 1 covers it.
- **Follow-to-bottom:** `toBottom()` forces a layout on every items change and again from the ResizeObserver
  (`Transcript.tsx:319-341`). It measured ~1% of the time.

## winfer code — harness backends (shared by the web UI and the TUI)
- **ChatGPT backend, every model request:**
  - `trace(&json!({ body.to_string().len(), prefix_hash(&body) }))` (`chatgpt.rs:549`) builds its arguments even when
    `WINFER_CODE_TRACE` is unset. The env-var check is inside `trace`, at `:1116`. That costs two full serializations,
    a deep clone and a SHA-256 of the whole history.
  - `json!(st.items)` deep-copies the history while holding the state lock (`:521`).
  - **Fix:** check the env var before building the arguments, and serialize once from borrowed data.
  - Not verified *(audit)*: old `input_image`s are never trimmed, and the MCP lock is held across tool calls, which
    lets one slow call stall other agents.
- **`session::list()`** (`session.rs:60-73`) fully parses all 776 session files (6.1 MB here), transcripts included,
  just to list them. It runs for `/resume`, `--continue` and agents' `list_chats`. **Fix:** a metadata index, or a
  struct without the transcript field, plus pruning.
- **Micro-compact dry run** (`microcompact.rs:73-91`, called from `claude.rs:479-485` and `codex.rs:521-528`) reads and
  parses the whole CLI session JSONL after every turn while context is between `trim_at` and `compact_at`. The largest
  Claude transcript here is 51 MB. **Fix:** skip when the size and mtime haven't changed, and prefilter lines before
  parsing them.
- **Checkpoints** take 1.9 GB on disk here (`harness/checkpoints`). *(audit)*: `gc.auto=0`, and the TUI waits for
  `git add -A` + `write-tree` + an index copy before sending each message (`checkpoint.rs:59`, `:87-90`).
- **Per-delta work in the backends is fine.** There is one `Value` parse per stdout line, and the TUI batches deltas
  per frame. `Config::load()` is not on a hot path.

## winfer — Rust side
- **Startup spends ~1.7 s in `ps` before the window appears — high.**
  - `lib.rs:138` `kill_orphans` runs in `setup()` before `create_main_window`.
  - It calls `reclaim_all` (`chrome.rs:256-265`), which runs `holders::end` plus one `clear_lock` per browser profile.
    Each of those runs `ps -axww -o pid=,command=` (`:288`).
  - Here that's 33 profiles × ~52 ms (measured), on every launch.
  - **Fix:** run one `ps` into a HashSet, do it on a background thread that browser launches wait for, and delete the
    profiles of removed workspaces.
- **The screencast has no backpressure — high.**
  - `browser/mod.rs:1239` starts it with `everyNthFrame: 1`, quality 80 and no `maxWidth/maxHeight`. Frames are
    scaled by DPR (`BrowserView.tsx:85`).
  - Each frame is acked as soon as it arrives (`mod.rs:402-407`), even while the panel is hidden or the app is
    minimized. JS then drops the frames it can't draw.
  - **Fix:**
    - Ack after the frame is drawn, so only one is in flight.
    - Cap the size to about 1.5× CSS pixels and lower the quality to about 60.
    - Stop the screencast when the document is hidden.
- **Hook server — medium.** It's one thread (`hooks.rs:360-361`). On Claude `Stop` it reads the whole transcript (up
  to 51 MB) before it replies (`:142-149`), so other panes' hooks queue up behind it (2 s client timeout). **Fix:**
  reply first, then scan on a worker, from a tail or a saved offset.
- **`Prices::load()` — medium.** It re-parses the 6.7 MB models.dev catalog (JSON inside a JSON string, so parsed
  twice) on every `usage_history`, `usage_dirs`, `usage_sessions` and `usage_by_dir` call (`usage_history.rs:772-776`;
  callers at `:816`, `:836`, `:855`, `:870`). `usage.rs` already caches the same catalog. **Fix:** a static cache keyed
  by the catalog digest.
- **`state_save` — medium.** It deletes and re-inserts every workspace and pane row on every change (debounced to
  400 ms) as a sync command, so on the main thread (`store.rs:249-256`).
- *(audit)* **Usage-history scans** open all ~4,200 agent logs on every scan (`usage_history.rs:400-461`).
- *(audit)* **Worktree polling** runs 5 + 3 git processes per worktree every 20 s (`worktree.rs:657-720`).
- *(audit)* **Several sync commands block the main thread:** `android_mirror_start`, keychain access and plugin
  listing. **Fix:** `#[tauri::command(async)]`.

## winfer — frontend
- **Whole-store subscriptions and `focusPane` — high.**
  - `AppShell.tsx:158` and about 18 always-mounted components call `useWorkspaces()` with no selector, and nothing is
    memoized.
  - `focusPane` (`workspaces.ts:458-468`) always rebuilds `workspaces` and then calls `updatePane`, which always
    rebuilds it too (`:564-569`).
  - The result: every mousedown in a pane re-renders the whole app, and every pointermove does during resize drags.
  - **Fix:** use selectors (or `useShallow`), return early when nothing changes, and `memo(PaneFrame)`.
- **Terminal resize — medium.** The ResizeObserver runs `fit()` once per frame per terminal while dragging, with 10k
  lines of scrollback to reflow (`xtermRegistry.ts:183`, `:485-492`, `:547`). **Fix:** defer fits until pointerup
  while `body.resizing` is set.
- **WebGL contexts — medium.** Each terminal's WebGL addon is never disposed on detach (`xtermRegistry.ts:522-557`).
  WebKit caps live WebGL contexts at about 16, and one that is lost falls back to the DOM renderer for good. Visiting
  Overview attaches every pane. **Fix:** dispose on detach, or keep only the most recently used ones (LRU), and retry
  WebGL after a loss.
- **Progress estimates — medium.** Every 60 s, each working agent gets 120 terminal lines read, 3 git commands and an
  LLM call (`progress.ts:294-307`). With the Codex or Claude provider, that LLM call is a CLI process. This doesn't
  pause while the window is hidden.
- **Usage store — low.** It writes `localStorage` on every change, with no debounce (`usage.ts:115-117`).
- **Mobile bridge — low.** On every workspaces change, its subscriber `JSON.stringify`s the layout, even with no phone
  paired (`mobile.ts:359-364`).
- **Bundle (Oct 22 `dist`) — low.** It is 316 chunks totalling 11.8 MB, mostly Shiki grammars, which are lazy and
  fine. But `App` (972 KB) and `keymap` (624 KB) load up front: `ViewerPanel` statically imports react-markdown,
  rehype-raw and Shiki, and `SettingsModal` is static too. **Fix:** `React.lazy`.

## TUI (secondary)
- `tui/view.rs:162-168`: the transcript cache resets completely when any cached item's version changes. A streaming
  reply that has finished fading in therefore rebuilds and deep-copies the whole transcript every frame.
- *(audit)*: a running background shell keeps the 25 fps tick on and disables the cache. `running_subagents()` scans
  every item several times per frame.

## Bug found along the way
- `claude.rs:1081-1084`: the `Write` diff is computed at tool end, after Claude Code has already written the file. So
  `old == new`, and Write cards show an empty diff.

## Suggested order
1. Memoize transcript rows and the Linkify result, and mount collapsed bodies lazily (item 1).
2. Hold off highlighting until a code block is closed (item 3).
3. Re-render only the last markdown block while streaming (item 2).
4. Make the shimmer and the welcome sky cheap (items 4–5); fix the title-tracking loop (item 6).
5. Fix the startup `ps` sweeps and add screencast backpressure.
6. Fix `focusPane` and the whole-store subscriptions in the app shell.
