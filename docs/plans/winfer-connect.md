# Winfer Connect — plan

Connect several winfer instances (your Mac Studio, laptop, a Linux box…) so each can see, open and drive the
other's sessions, and agents can coordinate across machines. Built like the mobile PWA bridge (pairing →
end-to-end encrypted frames → host answers requests about its panes), but desktop ⇄ desktop, mutual, and with
live terminals.

Part two (below): **`winferd`, a headless winfer** for servers that you pair and drive through Connect.

## Decisions (from Q&A)

| Topic | Decision |
|---|---|
| Version | **native-gpui only**. Shared `src-tauri/src` stays untouched (the README rule "compiled unchanged"). |
| Scope | **Live remote terminals**, **full native chat view** for remote winfer code panes, **cross-machine agents** (MCP), **native notifications**. |
| Transport | **Relay + LAN direct**: same Cloudflare relay as the phone, upgraded to a direct LAN socket when both are on one network. |
| Direction | **Mutual**: one pairing, both instances see and control each other. |
| Trust | **Full trust**: paired = full control both ways. Revoke to cut off. |
| Pairing | **LAN discovery + 6-digit code** confirmed on both screens; off-LAN fallback: paste a pair link. |
| UI | **Sidebar section per connected instance**; opening a remote session adds it as a **tile in the current local layout**. |
| PTY size | **Remote takes over**: opening/focusing a pane remotely resizes it to the remote tile; the host's tile shows "Viewed on ‹machine›" until clicked back. |

## What exists and gets reused

- `src-tauri/src/mobile/crypto.rs` — HKDF-SHA256 + AES-256-GCM on `ring` (also has X25519 and HMAC: no new crypto deps).
- `src-tauri/src/mobile/relay.rs` — reconnecting tokio-tungstenite client with ping/backoff (pattern to copy; it hardcodes `role=desktop`).
- `relay/src/index.ts` — Cloudflare Worker + one Durable Object per room, opaque ciphertext forwarding.
- `native/crates/winfer-app/src/ui/mobile.rs` — the host side for phones: pane snapshot (`MobilePane` shape), requests (`send`, `choose`, `approve`, `pane.new`…), chat-pane request forms. Connect's host reuses its snapshot/request helpers.
- `src-tauri/src/pty.rs` — per-pane 2 MiB replay ring (used for terminal attach snapshots).
- `native/crates/winfer-app/src/terminal.rs` — `Session` (alacritty grid; `write_screen`, `write_process`, `resize`): remote tiles get a `Session` whose process side is the wire.
- `native/crates/winfer-app/src/code/session.rs` — `CodeModel` parses `winfer code --serve` JSON lines (`Ev`) and writes `Cmd`s; remote chat tiles run the same model over the wire.
- `native/crates/winfer-app/src/ui/agent_api.rs` — native handling of `list_panes` / `read_pane` / `send_to_pane` / `spawn_agent`.
- Native notifications with Approve/Decline/Reply (`ui/root.rs`), presence/away logic in `mobile/mod.rs`.
- `WINFER_NATIVE_IDENTIFIER` — runs a second instance with its own data dir (two-instance dev/testing on one Mac).

## Architecture

```
winfer-app (GPUI)                                   winfer-app (GPUI)
  ui/connect/*  ── host role ─┐                ┌── host role ── ui/connect/*
               ── client role ┤                ├── client role
                              │                │
              winfer-connect crate (no GPUI): identity, peers, pairing, handshake,
              framing, transports, connection manager
                              │                │
                  LAN: mDNS + direct WebSocket (preferred)
                  WAN: Cloudflare relay room per pairing (always-on fallback)
```

### New crate `native/crates/winfer-connect`

GPUI-free, unit-testable, depends on `winfer-backend` for `mobile::crypto` helpers, plus `ring`, `tokio`,
`tokio-tungstenite`, and **`mdns-sd`** (new dep, pure Rust).

| Module | Responsibility |
|---|---|
| `identity.rs` | This instance: `instanceId` (16 random bytes), display name (ComputerName, editable), X25519 static key. Keychain service `<identifier>.connect`, account `identity`. |
| `peers.rs` | Paired instances `{peerId, name, pairKey, createdAt, lastSeen, lastAddr?, notify: bool}` in keychain account `peers`; `connect.json` in the data dir for non-secret prefs (enabled, discoverable, relay URL, has_peers). |
| `pairing.rs` | SAS pairing (below), pair links, 10-min expiry. |
| `handshake.rs` | Per-connection authenticated key exchange → session keys + counters. |
| `frame.rs` | Encrypt/decrypt, counter nonces, replay rejection, chunking >1 MiB. |
| `lan.rs` | WebSocket listener on an OS-assigned port, mDNS advertise/browse `_winfer-connect._tcp`. |
| `relay.rs` | Relay client for role `peer` (copy of mobile `relay.rs`, parameterized). |
| `link.rs` | One logical link per peer over the best transport; switching, resubscribe, presence, latency. |
| `proto.rs` | Message types (serde) shared by host and client. |

### App side `native/crates/winfer-app/src/ui/connect/`

| File | Responsibility |
|---|---|
| `host.rs` | Answers peers' requests about local panes: mirror, terminal/chat attach, input, resize, new/close pane, tool calls. Pushes status changes. |
| `client.rs` | Per-peer mirror state (workspaces, panes, statuses), remote tile sessions, request/ack plumbing. |
| `remote_term.rs` | `Session` backed by the wire (output → `write_screen`, keys/resize → `term.in`/`term.resize`). |
| `remote_chat.rs` | `CodeModel` transport over the wire (snapshot + live `Ev` lines in, `Cmd` out). |
| `sidebar.rs` | "Connected" sidebar section. |
| `settings.rs` | Settings → Connect page. |
| `pair_dialog.rs` | Nearby list, 6-digit confirmation, pair-link fallback. |
| `notify.rs` | Native notifications for remote panes. |
| `tools.rs` | Cross-machine MCP tool routing. |

## Security

Full trust means a paired machine can type into your shells, so the channel must be solid.

**Pairing (LAN, numeric comparison like Bluetooth):**
1. A sees B via mDNS (TXT: `id`, `name`, `v`), user clicks **Pair** on A. A connects to B's listener.
2. Both send ephemeral X25519 public keys + 16-byte nonces. Shared secret `ss`; transcript hash `th = SHA256(all handshake bytes)`.
3. Both show `SAS = 6 digits from HKDF(ss, th, "sas")`. B shows an incoming request "‹A› wants to connect · 123 456"; the user confirms on **both** machines (A shows the same code). Either side can cancel. Timeout 2 min.
4. `pairKey = HKDF(ss, th, "pair")` (32 bytes), stored on both sides with the other's `instanceId` + name. MITM chance: 10⁻⁶ per attempt, and attempts are visible.

**Pairing (off-LAN fallback):** A shows a pair link `winfer-connect:#p=base64url({v,r,k,id,n})` (one-time relay room + one-time key, 10-min TTL, like the phone's QR). Paste into B → both join the one-time room as `peer`, run the same X25519 exchange authenticated with the one-time key (HMAC), show the same 6-digit SAS for a final confirmation, derive `pairKey`, delete the one-time room.

**Per-connection handshake (both transports):** HMAC challenge-response with `authKey = HKDF(pairKey, "auth")` over fresh nonces from both sides → `sessionKey(A→B)`, `sessionKey(B→A)` = HKDF(pairKey, nonces, direction). Frames: AES-256-GCM, nonce = 64-bit counter (+4 zero bytes), so no nonce reuse and replays/reorders are rejected. A reconnect = new handshake = new keys.

**Relay room per pairing:** `roomId = HKDF(pairKey, "room")[..16]`, `authToken = HKDF(pairKey, "relay")` — both peers derive the same room, nothing extra to store. Revoke: `DELETE` the room (closes with 4001), drop the peer locally, send `bye` if online (the other side drops it too).

**LAN listener:** only accepts the handshake; unpaired connections can only start pairing (rate-limited: 1 pending request, 5 per minute, shown to the user). The "Discoverable" toggle stops advertising and pair requests while keeping paired links working. macOS shows its incoming-connections firewall prompt once.

## Relay change (`relay/`, deploy needed)

Backward compatible: add role `peer`.
- `?role=peer`: at most 2 sockets per room (a third replaces the oldest from the same `instanceId`, passed as `&inst=` and kept in the socket tag), frames from one peer go to the other.
- Presence frame for peer rooms: `{"t":"presence","peers":n}`.
- Rooms with a `peer` socket reject `desktop`/`phone` and vice versa.
- Vitest cases in `relay/test/` for forwarding, cap, replacement, mixing roles, revoke.

Relay traffic: terminal output is batched (≤ 1 frame / 16 ms per pane, larger when busy) and only flows for attached panes; idle links hibernate (ping auto-response).

## Transport selection (`link.rs`)

- Relay link per peer is always up while Connect is enabled (cheap, hibernating DO).
- If the peer is seen on mDNS (or its last LAN address answers), open LAN, handshake, then **move** traffic: send `switch` on both, resubscribe on the new transport, keep the relay as standby. LAN drop → fall back to relay within one ping (≤ 5 s on LAN, 30 s relay), resubscribe.
- Resubscribe is idempotent: mirror re-sent in full, terminals re-snapshot, chats re-snapshot. No message replay buffer needed.
- Link exposes `state` (offline / relay / LAN), RTT (ping every 5 s on LAN, 30 s relay), and bytes/sec for the UI.

## Wire protocol (inside encryption) — `docs/connect/protocol.md`

JSON, `t` = type, requests carry `id`, replies echo it (`ack {id, ok, error?}`). Terminal bytes are base64 in JSON on
the relay (text frames); LAN uses binary frames with the same encrypted payloads.

| Area | Messages |
|---|---|
| Session | `hello {instance, name, version, proto: 1, caps[]}`, `bye`, `switch`, `presence {active: bool}` (user active at that machine, for notification dedupe) |
| Mirror | `mirror.sub` → `mirror {workspaces[], panes[], providers[], defaultProvider, theme?}`, then `pane.status {pane}` / `mirror` on structural change. `Pane` = `MobilePane` + `kind: "terminal"\|"chat"`, `cwd`, `branch`, `cols`, `rows`, `viewedBy?` |
| Terminal | `term.attach {paneId, cols, rows}` → `term.snapshot {paneId, data, cols, rows}` then `term.out {paneId, seq, data}`; `term.in {paneId, data}`, `term.resize {paneId, cols, rows}`, `term.detach {paneId}`, `term.exit {paneId, code}`, `term.reclaim {paneId}` (host took the size back) |
| Chat | `chat.attach {paneId}` → `chat.snapshot {paneId, state}` then `chat.ev {paneId, seq, line}` (raw `Ev` JSON line); `chat.cmd {paneId, cmd}` (a `Cmd`), `chat.detach` |
| Panes | `pane.new {workspaceId, provider?, prompt?, worktree?, model?, effort?}` → `ack {paneId}`, `pane.close`, `pane.rename`, `approve` / `decline` / `interrupt` / `choose` (same as mobile) |
| Agents | `tool.call {id, name, args, caller}` → `tool.result {id, content, isError}` |

## Features

### 1. Pairing & settings
- **Settings → Connect**: enable toggle, this instance's name, Discoverable on LAN, relay URL (default `https://winfer.danehub.com`), **Nearby** (unpaired instances on the LAN with a Pair button), **Paired** list (name, online dot, LAN/Relay, RTT, last seen, Notifications toggle, Rename, Revoke), **Pair with link** (show/copy link, paste link).
- Pair dialog on both sides with the 6-digit code, Confirm/Cancel, and clear failure states (code mismatch, timeout, cancelled on the other side).
- Command palette: "Connect: Pair a machine…", "Connect: Open settings".

### 2. Sidebar "Connected" section
- One collapsible group per paired instance: name, status dot (LAN / Relay / Offline), RTT on hover.
- Under it, its workspaces → sessions with live status, needs-input badges, request text on hover — same row component as local sessions, marked with the machine.
- Click → open as a tile in the current workspace (or focus it if already open). Context menu: Open, Open in new workspace, Approve/Decline, Interrupt, Rename, Close on ‹machine›. "+" on the group → provider menu → new session on that machine (`pane.new`), optionally with worktree, then opens it.
- Offline peers stay listed greyed out with their last known sessions.

### 3. Remote terminal tiles
- `Pane` gets `#[serde(default)] remote: Option<RemoteRef { peer_id, pane_id, peer_name }>` so layouts with remote tiles persist; on restore they reattach when the peer is online, else show "‹machine› offline — reconnecting".
- `RemoteSession`: `term.attach` with the tile's size → replay snapshot into the alacritty grid → live `term.out`. Keys, paste, mouse reports → `term.in`. Selection, scrollback search, links, copy work locally on the grid.
- **Size (remote takes over):** attaching or focusing/typing in the remote tile sends `term.resize`; the host resizes the PTY and its own tile shows the grid at that size with a bar "Viewed on ‹machine› · Take back"; clicking/typing in the host tile reclaims (`term.reclaim` to the viewer, whose tile then shows "Sized by ‹host›" until focused again).
- Header shows the machine name chip; closing the tile detaches (the pane keeps running there); "Close on ‹machine›" kills it.
- Host tap: native `terminal.rs` already receives PTY bytes in a `Channel` closure; add a fan-out hook there (native-only) that forwards to attached peers, batched per 16 ms with backpressure (drop to snapshot-resync if a peer falls > 4 MiB behind).

### 4. Remote chat panes (full native chat view)
- Host sends `chat.snapshot` = the pane's full `CodeState` (add `Serialize`/`Deserialize` to `CodeState` and its items in `code/state.rs`; skip transient UI-only fields), then forwards each raw `Ev` JSON line the host's `CodeModel` reads.
- Client runs a `CodeModel` with a wire transport: snapshot → state, `chat.ev` lines through the same parser, `Cmd`s (send, steer, approve, answer, model/effort/mode, interrupt, compact, undo, stop task) → `chat.cmd`, which the host writes to the process exactly like its own composer does.
- Attachments (pasted images) travel inline in `Cmd::Send` (size-capped, 10 MiB); file links in the transcript open on the **remote** machine's paths read-only via a `file.read` request (viewer shows "on ‹machine›").
- Drafts are per machine (not synced).

### 5. Cross-machine agents (MCP)
Native-only, no change to the shared `src-tauri/src/mcp.rs` schema: Connect adds its tools in the native answer to `__plugin_tools` (`ui/agent_api.rs:123`), and pane references get a remote form.
- New tools `list_machines` (paired instances, online state, transport), `list_remote_panes {machine}` (panes numbered `‹machine›:‹n›`) and `spawn_remote_agent {machine, provider, prompt?, worktree?}`.
- Existing `read_pane` / `send_to_pane` already take `pane` as integer **or string**, so `"studio:3"` works without a schema change: `agent_api.rs` routes it via `tool.call`; the host executes it with its own `agent_api` code and returns the result. `caller` (machine + pane) is shown in the host's activity log.
- Timeouts: 30 s per call, clear errors ("‹machine› is offline").

### 6. Notifications
- Each instance notifies for its peers' panes on transitions to done / needs-input / error (native notification with Approve / Decline / Reply routed back to the host), per-peer toggle.
- Dedupe: no notification if the user was active at the host machine in the last 2 min (`presence {active}`), if the pane's tile is focused locally, or if the host itself is notifying (only one machine — the one with the most recent input — notifies).
- Throttle 1 per pane per 5 s (same as mobile push).

## Phases

| # | Phase | Deliverable | Test |
|---|---|---|---|
| 0 | Relay `peer` role | `relay/src/index.ts` + vitest; deploy | `pnpm test` in `relay/`, `wrangler deploy` |
| 1 | `winfer-connect` core | identity, peers, handshake, frame, relay transport, `link.rs` over relay only | unit tests + test vectors (`docs/connect/test-vectors.json`); two instances via `WINFER_NATIVE_IDENTIFIER` |
| 2 | Pairing UX | Settings → Connect, pair-link pairing over relay, revoke | manual 2-instance pairing, revoke both sides |
| 3 | LAN | listener, mDNS, LAN SAS pairing, transport switching | kill Wi-Fi / block port mid-session; failover ≤ 30 s |
| 4 | Mirror + sidebar + notifications | host mirror, "Connected" section, pane.new/approve/etc., notifications | statuses match within 1 s on LAN |
| 5 | Remote terminals | `Pane.remote`, `RemoteSession`, size takeover, persistence | vim/htop/Claude Code TUI remotely; restart with remote tiles |
| 6 | Remote chat | `CodeState` serde, wire `CodeModel` | long session (> 2 MiB) opens complete; approvals/questions from remote |
| 7 | Cross-machine agents | `list_machines`, routed tools | agent on A spawns and drives an agent on B end to end via `mcp-stdio` |
| 8 | Polish & docs | RTT/bandwidth display, `docs/connect/protocol.md`, native README checklist | `cargo test -p winfer-connect -p winfer-app`, clippy |

Phases 0–2 give a working encrypted link; 4 is the first user-visible value; 5–7 are independent after 4.

---

# Headless winfer (`winferd`)

A winfer without a window, installed on servers (or a Mac mini), that runs panes, agents, winfer code chats,
worktrees, review, a headless browser, usage tracking and the phone bridge. You drive it entirely through Winfer
Connect from a desktop (and from the phone). To a desktop it is just another paired machine, marked as a server.

## Decisions (from Q&A)

| Topic | Decision |
|---|---|
| Platforms | **Linux x86_64 + arm64** (install script + systemd), **macOS** (launchd), **Docker image** (multi-arch). No Windows. |
| Pairing | Desktop **"Add server…"** shows a one-line install+pair command to run over SSH; the server prints the 6-digit code in that SSH session, you confirm on the desktop. LAN discovery also works for servers on your LAN. |
| Agent auth | **Push from desktop**: copy selected winfer accounts' credentials to the server over the encrypted link. |
| Server scope | Connect host for terminals, agents and chats, plus **worktrees + review**, **phone pairs directly**, **headless browser**, **usage/limits**. |

## What already works without GPUI

- `winfer-backend` (the shared `src-tauri/src` modules) and the `tauri` shim crates have no GPUI dependency: PTYs, hooks, MCP server, `winfer code`/harness, accounts, usage, worktrees (`worktree.rs`), review backend (`review.rs`), mobile bridge, **browser** (`browser/`: already one headless Chromium driven over CDP with screencast).
- In `winfer-app`: `model/` (workspaces, panes, layout, snapshot JSON), `terminal.rs` (alacritty grid), `request.rs`, `code/state.rs`, `code/protocol.rs` are GPUI-free.
- GPUI-coupled logic that headless needs: `code/session.rs` (28 `cx` refs), `status.rs` (14), `ui/agent_api.rs` (19), `ui/mobile.rs` (20), and the non-view parts of `ui/review.rs`, `ui/worktrees.rs`, `ui/usage.rs`, `ui/browser.rs`.
- GPUI can't run on a display-less Linux server, so `winferd` must not link `gpui-kit` or `wry`.

## Architecture

```
            winfer-app (GPUI views)            winferd (no UI)
                     │                              │
                     └──────────┬───────────────────┘
                          winfer-core          ← new: GPUI-free engine extracted from winfer-app
                     ┌──────────┼──────────────┐
              winfer-connect  winfer-backend   tauri shim crates
```

### New crate `native/crates/winfer-core` (extraction, the big piece)

An `Engine` that owns workspaces, panes, sessions and their status, and exposes commands plus an event stream.
`winfer-app` keeps GPUI entities that hold the engine and re-render on its events. `winferd` runs the engine with no views.

- Move as-is: `model/`, `terminal.rs`, `request.rs`, `code/{state,protocol,names,bg_note,toolbody,viz}.rs`.
- Split into core + GPUI wrapper: `code/session.rs` (process pump, `Ev` parsing and `Cmd` writing go to core; `Context` notify becomes an engine event), `status.rs`, `ui/agent_api.rs` (tool logic to core, `ask_user` dialog stays in UI, and headless forwards it to a connected desktop or phone), `ui/mobile.rs` (host logic to core), and the data/actions parts of review, worktrees, usage and browser (tabs, screencast, input).
- Done incrementally, one module per PR, `cargo test -p winfer-app` green after each, with no behavior change in the desktop app.
- Connect's host side (`ui/connect/host.rs` in the Connect plan) is written against `winfer-core` from the start, so desktop and server share it.

### New crate/binary `native/crates/winferd`

- `winferd` (daemon) with subcommands:
  - `run`: foreground, used by systemd, launchd and Docker.
  - `pair <link>`: join a pairing started on the desktop, print the 6-digit code.
  - `status`: peers, transport, panes, versions.
  - `unpair <name>`, `update`, `logs`.
  - The agent-facing subcommands the desktop binary has: `mcp-stdio`, `mcp-filter`, `hook`, `statusline`, `code`. Agents on the server call `winferd hook …` etc.
- **Data dir:** `$XDG_DATA_HOME/winferd` (Linux), `~/Library/Application Support/dev.winfer.server` (macOS), `/data` (Docker). Same snapshot JSON as native.
- **Secrets:** Linux servers usually have no Secret Service/D-Bus. `winfer-connect`'s peer store and the mobile devices get a `SecretStore` abstraction: OS keychain on the desktop and macOS, and on Linux/Docker a `0600` file `secrets.json` in the data dir, encrypted with a key in `secrets.key` (also `0600`). It can optionally be bound to `WINFERD_SECRET` from the environment for Docker secrets. Today `mobile/devices.rs` calls `keyring` directly, so headless needs a small, additive shim in the shared backend (env-selected file backend), or a native-only override in the `winfer-backend` crate. Prefer the override to keep `src-tauri` unchanged.
- **Logs:** to stderr (journald / `docker logs`) plus the rotating `diagnostics` log. `winferd logs -f` tails it.

## Desktop side

- **Settings → Connect → Add server…** wizard:
  1. Pick a platform (Linux / macOS / Docker). The desktop creates a one-time pair link (the off-LAN pairing flow from the Connect plan).
  2. Shows a command to copy:
     - Linux/macOS: `curl -fsSL https://winfer.danehub.com/install.sh | sh -s -- --pair '<link>'`
     - Docker: `docker run -d --name winferd --restart unless-stopped -v winferd:/data -e WINFERD_PAIR='<link>' ghcr.io/<owner>/winferd`
  3. The server installs (`~/.local/bin/winferd`), registers the service (systemd user unit plus a `loginctl enable-linger` hint, or a launchd agent), starts, joins the one-time relay room and prints the **6-digit code** to the SSH session or `docker logs`. You confirm on the desktop. The link's one-time key already authenticates both sides, so the code is a final check against a wrong paste.
  4. Next steps on the desktop: **agents** (detects installed CLIs on the server and offers to install them in a remote shell tile: `npm i -g @anthropic-ai/claude-code`, Codex…), **accounts** (push credentials, below), **first workspace**.
- **Servers in the sidebar:** the same "Connected" group with a server icon, uptime, version, and "Update available".
- **New workspace on ‹server›:** pick a folder with a remote file browser (`fs.list`), or **clone a git repo** (URL + branch; uses the server's git credentials, with an optional pushed deploy key). Servers have no desktop layout: their workspaces are folders with sessions, and you open them as tiles locally.
- **Server health** in the group menu: CPU, memory, disk, load (`sys.stats`, every 10 s while the menu is open).

## Pushing agent accounts

- In Settings → Accounts, each account gets **Push to ‹server›**; the wizard also offers it.
- The desktop reads the credentials:
  - Claude: macOS Keychain entry named after the config dir, or `.credentials.json` on Linux.
  - Codex: `auth.json` in `CODEX_HOME`.
  - Gemini/others: their files where `providers.rs` knows them.
- They are sent as `account.push {provider, accountId, label, files{}}`. The server writes them into its own account config dir (`CLAUDE_CONFIG_DIR` / `CODEX_HOME` under its data dir, `0600`) and registers the account in its `accounts.json`, so failover order works on the server too.
- **Refresh-token rotation risk:** Claude/Codex OAuth rotate refresh tokens, so two machines refreshing one copy can sign each other out. Pushed accounts are **linked**: whichever side refreshes sends the new credentials to the other (`account.sync`, watching the creds file/keychain entry). If one side still ends up signed out, the desktop shows "Signed out on ‹server› · Push again". Alternative per account: **Move** (desktop forgets that sign-in).
- Pushes are logged on both sides. Unpairing offers to wipe pushed accounts from the server.

## Server features

| Feature | How |
|---|---|
| Terminals, agents, chats | Connect host on `winfer-core`, same as desktop. Panes and sessions survive daemon restarts (resume as the desktop does on relaunch). |
| Worktrees + review | Worktree per pane as on desktop. Review inbox items stream as `review.*` messages (diff stats, summary, tests, conflicts). Diffs are fetched on demand. Merge, discard, send back and the merge queue run on the server. Shown in the desktop's Review sidebar under the server. |
| Headless browser | The server runs its own Chromium (`browser/` backend, auto-download or system Chromium; Docker image ships it). The desktop browser panel attaches to a server workspace's browser: `browser.attach` → screencast JPEG frames (`browser.frame`, adaptive quality/fps, LAN-friendly, lower on the relay), input as `browser.input`, tabs and address bar via `browser.tabs` / `browser.nav`. Agents on the server use it through the existing CDP proxy. |
| Usage / limits | The server runs its own usage scanners and limit tracking for its accounts. `usage.get` returns the same shape as the phone's Usage screen. Desktop Usage gets a machine switcher. `usage_status` / `wait_for_usage_reset` work for server agents. Limit handoff ("Continue in …") is offered across machines too. |
| Phone pairs directly | The server runs the mobile bridge (shared `mobile/` backend + host logic from `winfer-core`). Pair from the desktop ("Pair phone with ‹server›" shows the QR, the link is generated on the server and forwarded) or with `winferd mobile pair` (QR drawn in the terminal). Web Push goes out from the server. It has no "at the desk" presence, so it always pushes unless a desktop reports you're active. |
| `ask_user` / `notify_user` | Server agents' questions go to connected desktops (dialog) and phones (push). The first answer wins and the rest are dismissed. |
| Cross-machine agents | Server agents get the same Connect MCP tools. Full trust applies both ways. |

## Distribution & updates

- **CI** (extend `.github/workflows/release.yml`): build `winferd` for `x86_64/aarch64-unknown-linux-gnu` (built on an old-glibc image or `cargo-zigbuild` for glibc 2.31+) and `aarch64/x86_64-apple-darwin`. Tarballs and checksums go to GitHub Releases. Multi-arch Docker image `ghcr.io/<owner>/winferd` (Debian slim + Node + git + Chromium, agents optional via `WINFERD_AGENTS=claude,codex`).
- **`install.sh`** is served by the relay worker's static assets at `/install.sh`. It detects OS and arch, verifies the checksum, installs, writes the service unit and runs `pair`.
- **Updates:** `winferd update` (same channel as the app updater). The desktop shows "Update available" per server, with a button that sends `server.update`. The server downloads, swaps the binary and restarts its service, and the link reconnects. Docker: pull a new image (the button shows the command).
- **Version skew:** the server reports `proto`/`caps` in `hello`; the desktop hides what the server lacks and recommends updating.

## Phases (after Connect phases 0–4)

| # | Phase | Deliverable | Test |
|---|---|---|---|
| H0 | Spike | `winfer-backend` + shim build and run on Linux with no X/Wayland (Docker), keyring replacement works | `cargo build -p winfer-backend --target x86_64-unknown-linux-gnu` in CI, smoke test in a container |
| H1 | `winfer-core` extraction | model, terminal, code session, status, agent API, mobile host moved; app wraps them | `cargo test -p winfer-app -p winfer-core`, desktop unchanged (manual smoke) |
| H2 | `winferd` MVP | daemon, data dir, file secret store, subcommands, Connect host, pairing via link, systemd/launchd units | desktop pairs with a Linux VM, opens a shell tile, runs `claude` |
| H3 | Install & release | `install.sh`, release CI, Docker image, "Add server…" wizard, update flow | fresh Ubuntu 22.04 arm64 + x86_64 VM, macOS, Docker: wizard end to end |
| H4 | Accounts push | push/link/move, sync on refresh, wipe on unpair | Claude + Codex sessions on the server for 24 h with both sides refreshing |
| H5 | Workspaces on server | remote folder browser, git clone, server health | clone, worktree pane, restart daemon, sessions resume |
| H6 | Review + worktrees | `review.*`, merge queue on the server | agent finishes on the server → review on desktop → merge |
| H7 | Usage + phone | usage/limits per machine, mobile bridge on the server | phone paired to the server gets push with desktop closed |
| H8 | Headless browser | Chromium on the server, screencast panel over Connect | agent uses browser tools on the server, user watches/clicks from desktop |

## Headless risks / notes

- **H1 is the long pole:** extraction touches the app's central code. Mitigation: one module per PR, desktop behavior unchanged, tests first.
- **Security:** a server is reachable from the internet only through the relay (outbound WebSocket). The LAN listener is off by default on `winferd` (`--lan` to enable) and never needs inbound ports, so no firewall changes. Full trust means a compromised desktop controls its servers. Documented, and revoke works from either side (`winferd unpair`).
- **Docker** needs `--init`/tini for PTY child reaping. Chromium in Docker needs `--no-sandbox` or a seccomp profile; the image handles it.
- **Relay bandwidth** for browser screencasts off-LAN: adaptive fps/quality, pause when the panel isn't visible.
- **One user per daemon**: multi-user servers run one `winferd` per Unix user (systemd user units).

---

## Risks / notes (Connect)

- **Relay cost & latency** for terminal streaming off-LAN: batching + attach-only streaming; RTT shown so slow links are obvious.
- **Version skew** between instances: `hello.proto` + `caps[]`; unknown messages answered with `error`, features hidden when a cap is missing.
- **`CodeState` serde** touches a central type; keep it additive (`#[serde(default)]`) and covered by a round-trip test.
- **Keychain prompts**: secrets only read when `has_peers` is set (same trick as mobile).
- **Same machine twice** (dev): distinct `instanceId` per identifier; LAN discovery ignores its own id.
- Tauri app: no Connect UI; a Tauri instance never appears on mDNS and can't pair (documented).
- Write Connect's host side against `winfer-core` (headless H1) where it already exists. If Connect phases run first, keep host logic GPUI-free so H1 only has to move it.
