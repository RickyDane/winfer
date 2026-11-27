# winfer mobile protocol (v1)

Phone PWA ⇄ Cloudflare relay ⇄ winfer desktop. Relay forwards opaque ciphertext; only desktop + phone hold the key.

One Cloudflare Worker (`relay/`) serves both:
- the PWA static assets (built from `mobile/`) at `/`
- the WebSocket relay at `/r/<roomId>`

Default host: `https://winfer.danehub.com` (also reachable at `https://winfer-mobile.t6mpknwd4z.workers.dev`) (desktop setting `mobile.relayUrl`).

## Pairing

Desktop generates per phone:
- `roomId` — 16 random bytes, base64url (no padding)
- `key` — 32 random bytes, base64url
- `deviceId` — 8 random bytes, base64url

Pair link (QR): `https://<host>/#p=<base64url(JSON)>` with JSON
`{"v":1,"r":roomId,"k":key,"d":deviceId,"n":"<desktop name>"}`.
The fragment never reaches the server. Desktop stores `{deviceId, roomId, key, name, createdAt, lastSeen, pushSubscription?}` in the OS keychain (one JSON blob, account `mobile-devices`). Pair links expire: desktop accepts the first `hello` within 10 min of creation, after which the device is marked `paired`; unpaired entries older than 10 min are dropped.

## Key derivation

HKDF-SHA256, salt = UTF-8 `"winfer-mobile-v1"`, IKM = `key` (32 bytes):
- `authToken` = HKDF(info=`"auth"`, 32 bytes) → base64url
- `encKey`    = HKDF(info=`"enc"`, 32 bytes) → AES-256-GCM key

## Relay

`GET /r/<roomId>?role=desktop|phone&auth=<authToken>` with `Upgrade: websocket`.

- One Durable Object per `roomId` (`idFromName(roomId)`).
- First successful connection stores `sha256(authToken)` in DO storage; later connections must match, else `403`.
- At most 1 desktop socket (new one replaces old) and 8 phone sockets per room.
- Frames are WebSocket **text** messages. Max 1 MiB.
  - Ciphertext frame: `base64url(nonce[12] || AES-GCM ciphertext+tag)` — desktop → all phones; phone → desktop. Never sent back to sender.
  - Control frame (relay → client only, plaintext JSON, starts with `{`): `{"t":"presence","desktop":bool,"phones":n}` sent on connect and whenever presence changes.
- Keepalive: clients send the literal text `ping` every 30 s; relay answers `pong` (handled without waking the DO). Neither is forwarded.
- Clients ignore ciphertext they cannot decrypt.
- `roomId` must be exactly 22 base64url chars, `authToken` exactly 43, else `400`.
- `DELETE /r/<roomId>?auth=<authToken>` wipes the room (used on revoke).
- Close codes: `4000` replaced by a newer desktop socket, `4001` room revoked (phone should drop the pairing; desktop should not reconnect).
- Idle rooms with no sockets are deleted after 30 days (DO alarm).

## Messages (plaintext inside encryption)

JSON object, `t` = type. Requests carry `id` (string); replies echo it.

### phone → desktop

| t | fields | reply |
|---|---|---|
| `hello` | `device` (deviceId), `ua` | `welcome` + `panes` |
| `panes.get` | | `panes` |
| `thread.get` | `paneId`, `limit?` (default 200) | `thread` |
| `send` | `paneId`, `text` | `ack` |
| `approve` / `decline` / `interrupt` | `paneId` | `ack` |
| `choose` | `paneId`, `option` (an option's `n`), `text?` (answer for a "Type something" row) | `ack` |
| `config.get` | `paneId` | `config` (`models: {id, name, detail?, efforts[]}[]`; empty unless the pane is a winfer code chat) |
| `models.get` | `provider` (an id from `providers`) | `models` (`provider`, `models: {id, name, detail?, efforts[]}[]`, `defaultModel?`, `defaultEffort?`, `defaultEfforts?`) |
| `pane.set` | `paneId`, `model?` (a model `id`), `effort?` (one of that model's `efforts`) | `ack` |
| `pane.new` | `workspaceId`, `provider?` (an id from `providers`; default: the desktop's default agent), `text?` (first message), `model?`, `effort?` | `ack` + `paneId` |
| `push.subscribe` | `subscription` (PushSubscription JSON) | `ack` |
| `push.unsubscribe` | | `ack` |

### desktop → phone

| t | fields |
|---|---|
| `welcome` | `id`, `desktop` (name), `version`, `vapidPublicKey` (base64url, uncompressed P-256) |
| `panes` | `id?`, `panes: Pane[]` (every workspace's, in desktop order), `workspaces: {id, name, active}[]`, `providers: {id, name}[]` (installed agents), `defaultProvider`, `theme?: {id, mode, ui}` (the desktop's themes.json entry; the phone wears its colors), `compactTools?: boolean` (desktop's compact tool calls setting, default true) — the whole mirror; sent on `panes.get` and whenever panes come, go or move, or workspaces or agents change |
| `status` | `pane: Pane`, `silent?` (pushed whenever a pane's status/message/request changes; `silent: true` for other changes such as a new title, which never push a notification) |
| `thread` | `id?` (absent for live updates), `paneId`, `messages: Message[]`, `source` (`claude`/`codex`/`opencode`/`winfer`/`screen`) |
| `ack` | `id`, `ok`, `error?` |
| `error` | `id?`, `error` |

```ts
type AgentStatus = "idle" | "working" | "needs-input" | "done" | "error";
interface Pane {
  id: string; title: string; workspace: string; workspaceId: string; archived: boolean;
  provider: string | null;
  status: AgentStatus; message: string | null; updatedAt: number; // ms epoch
  request: {
    kind: "approval" | "question"; text: string;
    // Numbered choices under the question (Claude Code / winfer code pickers). Typing `n` into
    // the pane picks one; "Type something." rows then take `text` + Enter.
    options?: { n: number; label: string; detail?: string }[];
  } | null;
  reply: string | null; // last assistant reply when done
  config?: { model: string; label: string; effort: string; efforts: string[]; mode?: string; yolo?: boolean; cwd?: string | null; branch?: string | null } | null; // winfer code chats only
}
interface Message {
  id: string; role: "user" | "assistant" | "tool" | "notice"; // also "bg", "thinking" (see mobile/src/protocol.ts)
  text: string;           // markdown for assistant, tool = one-line summary
  tool?: string;          // tool name for role=tool
  ts: number | null;      // ms epoch
  state?: "running" | "done" | "failed"; // tools: from the session (Claude: tool_result /
                          // <task-notification>; Codex: *_call_output; winfer code: saved state)
  background?: boolean;   // tools: a background shell or subagent, which outlives its call
  kind?: "compact" | "microcompact" | "account"; // notice (winfer code): compaction, micro-compaction, account change at a usage limit
  report?: Report;        // tools: a `visualize` call's validated report (see mobile/src/protocol.ts); the phone draws it as charts/tables instead of a tool row
}
```

Phone handles frames strictly in arrival order and re-requests `panes.get` every 20 s while visible. A `thread.get` for a live winfer code pane leases direct in-memory transcript updates for 60 s, coalesced over 50 ms. The phone renews every 15 s and reloads on status changes, reconnection and foregrounding. Other providers are polled every second while working or running background tools, otherwise every 15 s. Sends render optimistically, survive stale snapshots until their transcript echo arrives, and roll back on delivery failure.

## Side panel

The phone's Panel tab reaches the desktop's side panel per workspace: its browser, the device mirror (Android, and a
Mac's iOS Simulators), terminals and
files (native desktop: `ui/mobile_panel.rs`). Requests answer with `ack {ok, value?, error?}`; requests sent without
an `id` (input, frame acks) aren't answered. Any request renews the phone's lease; a phone silent for 45 s loses its
streams (terminal output, frames).

| t | fields | value |
|---|---|---|
| `panel.get` | `workspaceId` | `{workspaceId, name, root, terminals: [{id, title, provider, side, running}]}`: the side terminal `side-terminal-<workspaceId>` first, then the workspace's terminal panes (not chats) |
| `term.attach` / `term.in` / `term.resize` / `term.detach` | as in Winfer Connect (docs/connect/protocol.md, "Terminals and chats"); `paneId` may be a side terminal, which starts if it never ran | streamed: `term.snapshot` (the last 256 KiB, from a line), `term.out`, `term.exit`, `term.reclaim` |
| `browser.call` | `op`, `workspaceId`, op fields | per op |
| `android.call` | `op`, op fields, `workspaceId?` (ignored by the desktop; a phone paired with several machines routes by it) | per op |
| `files.call` | `op`, op fields | per op |
| `panel.frameDone` | `seq`, `feed?` (`browser` / `android`: which machine's frames it acknowledges) | — |

Browser ops: `state {retry?}` → `{workspaceId, ready, needsInstall, installing, error, tabs: [{id, url, title, favicon}], active, others}`
(starts the workspace's browser; `others`: `[{workspaceId, name, tabs, active}]`, the machine's other workspaces with tabs),
`tabs` → `{all: [{workspaceId, name, tabs, active}]}` (every workspace with tabs, starting no browser: the phone's
"All workspaces"), `install`, `select {tab}` (the desktop panel shows it too), `watch {tab, maxWidth}`,
`unwatch`, `input {tab, method, params}` (CDP `Input.dispatchMouseEvent` / `dispatchKeyEvent` / `insertText`, page CSS
pixels), `newTab {url?}` → tab id, `closeTab {tab}`, `navigate {tab, url}` (typed text becomes a URL or search),
`history {tab, delta}` (-1 back, 1 forward, 0 reload).

Device ops (`android.call`, frames as feed `android`, for iOS Simulators too): `state` → `{loaded, adb,
devices: [{serial, state, model, emulator}], avds, ios: {simctl, devices: [{udid, name, runtime, state, booted}]} | null,
current, starting, error, mirror: {serial, mode: scrcpy|stream|live|screenshots, error, showing, live, input?}}`
(`current`, `starting` and `mirror.serial` name a simulator by udid; `ios` is a Mac's, absent from older desktops;
`input: false` is a simulator that can only be watched), `watch {maxWidth}` (the mirror runs while a phone or the panel
shows it), `unwatch`, `select {serial}` (adb serial or udid), `startAvd {name}`, `startSim {udid}` (boots it),
`touch {kind: down|move|up, x, y}` (fractions of the screen; scrcpy and simulators play them live, else they become a tap
or swipe), `scroll {x, y, v}`, `key {code}` (Android keycodes; on a simulator home, power (locks), enter, delete, tab
and arrows work), `text {text}`.

Files ops (only paths in a workspace's folder or its agents' worktrees, or open in the viewer): `docs {workspaceId}` →
`{root, docs, groups, active}` (every file open in the viewer, as `groups: [{id, name, docs, here}]` by project — this
workspace's first, then the others and "Other files" — and `docs` all of them in that order), `list {workspaceId, dir?}` → `{root, dir, entries: [{name, dir,
size}]}`, `read {path}` → `{path, kind: text|markdown|image|binary|large, text? | mime + data (base64)?, size, modified}`
(text up to 400 KB, images up to 480 KB), `write {path, text, expected}` → `{modified}` (refused when the file changed
since `expected`), `comment {path, afterLine, body, expected}` (adds `<!-- @user: body -->` after the line), `open {path}`
(in the desktop's viewer, without switching its panel), `close {path}`.

Frames: one watch per phone (watching another replaces it). `panel.frame {feed: browser|android, seq, data (JPEG,
base64), width, height, workspaceId?, tab?}`: `width`/`height` are the page's viewport in CSS pixels (browser) or the
screen's pixels (Android), what taps map to. The next frame goes once the phone acks `seq` (or after 4 s); JPEG quality
follows how long acks take. Browser frames are screenshots of the tab's viewport (a still page repeats every 5 s);
`panel.error {feed, error}` when the tab can't be shown.

## Push

Desktop holds a VAPID P-256 keypair (keychain, account `mobile-vapid`) and sends Web Push (RFC 8291 aes128gcm, RFC 8292 VAPID, `TTL: 3600`, `Urgency: high`) directly to each device's subscription on transitions to `done`, `needs-input`, `error` — throttled to 1 per pane per 5 s. Only while the user is away from the desktop: no winfer window is focused, or one is but had no key, click or scroll for 60 min (the webview reports input via `mobile_activity`, at most every 20 s). VAPID `sub` = `mailto:noreply@winfer.dev`.

Payload JSON: `{"title":"<pane title>","body":"<message or status>","paneId":"…","status":"…"}`.
The service worker shows it and opens `/#/pane/<paneId>` on click. `404`/`410` from the push service → drop that subscription.
