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
| `pane.new` | `workspaceId`, `provider?` (an id from `providers`; default: the desktop's default agent), `text?` (first message) | `ack` + `paneId` |
| `push.subscribe` | `subscription` (PushSubscription JSON) | `ack` |
| `push.unsubscribe` | | `ack` |

### desktop → phone

| t | fields |
|---|---|
| `welcome` | `id`, `desktop` (name), `version`, `vapidPublicKey` (base64url, uncompressed P-256) |
| `panes` | `id?`, `panes: Pane[]` (every workspace's, in desktop order), `workspaces: {id, name, active}[]`, `providers: {id, name}[]` (installed agents), `defaultProvider` — the whole mirror; sent on `panes.get` and whenever panes come, go or move, or workspaces or agents change |
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
}
interface Message {
  id: string; role: "user" | "assistant" | "tool";
  text: string;           // markdown for assistant, tool = one-line summary
  tool?: string;          // tool name for role=tool
  ts: number | null;      // ms epoch
  state?: "running" | "done" | "failed"; // tools: from the session (Claude: tool_result /
                          // <task-notification>; Codex: *_call_output; winfer code: saved state)
  background?: boolean;   // tools: a background shell or subagent, which outlives its call
}
```

Phone handles frames strictly in arrival order and re-requests `panes.get` every 20 s while visible. A `thread.get` for a live winfer code pane leases direct in-memory transcript updates for 60 s, coalesced over 50 ms. The phone renews every 15 s and reloads on status changes, reconnection and foregrounding. Other providers are polled every second while working or running background tools, otherwise every 15 s. Sends render optimistically, survive stale snapshots until their transcript echo arrives, and roll back on delivery failure.

## Push

Desktop holds a VAPID P-256 keypair (keychain, account `mobile-vapid`) and sends Web Push (RFC 8291 aes128gcm, RFC 8292 VAPID, `TTL: 3600`, `Urgency: high`) directly to each device's subscription on transitions to `done`, `needs-input`, `error` — throttled to 1 per pane per 5 s. Only while the user is away from the desktop: no winfer window is focused, or one is but had no key, click or scroll for 60 min (the webview reports input via `mobile_activity`, at most every 20 s). VAPID `sub` = `mailto:noreply@winfer.dev`.

Payload JSON: `{"title":"<pane title>","body":"<message or status>","paneId":"…","status":"…"}`.
The service worker shows it and opens `/#/pane/<paneId>` on click. `404`/`410` from the push service → drop that subscription.
