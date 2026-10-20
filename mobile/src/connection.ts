import { BaseClient } from "./client";
import { deriveKeys, open, seal, type Keys } from "./crypto";
import { relayOrigin, type Pairing } from "./pairing";
import type { DesktopMsg, PhoneMsg, Presence } from "./protocol";

const REQUEST_TIMEOUT = 20_000;
const MAX_BACKOFF = 30_000;
/** How often the open app re-reads the desktop's panes, healing anything lost on the way. */
const RESYNC_EVERY = 20_000;

interface Pending {
  resolve: (m: DesktopMsg) => void;
  reject: (e: Error) => void;
  timer: ReturnType<typeof setTimeout>;
}

/** Phone side of the relay: one encrypted WebSocket to one paired desktop. */
export class RelayClient extends BaseClient {
  readonly name: string;
  private keys: Promise<Keys>;
  private ws: WebSocket | null = null;
  private pending = new Map<string, Pending>();
  private seq = 0;
  private attempts = 0;
  private retry: ReturnType<typeof setTimeout> | null = null;
  private closed = false;
  private keepalive: ReturnType<typeof setInterval> | null = null;
  private lastPong = 0;
  /** Frames are decrypted one after another: in parallel, a later one could land first. */
  private frames: Promise<void> = Promise.resolve();
  private resync: ReturnType<typeof setInterval> | null = null;

  constructor(private pairing: Pairing) {
    super();
    this.name = pairing.name;
    this.keys = deriveKeys(pairing.key);
    document.addEventListener("visibilitychange", this.onVisible);
    window.addEventListener("online", this.onVisible);
    void this.connect();
    this.resync = setInterval(() => {
      if (document.visibilityState === "visible" && this.state.socket === "open" && this.state.desktopOnline) this.request({ t: "panes.get" }).catch(() => {});
    }, RESYNC_EVERY);
  }

  private onVisible = () => {
    if (document.visibilityState !== "visible") return;
    if (this.state.socket === "closed") this.reconnect();
    // Back from background: statuses may have moved while we were frozen.
    else if (this.state.socket === "open" && this.state.desktopOnline) this.request({ t: "panes.get" }).catch(() => {});
  };

  private async connect() {
    if (this.closed) return;
    this.set({ socket: "connecting" });
    let keys: Keys;
    try {
      keys = await this.keys;
    } catch (e) {
      this.set({ socket: "closed", error: (e as Error).message });
      return;
    }
    const base = relayOrigin(this.pairing).replace(/^http/, "ws").replace(/\/$/, "");
    const ws = new WebSocket(`${base}/r/${this.pairing.roomId}?role=phone&auth=${keys.authToken}`);
    this.ws = ws;
    ws.onopen = () => {
      if (this.ws !== ws) return;
      this.attempts = 0;
      this.set({ socket: "open", error: null });
      this.startKeepalive(ws);
      // No hello here: the relay sends presence on connect, and that triggers it.
    };
    ws.onmessage = (ev) => {
      if (this.ws !== ws || typeof ev.data !== "string") return;
      if (ev.data === "pong") {
        this.lastPong = Date.now();
        return;
      }
      const data = ev.data;
      this.frames = this.frames.then(() => (this.ws === ws ? this.onFrame(data, keys) : undefined)).catch(() => {});
    };
    ws.onclose = (ev) => {
      if (this.ws !== ws) return;
      this.ws = null;
      this.stopKeepalive();
      this.failPending("connection lost");
      // Relay closes 4001 on revoke; an HTTP 403 shows as 1006, so hint after repeated failures.
      const err = this.attempts >= 3 ? "Can't reach the relay. Retrying…" : this.state.error;
      this.set({ socket: "closed", desktopOnline: null, error: ev.code === 4001 || ev.code === 1008 ? "This pairing was revoked on the desktop." : err });
      if (ev.code !== 4001 && ev.code !== 1008) this.schedule();
    };
  }

  /** Relay answers `ping` with `pong`; a silent socket (phone slept, NAT dropped) gets recycled. */
  private startKeepalive(ws: WebSocket) {
    this.stopKeepalive();
    this.lastPong = Date.now();
    this.keepalive = setInterval(() => {
      if (Date.now() - this.lastPong > 75_000) {
        this.reconnect();
        return;
      }
      if (ws.readyState === WebSocket.OPEN) ws.send("ping");
    }, 30_000);
  }

  private stopKeepalive() {
    if (this.keepalive) clearInterval(this.keepalive);
    this.keepalive = null;
  }

  private schedule() {
    if (this.closed || this.retry) return;
    const delay = Math.min(MAX_BACKOFF, 1000 * 2 ** this.attempts) * (0.75 + Math.random() * 0.5);
    this.attempts++;
    this.retry = setTimeout(() => {
      this.retry = null;
      void this.connect();
    }, delay);
  }

  private async onFrame(data: string, keys: Keys) {
    if (data.startsWith("{")) {
      let ctl: Presence;
      try {
        ctl = JSON.parse(data);
      } catch {
        return;
      }
      if (ctl.t !== "presence") return;
      const was = this.state.desktopOnline;
      this.set({ desktopOnline: ctl.desktop });
      // Desktop (re)joined: our earlier hello went nowhere.
      if (ctl.desktop && was !== true) void this.hello();
      return;
    }
    const msg = await open<DesktopMsg>(keys.encKey, data);
    if (!msg || typeof msg.t !== "string") return;
    this.apply(msg);
    const id = "id" in msg ? msg.id : undefined;
    const p = id ? this.pending.get(id) : undefined;
    if (p && id) {
      this.pending.delete(id);
      clearTimeout(p.timer);
      if (msg.t === "error") p.reject(new Error(msg.error));
      else if (msg.t === "ack" && !msg.ok) p.reject(new Error(msg.error || "Desktop refused"));
      else p.resolve(msg);
    }
  }

  private async hello() {
    try {
      await this.request({ t: "hello", device: this.pairing.deviceId, ua: navigator.userAgent });
    } catch {
      /* desktop offline; presence will retrigger */
    }
  }

  async request(msg: PhoneMsg): Promise<DesktopMsg> {
    const ws = this.ws;
    if (!ws || ws.readyState !== WebSocket.OPEN) throw new Error("Not connected");
    if (this.state.desktopOnline === false) throw new Error("Desktop is offline");
    const id = `${Date.now().toString(36)}-${++this.seq}`;
    const frame = await seal((await this.keys).encKey, { ...msg, id });
    return new Promise<DesktopMsg>((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pending.delete(id);
        reject(new Error("Desktop didn't answer"));
      }, REQUEST_TIMEOUT);
      this.pending.set(id, { resolve, reject, timer });
      ws.send(frame);
    });
  }

  private failPending(reason: string) {
    for (const p of this.pending.values()) {
      clearTimeout(p.timer);
      p.reject(new Error(reason));
    }
    this.pending.clear();
  }

  reconnect() {
    this.stopKeepalive();
    if (this.retry) clearTimeout(this.retry);
    this.retry = null;
    this.attempts = 0;
    const ws = this.ws;
    this.ws = null;
    ws?.close();
    void this.connect();
  }

  close() {
    this.closed = true;
    this.stopKeepalive();
    if (this.resync) clearInterval(this.resync);
    if (this.retry) clearTimeout(this.retry);
    document.removeEventListener("visibilitychange", this.onVisible);
    window.removeEventListener("online", this.onVisible);
    this.failPending("closed");
    const ws = this.ws;
    this.ws = null;
    ws?.close();
  }
}
