import { createCipheriv, createDecipheriv, hkdfSync } from "node:crypto";
import { existsSync, readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { b64uDecode, b64uEncode } from "./b64";
import { deriveKeys, open, seal } from "./crypto";
import { parsePairPayload } from "./pairing";

const KEY = b64uEncode(new Uint8Array(32).fill(1));
// Computed for key = 32 × 0x01 with salt "winfer-mobile-v1", info "auth".
const AUTH_VECTOR = "jAEmAUiO6YEo7hB8oaZ9AnCFx_KmDiQhO-16GmTzYUs";

function nodeEnc(): Buffer {
  return Buffer.from(hkdfSync("sha256", Buffer.alloc(32, 1), "winfer-mobile-v1", "enc", 32));
}

describe("crypto", () => {
  it("derives authToken matching an independent HKDF", async () => {
    const { authToken } = await deriveKeys(KEY);
    const expected = b64uEncode(new Uint8Array(hkdfSync("sha256", Buffer.alloc(32, 1), "winfer-mobile-v1", "auth", 32)));
    expect(authToken).toBe(expected);
    expect(authToken).toBe(AUTH_VECTOR);
  });

  it("round-trips frames", async () => {
    const { encKey } = await deriveKeys(KEY);
    const msg = { t: "send", paneId: "p1", text: "héllo 👋" };
    expect(await open(encKey, await seal(encKey, msg))).toEqual(msg);
  });

  it("opens frames sealed by node AES-GCM (desktop-compatible layout)", async () => {
    const { encKey } = await deriveKeys(KEY);
    const nonce = Buffer.alloc(12, 7);
    const c = createCipheriv("aes-256-gcm", nodeEnc(), nonce);
    const ct = Buffer.concat([c.update(JSON.stringify({ t: "ack", id: "1", ok: true })), c.final(), c.getAuthTag()]);
    expect(await open(encKey, b64uEncode(new Uint8Array(Buffer.concat([nonce, ct]))))).toEqual({ t: "ack", id: "1", ok: true });
  });

  it("seals frames node can open", async () => {
    const { encKey } = await deriveKeys(KEY);
    const buf = Buffer.from(b64uDecode(await seal(encKey, { t: "panes.get" })));
    const d = createDecipheriv("aes-256-gcm", nodeEnc(), buf.subarray(0, 12));
    d.setAuthTag(buf.subarray(buf.length - 16));
    const pt = Buffer.concat([d.update(buf.subarray(12, buf.length - 16)), d.final()]);
    expect(JSON.parse(pt.toString())).toEqual({ t: "panes.get" });
  });

  it("rejects foreign or corrupt frames", async () => {
    const { encKey } = await deriveKeys(KEY);
    const other = await deriveKeys(b64uEncode(new Uint8Array(32).fill(2)));
    expect(await open(encKey, await seal(other.encKey, { t: "x" }))).toBeNull();
    expect(await open(encKey, "not-a-frame")).toBeNull();
  });

  const vectors = new URL("../../docs/mobile/test-vectors.json", import.meta.url);
  it.skipIf(!existsSync(vectors))("matches desktop test vectors", async () => {
    const v = JSON.parse(readFileSync(vectors, "utf8"));
    const { authToken, encKey } = await deriveKeys(v.key);
    expect(authToken).toBe(v.authToken);
    if (v.frame && v.plaintext) expect(await open(encKey, v.frame)).toEqual(typeof v.plaintext === "string" ? JSON.parse(v.plaintext) : v.plaintext);
  });
});

describe("pair link", () => {
  const payload = b64uEncode(new TextEncoder().encode(JSON.stringify({ v: 1, r: "room_1", k: KEY, d: "dev1", n: "Mac" })));
  it("parses a full link and keeps its origin", () => {
    const p = parsePairPayload(`https://relay.example/#p=${payload}`, "http://localhost");
    expect(p).toMatchObject({ roomId: "room_1", key: KEY, deviceId: "dev1", name: "Mac", origin: "https://relay.example" });
  });
  it("rejects junk", () => {
    expect(() => parsePairPayload("hello", "x")).toThrow();
  });
});
