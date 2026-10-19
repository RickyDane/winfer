//! Paired phones and winfer's push identity, kept in the OS keychain: a device's pairing key
//! reads everything the desktop sends it, so it isn't written to disk in the clear.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::vault::values;

const SERVICE: &str = "dev.winfer.app.mobile";
const DEVICES: &str = "mobile-devices";
const VAPID: &str = "mobile-vapid";

/// How long a pairing link (QR code) can be used.
pub const PAIR_TTL_MS: u64 = 10 * 60 * 1000;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Device {
    pub device_id: String,
    pub room_id: String,
    /// The pairing key, base64url.
    pub key: String,
    pub name: String,
    pub created_at: u64,
    #[serde(default)]
    pub last_seen: Option<u64>,
    /// The phone said hello: the link has been used.
    #[serde(default)]
    pub paired: bool,
    #[serde(default)]
    pub push_subscription: Option<Value>,
}

impl Device {
    /// A pairing link nobody used in time.
    pub fn expired(&self, now: u64) -> bool {
        !self.paired && now.saturating_sub(self.created_at) > PAIR_TTL_MS
    }
}

pub fn load() -> Vec<Device> {
    values::get(SERVICE, DEVICES).ok().and_then(|json| serde_json::from_str(&json).ok()).unwrap_or_default()
}

pub fn save(devices: &[Device]) -> Result<(), String> {
    if devices.is_empty() {
        return values::delete(SERVICE, DEVICES);
    }
    values::set(SERVICE, DEVICES, &serde_json::to_string(devices).map_err(|e| e.to_string())?)
}

/// The VAPID key (PKCS#8), made on first use.
pub fn vapid() -> super::push::Vapid {
    if let Some(vapid) = values::get(SERVICE, VAPID).ok().and_then(|k| super::crypto::unb64(&k)).and_then(|k| super::push::Vapid::from_pkcs8(&k)) {
        return vapid;
    }
    let (vapid, pkcs8) = super::push::Vapid::generate();
    if let Err(err) = values::set(SERVICE, VAPID, &super::crypto::b64(&pkcs8)) {
        eprintln!("winfer: couldn't keep the push key: {err}");
    }
    vapid
}

/// A phone's name from its browser's user agent.
pub fn name_from_ua(ua: &str) -> String {
    let ua = ua.to_lowercase();
    if ua.contains("iphone") {
        "iPhone"
    } else if ua.contains("ipad") {
        "iPad"
    } else if ua.contains("android") {
        "Android phone"
    } else if ua.contains("mac os") {
        "Mac browser"
    } else {
        "Phone"
    }
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn devices_roundtrip_and_expire() {
        let device = Device {
            device_id: "d".into(),
            room_id: "r".into(),
            key: "k".into(),
            name: "Phone".into(),
            created_at: 1_000,
            last_seen: None,
            paired: false,
            push_subscription: None,
        };
        save(std::slice::from_ref(&device)).unwrap();
        assert_eq!(load(), [device.clone()]);
        assert!(!device.expired(1_000 + PAIR_TTL_MS));
        assert!(device.expired(1_001 + PAIR_TTL_MS));
        assert!(!Device { paired: true, ..device }.expired(u64::MAX));
        save(&[]).unwrap();
        assert!(load().is_empty());
    }

    #[test]
    fn names_phones() {
        assert_eq!(name_from_ua("Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X)"), "iPhone");
        assert_eq!(name_from_ua("Mozilla/5.0 (Linux; Android 15; Pixel 9)"), "Android phone");
    }
}
