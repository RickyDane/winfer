//! Update checks on the channel the user picked in settings. The updater plugin's own `check`
//! only knows the endpoints in tauri.conf.json, so this checks against the channel's feed and
//! hands back the same metadata, letting the frontend install it through the plugin as usual.

use serde::Serialize;
use tauri::{Manager, Runtime, Webview};
use tauri_plugin_updater::UpdaterExt;

/// latest.json on the latest (stable) release, and on the release Nebula builds replace
/// (see .github/workflows/release.yml).
const STABLE_FEED: &str = "https://github.com/RickyDane/winfer/releases/latest/download/latest.json";
const NEBULA_FEED: &str = "https://github.com/RickyDane/winfer/releases/download/nebula-channel/latest.json";

/// What `@tauri-apps/plugin-updater`'s `Update` class is constructed from.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateMetadata {
    rid: tauri::ResourceId,
    current_version: String,
    version: String,
    date: Option<String>,
    body: Option<String>,
    raw_json: serde_json::Value,
}

/// The newer release on `channel` ("stable" or "nebula"), if any. Never a downgrade: a
/// Nebula build switched to stable waits for the stable release of its version.
#[tauri::command]
pub async fn app_update_check<R: Runtime>(webview: Webview<R>, channel: String) -> Result<Option<UpdateMetadata>, String> {
    let feed = match channel.as_str() {
        "stable" => STABLE_FEED,
        "nebula" => NEBULA_FEED,
        other => return Err(format!("unknown update channel {other}")),
    };
    let url = feed.parse().map_err(|e| format!("{e}"))?;
    let updater = webview.updater_builder().endpoints(vec![url]).map_err(|e| e.to_string())?.build().map_err(|e| e.to_string())?;
    let Some(update) = updater.check().await.map_err(|e| e.to_string())? else { return Ok(None) };
    Ok(Some(UpdateMetadata {
        current_version: update.current_version.clone(),
        version: update.version.clone(),
        date: update.date.map(|d| d.to_string()),
        body: update.body.clone(),
        raw_json: update.raw_json.clone(),
        rid: webview.resources_table().add(update),
    }))
}
