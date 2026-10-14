//! Plugins: folders under `<app data>/plugins/<id>/` with a `plugin.json` manifest.
//!
//! A plugin can contribute agent CLIs (`providers`, same shape as resources/providers.json),
//! `recipes`, declarative `tools` (palette + voice actions that run Command Bus commands) and a
//! JavaScript `main` module the frontend runs. Rust only discovers, validates and reads them;
//! recipes and tools are passed through as JSON and validated by the frontend.
//!
//! Plugins run with the app's privileges, so every plugin starts disabled until the user turns it
//! on in Settings. Enabled ids are kept in `<app data>/plugins.json`.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::{OnceLock, RwLock};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::providers::{self, Provider};

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
    /// Why the plugin can't load (bad manifest, id clash, …).
    pub error: Option<String>,
}

struct Registry {
    root: PathBuf,
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
        settings: data_dir.join("plugins.json"),
        plugins: Vec::new(),
    };
    registry.plugins = scan(&registry.root, &enabled_ids(&registry.settings));
    let _ = REGISTRY.set(RwLock::new(registry));
}

#[derive(Default, Serialize, Deserialize)]
struct Settings {
    #[serde(default)]
    enabled: BTreeSet<String>,
}

fn enabled_ids(path: &Path) -> BTreeSet<String> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str::<Settings>(&s).ok())
        .unwrap_or_default()
        .enabled
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
            Some(PluginInfo {
                enabled: enabled.contains(&id),
                dir: dir.to_string_lossy().into_owned(),
                id,
                manifest,
                error,
            })
        })
        .collect()
}

fn registry() -> Result<&'static RwLock<Registry>, String> {
    REGISTRY.get().ok_or_else(|| "plugins aren't initialised".to_string())
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

/// Rescans the plugin folder and lists every plugin found.
#[tauri::command]
pub fn plugins_list() -> Result<Vec<PluginInfo>, String> {
    let mut registry = registry()?.write().unwrap();
    let enabled = enabled_ids(&registry.settings);
    registry.plugins = scan(&registry.root, &enabled);
    Ok(registry.plugins.clone())
}

#[tauri::command]
pub fn plugins_set_enabled(id: String, enabled: bool) -> Result<(), String> {
    let mut registry = registry()?.write().unwrap();
    let mut ids = enabled_ids(&registry.settings);
    if enabled {
        ids.insert(id.clone());
    } else {
        ids.remove(&id);
    }
    let json = serde_json::to_string_pretty(&Settings { enabled: ids }).map_err(|e| e.to_string())?;
    std::fs::write(&registry.settings, json).map_err(|e| e.to_string())?;
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
}
