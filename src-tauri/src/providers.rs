//! Agent CLI presets. Data-driven so new CLIs or changed flags don't need code changes.

use serde::{Deserialize, Serialize};
use std::sync::OnceLock;

use crate::env::login_env;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Provider {
    pub id: String,
    pub name: String,
    /// `None` means the user's login shell.
    pub command: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub resume_args: Vec<String>,
    #[serde(default)]
    pub aliases: Vec<String>,
    /// Reports status through hooks (see hooks.rs) instead of output heuristics.
    #[serde(default)]
    pub status_hooks: bool,
    /// Accepts an initial prompt as its last argument (otherwise it's typed in after start).
    #[serde(default)]
    pub prompt_arg: bool,
    /// Keys that accept a pending permission prompt (default: Enter).
    #[serde(default)]
    pub approve_input: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderInfo {
    #[serde(flatten)]
    pub provider: Provider,
    pub installed: bool,
}

static PROVIDERS: OnceLock<Vec<Provider>> = OnceLock::new();

pub fn all() -> &'static [Provider] {
    PROVIDERS.get_or_init(|| {
        serde_json::from_str(include_str!("../resources/providers.json"))
            .expect("resources/providers.json is valid")
    })
}

pub fn find(id: &str) -> Option<&'static Provider> {
    all().iter().find(|p| p.id == id)
}

pub fn is_installed(provider: &Provider) -> bool {
    let Some(command) = &provider.command else {
        return true;
    };
    let path = login_env().get("PATH").cloned().unwrap_or_default();
    let cwd = dirs::home_dir().unwrap_or_default();
    which::which_in(command, Some(path), cwd).is_ok()
}

#[tauri::command]
pub fn providers_list() -> Vec<ProviderInfo> {
    all()
        .iter()
        .map(|p| ProviderInfo {
            installed: is_installed(p),
            provider: p.clone(),
        })
        .collect()
}
