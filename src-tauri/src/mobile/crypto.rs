//! End-to-end encryption between winfer and a paired phone (docs/mobile/protocol.md): keys
//! derived from the pairing key with HKDF-SHA256, frames sealed with AES-256-GCM. The relay
//! sees the auth token only, never the encryption key.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ring::{aead, hkdf, rand::SecureRandom};

const SALT: &[u8] = b"winfer-mobile-v1";

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
    prk.expand(&[info], Len(len))
        .and_then(|okm| okm.fill(&mut out))
        .expect("HKDF output length is valid");
    out
}

pub fn b64(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

pub fn unb64(text: &str) -> Option<Vec<u8>> {
    // Push subscriptions sometimes carry padded base64url.
    URL_SAFE_NO_PAD.decode(text.trim_end_matches('=')).ok()
}

pub fn random<const N: usize>() -> [u8; N] {
    let mut out = [0; N];
    ring::rand::SystemRandom::new().fill(&mut out).expect("system randomness");
    out
}

/// The keys a pairing key stands for.
pub struct Keys {
    /// Shown to the relay to join the room.
    pub auth_token: String,
    enc: aead::LessSafeKey,
}

impl Keys {
    pub fn derive(key: &[u8]) -> Keys {
        let auth_token = b64(&hkdf(SALT, key, b"auth", 32));
        let enc = hkdf(SALT, key, b"enc", 32);
        let enc = aead::LessSafeKey::new(aead::UnboundKey::new(&aead::AES_256_GCM, &enc).expect("32-byte key"));
        Keys { auth_token, enc }
    }

    /// A frame: base64url(nonce || ciphertext + tag).
    pub fn seal(&self, plaintext: &[u8]) -> String {
        self.seal_with(plaintext, random())
    }

    pub fn seal_with(&self, plaintext: &[u8], nonce: [u8; 12]) -> String {
        let mut buf = plaintext.to_vec();
        self.enc
            .seal_in_place_append_tag(aead::Nonce::assume_unique_for_key(nonce), aead::Aad::empty(), &mut buf)
            .expect("AES-GCM seal");
        let mut frame = nonce.to_vec();
        frame.extend(buf);
        b64(&frame)
    }

    /// The plaintext of a frame; None when it isn't one sealed with this key.
    pub fn open(&self, frame: &str) -> Option<Vec<u8>> {
        let bytes = unb64(frame)?;
        if bytes.len() < 12 + 16 {
            return None;
        }
        let (nonce, sealed) = bytes.split_at(12);
        let mut buf = sealed.to_vec();
        let nonce = aead::Nonce::try_assume_unique_for_key(nonce).ok()?;
        let plain = self.enc.open_in_place(nonce, aead::Aad::empty(), &mut buf).ok()?;
        Some(plain.to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    /// The PWA runs the same vectors (docs/mobile/test-vectors.json) through WebCrypto.
    #[test]
    fn matches_the_shared_test_vectors() {
        let vectors: Value = serde_json::from_str(include_str!("../../../docs/mobile/test-vectors.json")).unwrap();
        let key = unb64(vectors["key"].as_str().unwrap()).unwrap();
        assert_eq!(key, [1u8; 32]);
        let keys = Keys::derive(&key);
        assert_eq!(keys.auth_token, vectors["authToken"].as_str().unwrap());
        assert_eq!(b64(&hkdf(SALT, &key, b"enc", 32)), vectors["encKey"].as_str().unwrap());
        let nonce: [u8; 12] = unb64(vectors["nonce"].as_str().unwrap()).unwrap().try_into().unwrap();
        let plaintext = vectors["plaintext"].as_str().unwrap();
        let frame = keys.seal_with(plaintext.as_bytes(), nonce);
        assert_eq!(frame, vectors["frame"].as_str().unwrap());
        assert_eq!(keys.open(&frame).unwrap(), plaintext.as_bytes());
    }

    #[test]
    fn rejects_frames_from_other_keys_and_tampering() {
        let keys = Keys::derive(&[7; 32]);
        let frame = keys.seal(b"hello");
        assert_eq!(keys.open(&frame).unwrap(), b"hello");
        assert!(Keys::derive(&[8; 32]).open(&frame).is_none());
        let mut bytes = unb64(&frame).unwrap();
        *bytes.last_mut().unwrap() ^= 1;
        assert!(keys.open(&b64(&bytes)).is_none());
        assert!(keys.open("not a frame").is_none());
        assert!(keys.open("{\"t\":\"presence\"}").is_none());
    }

    #[test]
    fn nonces_are_random() {
        let keys = Keys::derive(&[7; 32]);
        assert_ne!(keys.seal(b"x"), keys.seal(b"x"));
    }
}
