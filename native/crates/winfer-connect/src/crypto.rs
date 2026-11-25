//! Primitives on `ring`: HKDF-SHA256, HMAC-SHA256, SHA-256, X25519 and AES-256-GCM.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ring::{aead, agreement, digest, hkdf, hmac, rand::SecureRandom};

struct Len(usize);

impl hkdf::KeyType for Len {
    fn len(&self) -> usize {
        self.0
    }
}

/// HKDF-SHA256 (extract with `salt`, expand with `info`) to `len` bytes.
pub fn hkdf(salt: &[u8], ikm: &[u8], info: &[u8], len: usize) -> Vec<u8> {
    let prk = hkdf::Salt::new(hkdf::HKDF_SHA256, salt).extract(ikm);
    let mut out = vec![0; len];
    prk.expand(&[info], Len(len)).and_then(|okm| okm.fill(&mut out)).expect("HKDF output length is valid");
    out
}

pub fn hkdf32(salt: &[u8], ikm: &[u8], info: &[u8]) -> [u8; 32] {
    hkdf(salt, ikm, info, 32).try_into().expect("32 bytes")
}

/// Each part length-prefixed, so `["ab", "c"]` and `["a", "bc"]` differ.
fn framed(parts: &[&[u8]]) -> Vec<u8> {
    let mut out = Vec::new();
    for part in parts {
        out.extend_from_slice(&(part.len() as u32).to_be_bytes());
        out.extend_from_slice(part);
    }
    out
}

pub fn hmac(key: &[u8], parts: &[&[u8]]) -> [u8; 32] {
    hmac::sign(&hmac::Key::new(hmac::HMAC_SHA256, key), &framed(parts)).as_ref().try_into().expect("32 bytes")
}

/// Constant-time HMAC check.
pub fn hmac_verify(key: &[u8], parts: &[&[u8]], tag: &[u8]) -> bool {
    hmac::verify(&hmac::Key::new(hmac::HMAC_SHA256, key), &framed(parts), tag).is_ok()
}

pub fn sha256(parts: &[&[u8]]) -> [u8; 32] {
    digest::digest(&digest::SHA256, &framed(parts)).as_ref().try_into().expect("32 bytes")
}

pub fn b64(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

pub fn unb64(text: &str) -> Option<Vec<u8>> {
    URL_SAFE_NO_PAD.decode(text.trim_end_matches('=')).ok()
}

pub fn unb64_n<const N: usize>(text: &str) -> Option<[u8; N]> {
    unb64(text)?.try_into().ok()
}

pub fn random<const N: usize>() -> [u8; N] {
    let mut out = [0; N];
    ring::rand::SystemRandom::new().fill(&mut out).expect("system randomness");
    out
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

/// A one-shot X25519 key for a pairing.
pub struct Ephemeral {
    private: agreement::EphemeralPrivateKey,
    pub public: [u8; 32],
}

impl Ephemeral {
    pub fn new() -> Ephemeral {
        let rng = ring::rand::SystemRandom::new();
        let private = agreement::EphemeralPrivateKey::generate(&agreement::X25519, &rng).expect("X25519 key");
        let public = private.compute_public_key().expect("public key").as_ref().try_into().expect("32 bytes");
        Ephemeral { private, public }
    }

    /// The shared secret with the other side's public key; None for a bad key.
    pub fn agree(self, peer: &[u8; 32]) -> Option<[u8; 32]> {
        let peer = agreement::UnparsedPublicKey::new(&agreement::X25519, peer);
        agreement::agree_ephemeral(self.private, &peer, |ss| <[u8; 32]>::try_from(ss).ok()).ok().flatten()
    }
}

impl Default for Ephemeral {
    fn default() -> Self {
        Self::new()
    }
}

/// AES-256-GCM with 64-bit counter nonces: one key per direction, so a nonce never repeats and a
/// replayed, dropped or reordered frame fails to open.
pub struct Sealer {
    key: aead::LessSafeKey,
    counter: u64,
}

impl Sealer {
    pub fn new(key: &[u8; 32]) -> Sealer {
        Sealer { key: aead::LessSafeKey::new(aead::UnboundKey::new(&aead::AES_256_GCM, key).expect("32-byte key")), counter: 0 }
    }

    fn nonce(counter: u64) -> aead::Nonce {
        let mut n = [0u8; 12];
        n[4..].copy_from_slice(&counter.to_be_bytes());
        aead::Nonce::assume_unique_for_key(n)
    }

    pub fn seal(&mut self, plaintext: &[u8]) -> Vec<u8> {
        let mut buf = plaintext.to_vec();
        self.key.seal_in_place_append_tag(Self::nonce(self.counter), aead::Aad::empty(), &mut buf).expect("AES-GCM seal");
        self.counter += 1;
        buf
    }

    /// The next frame's plaintext, or None (and the counter doesn't move) when it isn't it.
    pub fn open(&mut self, sealed: &[u8]) -> Option<Vec<u8>> {
        let mut buf = sealed.to_vec();
        let plain = self.key.open_in_place(Self::nonce(self.counter), aead::Aad::empty(), &mut buf).ok()?.to_vec();
        self.counter += 1;
        Some(plain)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn x25519_agrees() {
        let (a, b) = (Ephemeral::new(), Ephemeral::new());
        let (pa, pb) = (a.public, b.public);
        assert_eq!(a.agree(&pb).unwrap(), b.agree(&pa).unwrap());
    }

    #[test]
    fn sealer_is_ordered_and_rejects_replays() {
        let key = [3u8; 32];
        let (mut tx, mut rx) = (Sealer::new(&key), Sealer::new(&key));
        let one = tx.seal(b"one");
        let two = tx.seal(b"two");
        assert!(rx.open(&two).is_none(), "out of order");
        assert_eq!(rx.open(&one).unwrap(), b"one");
        assert!(rx.open(&one).is_none(), "replay");
        assert_eq!(rx.open(&two).unwrap(), b"two");
    }

    #[test]
    fn hmac_is_length_framed() {
        assert_ne!(hmac(b"k", &[b"ab", b"c"]), hmac(b"k", &[b"a", b"bc"]));
        assert!(hmac_verify(b"k", &[b"x"], &hmac(b"k", &[b"x"])));
        assert!(!hmac_verify(b"k", &[b"x"], &hmac(b"k", &[b"y"])));
    }
}
