//! This instance's identity and its paired instances.
//!
//! `connect.json` in the data dir keeps the settings and identity (nothing secret); the pairing
//! keys are in the [`SecretStore`] under `peers`.

use crate::crypto::{b64, random, unb64_n};
use crate::keys::PairKeys;
use crate::pairing::{Kind, Me, Paired};
use crate::store::SecretStore;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const DEFAULT_RELAY_URL: &str = "https://winfer.danehub.com";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    #[serde(default = "yes")]
    pub enabled: bool,
    /// Advertise on the LAN and accept pair requests from it.
    #[serde(default = "yes")]
    pub discoverable: bool,
    #[serde(default)]
    pub relay_url: Option<String>,
    /// 16 random bytes, base64url; made on first start.
    #[serde(default)]
    pub instance_id: String,
    /// Shown to peers; the computer's name unless renamed.
    #[serde(default)]
    pub name: Option<String>,
    /// Whether the store has peers; spares reading it (and a keychain prompt) when not.
    #[serde(default)]
    pub has_peers: bool,
}

fn yes() -> bool {
    true
}

impl Default for Config {
    fn default() -> Self {
        Config { enabled: true, discoverable: true, relay_url: None, instance_id: String::new(), name: None, has_peers: false }
    }
}

impl Config {
    pub fn load(path: &Path) -> Config {
        let mut config: Config = std::fs::read_to_string(path).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        if unb64_n::<16>(&config.instance_id).is_none() {
            config.instance_id = b64(&random::<16>());
            let _ = config.save(path);
        }
        config
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        // Whole or not at all: a reader never sees a half-written file (and would make a new id).
        let tmp = path.with_extension(format!("{}-{:?}.tmp", std::process::id(), std::thread::current().id()).replace(['(', ')'], ""));
        std::fs::write(&tmp, serde_json::to_string_pretty(self).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, path).map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            e.to_string()
        })
    }

    pub fn relay_url(&self) -> String {
        self.relay_url.clone().filter(|u| !u.trim().is_empty()).unwrap_or_else(|| DEFAULT_RELAY_URL.into())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Peer {
    pub peer_id: String,
    pub name: String,
    #[serde(default)]
    pub kind: Kind,
    /// base64url.
    pub pair_key: String,
    pub created_at: u64,
    #[serde(default)]
    pub last_seen: Option<u64>,
    /// Notify about this peer's sessions.
    #[serde(default = "yes")]
    pub notify: bool,
    /// Where it last answered on the LAN (`ip:port`).
    #[serde(default)]
    pub last_addr: Option<String>,
    /// The relay both sides meet on: the pair link's. None: this instance's relay setting.
    #[serde(default)]
    pub relay_url: Option<String>,
}

impl Peer {
    pub fn from_paired(p: &Paired, now: u64) -> Peer {
        Peer { peer_id: p.peer_id.clone(), name: p.name.clone(), kind: p.kind, pair_key: b64(&p.pair_key), created_at: now, last_seen: Some(now), notify: true, last_addr: None, relay_url: None }
    }

    pub fn keys(&self) -> Option<PairKeys> {
        unb64_n::<32>(&self.pair_key).map(|k| PairKeys::derive(&k))
    }
}

const PEERS: &str = "peers";

/// Settings, identity and peers on disk.
pub struct Persist {
    pub config_path: PathBuf,
    pub store: SecretStore,
}

impl Persist {
    pub fn new(data_dir: &Path, store: SecretStore) -> Persist {
        Persist { config_path: data_dir.join("connect.json"), store }
    }

    pub fn config(&self) -> Config {
        Config::load(&self.config_path)
    }

    pub fn me(config: &Config, kind: Kind) -> Me {
        Me { id: config.instance_id.clone(), name: config.name.clone().unwrap_or_else(computer_name), kind }
    }

    pub fn peers(&self, config: &Config) -> Vec<Peer> {
        if !config.has_peers {
            return Vec::new();
        }
        self.store.get(PEERS).and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    }

    pub fn save_peers(&self, config: &mut Config, peers: &[Peer]) -> Result<(), String> {
        if peers.is_empty() {
            self.store.delete(PEERS)?;
        } else {
            self.store.set(PEERS, &serde_json::to_string(peers).map_err(|e| e.to_string())?)?;
        }
        if config.has_peers != !peers.is_empty() {
            config.has_peers = !peers.is_empty();
            config.save(&self.config_path)?;
        }
        Ok(())
    }
}

/// The computer's name (macOS ComputerName, else the host name).
pub fn computer_name() -> String {
    let run = |cmd: &str, args: &[&str]| {
        std::process::Command::new(cmd).args(args).output().ok().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).filter(|s| !s.is_empty())
    };
    #[cfg(target_os = "macos")]
    let name = run("scutil", &["--get", "ComputerName"]).or_else(|| run("hostname", &["-s"]));
    #[cfg(not(target_os = "macos"))]
    let name = std::env::var("COMPUTERNAME").ok().or_else(|| run("hostname", &[]));
    name.unwrap_or_else(|| "winfer".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_gets_an_id_once_and_peers_persist() {
        let dir = std::env::temp_dir().join(format!("wc-peers-{}", std::process::id()));
        let persist = Persist::new(&dir, SecretStore::memory());
        let mut config = persist.config();
        assert_eq!(config.instance_id.len(), 22);
        assert_eq!(persist.config().instance_id, config.instance_id, "stable across loads");
        assert!(persist.peers(&config).is_empty());
        let peer = Peer::from_paired(&Paired { peer_id: "P".repeat(22), name: "box".into(), kind: Kind::Server, pair_key: [4; 32] }, 7);
        persist.save_peers(&mut config, std::slice::from_ref(&peer)).unwrap();
        assert!(persist.config().has_peers);
        assert_eq!(persist.peers(&config), vec![peer.clone()]);
        assert_eq!(peer.keys().unwrap().pair_key, [4; 32]);
        let _ = std::fs::remove_dir_all(dir);
    }
}
