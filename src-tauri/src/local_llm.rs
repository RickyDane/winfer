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
    let path = c.path.trim().trim_matches('/');
    let path = if path.is_empty() { String::new() } else { format!("/{path}") };
    format!("{}{path}", origin(c))
}

/// `scheme://host:port`, without the API path.
fn origin(c: &LocalLlm) -> String {
    let host = c.host.trim().trim_end_matches('/');
    let host = if host.is_empty() { "127.0.0.1" } else { host };
    let (scheme, host) = match host.split_once("://") {
        Some((s, h)) => (s, h),
        None => ("http", host),
    };
    let port = if c.port == 0 { String::new() } else { format!(":{}", c.port) };
    format!("{scheme}://{host}{port}")
}

/// The effort levels a model's chat template takes, per server and model: `Some` once the server
/// answered (None inside: it doesn't show the template), absent until it has.
static TEMPLATE_LEVELS: Mutex<Option<HashMap<String, Option<Vec<String>>>>> = Mutex::new(None);

/// The reasoning levels `model`'s chat template takes, "none" first when it can switch thinking
/// off, read from the template the server shows (llama.cpp's `/props`; Unsloth Studio does the
/// same). None when the server doesn't show it or the template doesn't list its levels.
pub fn template_levels(c: &LocalLlm, model: &str) -> Option<Vec<String>> {
    let key = format!("{}#{model}", base_url(c));
    if let Some(known) = TEMPLATE_LEVELS.lock().unwrap().get_or_insert_with(HashMap::new).get(&key) {
        return known.clone();
    }
    // `/props` sits at the server's root, not under the API path; `model` picks one in router mode.
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(2)))
        .http_status_as_error(false)
        .build()
        .into();
    // Unreachable: ask again next time, the server may just not be up yet.
    let mut res = with_auth(agent.get(&format!("{}/props", origin(c))).query("model", model), &c.api_key).call().ok()?;
    let json: Value = if res.status().is_success() { res.body_mut().read_json().unwrap_or(Value::Null) } else { Value::Null };
    let levels = json["chat_template"].as_str().and_then(levels_in_template);
    TEMPLATE_LEVELS.lock().unwrap().get_or_insert_with(HashMap::new).insert(key, levels.clone());
    levels
}

/// Whether each server has oMLX's web search (`POST /v1/web/search`), asked once per server.
static WEB_SEARCH: Mutex<Option<HashMap<String, bool>>> = Mutex::new(None);

/// POSTs `body` to `path` under the API path and reads the JSON answer, whatever its status.
fn post_json(c: &LocalLlm, path: &str, body: &Value, timeout: Duration) -> Result<(u16, Value), String> {
    let url = format!("{}/{path}", base_url(c));
    let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(timeout)).http_status_as_error(false).build().into();
    let mut req = agent.post(&url);
    if !c.api_key.trim().is_empty() {
        req = req.header("Authorization", &format!("Bearer {}", c.api_key.trim()));
    }
    let mut res = req.send_json(body).map_err(|e| format!("couldn't reach {url}: {e}"))?;
    let status = res.status().as_u16();
    Ok((status, res.body_mut().read_json().unwrap_or(Value::Null)))
}

/// Whether the server searches the web for its models (oMLX: Settings → Web search there picks
/// the provider). It answers an empty query with an `{"ok": false}` payload; others with a 404.
pub fn has_web_search(c: &LocalLlm) -> bool {
    let key = base_url(c);
    if let Some(&known) = WEB_SEARCH.lock().unwrap().get_or_insert_with(HashMap::new).get(&key) {
        return known;
    }
    // Unreachable: ask again next time, the server may just not be up yet.
    let Ok((status, json)) = post_json(c, "web/search", &serde_json::json!({ "query": "" }), Duration::from_secs(3)) else { return false };
    let has = status == 200 && json.get("ok").is_some_and(Value::is_boolean);
    WEB_SEARCH.lock().unwrap().get_or_insert_with(HashMap::new).insert(key, has);
    has
}

/// Searches the web through the server, as numbered results (title, URL, snippet or page text).
pub fn web_search(c: &LocalLlm, query: &str) -> Result<String, String> {
    let (status, json) = post_json(c, "web/search", &serde_json::json!({ "query": query }), Duration::from_secs(90))?;
    if json["ok"] != Value::Bool(true) {
        return Err(json["error"]["message"].as_str().map_or_else(|| error_message(status, &json), str::to_string));
    }
    Ok(format_results(&json))
}

fn format_results(json: &Value) -> String {
    let results = json["results"].as_array().map(Vec::as_slice).unwrap_or_default();
    if results.is_empty() {
        return "No results. Try other words, or fetch a page you know.".into();
    }
    let field = |r: &Value, k: &str| r[k].as_str().map(str::trim).unwrap_or_default().to_string();
    results
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let text = Some(field(r, "content")).filter(|t| !t.is_empty()).unwrap_or_else(|| field(r, "snippet"));
            format!("{}. {}\n   {}\n   {}", i + 1, field(r, "title"), field(r, "url"), text.replace('\n', "\n   "))
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Effort names from lowest to highest, to order a template's levels.
pub const EFFORT_RANK: [&str; 7] = ["minimal", "low", "medium", "high", "xhigh", "max", "ultra"];

/// The levels a chat template takes: those in its `reasoning_effort ... not in (...)` check (what
/// it raises an error for otherwise), lowest first, after "none" when `enable_thinking` can switch
/// thinking off. A template with only the switch gives just "none"; one with neither, no levels.
pub fn levels_in_template(t: &str) -> Option<Vec<String>> {
    let toggle = t.contains("enable_thinking");
    let mut levels: Vec<String> = Vec::new();
    if t.contains("reasoning_effort") {
        for (at, _) in t.match_indices("not in") {
            let before = &t[t[..at].rfind("{%").unwrap_or(0)..at];
            if !before.contains("reasoning_effort") {
                continue;
            }
            let rest = t[at + 6..].trim_start();
            let close = match rest.chars().next() {
                Some('(') => ')',
                Some('[') => ']',
                _ => continue,
            };
            let Some(end) = rest.find(close) else { continue };
            for part in rest[1..end].split(',') {
                let level = part.trim().trim_matches(|c| c == '\'' || c == '"').trim().to_ascii_lowercase();
                if !level.is_empty() && !levels.contains(&level) {
                    levels.push(level);
                }
            }
        }
        if levels.is_empty() {
            return None;
        }
    }
    let rank = |l: &String| EFFORT_RANK.iter().position(|r| r == l).unwrap_or(EFFORT_RANK.len());
    levels.sort_by_key(rank);
    let off = levels.iter().position(|l| l == "none");
    if let Some(i) = off {
        levels.remove(i);
    }
    if toggle || off.is_some() {
        levels.insert(0, "none".into());
    }
    Some(levels)
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

#[cfg(test)]
mod template_tests {
    use super::{format_results, levels_in_template};

    #[test]
    fn formats_search_results() {
        let json = serde_json::json!({ "ok": true, "results": [
            { "title": "Tom Hiddleston", "url": "https://en.wikipedia.org/wiki/Tom_Hiddleston", "snippet": "born 9 February 1981" },
            { "title": "Page", "url": "https://x.dev", "snippet": "short", "content": "full\ntext" },
        ] });
        assert_eq!(
            format_results(&json),
            "1. Tom Hiddleston\n   https://en.wikipedia.org/wiki/Tom_Hiddleston\n   born 9 February 1981\n\n2. Page\n   https://x.dev\n   full\n   text"
        );
        assert!(format_results(&serde_json::json!({ "ok": true, "results": [] })).starts_with("No results"));
    }

    fn v(l: &[&str]) -> Option<Vec<String>> {
        Some(l.iter().map(|s| s.to_string()).collect())
    }

    #[test]
    fn reads_qwen38_levels_and_the_off_switch() {
        let t = "{%- if enable_thinking is undefined or enable_thinking is true %}\n{%- set resolved_reasoning_effort = reasoning_effort|default('xhigh') %}\n{%- if resolved_reasoning_effort not in ('xhigh', 'medium', 'low') %}{{- raise_exception('x') }}{%- endif %}";
        assert_eq!(levels_in_template(t), v(&["none", "low", "medium", "xhigh"]));
    }

    #[test]
    fn reads_bracketed_lists_without_a_switch() {
        let t = "{%- if reasoning_effort not in [\"low\", \"medium\", \"high\"] %}{{ raise_exception('x') }}{% endif %}";
        assert_eq!(levels_in_template(t), v(&["low", "medium", "high"]));
    }

    #[test]
    fn switch_only_and_no_controls() {
        assert_eq!(levels_in_template("{% if enable_thinking is false %}<think></think>{% endif %}"), v(&["none"]));
        assert_eq!(levels_in_template("{{ messages }}"), v(&[]));
        // Takes an effort but doesn't say which: unknown.
        assert_eq!(levels_in_template("{{ reasoning_effort }}"), None);
    }
}
