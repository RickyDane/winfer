// Key derivation + framing per docs/mobile/protocol.md:
//   authToken = HKDF-SHA256(key, salt "winfer-mobile-v1", info "auth", 32) → base64url
//   encKey    = HKDF-SHA256(key, salt "winfer-mobile-v1", info "enc", 32) → AES-256-GCM
//   frame     = base64url(nonce[12] || ciphertext+tag)
import { b64uDecode, b64uEncode, utf8 } from "./b64";

const SALT = utf8("winfer-mobile-v1");

export interface Keys {
  authToken: string;
  encKey: CryptoKey;
}

export async function deriveKeys(keyB64u: string): Promise<Keys> {
  const raw = b64uDecode(keyB64u);
  if (raw.length !== 32) throw new Error("pairing key must be 32 bytes");
  const ikm = await crypto.subtle.importKey("raw", raw, "HKDF", false, ["deriveBits", "deriveKey"]);
  const auth = await crypto.subtle.deriveBits({ name: "HKDF", hash: "SHA-256", salt: SALT, info: utf8("auth") }, ikm, 256);
  const encKey = await crypto.subtle.deriveKey(
    { name: "HKDF", hash: "SHA-256", salt: SALT, info: utf8("enc") },
    ikm,
    { name: "AES-GCM", length: 256 },
    false,
    ["encrypt", "decrypt"],
  );
  return { authToken: b64uEncode(new Uint8Array(auth)), encKey };
}

export async function seal(encKey: CryptoKey, msg: unknown): Promise<string> {
  const nonce = crypto.getRandomValues(new Uint8Array(new ArrayBuffer(12)));
  const ct = new Uint8Array(await crypto.subtle.encrypt({ name: "AES-GCM", iv: nonce }, encKey, utf8(JSON.stringify(msg))));
  const out = new Uint8Array(12 + ct.length);
  out.set(nonce);
  out.set(ct, 12);
  return b64uEncode(out);
}

/** Decrypt a frame; null when it isn't ours (wrong key, corrupt, not JSON). */
export async function open<T = unknown>(encKey: CryptoKey, frame: string): Promise<T | null> {
  try {
    const buf = b64uDecode(frame);
    if (buf.length < 12 + 16) return null;
    const pt = await crypto.subtle.decrypt({ name: "AES-GCM", iv: buf.subarray(0, 12) }, encKey, buf.subarray(12));
    return JSON.parse(new TextDecoder().decode(pt)) as T;
  } catch {
    return null;
  }
}
