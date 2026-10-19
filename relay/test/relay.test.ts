import { exports } from "cloudflare:workers";
import { describe, expect, it } from "vitest";

const b64 = (n: number, fill: number) =>
  btoa(String.fromCharCode(...new Uint8Array(n).fill(fill)))
    .replace(/\+/g, "-")
    .replace(/\//g, "_")
    .replace(/=+$/, "");

let roomSeq = 0;
const newRoom = () => b64(16, ++roomSeq);
const AUTH = b64(32, 7);
const WRONG = b64(32, 8);

interface Client {
  ws: WebSocket;
  frames: string[];
  next(pred?: (f: string) => boolean): Promise<string>;
}

async function connect(room: string, role: string, auth = AUTH): Promise<Client> {
  const res = await exports.default.fetch(`http://relay/r/${room}?role=${role}&auth=${auth}`, {
    headers: { Upgrade: "websocket" },
  });
  const ws = res.webSocket;
  if (!ws) throw new Error(`no websocket: ${res.status}`);
  ws.accept();
  const frames: string[] = [];
  const waiters: Array<() => void> = [];
  ws.addEventListener("message", (e) => {
    frames.push(String(e.data));
    waiters.splice(0).forEach((w) => w());
  });
  let cursor = 0;
  const next = async (pred: (f: string) => boolean = () => true) => {
    for (;;) {
      while (cursor < frames.length) {
        const f = frames[cursor++];
        if (pred(f)) return f;
      }
      await new Promise<void>((resolve, reject) => {
        const t = setTimeout(() => reject(new Error("timeout")), 2000);
        waiters.push(() => {
          clearTimeout(t);
          resolve();
        });
      });
    }
  };
  return { ws, frames, next };
}

const isData = (f: string) => !f.startsWith("{");
const isPresence = (f: string) => f.startsWith("{");

describe("relay", () => {
  it("rejects malformed room/auth", async () => {
    const res = await exports.default.fetch(`http://relay/r/short?role=phone&auth=${AUTH}`, {
      headers: { Upgrade: "websocket" },
    });
    expect(res.status).toBe(400);
  });

  it("forwards desktop ⇄ phone, never echoes to sender", async () => {
    const room = newRoom();
    const desktop = await connect(room, "desktop");
    const phone = await connect(room, "phone");

    const presence = JSON.parse(await phone.next(isPresence));
    expect(presence).toEqual({ t: "presence", desktop: true, phones: 1 });

    phone.ws.send("cGhvbmU");
    expect(await desktop.next(isData)).toBe("cGhvbmU");

    desktop.ws.send("ZGVza3RvcA");
    expect(await phone.next(isData)).toBe("ZGVza3RvcA");

    // Neither side got its own frame back.
    expect(phone.frames.filter(isData)).toEqual(["ZGVza3RvcA"]);
    expect(desktop.frames.filter(isData)).toEqual(["cGhvbmU"]);
  });

  it("rejects a wrong auth token once claimed", async () => {
    const room = newRoom();
    await connect(room, "desktop");
    await expect(connect(room, "phone", WRONG)).rejects.toThrow(/403/);
  });

  it("drops client-sent control frames", async () => {
    const room = newRoom();
    const desktop = await connect(room, "desktop");
    const phone = await connect(room, "phone");
    phone.ws.send('{"t":"presence","desktop":false}');
    phone.ws.send("b2s");
    expect(await desktop.next(isData)).toBe("b2s");
  });

  it("replaces an older desktop connection", async () => {
    const room = newRoom();
    const first = await connect(room, "desktop");
    const closed = new Promise<number>((r) => first.ws.addEventListener("close", (e) => r(e.code)));
    const second = await connect(room, "desktop");
    expect(await closed).toBe(4000);
    const phone = await connect(room, "phone");
    phone.ws.send("aGk");
    expect(await second.next(isData)).toBe("aGk");
  });

  it("DELETE wipes the room so a new key can claim it", async () => {
    const room = newRoom();
    const phone = await connect(room, "phone");
    const closed = new Promise<number>((r) => phone.ws.addEventListener("close", (e) => r(e.code)));

    const bad = await exports.default.fetch(`http://relay/r/${room}?auth=${WRONG}`, { method: "DELETE" });
    expect(bad.status).toBe(403);
    const ok = await exports.default.fetch(`http://relay/r/${room}?auth=${AUTH}`, { method: "DELETE" });
    expect(ok.status).toBe(200);
    expect(await closed).toBe(4001);

    await expect(connect(room, "desktop", WRONG)).resolves.toBeTruthy();
  });

  it("serves the PWA for non-relay paths", async () => {
    const res = await exports.default.fetch("http://relay/");
    expect(res.status).not.toBe(426);
  });
});
