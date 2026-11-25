//! Where pairing keys live: the OS keychain on desktops, a `0600` JSON file on servers (Linux
//! servers rarely run a Secret Service), memory in tests. `WINFER_VAULT_FILE` switches the
//! keychain to that file, as it does for the rest of winfer (throwaway and second instances).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub enum SecretStore {
    Keychain { service: String },
    File { path: PathBuf, service: String },
    Memory(Mutex<HashMap<String, String>>),
}

impl SecretStore {
    pub fn keychain(service: &str) -> SecretStore {
        match std::env::var_os("WINFER_VAULT_FILE") {
            Some(path) => SecretStore::File { path: path.into(), service: service.into() },
            None => SecretStore::Keychain { service: service.into() },
        }
    }

    pub fn file(path: &Path, service: &str) -> SecretStore {
        SecretStore::File { path: path.into(), service: service.into() }
    }

    pub fn memory() -> SecretStore {
        SecretStore::Memory(Mutex::default())
    }

    fn read(path: &Path) -> HashMap<String, String> {
        std::fs::read_to_string(path).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    }

    fn write(path: &Path, map: &HashMap<String, String>) -> Result<(), String> {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let json = serde_json::to_string(map).map_err(|e| e.to_string())?;
        let tmp = path.with_extension("tmp");
        {
            use std::io::Write;
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create(true).truncate(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(&tmp).map_err(|e| e.to_string())?;
            file.write_all(json.as_bytes()).map_err(|e| e.to_string())?;
        }
        std::fs::rename(&tmp, path).map_err(|e| e.to_string())
    }

    pub fn get(&self, key: &str) -> Option<String> {
        match self {
            SecretStore::Keychain { service } => keyring::Entry::new(service, key).ok()?.get_password().ok(),
            SecretStore::File { path, service } => Self::read(path).get(&format!("{service}/{key}")).cloned(),
            SecretStore::Memory(map) => map.lock().unwrap().get(key).cloned(),
        }
    }

    pub fn set(&self, key: &str, value: &str) -> Result<(), String> {
        match self {
            SecretStore::Keychain { service } => keyring::Entry::new(service, key).and_then(|e| e.set_password(value)).map_err(|e| e.to_string()),
            SecretStore::File { path, service } => {
                let mut map = Self::read(path);
                map.insert(format!("{service}/{key}"), value.into());
                Self::write(path, &map)
            }
            SecretStore::Memory(map) => {
                map.lock().unwrap().insert(key.into(), value.into());
                Ok(())
            }
        }
    }

    pub fn delete(&self, key: &str) -> Result<(), String> {
        match self {
            SecretStore::Keychain { service } => match keyring::Entry::new(service, key).and_then(|e| e.delete_credential()) {
                Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
                Err(e) => Err(e.to_string()),
            },
            SecretStore::File { path, service } => {
                let mut map = Self::read(path);
                map.remove(&format!("{service}/{key}"));
                Self::write(path, &map)
            }
            SecretStore::Memory(map) => {
                map.lock().unwrap().remove(key);
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_store_round_trips_privately() {
        let dir = std::env::temp_dir().join(format!("wc-store-{}", std::process::id()));
        let path = dir.join("secrets.json");
        let store = SecretStore::file(&path, "svc");
        store.set("peers", "[1]").unwrap();
        assert_eq!(store.get("peers").as_deref(), Some("[1]"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        }
        store.delete("peers").unwrap();
        assert_eq!(store.get("peers"), None);
        let _ = std::fs::remove_dir_all(dir);
    }
}
