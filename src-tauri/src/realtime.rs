//! Live voice: a spoken conversation with OpenAI's realtime model that controls winfer.
//! The webview talks to OpenAI over WebRTC; this side mints its short-lived client secret
//! (ChatGPT sign-in first, the user's OpenAI API key as fallback) and runs the agent-browser
//! MCP server for the voice model's browser tools, since voice isn't a pane with its own.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde::Serialize;
use serde_json::{Value, json};
use tauri::{AppHandle, Manager};

use crate::harness::mcp_client::Server;

const CLIENT_SECRETS_URL: &str = "https://api.openai.com/v1/realtime/client_secrets";
const KEY_SERVICE: &str = "dev.winfer.app.voice";
const KEY_NAME: &str = "OPENAI_API_KEY";
pub const DEFAULT_MODEL: &str = "gpt-realtime";

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientSecret {
    value: String,
    expires_at: u64,
    auth: &'static str,
}

fn secret_body(model: &str, voice: &str) -> Value {
    json!({
        "expires_after": { "anchor": "created_at", "seconds": 600 },
        "session": {
            "type": "realtime",
            "model": if model.is_empty() { DEFAULT_MODEL } else { model },
            "audio": { "output": { "voice": if voice.is_empty() { "marin" } else { voice } } },
        },
    })
}

/// The secret from a client_secrets reply (`value` at the top, or nested in older replies).
fn parse_secret(body: &Value) -> Option<(String, u64)> {
    let value = body["value"].as_str().or_else(|| body["client_secret"]["value"].as_str())?;
    let expires = body["expires_at"].as_u64().or_else(|| body["client_secret"]["expires_at"].as_u64()).unwrap_or(0);
    Some((value.to_string(), expires))
}

/// (status, body) of one client_secrets request with `token`.
fn request(token: &str, body: &Value) -> Result<(u16, Value), String> {
    let mut res = crate::chatgpt::http()
        .post(CLIENT_SECRETS_URL)
        .header("Authorization", &format!("Bearer {token}"))
        .send_json(body)
        .map_err(|e| format!("couldn't reach OpenAI: {e}"))?;
    let status = res.status().as_u16();
    Ok((status, res.body_mut().read_json().unwrap_or(Value::Null)))
}

fn api_key() -> Option<String> {
    crate::vault::values::get(KEY_SERVICE, KEY_NAME).ok().filter(|k| !k.trim().is_empty())
}

/// Tries each (auth, token) in order; a rejected ChatGPT token falls through to the API key.
fn mint(attempts: &[(&'static str, Result<String, String>)], body: &Value, send: impl Fn(&str, &Value) -> Result<(u16, Value), String>) -> Result<ClientSecret, String> {
    let mut errors = Vec::new();
    for (auth, token) in attempts {
        let token = match token {
            Ok(t) => t,
            Err(e) => {
                errors.push(e.clone());
                continue;
            }
        };
        let (status, reply) = send(token, body)?;
        if (200..300).contains(&status) {
            let (value, expires_at) = parse_secret(&reply).ok_or("OpenAI sent no client secret")?;
            return Ok(ClientSecret { value, expires_at, auth });
        }
        errors.push(crate::chatgpt::api_error(status, &reply).1);
    }
    if !attempts.iter().any(|(auth, t)| *auth == "apikey" && t.is_ok()) {
        errors.push("Add an OpenAI API key in Settings → Voice to use live voice.".into());
    }
    Err(errors.join(" "))
}

#[tauri::command]
/// `auth: "apikey"` skips the ChatGPT sign-in (live voice retries with the key when a
/// ChatGPT-backed session fails).
pub async fn realtime_client_secret(model: String, voice: String, auth: Option<String>) -> Result<ClientSecret, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let mut attempts = Vec::new();
        if crate::chatgpt::signed_in() && auth.as_deref() != Some("apikey") {
            attempts.push(("chatgpt", crate::chatgpt::access_token()));
        }
        attempts.push(("apikey", api_key().ok_or_else(String::new)));
        let attempts: Vec<_> = attempts.into_iter().filter(|(_, t)| !matches!(t, Err(e) if e.is_empty())).collect();
        mint(&attempts, &secret_body(&model, &voice), request)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub fn realtime_set_api_key(key: Option<String>) -> Result<(), String> {
    match key.map(|k| k.trim().to_string()).filter(|k| !k.is_empty()) {
        Some(k) => crate::vault::values::set(KEY_SERVICE, KEY_NAME, &k),
        None => crate::vault::values::delete(KEY_SERVICE, KEY_NAME),
    }
}

#[tauri::command]
pub fn realtime_has_api_key() -> bool {
    api_key().is_some()
}

/// The agent-browser MCP server per workspace, started on first use.
#[derive(Default)]
pub struct LiveBrowser {
    servers: Mutex<HashMap<String, Arc<Mutex<Server>>>>,
}

impl LiveBrowser {
    fn server(&self, app: &AppHandle, workspace_id: &str) -> Result<Arc<Mutex<Server>>, String> {
        if let Some(s) = self.servers.lock().unwrap().get(workspace_id) {
            return Ok(s.clone());
        }
        let url = app
            .state::<crate::browser::BrowserManager>()
            .cdp_url(app, workspace_id)
            .ok_or("The agent browser isn't available (no Chrome or Chromium installed).")?;
        let spec = crate::browser::playwright_server(&url);
        let spec = json!({ "command": spec.command, "args": spec.args });
        let cwd = dirs::home_dir().unwrap_or_else(std::env::temp_dir);
        let server = Arc::new(Mutex::new(Server::start("winfer-browser", &spec, &cwd)?));
        self.servers.lock().unwrap().insert(workspace_id.to_string(), server.clone());
        Ok(server)
    }

    pub fn shutdown(&self) {
        self.servers.lock().unwrap().clear();
    }
}

#[tauri::command]
pub async fn realtime_browser_tools(app: AppHandle, workspace_id: String) -> Result<Vec<Value>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let server = app.state::<LiveBrowser>().server(&app, &workspace_id)?;
        let tools = server.lock().unwrap().tools.clone();
        Ok(tools)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[derive(Serialize)]
pub struct ToolReply {
    ok: bool,
    text: String,
}

#[tauri::command]
pub async fn realtime_browser_call(app: AppHandle, workspace_id: String, name: String, args: Value) -> Result<ToolReply, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let live = app.state::<LiveBrowser>();
        let server = live.server(&app, &workspace_id)?;
        let (ok, text) = server.lock().unwrap().call_tool(&name, args);
        // A dead server (browser restarted, npx crashed) is replaced on the next call.
        if !ok && text.contains("didn't answer") {
            live.servers.lock().unwrap().remove(&workspace_id);
        }
        Ok(ToolReply { ok, text })
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Owner of live voice's background tasks; their exit notices arrive as `bg://note` for it.
pub const VOICE_OWNER: &str = "voice";

/// The bg tool's schema, so voice describes it the same way agents see it.
#[tauri::command]
pub fn realtime_bg_tool() -> Value {
    crate::harness::bg::mcp_tool(false)
}

/// Runs a `bg` call for live voice in `cwd` (the workspace folder).
#[tauri::command]
pub async fn realtime_bg(args: Value, cwd: String) -> Result<ToolReply, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let call = crate::mcp::Call { id: String::new(), tool: "bg".into(), args, pane_id: VOICE_OWNER.into(), cwd: Some(cwd) };
        let reply = crate::mcp::bg_call(&call);
        Ok(ToolReply { ok: reply.ok, text: reply.text })
    })
    .await
    .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn body_defaults_model_and_voice() {
        let b = secret_body("", "");
        assert_eq!(b["session"]["model"], DEFAULT_MODEL);
        assert_eq!(b["session"]["type"], "realtime");
        assert_eq!(b["session"]["audio"]["output"]["voice"], "marin");
        assert_eq!(secret_body("gpt-x", "ash")["session"]["model"], "gpt-x");
    }

    #[test]
    fn parses_both_reply_shapes() {
        assert_eq!(parse_secret(&json!({ "value": "ek_1", "expires_at": 5 })), Some(("ek_1".into(), 5)));
        assert_eq!(parse_secret(&json!({ "client_secret": { "value": "ek_2", "expires_at": 7 } })), Some(("ek_2".into(), 7)));
        assert_eq!(parse_secret(&json!({})), None);
    }

    #[test]
    fn rejected_chatgpt_token_falls_back_to_api_key() {
        let attempts = [("chatgpt", Ok("chat".to_string())), ("apikey", Ok("sk".to_string()))];
        let send = |token: &str, _: &Value| {
            Ok(if token == "chat" { (403, json!({ "error": { "message": "nope" } })) } else { (200, json!({ "value": "ek_9", "expires_at": 1 })) })
        };
        let s = mint(&attempts, &secret_body("", ""), send).unwrap();
        assert_eq!((s.value.as_str(), s.auth), ("ek_9", "apikey"));
    }

    #[test]
    fn no_key_asks_for_one() {
        let attempts = [("chatgpt", Ok("chat".to_string()))];
        let err = mint(&attempts, &secret_body("", ""), |_, _| Ok((401, json!({ "error": { "message": "bad" } })))).err().unwrap();
        assert!(err.contains("OpenAI 401: bad") && err.contains("Settings → Voice"), "{err}");
    }
}
