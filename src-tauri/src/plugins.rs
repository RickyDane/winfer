//! Plugins: folders under `<app data>/plugins/<id>/` with a `plugin.json` manifest.
//!
//! A plugin can contribute agent CLIs (`providers`, same shape as resources/providers.json),
//! `recipes`, declarative `tools` (palette + voice actions that run Command Bus commands) and a
//! JavaScript `main` module the frontend runs. Rust only discovers, validates and reads them;
//! recipes and tools are passed through as JSON and validated by the frontend.
//!
//! The plugin store is the repo's `plugins/` folder on GitHub: every subfolder with a
//! `plugin.json` is a plugin, offered at whatever version its manifest says, and installing
//! downloads the folder's files. Plugins run with the app's privileges, so a plugin someone copies
//! into the folder starts disabled; installing from the store turns it on (after the user
//! agreed). The choices are kept in `<app data>/plugins.json`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{OnceLock, RwLock};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::providers::{self, Provider};

/// The GitHub repo, branch and folder the store lists; `WINFER_PLUGIN_STORE` (a local folder,
/// e.g. a checkout's `plugins/`) replaces them for development.
const STORE_REPO: &str = "RickyDane/winfer";
const STORE_BRANCH: &str = "main";
const STORE_FOLDER: &str = "plugins";
/// Marks a plugin folder installed from the store.
const STORE_MARKER: &str = ".store.json";
const MAX_FILE_BYTES: u64 = 5 * 1024 * 1024;
const MAX_FILES: usize = 500;

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
    /// Words the store's search matches.
    #[serde(default)]
    pub tags: Vec<String>,
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
    /// The version installed from the store; updates are offered when the store's differs.
    pub store_version: Option<String>,
    /// Why the plugin can't load (bad manifest, id clash, …).
    pub error: Option<String>,
}

/// One plugin the store offers, from its `plugin.json`.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct StoreEntry {
    pub id: String,
    pub name: String,
    /// Any text, or empty: it's only compared with the installed one.
    pub version: String,
    pub description: String,
    pub author: Option<String>,
    pub tags: Vec<String>,
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

/// Where the store's plugins come from.
#[derive(Debug, Clone)]
enum Store {
    /// `STORE_FOLDER` of `STORE_REPO` on GitHub.
    GitHub,
    /// A local folder laid out the same way.
    Local(PathBuf),
}

fn store() -> Store {
    match std::env::var("WINFER_PLUGIN_STORE") {
        Ok(dir) if !dir.is_empty() => Store::Local(PathBuf::from(dir)),
        _ => Store::GitHub,
    }
}

/// What the store holds: each plugin folder's files (paths inside it, `/`-separated), and for
/// GitHub the commit they're read from, so a listing and its downloads agree.
struct Listing {
    rev: String,
    plugins: BTreeMap<String, Vec<String>>,
}

fn http_get(url: &str) -> Result<ureq::http::Response<ureq::Body>, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(30)))
        .build()
        .into();
    // GitHub's API refuses requests without a User-Agent.
    agent
        .get(url)
        .header("User-Agent", "winfer")
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| match e {
            ureq::Error::StatusCode(403 | 429) => "GitHub's rate limit for the plugin store was reached; try again in a while".to_string(),
            e => format!("couldn't reach the plugin store: {e}"),
        })
}

fn github_json(path: &str) -> Result<Value, String> {
    http_get(&format!("https://api.github.com/repos/{STORE_REPO}/{path}"))?
        .body_mut()
        .read_json::<Value>()
        .map_err(|e| format!("the plugin store's answer is invalid: {e}"))
}

/// Groups `id/rest/of/path` paths by plugin folder; hidden files and folders are left out.
fn group_files(paths: impl IntoIterator<Item = String>) -> BTreeMap<String, Vec<String>> {
    let mut plugins: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for path in paths {
        let Some((id, rel)) = path.split_once('/') else { continue };
        if valid_id(id) && !rel.is_empty() && !path.split('/').any(|part| part.starts_with('.')) {
            plugins.entry(id.to_string()).or_default().push(rel.to_string());
        }
    }
    plugins.retain(|_, files| files.iter().any(|f| f == "plugin.json"));
    for files in plugins.values_mut() {
        files.sort();
    }
    plugins
}

fn local_files(dir: &Path, prefix: &str, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = if prefix.is_empty() { name } else { format!("{prefix}/{name}") };
        if entry.path().is_dir() {
            local_files(&entry.path(), &path, out);
        } else {
            out.push(path);
        }
    }
}

fn list_store(store: &Store) -> Result<Listing, String> {
    match store {
        Store::Local(dir) => {
            if !dir.is_dir() {
                return Err(format!("the plugin store folder {} doesn't exist", dir.display()));
            }
            let mut paths = Vec::new();
            local_files(dir, "", &mut paths);
            Ok(Listing { rev: String::new(), plugins: group_files(paths) })
        }
        Store::GitHub => {
            let commit = github_json(&format!("commits/{STORE_BRANCH}"))?;
            let rev = commit["sha"].as_str().ok_or("the plugin store's answer has no commit")?.to_string();
            let tree = commit["commit"]["tree"]["sha"].as_str().ok_or("the plugin store's answer has no tree")?;
            let tree = github_json(&format!("git/trees/{tree}?recursive=1"))?;
            let prefix = format!("{STORE_FOLDER}/");
            let paths = tree["tree"]
                .as_array()
                .ok_or("the plugin store's answer has no files")?
                .iter()
                .filter(|item| item["type"] == "blob")
                .filter_map(|item| item["path"].as_str()?.strip_prefix(&prefix).map(str::to_string));
            Ok(Listing { rev, plugins: group_files(paths) })
        }
    }
}

/// Percent-encodes a path's segments for a URL.
fn url_path(path: &str) -> String {
    path.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// One file of a store plugin.
fn fetch(store: &Store, rev: &str, id: &str, rel: &str) -> Result<Vec<u8>, String> {
    let path = format!("{id}/{rel}");
    match store {
        Store::Local(dir) => {
            let file = resolve_inside(dir, &path)?;
            if file.metadata().map_err(|e| format!("couldn't read {}: {e}", file.display()))?.len() > MAX_FILE_BYTES {
                return Err(format!("{path} is too big"));
            }
            std::fs::read(&file).map_err(|e| format!("couldn't read {}: {e}", file.display()))
        }
        Store::GitHub => http_get(&format!("https://raw.githubusercontent.com/{STORE_REPO}/{rev}/{STORE_FOLDER}/{}", url_path(&path)))?
            .body_mut()
            .with_config()
            .limit(MAX_FILE_BYTES)
            .read_to_vec()
            .map_err(|e| format!("couldn't download {path}: {e}")),
    }
}

/// A store plugin's manifest, checked like an installed one's.
fn store_manifest(store: &Store, rev: &str, id: &str) -> Result<Manifest, String> {
    let manifest: Manifest = serde_json::from_slice(&fetch(store, rev, id, "plugin.json")?).map_err(|e| format!("{id}: invalid plugin.json: {e}"))?;
    if manifest.id != id {
        return Err(format!("{id}: plugin.json says it's “{}”", manifest.id));
    }
    Ok(manifest)
}

/// Every plugin in the store; folders whose manifest is broken are left out.
fn read_store(store: &Store) -> Result<Vec<StoreEntry>, String> {
    let listing = list_store(store)?;
    let ids: Vec<&String> = listing.plugins.keys().collect();
    // The manifests are fetched in parallel; GitHub's raw files aren't rate limited.
    let manifests: Vec<Option<Manifest>> = std::thread::scope(|scope| {
        let handles: Vec<_> = ids.iter().map(|id| scope.spawn(|| store_manifest(store, &listing.rev, id).ok())).collect();
        handles.into_iter().map(|h| h.join().ok().flatten()).collect()
    });
    Ok(manifests
        .into_iter()
        .flatten()
        .map(|m| StoreEntry {
            version: m.version.unwrap_or_default(),
            description: m.description.unwrap_or_default(),
            id: m.id,
            name: m.name,
            author: m.author,
            tags: m.tags,
        })
        .collect())
}

/// Downloads a store plugin into `root/<id>`; replaces an older copy only once the new one is
/// complete.
fn install(root: &Path, store: &Store, id: &str) -> Result<(), String> {
    let listing = list_store(store)?;
    let files = listing.plugins.get(id).ok_or_else(|| format!("“{id}” isn't in the plugin store"))?;
    if files.len() > MAX_FILES {
        return Err(format!("{id} has more than {MAX_FILES} files"));
    }
    let staging = root.join(format!(".install-{id}"));
    let _ = std::fs::remove_dir_all(&staging);
    let result = (|| {
        for rel in files {
            let target = resolve_inside(&staging, rel)?;
            let bytes = fetch(store, &listing.rev, id, rel)?;
            std::fs::create_dir_all(target.parent().unwrap()).map_err(|e| e.to_string())?;
            std::fs::write(&target, bytes).map_err(|e| e.to_string())?;
        }
        // Checked under its final name, which the manifest's id must match.
        let manifest: Manifest = serde_json::from_slice(&std::fs::read(staging.join("plugin.json")).map_err(|e| e.to_string())?)
            .map_err(|e| format!("invalid plugin.json: {e}"))?;
        if manifest.id != id {
            return Err(format!("plugin.json says it's “{}”", manifest.id));
        }
        let version = manifest.version.unwrap_or_default();
        std::fs::write(staging.join(STORE_MARKER), serde_json::json!({ "version": version }).to_string()).map_err(|e| e.to_string())?;
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

/// A file's bytes, for plugins reading big project files (a knowledge graph) without the shell
/// round trips `vault.exec` needs, since that keeps only the end of a command's output. Plugins can
/// already read any file through `vault.exec`, so this grants nothing new.
#[tauri::command]
pub async fn plugins_read_file(path: String) -> Result<tauri::ipc::Response, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let path = crate::editors::expand_home(&path);
        if !path.is_absolute() {
            return Err(format!("{} isn't an absolute path", path.display()));
        }
        std::fs::read(&path).map(tauri::ipc::Response::new).map_err(|e| format!("can't read {}: {e}", path.display()))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Writes bytes to a file (creating its folder), for plugins saving what they make (a screen
/// recording). The body is the raw bytes; the `x-path` header is the base64 of the UTF-8 path,
/// absolute or starting with `~`. Plugins can already write any file through `vault.exec`.
#[tauri::command]
pub async fn plugins_write_file(request: tauri::ipc::Request<'_>) -> Result<String, String> {
    use base64::Engine;
    let tauri::ipc::InvokeBody::Raw(data) = request.body() else {
        return Err("expected the file's bytes as the request body".into());
    };
    let encoded = request.headers().get("x-path").and_then(|v| v.to_str().ok()).ok_or("missing x-path header")?;
    let bytes = base64::engine::general_purpose::STANDARD.decode(encoded).map_err(|e| format!("bad x-path header: {e}"))?;
    let path = crate::editors::expand_home(&String::from_utf8(bytes).map_err(|e| e.to_string())?);
    if !path.is_absolute() {
        return Err(format!("{} isn't an absolute path", path.display()));
    }
    let data = data.clone();
    tauri::async_runtime::spawn_blocking(move || {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("can't create {}: {e}", dir.display()))?;
        }
        std::fs::write(&path, data).map_err(|e| format!("can't write {}: {e}", path.display()))?;
        Ok(path.to_string_lossy().into_owned())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// The plugins the store offers.
#[tauri::command]
pub async fn plugins_catalog() -> Result<Vec<StoreEntry>, String> {
    tauri::async_runtime::spawn_blocking(|| read_store(&store())).await.map_err(|e| e.to_string())?
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
        install(&root, &store(), &id)?;
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

    /// A local store folder with one plugin (and the files that aren't plugins).
    fn local_store(name: &str, version: &str) -> Store {
        let base = temp_dir(name);
        let manifest = format!(r#"{{ "id": "demo", "name": "Demo", "version": "{version}", "main": "lib/index.js", "tags": ["x"] }}"#);
        for (path, text) in [("demo/plugin.json", manifest.as_str()), ("demo/lib/index.js", "export function activate() {}"), ("demo/.DS_Store", ""), ("README.md", ""), ("catalog.json", "{}"), ("notes/todo.txt", "")] {
            let file = base.join(path);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(&file, text).unwrap();
        }
        Store::Local(base)
    }

    #[test]
    fn lists_plugin_folders_without_a_catalog() {
        let store = local_store("store-list", "2025.1-beta");
        let listing = list_store(&store).unwrap();
        assert_eq!(listing.plugins.keys().collect::<Vec<_>>(), ["demo"]);
        assert_eq!(listing.plugins["demo"], ["lib/index.js", "plugin.json"]);
        let entries = read_store(&store).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!((entries[0].version.as_str(), entries[0].tags.as_slice()), ("2025.1-beta", ["x".to_string()].as_slice()));
    }

    #[test]
    fn groups_github_paths_by_plugin() {
        let paths = ["a/plugin.json", "a/src/x.js", "a/.hidden/y", "b/readme.md", "Bad Id/plugin.json", "catalog.json"].map(String::from);
        let plugins = group_files(paths);
        assert_eq!(plugins.len(), 1);
        assert_eq!(plugins["a"], ["plugin.json", "src/x.js"]);
        assert_eq!(url_path("a b/ü.js"), "a%20b/%C3%BC.js");
    }

    #[test]
    fn installs_and_updates_from_the_store() {
        let root = temp_dir("store-root");
        install(&root, &local_store("store-ok", "1.0.0"), "demo").unwrap();
        let plugins = scan(&root, &BTreeSet::new());
        assert_eq!(plugins.len(), 1);
        assert!(plugins[0].from_store && plugins[0].error.is_none());
        assert_eq!(plugins[0].store_version.as_deref(), Some("1.0.0"));
        assert!(root.join("demo/lib/index.js").exists() && !root.join("demo/.DS_Store").exists());
        // An update to any other version replaces the folder and leaves no staging copy behind.
        install(&root, &local_store("store-ok", "0.9"), "demo").unwrap();
        assert_eq!(scan(&root, &BTreeSet::new())[0].store_version.as_deref(), Some("0.9"));
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 1);
        assert!(install(&root, &local_store("store-ok", "1"), "missing").unwrap_err().contains("isn't in the plugin store"));
    }

    #[test]
    fn every_store_plugin_installs_and_loads() {
        let store = Store::Local(PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../plugins")));
        let root = temp_dir("repo-store");
        let entries = read_store(&store).unwrap();
        assert!(entries.iter().any(|p| p.id == "secrets"));
        for entry in &entries {
            install(&root, &store, &entry.id).unwrap();
        }
        for plugin in scan(&root, &BTreeSet::new()) {
            assert!(plugin.error.is_none(), "{}: {:?}", plugin.id, plugin.error);
        }
    }
}
