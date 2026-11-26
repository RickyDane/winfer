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
        let tmp = path.with_extension(format!("{}-{:?}.tmp", std::process::id(), std::thread::current().id()).replace(['(', ')'], ""));
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

    /// Changes the file under `<file>.lock`, which winfer's vault takes too (both keep values in
    /// it with `WINFER_VAULT_FILE`): two read-modify-writes at once dropped each other's values.
    fn change(path: &Path, f: impl FnOnce(&mut HashMap<String, String>)) -> Result<(), String> {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let lock = std::fs::OpenOptions::new().create(true).write(true).truncate(false).open(path.with_extension("lock"));
        if let Ok(lock) = &lock {
            let _ = lock.lock();
        }
        let mut map = Self::read(path);
        f(&mut map);
        Self::write(path, &map)
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
            SecretStore::File { path, service } => Self::change(path, |map| {
                map.insert(format!("{service}/{key}"), value.into());
            }),
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
            SecretStore::File { path, service } => Self::change(path, |map| {
                map.remove(&format!("{service}/{key}"));
            }),
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

    #[test]
    fn writers_sharing_a_file_keep_each_others_values() {
        // On a server, Connect and the rest of winfer keep their values in one vault file.
        let dir = std::env::temp_dir().join(format!("wc-store-race-{}", std::process::id()));
        let path = dir.join("vault.json");
        let threads: Vec<_> = (0..4)
            .map(|t| {
                let path = path.clone();
                std::thread::spawn(move || {
                    let store = SecretStore::file(&path, &format!("svc{t}"));
                    for i in 0..25 {
                        store.set(&format!("k{i}"), "v").unwrap();
                    }
                })
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
        let map = SecretStore::read(&path);
        assert_eq!(map.len(), 100, "values were lost");
        let _ = std::fs::remove_dir_all(dir);
    }
}
