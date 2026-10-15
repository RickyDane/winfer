//! Plugins: folders under `<app data>/plugins/<id>/` with a `plugin.json` manifest.
//!
//! A plugin can contribute agent CLIs (`providers`, same shape as resources/providers.json),
//! `recipes`, declarative `tools` (palette + voice actions that run Command Bus commands) and a
//! JavaScript `main` module the frontend runs. Rust only discovers, validates and reads them;
//! recipes and tools are passed through as JSON and validated by the frontend.
//!
//! The plugin store is the repo's `plugins/` folder: `catalog.json` lists each plugin's files
//! with their SHA-256, and installing downloads and checks them. Plugins run with the app's
//! privileges, so a plugin someone copies into the folder starts disabled; installing from the
//! store turns it on (after the user agreed). The choices are kept in `<app data>/plugins.json`.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::{OnceLock, RwLock};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::providers::{self, Provider};

/// Where the store's catalog and files are; `WINFER_PLUGIN_STORE` (a URL or a local folder,
/// e.g. a checkout's `plugins/`) overrides it for development.
const STORE: &str = "https://raw.githubusercontent.com/RickyDane/winfer/main/plugins";
/// Marks a plugin folder installed from the store.
const STORE_MARKER: &str = ".store.json";
const MAX_FILE_BYTES: u64 = 5 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub author: Option<String>,
    /// JavaScript module (ES module, relative to the plugin folder) exporting `activate(api)`.
    #[serde(default)]
    pub main: Option<String>,
    #[serde(default)]
    pub providers: Vec<Provider>,
    #[serde(default)]
    pub recipes: Vec<Value>,
    #[serde(default)]
    pub tools: Vec<Value>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginInfo {
    /// The folder name; equals `manifest.id` for a valid plugin.
    pub id: String,
    pub dir: String,
    pub manifest: Option<Manifest>,
    pub enabled: bool,
    /// Installed from the store (so it can be updated and uninstalled from there).
    pub from_store: bool,
    /// The catalog version installed from the store, which may differ from `manifest.version`;
    /// updates are offered when the catalog's version differs from this one.
    pub store_version: Option<String>,
    /// Why the plugin can't load (bad manifest, id clash, …).
    pub error: Option<String>,
}

/// One plugin in the store's `catalog.json`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CatalogEntry {
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    pub files: Vec<CatalogFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CatalogFile {
    pub path: String,
    pub sha256: String,
}

#[derive(Debug, Deserialize)]
struct Catalog {
    plugins: Vec<CatalogEntry>,
}

struct Registry {
    root: PathBuf,
    data: PathBuf,
    settings: PathBuf,
    plugins: Vec<PluginInfo>,
}

static REGISTRY: OnceLock<RwLock<Registry>> = OnceLock::new();

/// Sets the plugin folder (created if missing) and loads what's in it.
pub fn init(data_dir: &Path) {
    let root = data_dir.join("plugins");
    let _ = std::fs::create_dir_all(&root);
    let mut registry = Registry {
        root,
        data: data_dir.join("plugin-data"),
        settings: data_dir.join("plugins.json"),
        plugins: Vec::new(),
    };
    registry.plugins = scan(&registry.root, &read_settings(&registry.settings).enabled);
    let _ = REGISTRY.set(RwLock::new(registry));
}

#[derive(Default, Serialize, Deserialize)]
struct Settings {
    #[serde(default)]
    enabled: BTreeSet<String>,
}

fn read_settings(path: &Path) -> Settings {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str::<Settings>(&s).ok())
        .unwrap_or_default()
}

fn write_enabled(path: &Path, id: &str, enabled: bool) -> Result<(), String> {
    let mut settings = read_settings(path);
    if enabled {
        settings.enabled.insert(id.to_string());
    } else {
        settings.enabled.remove(id);
    }
    let json = serde_json::to_string_pretty(&settings).map_err(|e| e.to_string())?;
    std::fs::write(path, json).map_err(|e| e.to_string())
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id.chars().next().is_some_and(|c| c.is_ascii_alphanumeric())
        && id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_' || c == '.')
}

/// Reads and checks one plugin folder's manifest.
fn load_manifest(dir: &Path) -> Result<Manifest, String> {
    let text = std::fs::read_to_string(dir.join("plugin.json")).map_err(|e| format!("can't read plugin.json: {e}"))?;
    let manifest: Manifest = serde_json::from_str(&text).map_err(|e| format!("invalid plugin.json: {e}"))?;
    let folder = dir.file_name().and_then(|n| n.to_str()).unwrap_or_default();
    if !valid_id(&manifest.id) {
        return Err(format!("invalid id “{}” (use lowercase letters, digits, - _ .)", manifest.id));
    }
    if manifest.id != folder {
        return Err(format!("id “{}” doesn't match its folder name “{folder}”", manifest.id));
    }
    if let Some(main) = &manifest.main {
        resolve_inside(dir, main)?;
    }
    for p in &manifest.providers {
        if !valid_id(&p.id) {
            return Err(format!("invalid provider id “{}”", p.id));
        }
    }
    Ok(manifest)
}

/// A path inside the plugin folder; `..` and absolute paths are refused.
fn resolve_inside(dir: &Path, relative: &str) -> Result<PathBuf, String> {
    let rel = Path::new(relative);
    let escapes = rel.is_absolute()
        || rel
            .components()
            .any(|c| !matches!(c, std::path::Component::Normal(_) | std::path::Component::CurDir));
    if escapes {
        return Err(format!("“{relative}” must be a path inside the plugin folder"));
    }
    Ok(dir.join(rel))
}

/// Every plugin folder, sorted by id. Providers whose id clashes with a built-in agent or an
/// earlier plugin's provider make that plugin fail to load rather than shadow the other one.
fn scan(root: &Path, enabled: &BTreeSet<String>) -> Vec<PluginInfo> {
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(root)
        .map(|entries| entries.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect())
        .unwrap_or_default();
    dirs.sort();
    let mut provider_ids: BTreeSet<String> = providers::builtin().iter().map(|p| p.id.clone()).collect();
    dirs.into_iter()
        .filter_map(|dir| {
            let id = dir.file_name()?.to_str()?.to_string();
            if id.starts_with('.') {
                return None;
            }
            let (manifest, mut error) = match load_manifest(&dir) {
                Ok(m) => (Some(m), None),
                Err(e) => (None, Some(e)),
            };
            if let Some(m) = &manifest {
                if let Some(clash) = m.providers.iter().find(|p| provider_ids.contains(&p.id)) {
                    error = Some(format!("provider id “{}” is already taken", clash.id));
                } else {
                    provider_ids.extend(m.providers.iter().map(|p| p.id.clone()));
                }
            }
            let marker = dir.join(STORE_MARKER);
            Some(PluginInfo {
                enabled: enabled.contains(&id),
                from_store: marker.exists(),
                store_version: store_version(&marker),
                dir: dir.to_string_lossy().into_owned(),
                id,
                manifest,
                error,
            })
        })
        .collect()
}

/// The `version` a store install recorded in its marker file.
fn store_version(marker: &Path) -> Option<String> {
    let text = std::fs::read_to_string(marker).ok()?;
    let value: Value = serde_json::from_str(&text).ok()?;
    value.get("version")?.as_str().map(str::to_string)
}

fn registry() -> Result<&'static RwLock<Registry>, String> {
    REGISTRY.get().ok_or_else(|| "plugins aren't initialised".to_string())
}

/// Whether a plugin is installed, working and turned on.
pub fn is_enabled(id: &str) -> bool {
    REGISTRY
        .get()
        .is_some_and(|r| r.read().unwrap().plugins.iter().any(|p| p.id == id && p.enabled && p.error.is_none()))
}

/// Where a plugin keeps its own files (`<app data>/plugin-data/<id>`).
pub fn data_dir(id: &str) -> Result<PathBuf, String> {
    if !valid_id(id) {
        return Err("invalid plugin id".into());
    }
    Ok(registry()?.read().unwrap().data.join(id))
}

/// Agent CLIs contributed by enabled, working plugins.
pub fn providers() -> Vec<Provider> {
    let Some(registry) = REGISTRY.get() else {
        return Vec::new();
    };
    registry
        .read()
        .unwrap()
        .plugins
        .iter()
        .filter(|p| p.enabled && p.error.is_none())
        .filter_map(|p| p.manifest.as_ref())
        .flat_map(|m| m.providers.iter().cloned())
        .collect()
}

// ---------------------------------------------------------------------------------------------
// Store

fn store_base() -> String {
    std::env::var("WINFER_PLUGIN_STORE").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| STORE.into())
}

/// A file from the store: fetched over HTTPS, or read from a local folder.
fn fetch(base: &str, path: &str) -> Result<Vec<u8>, String> {
    let base = base.trim_end_matches('/');
    if base.starts_with("https://") || base.starts_with("http://") {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(std::time::Duration::from_secs(30)))
            .build()
            .into();
        let mut res = agent.get(&format!("{base}/{path}")).call().map_err(|e| format!("couldn't download {path}: {e}"))?;
        res.body_mut()
            .with_config()
            .limit(MAX_FILE_BYTES)
            .read_to_vec()
            .map_err(|e| format!("couldn't download {path}: {e}"))
    } else {
        let file = Path::new(base).join(path);
        if file.metadata().map_err(|e| format!("couldn't read {}: {e}", file.display()))?.len() > MAX_FILE_BYTES {
            return Err(format!("{path} is too big"));
        }
        std::fs::read(&file).map_err(|e| format!("couldn't read {}: {e}", file.display()))
    }
}

fn read_catalog(base: &str) -> Result<Vec<CatalogEntry>, String> {
    let bytes = fetch(base, "catalog.json")?;
    let catalog: Catalog = serde_json::from_slice(&bytes).map_err(|e| format!("the plugin catalog is invalid: {e}"))?;
    Ok(catalog.plugins.into_iter().filter(|p| valid_id(&p.id)).collect())
}

/// Downloads a store plugin into `root/<id>`, checking every file's hash; replaces an older
/// copy only once the new one is complete.
fn install(root: &Path, base: &str, id: &str) -> Result<(), String> {
    let entry = read_catalog(base)?.into_iter().find(|p| p.id == id).ok_or_else(|| format!("“{id}” isn't in the plugin store"))?;
    if !entry.files.iter().any(|f| f.path == "plugin.json") {
        return Err(format!("{id} has no plugin.json"));
    }
    let staging = root.join(format!(".install-{id}"));
    let _ = std::fs::remove_dir_all(&staging);
    let result = (|| {
        for file in &entry.files {
            let target = resolve_inside(&staging, &file.path)?;
            let bytes = fetch(base, &format!("{id}/{}", file.path))?;
            if hex::encode(Sha256::digest(&bytes)) != file.sha256.to_ascii_lowercase() {
                return Err(format!("{} doesn't match the catalog (changed or corrupted download)", file.path));
            }
            std::fs::create_dir_all(target.parent().unwrap()).map_err(|e| e.to_string())?;
            std::fs::write(&target, bytes).map_err(|e| e.to_string())?;
        }
        std::fs::write(staging.join(STORE_MARKER), serde_json::json!({ "version": entry.version }).to_string()).map_err(|e| e.to_string())?;
        // Checked under its final name, which the manifest's id must match.
        let manifest: Manifest = serde_json::from_slice(&std::fs::read(staging.join("plugin.json")).map_err(|e| e.to_string())?)
            .map_err(|e| format!("invalid plugin.json: {e}"))?;
        if manifest.id != id {
            return Err(format!("plugin.json says it's “{}”", manifest.id));
        }
        let target = root.join(id);
        if target.exists() {
            std::fs::remove_dir_all(&target).map_err(|e| format!("couldn't replace the old version: {e}"))?;
        }
        std::fs::rename(&staging, &target).map_err(|e| e.to_string())
    })();
    if result.is_err() {
        let _ = std::fs::remove_dir_all(&staging);
    }
    result
}

// ---------------------------------------------------------------------------------------------
// Commands

/// Rescans the plugin folder and lists every plugin found.
#[tauri::command]
pub fn plugins_list() -> Result<Vec<PluginInfo>, String> {
    let mut registry = registry()?.write().unwrap();
    let enabled = read_settings(&registry.settings).enabled;
    registry.plugins = scan(&registry.root, &enabled);
    Ok(registry.plugins.clone())
}

#[tauri::command]
pub fn plugins_set_enabled(id: String, enabled: bool) -> Result<(), String> {
    let mut registry = registry()?.write().unwrap();
    write_enabled(&registry.settings, &id, enabled)?;
    if let Some(plugin) = registry.plugins.iter_mut().find(|p| p.id == id) {
        plugin.enabled = enabled;
    }
    Ok(())
}

#[tauri::command]
pub fn plugins_dir() -> Result<String, String> {
    Ok(registry()?.read().unwrap().root.to_string_lossy().into_owned())
}

/// The source of an enabled plugin's `main` module.
#[tauri::command]
pub fn plugins_source(id: String) -> Result<String, String> {
    let registry = registry()?.read().unwrap();
    let plugin = registry.plugins.iter().find(|p| p.id == id).ok_or("unknown plugin")?;
    if !plugin.enabled {
        return Err("plugin is disabled".into());
    }
    let main = plugin.manifest.as_ref().and_then(|m| m.main.as_deref()).ok_or("plugin has no main module")?;
    let path = resolve_inside(Path::new(&plugin.dir), main)?;
    std::fs::read_to_string(&path).map_err(|e| format!("can't read {}: {e}", path.display()))
}

/// The plugins the store offers.
#[tauri::command]
pub async fn plugins_catalog() -> Result<Vec<CatalogEntry>, String> {
    tauri::async_runtime::spawn_blocking(|| read_catalog(&store_base())).await.map_err(|e| e.to_string())?
}

/// Installs (or updates) a plugin from the store and turns it on.
#[tauri::command]
pub async fn plugins_install(id: String) -> Result<(), String> {
    if !valid_id(&id) {
        return Err("invalid plugin id".into());
    }
    tauri::async_runtime::spawn_blocking(move || {
        let (root, settings) = {
            let registry = registry()?.read().unwrap();
            (registry.root.clone(), registry.settings.clone())
        };
        install(&root, &store_base(), &id)?;
        write_enabled(&settings, &id, true)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Deletes a plugin's folder. Its stored data (and secrets) stay, for a reinstall.
#[tauri::command]
pub fn plugins_uninstall(id: String) -> Result<(), String> {
    if !valid_id(&id) {
        return Err("invalid plugin id".into());
    }
    let registry = registry()?.read().unwrap();
    write_enabled(&registry.settings, &id, false)?;
    let dir = registry.root.join(&id);
    if dir.exists() {
        std::fs::remove_dir_all(&dir).map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("winfer-plugins-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_plugin(root: &Path, folder: &str, manifest: &str) {
        std::fs::create_dir_all(root.join(folder)).unwrap();
        std::fs::write(root.join(folder).join("plugin.json"), manifest).unwrap();
    }

    #[test]
    fn scans_valid_and_broken_plugins() {
        let root = temp_dir("scan");
        write_plugin(
            &root,
            "hello",
            r#"{ "id": "hello", "name": "Hello", "main": "index.js",
                 "providers": [{ "id": "hello-cli", "name": "Hello CLI", "command": "hello" }],
                 "tools": [{ "id": "say_hi" }] }"#,
        );
        write_plugin(&root, "mismatch", r#"{ "id": "other", "name": "Other" }"#);
        write_plugin(&root, "escape", r#"{ "id": "escape", "name": "Escape", "main": "../evil.js" }"#);
        write_plugin(&root, "clash", r#"{ "id": "clash", "name": "Clash", "providers": [{ "id": "claude", "name": "Fake" }] }"#);
        write_plugin(&root, "broken", "{ not json");
        std::fs::create_dir_all(root.join(".hidden")).unwrap();

        let plugins = scan(&root, &BTreeSet::from(["hello".to_string()]));
        let by_id = |id: &str| plugins.iter().find(|p| p.id == id).unwrap();
        assert_eq!(plugins.len(), 5);
        assert!(by_id("hello").enabled && by_id("hello").error.is_none());
        assert_eq!(by_id("hello").manifest.as_ref().unwrap().providers[0].id, "hello-cli");
        assert!(by_id("mismatch").error.as_ref().unwrap().contains("folder name"));
        assert!(by_id("escape").error.as_ref().unwrap().contains("inside the plugin folder"));
        assert!(by_id("clash").error.as_ref().unwrap().contains("already taken"));
        assert!(by_id("broken").error.as_ref().unwrap().contains("invalid plugin.json"));
        assert!(!by_id("broken").enabled);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn checks_ids_and_paths() {
        assert!(valid_id("my-plugin_2.0"));
        assert!(!valid_id("My Plugin"));
        assert!(!valid_id("../x"));
        assert!(!valid_id(""));
        let dir = Path::new("/plugins/x");
        assert!(resolve_inside(dir, "dist/index.js").is_ok());
        assert!(resolve_inside(dir, "./index.js").is_ok());
        assert!(resolve_inside(dir, "/etc/passwd").is_err());
        assert!(resolve_inside(dir, "a/../../b.js").is_err());
    }

    /// A local store folder with one plugin; `tamper` changes a file after hashing it.
    fn store(name: &str, tamper: bool) -> PathBuf {
        let base = temp_dir(name);
        let files = [("plugin.json", r#"{ "id": "demo", "name": "Demo", "main": "lib/index.js" }"#), ("lib/index.js", "export function activate() {}")];
        let mut entries = Vec::new();
        for (path, text) in files {
            let file = base.join("demo").join(path);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(&file, text).unwrap();
            entries.push(serde_json::json!({ "path": path, "sha256": hex::encode(Sha256::digest(text.as_bytes())) }));
        }
        if tamper {
            std::fs::write(base.join("demo/lib/index.js"), "alert('evil')").unwrap();
        }
        // The catalog's version needn't match plugin.json's (which has none here).
        let catalog = serde_json::json!({ "plugins": [{ "id": "demo", "name": "Demo", "version": "1.0.0", "files": entries }] });
        std::fs::write(base.join("catalog.json"), catalog.to_string()).unwrap();
        base
    }

    #[test]
    fn installs_and_updates_from_the_store() {
        let base = store("store-ok", false);
        let root = temp_dir("store-root");
        assert_eq!(read_catalog(base.to_str().unwrap()).unwrap()[0].files.len(), 2);
        install(&root, base.to_str().unwrap(), "demo").unwrap();
        let plugins = scan(&root, &BTreeSet::new());
        assert_eq!(plugins.len(), 1);
        assert!(plugins[0].from_store && plugins[0].error.is_none());
        assert_eq!(plugins[0].store_version.as_deref(), Some("1.0.0"));
        assert_eq!(plugins[0].manifest.as_ref().unwrap().version, None);
        assert!(root.join("demo/lib/index.js").exists());
        // Reinstalling (an update) replaces the folder and leaves no staging copy behind.
        install(&root, base.to_str().unwrap(), "demo").unwrap();
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 1);
        assert!(install(&root, base.to_str().unwrap(), "missing").unwrap_err().contains("isn't in the plugin store"));
    }

    #[test]
    fn refuses_files_that_dont_match_the_catalog() {
        let base = store("store-bad", true);
        let root = temp_dir("store-bad-root");
        let err = install(&root, base.to_str().unwrap(), "demo").unwrap_err();
        assert!(err.contains("doesn't match the catalog"), "{err}");
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 0, "nothing is left behind");
    }

    #[test]
    fn every_store_plugin_installs_and_loads() {
        let base = concat!(env!("CARGO_MANIFEST_DIR"), "/../plugins");
        let root = temp_dir("repo-store");
        let catalog = read_catalog(base).unwrap();
        assert!(catalog.iter().any(|p| p.id == "secrets"));
        for entry in &catalog {
            install(&root, base, &entry.id).unwrap();
        }
        for plugin in scan(&root, &BTreeSet::new()) {
            assert!(plugin.error.is_none(), "{}: {:?}", plugin.id, plugin.error);
        }
    }
}
