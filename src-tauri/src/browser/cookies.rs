//! Each workspace's cookies, kept across restarts. A workspace's browser context lives in memory
//! only (like an incognito window), so its cookies are saved here, encrypted with a key kept in
//! the OS keychain, and put back when the context is created again.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use base64::Engine;
use ring::aead::{AES_256_GCM, Aad, LessSafeKey, NONCE_LEN, Nonce, UnboundKey};
use ring::rand::{SecureRandom, SystemRandom};
use serde_json::{Map, Value};

const SERVICE: &str = "dev.winfer.app.browser";
const KEY_NAME: &str = "cookie-key";
const FORMAT: &str = "winfer-cookies-v1:";

pub type Key = [u8; 32];

/// The encryption key, made on first use. None if the keychain can't be used: cookies are then
/// not kept (never written in the clear).
pub fn key() -> Option<Key> {
    static KEY: OnceLock<Option<Key>> = OnceLock::new();
    *KEY.get_or_init(|| {
        let b64 = base64::engine::general_purpose::STANDARD;
        let stored = crate::vault::values::get(SERVICE, KEY_NAME).ok().and_then(|s| b64.decode(s.trim()).ok());
        if let Some(key) = stored.and_then(|k| Key::try_from(k).ok()) {
            return Some(key);
        }
        let mut key = [0u8; 32];
        SystemRandom::new().fill(&mut key).ok()?;
        match crate::vault::values::set(SERVICE, KEY_NAME, &b64.encode(key)) {
            Ok(()) => Some(key),
            Err(err) => {
                super::log::warning!("no keychain for the browser's cookies, they won't be kept: {err}");
                None
            }
        }
    })
}

pub fn path(dir: &Path, workspace: &str) -> PathBuf {
    dir.join(format!("{}.cookies", super::sanitize(workspace)))
}

/// The saved cookies, as `Storage.setCookies` takes them (expired ones dropped).
pub fn load(path: &Path, key: &Key) -> Vec<Value> {
    let Ok(text) = std::fs::read_to_string(path) else { return Vec::new() };
    let Some(plain) = decrypt(&text, key) else {
        super::log::warning!("couldn't read the saved cookies in {}", path.display());
        return Vec::new();
    };
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64());
    serde_json::from_slice::<Vec<Value>>(&plain)
        .unwrap_or_default()
        .into_iter()
        .filter(|c| c["expires"].as_f64().is_none_or(|at| at > now))
        .collect()
}

/// Saves `cookies` (from `Storage.getCookies`); returns what it wrote, to skip unchanged saves.
pub fn store(path: &Path, key: &Key, cookies: &[Value]) -> Result<Vec<Value>, String> {
    let params: Vec<Value> = cookies.iter().filter_map(param).collect();
    let plain = serde_json::to_vec(&params).map_err(|e| e.to_string())?;
    let text = encrypt(&plain, key)?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
    }
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())?;
    Ok(params)
}

/// A `Storage.getCookies` cookie as a `Storage.setCookies` parameter (None for ones that can't be
/// set again).
pub fn param(cookie: &Value) -> Option<Value> {
    if cookie["partitionKeyOpaque"] == true || cookie["name"].as_str().is_none() {
        return None;
    }
    let mut out = Map::new();
    for field in ["name", "value", "domain", "path", "secure", "httpOnly", "sameSite", "priority", "sourceScheme", "sourcePort", "partitionKey"] {
        if !cookie[field].is_null() {
            out.insert(field.into(), cookie[field].clone());
        }
    }
    // Session cookies come back as session cookies (they hold most logins).
    if cookie["session"] != true
        && let Some(expires) = cookie["expires"].as_f64().filter(|e| *e > 0.0)
    {
        out.insert("expires".into(), expires.into());
    }
    Some(Value::Object(out))
}

fn cipher(key: &Key) -> Option<LessSafeKey> {
    UnboundKey::new(&AES_256_GCM, key).ok().map(LessSafeKey::new)
}

fn encrypt(plain: &[u8], key: &Key) -> Result<String, String> {
    let cipher = cipher(key).ok_or("bad key")?;
    let mut nonce = [0u8; NONCE_LEN];
    SystemRandom::new().fill(&mut nonce).map_err(|_| "no randomness")?;
    let mut data = plain.to_vec();
    cipher
        .seal_in_place_append_tag(Nonce::assume_unique_for_key(nonce), Aad::empty(), &mut data)
        .map_err(|_| "encryption failed")?;
    let mut out = nonce.to_vec();
    out.extend(data);
    Ok(format!("{FORMAT}{}", base64::engine::general_purpose::STANDARD.encode(out)))
}

fn decrypt(text: &str, key: &Key) -> Option<Vec<u8>> {
    let bytes = base64::engine::general_purpose::STANDARD.decode(text.trim().strip_prefix(FORMAT)?).ok()?;
    if bytes.len() < NONCE_LEN {
        return None;
    }
    let (nonce, data) = bytes.split_at(NONCE_LEN);
    let mut data = data.to_vec();
    let nonce = Nonce::assume_unique_for_key(nonce.try_into().ok()?);
    let plain = cipher(key)?.open_in_place(nonce, Aad::empty(), &mut data).ok()?;
    Some(plain.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn keeps_cookies_encrypted_and_drops_expired_ones() {
        let dir = std::env::temp_dir().join(format!("winfer-cookies-{:x}", rand::random::<u32>()));
        let file = path(&dir, "ws/1");
        let key = [7u8; 32];
        let cookies = [
            json!({ "name": "sid", "value": "secret-session", "domain": "example.com", "path": "/", "expires": -1, "size": 20, "httpOnly": true, "secure": true, "session": true, "sameSite": "Lax", "priority": "Medium", "sourceScheme": "Secure", "sourcePort": 443 }),
            json!({ "name": "old", "value": "x", "domain": "example.com", "path": "/", "expires": 1000.0, "session": false }),
            json!({ "name": "pref", "value": "dark", "domain": ".example.com", "path": "/", "expires": 4102444800.0, "session": false }),
            json!({ "name": "opaque", "value": "x", "domain": "a.com", "path": "/", "partitionKeyOpaque": true }),
        ];
        store(&file, &key, &cookies).unwrap();
        let raw = std::fs::read_to_string(&file).unwrap();
        assert!(raw.starts_with(FORMAT) && !raw.contains("secret-session"), "stored encrypted");

        let loaded = load(&file, &key);
        let names: Vec<_> = loaded.iter().map(|c| c["name"].as_str().unwrap()).collect();
        assert_eq!(names, ["sid", "pref"]);
        assert!(loaded[0].get("expires").is_none(), "session cookies stay session cookies");
        assert!(loaded[0].get("size").is_none() && loaded[0].get("session").is_none(), "only setCookies fields");
        assert_eq!(loaded[0]["httpOnly"], true);
        assert_eq!(loaded[1]["expires"], 4102444800.0);

        assert!(load(&file, &[8u8; 32]).is_empty(), "another key can't read it");
        let _ = std::fs::remove_dir_all(dir);
    }
}
