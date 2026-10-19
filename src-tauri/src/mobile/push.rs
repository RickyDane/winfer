//! Web Push to paired phones, straight from winfer to the browser's push service: payloads
//! encrypted per RFC 8291 (aes128gcm), requests signed per RFC 8292 (VAPID). Works while the
//! phone's app is closed, and the relay never sees the notification.

use std::time::Duration;

use ring::{aead, agreement, rand::SystemRandom, signature};
use ring::signature::KeyPair;
use serde_json::{Value, json};

use super::crypto::{b64, hkdf, random, unb64};

const SUBJECT: &str = "mailto:noreply@winfer.dev";
/// One record holds the whole payload; push services accept up to 4096 bytes.
const RECORD_SIZE: u32 = 4096;

/// winfer's VAPID identity: a P-256 signing key, kept in the keychain as PKCS#8.
pub struct Vapid {
    key: signature::EcdsaKeyPair,
    /// Uncompressed public key (65 bytes), the PWA's `applicationServerKey`.
    pub public: Vec<u8>,
}

impl Vapid {
    pub fn generate() -> (Vapid, Vec<u8>) {
        let rng = SystemRandom::new();
        let pkcs8 = signature::EcdsaKeyPair::generate_pkcs8(&signature::ECDSA_P256_SHA256_FIXED_SIGNING, &rng).expect("P-256 key");
        let pkcs8 = pkcs8.as_ref().to_vec();
        (Vapid::from_pkcs8(&pkcs8).expect("fresh key parses"), pkcs8)
    }

    pub fn from_pkcs8(pkcs8: &[u8]) -> Option<Vapid> {
        let key = signature::EcdsaKeyPair::from_pkcs8(&signature::ECDSA_P256_SHA256_FIXED_SIGNING, pkcs8, &SystemRandom::new()).ok()?;
        let public = key.public_key().as_ref().to_vec();
        Some(Vapid { key, public })
    }

    /// The `Authorization` header for a push endpoint, valid for 12 hours.
    pub fn authorization(&self, endpoint: &str, now_secs: u64) -> Option<String> {
        let header = b64(br#"{"typ":"JWT","alg":"ES256"}"#);
        let claims = b64(json!({"aud": origin(endpoint)?, "exp": now_secs + 12 * 3600, "sub": SUBJECT}).to_string().as_bytes());
        let input = format!("{header}.{claims}");
        let sig = self.key.sign(&SystemRandom::new(), input.as_bytes()).ok()?;
        Some(format!("vapid t={input}.{}, k={}", b64(sig.as_ref()), b64(&self.public)))
    }
}

/// `https://host[:port]` of a URL.
fn origin(url: &str) -> Option<&str> {
    let rest = url.find("://").map(|i| i + 3)?;
    let end = url[rest..].find('/').map_or(url.len(), |i| rest + i);
    Some(&url[..end])
}

/// Encrypts `payload` for a subscription's `p256dh` key and `auth` secret (RFC 8291).
pub fn encrypt(payload: &[u8], ua_public: &[u8], auth: &[u8]) -> Option<Vec<u8>> {
    let private = agreement::EphemeralPrivateKey::generate(&agreement::ECDH_P256, &SystemRandom::new()).ok()?;
    encrypt_with(payload, ua_public, auth, private, random())
}

fn encrypt_with(payload: &[u8], ua_public: &[u8], auth: &[u8], private: agreement::EphemeralPrivateKey, salt: [u8; 16]) -> Option<Vec<u8>> {
    if payload.len() + 1 + 16 > RECORD_SIZE as usize - 86 {
        return None;
    }
    let as_public = private.compute_public_key().ok()?.as_ref().to_vec();
    let secret = agreement::agree_ephemeral(private, &agreement::UnparsedPublicKey::new(&agreement::ECDH_P256, ua_public), |s| s.to_vec()).ok()?;
    let mut key_info = b"WebPush: info\0".to_vec();
    key_info.extend(ua_public);
    key_info.extend(&as_public);
    let ikm = hkdf(auth, &secret, &key_info, 32);
    let cek = hkdf(&salt, &ikm, b"Content-Encoding: aes128gcm\0", 16);
    let nonce: [u8; 12] = hkdf(&salt, &ikm, b"Content-Encoding: nonce\0", 12).try_into().ok()?;

    let mut record = payload.to_vec();
    record.push(2); // padding delimiter of the last (only) record
    let key = aead::LessSafeKey::new(aead::UnboundKey::new(&aead::AES_128_GCM, &cek).ok()?);
    key.seal_in_place_append_tag(aead::Nonce::assume_unique_for_key(nonce), aead::Aad::empty(), &mut record).ok()?;

    let mut body = salt.to_vec();
    body.extend(RECORD_SIZE.to_be_bytes());
    body.push(as_public.len() as u8);
    body.extend(&as_public);
    body.extend(record);
    Some(body)
}

pub enum Outcome {
    Sent,
    /// The subscription is gone (404/410): forget it.
    Gone,
    Failed(String),
}

/// Sends one notification to a `PushSubscription` (its JSON form).
pub fn send(vapid: &Vapid, subscription: &Value, payload: &Value) -> Outcome {
    let (Some(endpoint), Some(p256dh), Some(auth)) = (
        subscription["endpoint"].as_str(),
        subscription["keys"]["p256dh"].as_str().and_then(unb64),
        subscription["keys"]["auth"].as_str().and_then(unb64),
    ) else {
        return Outcome::Gone;
    };
    let Some(body) = encrypt(payload.to_string().as_bytes(), &p256dh, &auth) else {
        return Outcome::Failed("couldn't encrypt the notification".into());
    };
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let Some(authorization) = vapid.authorization(endpoint, now) else {
        return Outcome::Failed("bad push endpoint".into());
    };
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(15)))
        .http_status_as_error(false)
        .build()
        .into();
    let result = agent
        .post(endpoint)
        .header("Authorization", &authorization)
        .header("TTL", "3600")
        .header("Urgency", "high")
        .header("Content-Encoding", "aes128gcm")
        .header("Content-Type", "application/octet-stream")
        .send(&body[..]);
    match result {
        Ok(r) => match r.status().as_u16() {
            200..=299 => Outcome::Sent,
            404 | 410 => Outcome::Gone,
            code => Outcome::Failed(format!("push service answered {code}")),
        },
        Err(e) => Outcome::Failed(e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What the browser does with a push message, to check our side of RFC 8291.
    fn decrypt(body: &[u8], ua_private: agreement::EphemeralPrivateKey, ua_public: &[u8], auth: &[u8]) -> Vec<u8> {
        let salt = &body[..16];
        assert_eq!(u32::from_be_bytes(body[16..20].try_into().unwrap()), RECORD_SIZE);
        let idlen = body[20] as usize;
        let as_public = &body[21..21 + idlen];
        let secret = agreement::agree_ephemeral(ua_private, &agreement::UnparsedPublicKey::new(&agreement::ECDH_P256, as_public), |s| s.to_vec()).unwrap();
        let mut key_info = b"WebPush: info\0".to_vec();
        key_info.extend(ua_public);
        key_info.extend(as_public);
        let ikm = hkdf(auth, &secret, &key_info, 32);
        let cek = hkdf(salt, &ikm, b"Content-Encoding: aes128gcm\0", 16);
        let nonce: [u8; 12] = hkdf(salt, &ikm, b"Content-Encoding: nonce\0", 12).try_into().unwrap();
        let key = aead::LessSafeKey::new(aead::UnboundKey::new(&aead::AES_128_GCM, &cek).unwrap());
        let mut record = body[21 + idlen..].to_vec();
        let plain = key.open_in_place(aead::Nonce::assume_unique_for_key(nonce), aead::Aad::empty(), &mut record).unwrap();
        assert_eq!(plain.last(), Some(&2));
        plain[..plain.len() - 1].to_vec()
    }

    #[test]
    fn push_payload_roundtrip() {
        let rng = SystemRandom::new();
        let ua_private = agreement::EphemeralPrivateKey::generate(&agreement::ECDH_P256, &rng).unwrap();
        let ua_public = ua_private.compute_public_key().unwrap().as_ref().to_vec();
        let auth: [u8; 16] = random();
        let payload = br#"{"title":"Claude","body":"finished","paneId":"p1","status":"done"}"#;
        let body = encrypt(payload, &ua_public, &auth).unwrap();
        assert_eq!(decrypt(&body, ua_private, &ua_public, &auth), payload);
    }

    #[test]
    fn refuses_payloads_too_big_for_one_record() {
        assert!(encrypt(&[b'x'; 4000], &[4; 65], &[0; 16]).is_none());
    }

    #[test]
    fn vapid_header_is_a_verifiable_es256_jwt() {
        let (vapid, pkcs8) = Vapid::generate();
        assert_eq!(vapid.public.len(), 65);
        assert_eq!(Vapid::from_pkcs8(&pkcs8).unwrap().public, vapid.public);
        let header = vapid.authorization("https://fcm.googleapis.com/fcm/send/abc", 1_000).unwrap();
        let (t, k) = header.strip_prefix("vapid t=").unwrap().split_once(", k=").unwrap();
        assert_eq!(unb64(k).unwrap(), vapid.public);
        let (signed, sig) = t.rsplit_once('.').unwrap();
        let claims: Value = serde_json::from_slice(&unb64(signed.split('.').nth(1).unwrap()).unwrap()).unwrap();
        assert_eq!(claims["aud"], "https://fcm.googleapis.com");
        assert_eq!(claims["exp"], 1_000 + 12 * 3600);
        signature::UnparsedPublicKey::new(&signature::ECDSA_P256_SHA256_FIXED, &vapid.public)
            .verify(signed.as_bytes(), &unb64(sig).unwrap())
            .unwrap();
    }

    #[test]
    fn origin_of_endpoints() {
        assert_eq!(origin("https://web.push.apple.com/QK0x"), Some("https://web.push.apple.com"));
        assert_eq!(origin("https://h:8443"), Some("https://h:8443"));
        assert_eq!(origin("nope"), None);
    }
}
