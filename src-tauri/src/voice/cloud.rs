//! Cloudflare Workers AI Clef decision model (System One format). Credentials live in the
//! OS keychain; `CF_ACCOUNT_ID` / `CF_API_TOKEN` env vars work for development.

use std::time::Duration;

use serde_json::Value;

const SERVICE: &str = "dev.winfer.app";
const ACCOUNT_KEY: &str = "cloudflare-account-id";
const TOKEN_KEY: &str = "cloudflare-api-token";

pub struct Credentials {
    pub account_id: String,
    pub token: String,
}

fn entry(key: &str) -> Result<keyring::Entry, String> {
    keyring::Entry::new(SERVICE, key).map_err(|e| e.to_string())
}

pub fn load() -> Option<Credentials> {
    let from_env = std::env::var("CF_ACCOUNT_ID")
        .ok()
        .zip(std::env::var("CF_API_TOKEN").ok());
    if let Some((account_id, token)) = from_env {
        return Some(Credentials { account_id, token });
    }
    let account_id = entry(ACCOUNT_KEY).ok()?.get_password().ok()?;
    let token = entry(TOKEN_KEY).ok()?.get_password().ok()?;
    Some(Credentials { account_id, token })
}

pub fn save(account_id: &str, token: &str) -> Result<(), String> {
    entry(ACCOUNT_KEY)?
        .set_password(account_id.trim())
        .map_err(|e| e.to_string())?;
    entry(TOKEN_KEY)?
        .set_password(token.trim())
        .map_err(|e| e.to_string())
}

pub fn clear() -> Result<(), String> {
    for key in [ACCOUNT_KEY, TOKEN_KEY] {
        let _ = entry(key)?.delete_credential();
    }
    Ok(())
}

/// Sends a System One request (`{ model, state, questions }`) and returns `result`.
pub fn decide(creds: &Credentials, model: &str, body: &Value) -> Result<Value, String> {
    let url = format!(
        "https://api.cloudflare.com/client/v4/accounts/{}/ai/run/@cf/cloudflare/{model}",
        creds.account_id
    );
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(8)))
        .http_status_as_error(false)
        .build()
        .into();
    let mut response = agent
        .post(&url)
        .header("Authorization", &format!("Bearer {}", creds.token))
        .send_json(body)
        .map_err(|e| format!("couldn't reach Cloudflare: {e}"))?;
    let status = response.status();
    let json: Value = response
        .body_mut()
        .read_json()
        .map_err(|e| format!("bad response from Cloudflare ({status}): {e}"))?;
    if !status.is_success() || json["success"] == false {
        let message = json["errors"][0]["message"]
            .as_str()
            .unwrap_or("request failed");
        return Err(format!("Cloudflare {status}: {message}"));
    }
    Ok(json["result"].clone())
}
