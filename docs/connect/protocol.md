# Winfer Connect protocol (v1)

winfer ⇄ winfer, mutual and end-to-end encrypted. Native app only (`native/crates/winfer-connect`, app glue in
`native/crates/winfer-app/src/{connect,connect_term}.rs` and `ui/{connect_ui,settings_connect}.rs`).
Plan and decisions: `docs/plans/winfer-connect.md`.

## Identity and storage

- `connect.json` in the data dir: `{enabled, discoverable, relayUrl?, instanceId, name?, hasPeers}`. `instanceId` is
  16 random bytes, base64url (22 chars), made on first start.
- Paired instances in the secret store (OS keychain, service `<identifier>.connect`, account `peers`; a JSON file when
  `WINFER_VAULT_FILE` is set): `[{peerId, name, kind: desktop|server, pairKey, createdAt, lastSeen?, notify,
  lastAddr?, relayUrl?}]`. `relayUrl` is the relay the pairing met on, when it isn't this instance's own.

## Frames

Every transport carries WebSocket **text** frames:

- `h.<base64url(JSON)>` — handshake and pairing messages, in the clear (no secrets in them).
- `<base64url(AES-256-GCM ciphertext+tag)>` — everything after the handshake. Plaintext byte 0 is `1` when more chunks
  of the same message follow, `0` on its last; the rest is a chunk (≤ 512 KiB) of a UTF-8 JSON message (≤ 64 MiB).

Nonces are 4 zero bytes + a 64-bit big-endian counter per direction, starting at 0 for each handshake: a replayed,
dropped or reordered frame fails to open, and the side that sees that starts over.

## Keys

HKDF-SHA256, salt `"winfer-connect-v1"`, IKM = `pairKey` (32 bytes):

| info | length | use |
|---|---|---|
| `room` | 16 | relay room id (base64url) |
| `relay` | 32 | relay auth token (base64url) |
| `auth` | 32 | HMAC key of the per-connection handshake |

Pair links use the same with IKM = the link's one-time key and infos `pair-room`, `pair-relay`, `pair-auth`.

## Per-connection handshake

Fixed roles: the instance with the smaller `instanceId` (string order) is the initiator (I), the other the responder
(R). HMAC parts are length-prefixed (u32 BE).

```
R → I  hs0                                         optional: "start a handshake" (R (re)connected)
I → R  hs1 {v:1, i: I, n: Ni(16)}
R → I  hs2 {v:1, i: R, n: Nr(16), m: HMAC(auth, "resp", Ni, Nr, I, R)}
I → R  hs3 {m: HMAC(auth, "init", Ni, Nr, I, R)}
keys   i2r = HKDF(salt = Ni‖Nr, pairKey, "i2r"), r2i = HKDF(…, "r2i")
```

A new `hs1` always restarts the responder. The initiator retries `hs1` every 5 s while the other side is present and no
channel is up.

## Pairing

```
J → H  p1 {v:1, i, name, kind, e: X25519 public, n(16), m?: HMAC(linkAuth, "p1", i, e, n)}
H → J  p2 {v:1, i, name, kind, e, n, m?: HMAC(linkAuth, "p2", th1, i, e, n)}     th1 = SHA256(i, name, e, n of p1)
       th = SHA256("winfer-connect-pair-v1", p1 fields, p2 fields); ss = X25519; salt = linkKey(or empty) ‖ th
       code = u32(HKDF(salt, ss, "sas")[0..4]) mod 10^6, shown "123 456"
       pairKey = HKDF(salt, ss, "pair"); confirmKey = HKDF(salt, ss, "confirm")
both   p3 {m: HMAC(confirmKey, "p3", own id)}         after the user compares the codes
either px {reason}                                    cancel
```

- **Link pairing:** the host makes `winfer-connect:#p=<base64url(JSON {v:1, k: linkKey, i, n, u: relay URL, x:
  expiry ms})>` (10 min). Both join the one-time room derived from `k` as role `peer`; the joiner sends `p1` when the
  relay reports 2 peers. The host deletes the room afterwards. A server joining with `winferd pair` confirms by itself.
- **LAN pairing:** the joiner dials the host's LAN port and opens with `lan {want: "pair"}`; the host shows an incoming
  request. Only when discoverable and not already pairing (else `px`). 2 min limit.

## Relay (`relay/`)

`GET /r/<room>?role=peer&inst=<instanceId>&auth=<token>` (WebSocket). Peer rooms hold two instances; a reconnecting
instance replaces its own old socket (`4000`), a third instance gets `429`, a room is either a mobile room or a peer
room (`409` otherwise). Presence: `{"t":"presence","peers":n}`. Ping/pong, 1 MiB frames, `DELETE` (revoke, `4001`)
and the 30-day idle cleanup as for mobile rooms.

## LAN

Each instance listens on an OS-picked TCP port (WebSocket, no TLS: frames are already encrypted) and, when
discoverable, advertises `_winfer-connect._tcp.local.` with TXT `i` (instance id), `n` (name), `k` (kind), `v=1`.
A connection opens with `h.{"t":"lan","v":1,"i":<caller>,"want":"link"|"pair"}`. For a paired instance the smaller id
dials (or whoever isn't advertising); the link prefers the LAN path while its handshake is up, pings every 5 s and
drops it after 3 missed pongs. The relay stays connected as the fallback (pings every 30 s). Every switch of the path
carrying traffic is an `Up` for the layers above, which resubscribe.

## Messages (inside the channel)

JSON, `t` = type. Requests carry `id`; replies echo it. Link internals: `_ping`/`_pong {ts}`, `bye` (unpaired: the
other side forgets the pairing).

### Mirror (the mobile protocol)

A peer is a mobile device `connect:<peerId>` to the host's mobile bridge, so every request of
[docs/mobile/protocol.md](../mobile/protocol.md) works: `panes.get`, `thread.get`, `send`, `approve`, `decline`,
`interrupt`, `choose`, `config.get`, `models.get`, `pane.set`, `pane.new`, `pane.close`, `usage.get`, with the same
replies. A peer that sent `panes.get` gets the `panes`/`status` pushes the phones get. Panes that are themselves
other machines' (remote tiles) are left out. `connect.presence {active: true}` tells the other side the user is at this
machine (sent at most every 20 s while active), so it holds back its notifications for 2 min.

### Terminals and chats

| t | from | fields |
|---|---|---|
| `term.attach` | client | `paneId`, `cols`, `rows` — starts the pane if it never ran; the client takes over its size |
| `term.snapshot` | host | `paneId`, `data` (base64: the last 1 MiB of output, 8 MiB for chat panes, cut at a line), `chat`, `provider` |
| `term.out` | host | `paneId`, `data` (base64), in order after the snapshot |
| `term.in` | client | `paneId`, `data` (base64) — written to the process as is |
| `term.resize` | client | `paneId`, `cols`, `rows` — the client sizes the pane until the host takes it back |
| `term.reclaim` | host | `paneId` — the host's tile took the size back |
| `term.detach` | client | `paneId` |
| `term.exit` | host | `paneId`, `code`, `gone?` |

A winfer code chat pane is a `winfer code --serve` process: its output is JSON lines, so the client runs the same
chat model on the replayed and live bytes and writes its commands as `term.in`.

### Agent tools

`tool.call {id, name, args, caller}` → `tool.result {id, ok, text}`. Hosts run `read_pane`, `send_to_pane` (`args.pane`
= the host's pane id) and `spawn_agent` (`args.workspace` = a workspace name, default the active one) exactly as for
their own agents. Clients time out after 30 s.

### Servers and shared sign-ins

| t | from | fields |
|---|---|---|
| `server.update` | client | — → `ack {ok, version?, error?}`; the server updates to the newest release and restarts |
| `fs.list` | client | `path` (`~` for home) → `fs {path, parent?, git, dirs: [{name, git}], truncated, error?}` |
| `workspace.new` | client | `path`, `name?` → `ack {ok, workspaceId}` (an existing workspace for that folder is reused) |
| `git.clone` | client | `url`, `branch?`, `parent` → `ack {ok, workspaceId, path}` once cloned (into `parent/<repo>`) |
| `account.push` | either | `provider` (`claude`/`codex`), `accountId` (the sender's), `label`, `parts` → `ack {ok, accountId (the receiver's), label}` |
| `account.sync` | either | `accountId` (the receiver's), `parts`: a linked sign-in changed (refresh) |

### Usage on a server

A server polls its own sign-ins' limits (as the desktop polls its own) and reads its agents' logs for token history.
`usage.get {range?, refresh?, raw?}` → `usage {providers, now, tokens}`, the same answer the desktop gives its phone;
with `raw: true` also `raw: {snapshots, samples, accounts, rows}`, which a paired desktop's Usage page shows when that
server is picked ("This Mac ▾" next to the range).

### Phones paired with a server

A phone can pair with a server directly (`winferd mobile pair` prints the QR code, or Settings → Mobile → Your servers
on a paired desktop sends `mobile.pair` → `mobile.pairing {deviceId, link, qrSvg, expiresAt}`). The phone speaks the
mobile protocol (docs/mobile/protocol.md) to the server, which answers it as it answers paired desktops; pushes go out
on every finish and question (no one sits at a server). Its phones' keys live in `vault.json` (0600) in its data folder.
A chat pane's `thread` and `request` come from the server's copy of the chat's events, as a desktop gives them; `approve`,
`decline`, `choose` and `send` answer what it waits on. Asking for an agent that isn't running starts it.

### Browser on a server

Each server workspace has the headless Chromium its agents drive (Debian's `chromium` in the image, else
`WINFER_CHROME` or the downloaded headless shell; run as root it gets `--no-sandbox`). A paired desktop's browser panel
shows it under the id `‹peer›|‹workspace›`: as a tab group after the workspace's own tabs when the workspace has a tile
of that server, and with the other groups in Overview. Frames are JPEG, each acknowledged before the next is sent, so
the frame rate follows the link (LAN or relay).

| t | from | fields |
|---|---|---|
| `browser.call` | desktop | `op`, `workspaceId` (the server's), op fields; with an `id`, answered by `ack {ok, value?, error?}`. `input` and `frameDone` come without one and aren't answered |
| `browser.frame` | server | `attachId`, `data` (JPEG, base64), to the desktop that attached |
| `browser.event` | server | `event` (a `browser://…` name), `payload` (with `workspace`, the workspace's name); to every paired desktop, `browser://console` only to the one watching that workspace |

Ops: `list` (every open workspace browser: `[{workspaceId, workspace, tabs}]`), `status`, `install`, `open` (tabs,
starting the browser), `newTab {url}` (tab id), `closeTab`, `navigate {tab, url}`, `history {tab, delta}`,
`attach {tab, viewport, attachId}`, `frameDone {attachId}`, `detach {attachId}`, `resize {tab, viewport}`,
`input {tab, method, params}` (applied in order), `certs {allow}`, `consoleClear`, `pickStart`/`pickCancel`/
`pickHover {tab, x, y}`, `selectProbe {tab, x, y}`, `selectPick {tab, index}`, `selection`, `imageAt {tab, x, y,
withData}`, `pasteImage {tab, data, mime}`. A desktop asks for `list` when a server's link comes up; its tabs leave the
panel when it goes down.

### Review on a server

When an agent on a server finishes work (its worktree has changes), the server makes a review of it: diff stats, a
summary written by an agent CLI installed there (`WINFERD_REVIEW_SUMMARY=codex|claude|off`), its test command and
conflicts with the base. Paired desktops get the list whenever it changes and after `panes.get`; merges run in the
server's merge queue, in the agent's worktree there.

| t | from | fields |
|---|---|---|
| `reviews` | server | `items: [{id, paneId, workspaceId, provider, title, prompt?, finishedAt, branch?, base?, dir, stats?, lastOutput, summary: {state, text?, error?}, tests: {state, run?: {command, ok, ms}, error?}, testCommand?, conflict?: {base, text}}]`, `queue: [{reviewId, branch, base, state (waiting/syncing/testing/merging/merged/conflict/failed), detail?, files}]` |
| `review.merge` | client | `reviewId`, `tests` (run them first) → `ack`; queued, progress comes in `reviews` |
| `review.tests` | client | `reviewId` → `ack`; the result comes in `reviews` |
| `review.send` | client | `reviewId`, `text`: typed into the agent's pane; the review closes (a new one comes when it finishes again) |
| `review.resolve` | client | `reviewId`: asks the agent to merge its base in and resolve the queue's conflict |
| `review.discard` | client | `reviewId`: closes the pane, deletes the worktree and its branch |
| `review.dismiss` / `review.unqueue` / `review.clear` | client | `reviewId` (not for `clear`): drop the review / take it out of the queue / clear finished queue items |

A server's `panes` mirror carries `server: {version, latest, update, docker, os, arch, stats: {cpus, load, memTotal, memUsed, diskTotal, diskUsed, uptime}}`. Sign-ins are never logged
(`WINFER_CONNECT_TRACE` leaves `parts` out).
