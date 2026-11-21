# winfer native

The GPUI build of winfer. It stands beside the Tauri app in `../src-tauri` and doesn't change it.

```sh
cargo run -p winfer-app --bin winfer-native
cargo test -p winfer-app
cargo test -p winfer-backend   # the Tauri app's own backend tests, run through the shim
```

## How it reuses the Tauri app

- `crates/winfer-backend` compiles `../src-tauri/src/*` by `#[path]`, unchanged. Only `lib.rs`, `menu.rs` and
  `undock.rs` are left out.
- `crates/tauri*` are small stand-ins for the Tauri APIs those modules use. Events and channels end at closures the
  GPUI app registers (`AppHandle::listen_any`, `Channel::new`).
- `crates/winfer-app` is the UI and the logic that lived in TypeScript.
- Data lives in `dev.winfer.native`, apart from the Tauri app's `dev.winfer.app`. The saved snapshot has the same JSON.
  Exceptions, hard-coded in the shared backend: accounts (`accounts.rs`) and harness settings (`harness/config.rs`) are
  shared with the Tauri app, and `browser.log` is written to its folder.

Decisions: plugins will run in an embedded wry webview (they are DOM and JS); voice will use native WebRTC.

## Status

Not feature complete. `src/**` paths below are the Tauri frontend still to port.

Done:

- [x] Backend through the shim; agent subcommands (`mcp-stdio`, `mcp-filter`, `hook`, `statusline`, `code`)
- [x] Workspace state and snapshot persistence, layout tree, presets, overview sync
- [x] Terminal: alacritty core, renderer, keyboard, mouse, selection, paste, OSC 9/777, dev-URL detection
- [x] Shell: title bar, sidebar, tiles, gutters, pane drag and drop, zoom, archive, overview
- [x] Agent status from hooks and output; worktree per pane
- [x] Browser panel: tabs with favicons, address bar, screencast, input; phone view and page resize handles, console, element picker, agent cursor, undock into its own window
- [x] Command bus and full keymap (user overrides honored), command palette, shortcut sheet
- [x] winfer MCP tools for agents: `list_panes`, `read_pane`, `send_to_pane`, `spawn_agent`, `open_url`, `notify_user`,
  `ask_user` (dialog), `bg` notes. Exercised end to end through `mcp-stdio` except `open_url` and `ask_user`
- [x] New pane dialog (branches, worktrees), model picker, worktree changes view, merge, discard
- [x] Side panel tabs, side terminal, file viewer (markdown, highlighted code, images, live reload, edit, comments)
- [x] Terminal links (URLs and file paths), `open_file` MCP tool
- [x] Review inbox (diff stats, LLM summary, tests, screenshot, conflict warnings), merge queue, history
- [x] Usage: live numbers per account, title-bar meters and popover, warnings, `usage_status`, `wait_for_usage_reset`,
  start account per pane
- [x] Orchestration: race, A/B test, results with winner verdict, broadcast, recipes, agent bar
- [x] Progress estimates, session titles, Sessions overlay, pane header progress bar
- [x] Plugin host in a wry webview: plugins' code, pages (title bar tabs, Apps menu, pins), palette tools, recipes,
  agent tools over MCP, panel buttons, `api.confirm` dialog, vault, file access, pane events
- [x] Sidebar (Sessions/Review/History, workspaces and sessions with drag reorder, context menus, rename, tooltips,
  Latest feed, footer); pane header (menu, rename, context meter and card, cost, glass design); scrollback search;
  notifications with the agent's question and Approve/Decline/Reply
- [x] Branch dialog (local/remote, new branch, pull latest, as agent or workspace); sidebar worktree list with
  open/resume, menus, remove, bulk clean-up and selection
- [x] Settings → Accounts (add, sign in, order, enable, remove) and account failover for terminal panes;
  Settings → Usage (cost/token charts, models, by task, limits with projections, tool stats)
- [x] Title bar: provider menu on New (usage left, default star, per-model start), live-voice button, TLS toggle in the browser, pane notes, review "send back", full Diagnostics page (memory timeline, log folder, crash log)
- [x] Mobile bridge: same wire protocol (mirror, threads, send, choose, approve, new pane). Chat panes (winfer code) stream their live thread to the phone and hand it their exact approvals and question forms, answered from the phone; plugin approvals show on the phone too
- [x] Android mirror: live H.264 (openh264) with screenshot fallback, tap, swipe, scroll, keys, emulator start
- [x] Limit handoff ("Continue in …") and Codex reset credits
- [x] Settings → Voice (with native microphone test), AI, winfer code, Mobile, Plugins, Skills
- [x] Viewer: PDFs (hayro, page and zoom), markdown comments per block
- [x] Settings → winfer code → Local models (host, key, thinking levels and budgets, Test) and the local subagent model
- [x] winfer code chat pane (the default; Settings → winfer code → "Use TUI style" brings the terminal UI back): transcript with markdown, tool rows and diffs, approvals and questions, plan, slash commands, `@` files, attachments, model/effort/mode pickers, sessions, usage, tasks, context meter. chat panes animate in (rows fade up, spinners, a working indicator, sheets); images open full size, links open in the browser or viewer, and file names in a reply's inline code open in the file viewer; the transcript is virtualized, so long sessions need no paging
- [x] Live voice with an OpenAI API key: Realtime API over WebSocket (cpal audio), winfer's tools (registry, agent, browser, bg), HUD, mute; ⌘⌥V. Untested against the live API
- [x] Updates: Stable/Nebula channel, check, What's new, and install in place ("Install and restart"): the signed bundle
  from `latest-native.json` replaces the running app (see Packaging)
- [x] Settings: General, Theme, Agents, Data, Diagnostics, pane cost; shortcut editor in the shortcut sheet

Partial:

- [ ] Live voice on a ChatGPT plan (needs WebRTC; the API-key path works)

Not started, roughly in priority order:

- [ ] Windows and Linux installers (they ship as a portable `.exe` and a `.tar.gz` of the binary); a Windows icon and
  code signing

## Packaging

`native/scripts/bundle-{macos,windows,linux}.sh [target]` build into `target/bundle`:

| Platform | Download | Update bundle |
| --- | --- | --- |
| macOS | `winfer-native-macos-<arch>.dmg` | `winfer-native-macos-<arch>.app.tar.gz` (winfer.app) |
| Windows | `winfer-native-windows-x86_64.exe` (portable) | the same `.exe` |
| Linux | `winfer-native-linux-x86_64.tar.gz` (the binary; needs WebKitGTK 4.1, GTK 3, ALSA) | the same `.tar.gz` |

On macOS, `WINFER_SIGN_IDENTITY` signs with a Developer ID and the hardened runtime and `WINFER_NOTARIZE=1` notarizes
and staples (see the script); otherwise the app is ad-hoc signed.

`.github/workflows/native.yml` runs the tests on macOS, Linux and Windows. Releases come from `release.yml`: every Nebula
(`vX.Y.Z-N`) and stable (`vX.Y.Z`) build of the Tauri app also bundles the native app of the same version into the
same release (`native-bundle`), with the version stamped into `Cargo.toml`. `native-updater-json` signs the update
bundles with the Tauri app's updater key and writes `latest-native.json`, the native app's feed, next to the Tauri
app's `latest.json`: on the release itself (stable apps read the latest release's) and on the `nebula-channel` release
(Nebula apps). `src/updates.rs` checks the channel's feed, verifies the bundle against `plugins.updater.pubkey` in
`src-tauri/tauri.conf.json`, moves it in place of the running app (the `.app` on macOS, the executable elsewhere; it
needs write access to that folder) and relaunches; the new copy waits for the old one to quit.

## Development aids (debug builds)

- `WINFER_NATIVE_SPAWN=shell,claude`: opens these panes at launch
- `WINFER_NATIVE_CODE_WEB=1`: winfer code panes show the chat, whatever the setting
- `WINFER_NATIVE_PROMPT=…`: the start prompt of those panes
- `WINFER_NATIVE_CODE_DEMO=1` (`welcome`, `draft`, `fast`, `model`, `effort`, `mode`, `diff`, `phone`): chat panes play a scripted session instead of starting a process, to check the styling without a login
- `WINFER_VAULT_FILE=path`: keep the keychain values (paired phones, keys) in a file instead, so a test copy can't touch the real ones
- `WINFER_NATIVE_LAYER=browser|palette|…`, `WINFER_NATIVE_MOBILE=1`, `WINFER_NATIVE_INSTALL_BROWSER=1`: open a layer, the phone view, or download Chromium at launch
- `WINFER_NATIVE_URL=https://…`: loads this page in the browser panel
- `WINFER_NATIVE_WORKSPACE=<dir>`: opens a workspace for this folder at launch
- `WINFER_NATIVE_LAYER=palette|new-pane|shortcuts|race|abtest|branch|page:<plugin>/<page>|broadcast|recipes|sessions|side-terminal|settings:<page>|file:<path>`: opens that layer at launch
- `WINFER_NATIVE_HOOK_FILE=<path>`: writes the hook server's address and token there, for test clients
- `WINFER_CHROME=<path>`: uses this browser (a backend setting)
