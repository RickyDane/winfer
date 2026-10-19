// winfer mobile relay: dumb, authenticated WebSocket pipe between one winfer desktop
// and its paired phones. Frames are end-to-end encrypted; the relay never sees plaintext.
// Protocol: docs/mobile/protocol.md
import { DurableObject } from "cloudflare:workers";

type Role = "desktop" | "phone";

const ROOM_RE = /^[A-Za-z0-9_-]{22}$/; // 16 bytes, base64url
const AUTH_RE = /^[A-Za-z0-9_-]{43}$/; // 32 bytes, base64url
const MAX_PHONES = 8;
const MAX_FRAME = 1 << 20;
const IDLE_MS = 30 * 24 * 60 * 60 * 1000;

const text = (body: string, status: number) =>
  new Response(body, { status, headers: { "content-type": "text/plain; charset=utf-8" } });

export default {
  async fetch(request, env): Promise<Response> {
    const url = new URL(request.url);
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
    if (role !== "desktop" && role !== "phone") return text("bad role", 400);

    if (stored === undefined) {
      // First connection claims the room. Room ids are 128-bit random, so unguessable.
      await this.ctx.storage.put("authHash", presented);
    } else if (!authorized) {
      return text("forbidden", 403);
    }

    if (role === "desktop") {
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
    this.ctx.acceptWebSocket(server, [role]);
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

    const target: Role = this.roleOf(ws) === "desktop" ? "phone" : "desktop";
    for (const peer of this.ctx.getWebSockets(target)) {
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
    return this.ctx.getTags(ws).includes("desktop") ? "desktop" : "phone";
  }

  private broadcastPresence(leaving?: WebSocket): void {
    const sockets = this.ctx
      .getWebSockets()
      .filter((ws) => ws !== leaving && ws.readyState === WebSocket.OPEN);
    const frame = JSON.stringify({
      t: "presence",
      desktop: sockets.some((ws) => this.roleOf(ws) === "desktop"),
      phones: sockets.filter((ws) => this.roleOf(ws) === "phone").length,
    });
    for (const ws of sockets) {
      try {
        ws.send(frame);
      } catch {
        // closing
      }
    }
  }
}
