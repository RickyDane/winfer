// winfer relay: dumb, authenticated WebSocket pipe. Two kinds of rooms:
// - mobile: one winfer desktop and its paired phones (docs/mobile/protocol.md)
// - peer: two paired winfer instances, Winfer Connect (docs/connect/protocol.md)
// Frames are end-to-end encrypted; the relay never sees plaintext.
import { DurableObject } from "cloudflare:workers";

type Role = "desktop" | "phone" | "peer";
type Kind = "mobile" | "peer";

const ROOM_RE = /^[A-Za-z0-9_-]{22}$/; // 16 bytes, base64url
const AUTH_RE = /^[A-Za-z0-9_-]{43}$/; // 32 bytes, base64url
const INST_RE = /^[A-Za-z0-9_-]{22}$/; // Winfer Connect instance id, 16 bytes
const MAX_PHONES = 8;
const MAX_PEERS = 2;
const MAX_FRAME = 1 << 20;
const IDLE_MS = 30 * 24 * 60 * 60 * 1000;
// winferd's installer (native/crates/winferd/install.sh) is uploaded with every release.
const RELEASES = "https://api.github.com/repos/RickyDane/winfer/releases?per_page=30";
const INSTALL_TTL = 300;

export interface Release {
  tag_name: string;
  prerelease: boolean;
  draft: boolean;
  assets: { name: string }[];
}

/** The release to install from: the newest stable one that ships winferd, else the newest
 * pre-release (Nebula) that does. Old stable releases predate winferd. */
export function pickRelease(releases: Release[]): string | undefined {
  const ships = (r: Release) => !r.draft && r.assets.some((a) => a.name === "install.sh");
  return (releases.find((r) => ships(r) && !r.prerelease) ?? releases.find(ships))?.tag_name;
}

/** Pins the script to `tag`, so it downloads winferd from the same release. */
export function pinVersion(script: string, tag: string): string {
  return script.replace('VERSION="${WINFERD_VERSION:-}"', `VERSION="\${WINFERD_VERSION:-${tag}}"`);
}

async function installScript(): Promise<Response> {
  const cache = caches.default;
  const key = new Request("https://relay.cache/install.sh");
  const hit = await cache.match(key);
  if (hit) return hit;
  const gh = { headers: { "user-agent": "winfer-relay", accept: "application/vnd.github+json" } };
  const list = await fetch(RELEASES, gh);
  if (!list.ok) return text(`echo "winfer: couldn't list releases (${list.status})" >&2; exit 1\n`, 502);
  const tag = pickRelease(await list.json());
  if (!tag) return text('echo "winfer: no release ships winferd yet" >&2; exit 1\n', 404);
  const sh = await fetch(`https://github.com/RickyDane/winfer/releases/download/${tag}/install.sh`);
  if (!sh.ok) return text(`echo "winfer: couldn't fetch the installer (${sh.status})" >&2; exit 1\n`, 502);
  const res = new Response(pinVersion(await sh.text(), tag), {
    headers: { "content-type": "text/plain; charset=utf-8", "cache-control": `public, max-age=${INSTALL_TTL}` },
  });
  await cache.put(key, res.clone());
  return res;
}

const text = (body: string, status: number) =>
  new Response(body, { status, headers: { "content-type": "text/plain; charset=utf-8" } });

export default {
  async fetch(request, env): Promise<Response> {
    const url = new URL(request.url);
    if (url.pathname === "/install.sh") return installScript();
    const match = url.pathname.match(/^\/r\/([^/]+)\/?$/);
    if (!match) return env.ASSETS.fetch(request);

    const room = match[1];
    const auth = url.searchParams.get("auth") ?? "";
    if (!ROOM_RE.test(room) || !AUTH_RE.test(auth)) return text("bad room or auth", 400);

    const stub = env.ROOM.get(env.ROOM.idFromName(room));
    return stub.fetch(request);
  },
} satisfies ExportedHandler<Env>;

async function sha256(value: string): Promise<ArrayBuffer> {
  return crypto.subtle.digest("SHA-256", new TextEncoder().encode(value));
}

export class Room extends DurableObject<Env> {
  constructor(ctx: DurableObjectState, env: Env) {
    super(ctx, env);
    // Keepalive without waking the object from hibernation.
    this.ctx.setWebSocketAutoResponse(new WebSocketRequestResponsePair("ping", "pong"));
  }

  async fetch(request: Request): Promise<Response> {
    const url = new URL(request.url);
    const auth = url.searchParams.get("auth") ?? "";
    const stored = await this.ctx.storage.get<ArrayBuffer>("authHash");
    const presented = await sha256(auth);
    const authorized = stored !== undefined && crypto.subtle.timingSafeEqual(stored, presented);

    if (request.method === "DELETE") {
      if (!authorized) return text("forbidden", 403);
      for (const ws of this.ctx.getWebSockets()) {
        try {
          ws.close(4001, "revoked");
        } catch {
          // already closing
        }
      }
      await this.ctx.storage.deleteAlarm();
      await this.ctx.storage.deleteAll();
      return text("deleted", 200);
    }

    if (request.method !== "GET" || request.headers.get("Upgrade")?.toLowerCase() !== "websocket") {
      return text("expected websocket upgrade", 426);
    }

    const role = url.searchParams.get("role");
    if (role !== "desktop" && role !== "phone" && role !== "peer") return text("bad role", 400);
    const inst = url.searchParams.get("inst") ?? "";
    if (role === "peer" && !INST_RE.test(inst)) return text("bad instance", 400);
    const kind: Kind = role === "peer" ? "peer" : "mobile";

    if (stored === undefined) {
      // First connection claims the room. Room ids are 128-bit random, so unguessable.
      await this.ctx.storage.put({ authHash: presented, kind });
    } else if (!authorized) {
      return text("forbidden", 403);
    } else if (((await this.ctx.storage.get<Kind>("kind")) ?? "mobile") !== kind) {
      return text("wrong room kind", 409);
    }

    const tags: string[] = [role];
    if (role === "peer") {
      // A reconnecting instance replaces its own old socket; a room holds the two paired instances.
      for (const old of this.ctx.getWebSockets(`inst:${inst}`)) {
        try {
          old.close(4000, "replaced");
        } catch {
          // already closing
        }
      }
      const others = new Set(
        this.ctx
          .getWebSockets("peer")
          .filter((ws) => ws.readyState === WebSocket.OPEN)
          .flatMap((ws) => this.ctx.getTags(ws).filter((t) => t.startsWith("inst:") && t !== `inst:${inst}`)),
      );
      if (others.size + 1 > MAX_PEERS) {
        return text("too many peers", 429);
      }
      tags.push(`inst:${inst}`);
    } else if (role === "desktop") {
      for (const old of this.ctx.getWebSockets("desktop")) {
        try {
          old.close(4000, "replaced");
        } catch {
          // already closing
        }
      }
    } else if (this.ctx.getWebSockets("phone").length >= MAX_PHONES) {
      return text("too many phones", 429);
    }

    const pair = new WebSocketPair();
    const [client, server] = [pair[0], pair[1]];
    this.ctx.acceptWebSocket(server, tags);
    await this.ctx.storage.setAlarm(Date.now() + IDLE_MS);
    this.broadcastPresence();
    return new Response(null, { status: 101, webSocket: client });
  }

  webSocketMessage(ws: WebSocket, message: string | ArrayBuffer): void {
    if (typeof message !== "string") {
      ws.close(1003, "text frames only");
      return;
    }
    if (message.length > MAX_FRAME) {
      ws.close(1009, "frame too large");
      return;
    }
    // Control frames are relay → client only.
    if (message.startsWith("{")) return;

    const role = this.roleOf(ws);
    const target: Role = role === "peer" ? "peer" : role === "desktop" ? "phone" : "desktop";
    for (const peer of this.ctx.getWebSockets(target)) {
      if (peer === ws) continue;
      try {
        peer.send(message);
      } catch {
        // peer is closing; its close handler updates presence
      }
    }
  }

  webSocketClose(ws: WebSocket, code: number, reason: string): void {
    try {
      ws.close(code, reason);
    } catch {
      // already closed
    }
    this.broadcastPresence(ws);
  }

  webSocketError(ws: WebSocket): void {
    this.broadcastPresence(ws);
  }

  async alarm(): Promise<void> {
    if (this.ctx.getWebSockets().length > 0) {
      await this.ctx.storage.setAlarm(Date.now() + IDLE_MS);
      return;
    }
    await this.ctx.storage.deleteAll();
  }

  private roleOf(ws: WebSocket): Role {
    const tags = this.ctx.getTags(ws);
    return tags.includes("peer") ? "peer" : tags.includes("desktop") ? "desktop" : "phone";
  }

  private broadcastPresence(leaving?: WebSocket): void {
    const sockets = this.ctx
      .getWebSockets()
      .filter((ws) => ws !== leaving && ws.readyState === WebSocket.OPEN);
    const peers = sockets.filter((ws) => this.roleOf(ws) === "peer").length;
    const frame = JSON.stringify(
      peers > 0
        ? { t: "presence", peers }
        : {
            t: "presence",
            desktop: sockets.some((ws) => this.roleOf(ws) === "desktop"),
            phones: sockets.filter((ws) => this.roleOf(ws) === "phone").length,
          },
    );
    for (const ws of sockets) {
      try {
        ws.send(frame);
      } catch {
        // closing
      }
    }
  }
}
