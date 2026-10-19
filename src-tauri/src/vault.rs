//! Plugin vaults: secrets a plugin keeps for the user, stored in the OS keychain.
//!
//! Each plugin gets its own keychain service (`dev.winfer.app.plugin.<id>`); the list of keys
//! and their descriptions (never the values) is kept in `<app data>/plugin-data/<id>/vault.json`.
//! Values leave Rust only to be shown to the user (`vault_reveal`), as environment variables
//! of a command whose output is redacted (`vault_exec`), or typed into a browser field
//! (`browser::browser_fill_secret`) — never in a reply to an agent.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::plugins;

/// What the vault knows about a secret besides its value; plugins choose the fields.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SecretInfo {
    pub key: String,
    /// Free-form metadata the plugin stores with the key (description, kind, …).
    #[serde(default)]
    pub meta: serde_json::Map<String, Value>,
    /// Unix milliseconds of the last value change.
    #[serde(default)]
    pub updated_at: u64,
}

#[derive(Default, Serialize, Deserialize)]
struct Index {
    #[serde(default)]
    secrets: BTreeMap<String, SecretInfo>,
}

/// Environment-variable style names, so a key can be used as `$KEY` in a command.
pub fn valid_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= 128
        && key.chars().next().is_some_and(|c| c.is_ascii_uppercase() || c == '_')
        && key.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
}

fn service(plugin: &str) -> String {
    format!("dev.winfer.app.plugin.{plugin}")
}

#[cfg(not(test))]
pub(crate) mod values {
    fn entry(service: &str, key: &str) -> Result<keyring::Entry, String> {
        keyring::Entry::new(service, key).map_err(|e| e.to_string())
    }
    pub fn get(service: &str, key: &str) -> Result<String, String> {
        entry(service, key)?.get_password().map_err(|e| e.to_string())
    }
    pub fn set(service: &str, key: &str, value: &str) -> Result<(), String> {
        entry(service, key)?.set_password(value).map_err(|e| e.to_string())
    }
    pub fn delete(service: &str, key: &str) -> Result<(), String> {
        match entry(service, key)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            #[cfg(target_os = "macos")]
            Err(e) => delete_foreign(service, key).map_err(|_| e.to_string()),
            #[cfg(not(target_os = "macos"))]
            Err(e) => Err(e.to_string()),
        }
    }

    /// An item another build of winfer created (each dev build or re-signed release is a
    /// different app to the keychain) refuses SecItemDelete with "Invalid attempt to change
    /// the owner of this item". The legacy SecKeychainItemDelete still removes it, the way
    /// Keychain Access and `security delete-generic-password` do, without reading the value.
    #[cfg(target_os = "macos")]
    fn delete_foreign(service: &str, key: &str) -> Result<(), ()> {
        use security_framework::item::{ItemClass, ItemSearchOptions, Reference, SearchResult};
        let found = ItemSearchOptions::new()
            .class(ItemClass::generic_password())
            .service(service)
            .account(key)
            .load_refs(true)
            .search()
            .map_err(|_| ())?;
        for result in found {
            if let SearchResult::Ref(Reference::KeychainItem(item)) = result {
                item.delete();
            }
        }
        // SecKeychainItemDelete's status isn't returned, so check the item is gone.
        let left = ItemSearchOptions::new()
            .class(ItemClass::generic_password())
            .service(service)
            .account(key)
            .search();
        if left.is_ok_and(|r| !r.is_empty()) { Err(()) } else { Ok(()) }
    }
}

/// Tests keep values in memory instead of the developer's keychain.
#[cfg(test)]
pub(crate) mod values {
    use std::collections::HashMap;
    use std::sync::Mutex;
    static VALUES: Mutex<Option<HashMap<(String, String), String>>> = Mutex::new(None);
    pub fn get(service: &str, key: &str) -> Result<String, String> {
        VALUES.lock().unwrap().get_or_insert_default().get(&(service.into(), key.into())).cloned().ok_or("no value".into())
    }
    pub fn set(service: &str, key: &str, value: &str) -> Result<(), String> {
        VALUES.lock().unwrap().get_or_insert_default().insert((service.into(), key.into()), value.into());
        Ok(())
    }
    pub fn delete(service: &str, key: &str) -> Result<(), String> {
        VALUES.lock().unwrap().get_or_insert_default().remove(&(service.into(), key.into()));
        Ok(())
    }
}

fn index_path(dir: &Path) -> PathBuf {
    dir.join("vault.json")
}

fn read_index(dir: &Path) -> Index {
    std::fs::read_to_string(index_path(dir)).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

fn write_index(dir: &Path, index: &Index) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let json = serde_json::to_string_pretty(index).map_err(|e| e.to_string())?;
    std::fs::write(index_path(dir), json).map_err(|e| e.to_string())
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

/// The vault folder of an enabled plugin.
fn vault_dir(plugin: &str) -> Result<PathBuf, String> {
    if !plugins::is_enabled(plugin) {
        return Err(format!("plugin “{plugin}” isn't enabled"));
    }
    plugins::data_dir(plugin)
}

fn list(dir: &Path) -> Vec<SecretInfo> {
    read_index(dir).secrets.into_values().collect()
}

/// Stores metadata, and the value when given (None keeps the current one).
fn set(dir: &Path, plugin: &str, key: &str, value: Option<&str>, meta: serde_json::Map<String, Value>) -> Result<SecretInfo, String> {
    if !valid_key(key) {
        return Err(format!("“{key}” isn't a valid name: use capital letters, digits and _ (like API_TOKEN)"));
    }
    let mut index = read_index(dir);
    let existing = index.secrets.get(key).cloned();
    if existing.is_none() && value.is_none() {
        return Err(format!("{key} needs a value"));
    }
    if let Some(value) = value {
        values::set(&service(plugin), key, value)?;
    }
    let info = SecretInfo {
        key: key.to_string(),
        meta,
        updated_at: if value.is_some() { now_ms() } else { existing.map_or(0, |e| e.updated_at) },
    };
    index.secrets.insert(key.to_string(), info.clone());
    write_index(dir, &index)?;
    Ok(info)
}

fn delete(dir: &Path, plugin: &str, key: &str) -> Result<(), String> {
    let mut index = read_index(dir);
    index.secrets.remove(key);
    values::delete(&service(plugin), key)?;
    write_index(dir, &index)
}

/// A stored value, for code in this crate that uses it on the user's behalf.
pub fn value(plugin: &str, key: &str) -> Result<String, String> {
    let dir = vault_dir(plugin)?;
    if !read_index(&dir).secrets.contains_key(key) {
        return Err(format!("no secret named {key}"));
    }
    values::get(&service(plugin), key).map_err(|e| format!("couldn't read {key} from the keychain: {e}"))
}

/// Replaces every occurrence of a secret (as is, base64- or URL-encoded) with `[secret KEY]`.
/// Catches accidental echoes; it can't stop a command that deliberately transforms a value.
pub fn redact(text: &str, secrets: &[(String, String)]) -> String {
    let mut out = text.to_string();
    let mut forms: Vec<(String, &str)> = Vec::new();
    for (key, value) in secrets {
        // Very short values would mangle unrelated output.
        if value.chars().count() < 4 {
            continue;
        }
        forms.push((value.clone(), key));
        forms.push((base64::engine::general_purpose::STANDARD.encode(value), key));
        forms.push((base64::engine::general_purpose::STANDARD.encode(value).trim_end_matches('=').to_string(), key));
        let encoded: String = value
            .bytes()
            .map(|b| if b.is_ascii_alphanumeric() || b"-._~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") })
            .collect();
        forms.push((encoded, key));
    }
    // Longest first, so a value isn't half-replaced by a shorter form of another.
    forms.sort_by_key(|(form, _)| std::cmp::Reverse(form.len()));
    for (form, key) in forms {
        if form.len() >= 4 {
            out = out.replace(&form, &format!("[secret {key}]"));
        }
    }
    out
}

#[tauri::command]
pub fn vault_list(plugin: String) -> Result<Vec<SecretInfo>, String> {
    Ok(list(&vault_dir(&plugin)?))
}

#[tauri::command]
pub fn vault_set(plugin: String, key: String, value: Option<String>, meta: Option<serde_json::Map<String, Value>>) -> Result<SecretInfo, String> {
    set(&vault_dir(&plugin)?, &plugin, &key, value.as_deref(), meta.unwrap_or_default())
}

#[tauri::command]
pub fn vault_delete(plugin: String, key: String) -> Result<(), String> {
    delete(&vault_dir(&plugin)?, &plugin, &key)
}

/// The value, for showing it to the user on the plugin's page.
#[tauri::command]
pub fn vault_reveal(plugin: String, key: String) -> Result<String, String> {
    value(&plugin, &key)
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecResult {
    /// Exit code; None if it was stopped at the timeout or killed by a signal.
    pub code: Option<i32>,
    pub timed_out: bool,
    /// stdout + stderr with the secrets redacted (the last 20 000 characters).
    pub output: String,
}

/// Runs a shell command with the named secrets as environment variables; the output comes
/// back with their values redacted.
#[tauri::command]
pub async fn vault_exec(plugin: String, command: String, cwd: String, keys: Vec<String>, timeout_sec: Option<u64>) -> Result<ExecResult, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let env = keys.iter().map(|k| value(&plugin, k).map(|v| (k.clone(), v))).collect::<Result<Vec<_>, _>>()?;
        let dir = if cwd.is_empty() { dirs::home_dir().unwrap_or_default() } else { PathBuf::from(cwd) };
        let timeout = Duration::from_secs(timeout_sec.unwrap_or(120).clamp(1, 1800));
        let run = crate::review::run_shell(&dir, &command, &env, timeout)?;
        Ok(ExecResult {
            code: run.status.and_then(|s| s.code()),
            timed_out: run.status.is_none(),
            output: crate::review::tail(redact(&run.output, &env), 20_000),
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn keys_look_like_env_vars() {
        assert!(valid_key("API_TOKEN"));
        assert!(valid_key("_X1"));
        assert!(!valid_key("api_token"));
        assert!(!valid_key("1TOKEN"));
        assert!(!valid_key("A-B"));
        assert!(!valid_key(""));
    }

    #[test]
    fn stores_metadata_on_disk_and_values_elsewhere() {
        let dir = std::env::temp_dir().join(format!("winfer-vault-{:x}", rand::random::<u64>()));
        let meta = json!({ "description": "Deploy token", "kind": "token" }).as_object().unwrap().clone();
        assert!(set(&dir, "p", "TOKEN", None, meta.clone()).unwrap_err().contains("needs a value"));
        set(&dir, "p", "TOKEN", Some("s3cret-value"), meta.clone()).unwrap();
        let on_disk = std::fs::read_to_string(index_path(&dir)).unwrap();
        assert!(on_disk.contains("Deploy token") && !on_disk.contains("s3cret-value"));
        assert_eq!(values::get(&service("p"), "TOKEN").unwrap(), "s3cret-value");

        // Editing only the description keeps the value and its timestamp.
        let before = list(&dir)[0].updated_at;
        let mut changed = meta.clone();
        changed.insert("description".into(), json!("Prod deploy token"));
        set(&dir, "p", "TOKEN", None, changed).unwrap();
        let after = &list(&dir)[0];
        assert_eq!((after.updated_at, after.meta["description"].as_str()), (before, Some("Prod deploy token")));
        assert_eq!(values::get(&service("p"), "TOKEN").unwrap(), "s3cret-value");

        delete(&dir, "p", "TOKEN").unwrap();
        assert!(list(&dir).is_empty());
        assert!(values::get(&service("p"), "TOKEN").is_err());
        assert!(set(&dir, "p", "bad name", Some("x"), Default::default()).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn redacts_plain_and_encoded_values() {
        let secrets = [("TOKEN".to_string(), "hunter2!x".to_string()), ("PIN".to_string(), "12".to_string())];
        let b64 = base64::engine::general_purpose::STANDARD.encode("hunter2!x");
        let text = format!("token=hunter2!x b64={b64} url=hunter2%21x pin=12");
        assert_eq!(redact(&text, &secrets), "token=[secret TOKEN] b64=[secret TOKEN] url=[secret TOKEN] pin=12");
    }

    #[cfg(unix)]
    #[test]
    fn exec_output_never_contains_the_value() {
        let secrets = [("API_KEY".to_string(), "sk-live-123456".to_string())];
        let run = crate::review::run_shell(&std::env::temp_dir(), "echo key is $API_KEY", &secrets, Duration::from_secs(10)).unwrap();
        assert_eq!(redact(&run.output, &secrets).trim(), "key is [secret API_KEY]");
    }
}
