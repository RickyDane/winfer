//! A local (or any self-hosted) OpenAI-compatible server for `winfer code`: vLLM, LM Studio,
//! Ollama, llama.cpp, mtplx… Settings → winfer code → Local models sets where it listens. The
//! agent loop is the ChatGPT backend's (Responses API with winfer's own tools), pointed here.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use serde_json::Value;

use crate::harness::config::{Config, LocalLlm};

/// The server's base URL (`http://host:port/v1`). `host` may carry its own scheme
/// (`https://box.lan`); a port of 0 leaves it out.
pub fn base_url(c: &LocalLlm) -> String {
    let host = c.host.trim().trim_end_matches('/');
    let host = if host.is_empty() { "127.0.0.1" } else { host };
    let (scheme, host) = match host.split_once("://") {
        Some((s, h)) => (s, h),
        None => ("http", host),
    };
    let port = if c.port == 0 { String::new() } else { format!(":{}", c.port) };
    let path = c.path.trim().trim_matches('/');
    let path = if path.is_empty() { String::new() } else { format!("/{path}") };
    format!("{scheme}://{host}{port}{path}")
}

fn settings() -> LocalLlm {
    Config::load().local
}

/// A model the server lists, with its context window when it says.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Model {
    pub id: String,
    pub context: Option<u64>,
}

/// Context windows seen in `/models`, so a session knows its model's without asking again.
static WINDOWS: Mutex<Option<HashMap<String, u64>>> = Mutex::new(None);

fn with_auth(req: ureq::RequestBuilder<ureq::typestate::WithoutBody>, key: &str) -> ureq::RequestBuilder<ureq::typestate::WithoutBody> {
    if key.trim().is_empty() { req } else { req.header("Authorization", &format!("Bearer {}", key.trim())) }
}

/// `GET /models` on the configured server (or `c`, to test settings before saving).
pub fn list(c: &LocalLlm) -> Result<Vec<Model>, String> {
    let url = format!("{}/models", base_url(c));
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(4)))
        .http_status_as_error(false)
        .build()
        .into();
    let mut res = with_auth(agent.get(&url), &c.api_key).call().map_err(|e| format!("couldn't reach {url}: {e}"))?;
    let status = res.status().as_u16();
    let json: Value = res.body_mut().read_json().unwrap_or(Value::Null);
    if !(200..300).contains(&status) {
        return Err(error_message(status, &json));
    }
    // OpenAI's `{ data: [{ id }] }`; Ollama's native `{ models: [{ name }] }` too.
    let list = json["data"].as_array().or_else(|| json["models"].as_array()).cloned().unwrap_or_default();
    let models: Vec<Model> = list
        .iter()
        .filter(|m| m["capability"].as_str().is_none_or(|c| c == "chat"))
        .filter_map(|m| {
            let id = m["id"].as_str().or_else(|| m["name"].as_str())?.to_string();
            let context = ["context_length", "max_context_length", "max_model_len", "context_window", "loaded_context_length"]
                .iter()
                .find_map(|k| m[*k].as_u64())
                .or_else(|| m["meta"]["n_ctx_train"].as_u64());
            Some(Model { id, context })
        })
        .collect();
    let mut w = WINDOWS.lock().unwrap();
    let map = w.get_or_insert_with(HashMap::new);
    for m in &models {
        if let Some(c) = m.context {
            map.insert(m.id.clone(), c);
        }
    }
    Ok(models)
}

/// The context window for `model`: the setting, else what the server lists, else 32k.
pub fn window(model: &str) -> u64 {
    let c = settings();
    if c.context_window > 0 {
        return c.context_window;
    }
    let known = WINDOWS.lock().unwrap().as_ref().and_then(|m| m.get(model).copied());
    known.or_else(|| list(&c).ok()?.into_iter().find(|m| m.id == model)?.context).unwrap_or(32_768)
}

/// Whether the user turned local models on.
pub fn enabled() -> bool {
    settings().enabled
}

/// The configured server's address, for messages.
pub fn address() -> String {
    base_url(&settings())
}

pub(crate) fn error_message(status: u16, body: &Value) -> String {
    let m = body["error"]["message"]
        .as_str()
        .or_else(|| body["error"].as_str())
        .or_else(|| body["detail"].as_str())
        .or_else(|| body["message"].as_str());
    match m {
        Some(m) => format!("Local model server {status}: {m}"),
        None => format!("Local model server request failed ({status})"),
    }
}

/// Opens a streaming Responses API call on the local server (no overall timeout: local models
/// can take minutes to prefill a long context).
pub(crate) fn open_stream(body: &Value) -> Result<impl std::io::Read + Send + use<>, String> {
    let c = settings();
    let url = format!("{}/responses", base_url(&c));
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(10)))
        .http_status_as_error(false)
        .build()
        .into();
    let mut req = agent.post(&url).header("Accept", "text/event-stream");
    if !c.api_key.trim().is_empty() {
        req = req.header("Authorization", &format!("Bearer {}", c.api_key.trim()));
    }
    let mut res = req.send_json(body).map_err(|e| format!("couldn't reach the local model server at {url}: {e}"))?;
    let status = res.status().as_u16();
    if !(200..300).contains(&status) {
        let json: Value = res.body_mut().read_json().unwrap_or(Value::Null);
        let mut msg = error_message(status, &json);
        if status == 404 {
            msg.push_str(&format!(" — {url} isn't there; winfer code needs a server with the OpenAI Responses API (/v1/responses)."));
        }
        return Err(msg);
    }
    Ok(res.into_body().into_reader())
}

/// Settings → winfer code → Local models → Test: lists the models at the given (unsaved) settings.
#[tauri::command]
pub async fn local_llm_test(settings: LocalLlm) -> Result<Vec<Model>, String> {
    tauri::async_runtime::spawn_blocking(move || list(&settings)).await.map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(host: &str, port: u16, path: &str) -> String {
        base_url(&LocalLlm { host: host.into(), port, path: path.into(), ..Default::default() })
    }

    #[test]
    fn builds_base_urls() {
        assert_eq!(at("127.0.0.1", 8000, "/v1"), "http://127.0.0.1:8000/v1");
        assert_eq!(at("", 8000, "v1/"), "http://127.0.0.1:8000/v1");
        assert_eq!(at("https://box.lan/", 0, "/v1"), "https://box.lan/v1");
        assert_eq!(at("localhost", 11434, ""), "http://localhost:11434");
    }
}
