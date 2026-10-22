//! Live voice on a ChatGPT plan, the way the Codex app runs it: the `gpt-live-1-codex` voice
//! model on chatgpt.com's realtime calls endpoint, with a control socket ("sideband") on
//! api.openai.com. The webview holds the audio over WebRTC; this side creates the call from its
//! SDP offer, so tokens never reach the page, and relays the control socket's events.
//!
//! On a ChatGPT plan the voice model can't call functions: it hands each request over as text
//! (`delegation.created`), winfer runs it with a tool-calling model (`chatgpt_tool_turn`) and
//! gives the answer back for the voice to speak (`delegation.context.append`).
//!
//! The call needs the Codex CLI's ChatGPT login: the endpoint refuses winfer's own "Sign in with
//! ChatGPT" token (a third-party app token). The tool-running model uses either.

use std::io::{BufRead, BufReader};
use std::sync::Mutex;

use base64::Engine;
use futures_util::{SinkExt, StreamExt};
use serde::Serialize;
use serde_json::{Value, json};
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

pub const MODEL: &str = "gpt-live-1-codex";
const NO_CODEX_LOGIN: &str = "Live voice on a ChatGPT plan uses the Codex CLI's ChatGPT login: install Codex and run `codex login` (or open Codex once if its login expired).";
const CALLS_URL: &str = "https://chatgpt.com/backend-api/codex/realtime/calls?intent=quicksilver&architecture=avas";
const SIDEBAND_BASE: &str = "wss://api.openai.com/v1/live";
const CODEX_RESPONSES_URL: &str = "https://chatgpt.com/backend-api/codex/responses";
/// The call endpoint refuses live calls without it.
const ALPHA: (&str, &str) = ("openai-alpha", "quicksilver=v2");
const ORIGINATOR: &str = "winfer";
pub const VOICES: [&str; 9] = ["cove", "juniper", "maple", "spruce", "ember", "vale", "breeze", "arbor", "sol"];
const DEFAULT_VOICE: &str = "cove";

/// A ChatGPT token and where it came from.
#[derive(Clone)]
struct Login {
    token: String,
    account: Option<String>,
    source: &'static str,
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

fn claims(token: &str) -> Option<Value> {
    let payload = token.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(payload.trim_end_matches('=')).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// The ChatGPT account a token bills to.
fn account_of(token: &str) -> Option<String> {
    claims(token)?["https://api.openai.com/auth"]["chatgpt_account_id"].as_str().map(str::to_string)
}

fn codex_home() -> Option<std::path::PathBuf> {
    std::env::var_os("CODEX_HOME").map(Into::into).or_else(|| dirs::home_dir().map(|h| h.join(".codex")))
}

/// The Codex CLI's ChatGPT login, if it has a token that's still valid. Not refreshed here:
/// refresh tokens rotate, so refreshing would sign the Codex CLI out.
fn codex_login() -> Option<Login> {
    let auth: Value = serde_json::from_str(&std::fs::read_to_string(codex_home()?.join("auth.json")).ok()?).ok()?;
    let token = auth["tokens"]["access_token"].as_str()?.to_string();
    if claims(&token)?["exp"].as_u64()? < now() + 60 {
        return None;
    }
    let account = auth["tokens"]["account_id"].as_str().map(str::to_string).or_else(|| account_of(&token));
    Some(Login { token, account, source: "codex" })
}

fn with_login(req: ureq::RequestBuilder<ureq::typestate::WithBody>, login: &Login) -> ureq::RequestBuilder<ureq::typestate::WithBody> {
    let req = req.header("Authorization", &format!("Bearer {}", login.token)).header("originator", ORIGINATOR);
    match &login.account {
        Some(a) => req.header("ChatGPT-Account-Id", a),
        None => req,
    }
}

pub fn session(instructions: &str, voice: &str) -> Value {
    let voice = VOICES.iter().find(|v| **v == voice.trim().to_lowercase()).copied().unwrap_or(DEFAULT_VOICE);
    json!({
        "model": MODEL,
        "instructions": instructions,
        "audio": { "output": { "voice": voice } },
        "delegation": { "type": "client" },
    })
}

/// The call id at the end of a created call's `Location` (`…/rtc_abc` or a UUID).
fn call_id_from_location(location: &str) -> Option<String> {
    location.split('?').next().unwrap_or(location).rsplit('/').find(|s| (s.starts_with("rtc_") && s.len() > 4) || (s.len() == 36 && s.matches('-').count() == 4)).map(str::to_string)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CallAnswer {
    answer_sdp: String,
    call_id: String,
    source: &'static str,
}

/// The live call: its id and login (to join), and the control socket once joined.
#[derive(Default)]
pub struct ChatgptVoice {
    call: Mutex<Option<(String, Login)>>,
    socket: Mutex<Option<mpsc::UnboundedSender<Message>>>,
}

fn create(login: &Login, offer: &str, instructions: &str, voice: &str) -> Result<(u16, String, Option<String>), String> {
    let mut res = with_login(crate::chatgpt::http().post(CALLS_URL), login)
        .header(ALPHA.0, ALPHA.1)
        .send_json(json!({ "sdp": offer, "session": session(instructions, voice) }))
        .map_err(|e| format!("couldn't reach ChatGPT: {e}"))?;
    let status = res.status().as_u16();
    let location = res.headers().get("location").and_then(|v| v.to_str().ok()).map(str::to_string);
    Ok((status, res.body_mut().read_to_string().unwrap_or_default(), location))
}

fn refusal(status: u16, body: &str) -> String {
    let detail = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| v.pointer("/error/message").or_else(|| v.get("detail")).and_then(Value::as_str).map(str::to_string))
        .unwrap_or_else(|| body.chars().take(200).collect());
    match status {
        401 | 403 => format!("ChatGPT refused the Codex login for live voice ({status}): {detail}. Run `codex login` again."),
        429 => "Your ChatGPT plan's voice limit is reached. Try again after it resets.".into(),
        _ => format!("ChatGPT live voice isn't available ({status}): {detail}"),
    }
}

/// Creates the call from the webview's SDP offer; returns OpenAI's answer.
#[tauri::command]
pub async fn chatgpt_voice_call(app: AppHandle, sdp: String, instructions: String, voice: String) -> Result<CallAnswer, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let login = codex_login().ok_or(NO_CODEX_LOGIN)?;
        let (status, body, location) = create(&login, &sdp, &instructions, &voice)?;
        if !(200..300).contains(&status) {
            return Err(refusal(status, &body));
        }
        let call_id = location.as_deref().and_then(call_id_from_location).ok_or("ChatGPT sent no call id")?;
        if !body.trim_start().starts_with("v=") {
            return Err("ChatGPT sent no SDP answer".into());
        }
        let source = login.source;
        *app.state::<ChatgptVoice>().call.lock().unwrap() = Some((call_id.clone(), login));
        Ok(CallAnswer { answer_sdp: body, call_id, source })
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Joins the call's control socket; its messages arrive as `chatgpt-voice://event` (the raw
/// JSON text) and its end as `chatgpt-voice://closed`.
#[tauri::command]
pub async fn chatgpt_voice_join(app: AppHandle) -> Result<(), String> {
    let (call_id, login) = app.state::<ChatgptVoice>().call.lock().unwrap().clone().ok_or("no live voice call")?;
    let mut request = format!("{SIDEBAND_BASE}/{call_id}").into_client_request().map_err(|e| e.to_string())?;
    let headers = request.headers_mut();
    let mut set = |k: &'static str, v: &str| v.parse().map(|v| headers.insert(k, v)).map_err(|_| format!("bad {k} header"));
    set("authorization", &format!("Bearer {}", login.token))?;
    set("originator", ORIGINATOR)?;
    set(ALPHA.0, ALPHA.1)?;
    if let Some(a) = &login.account {
        set("chatgpt-account-id", a)?;
    }
    let (ws, _) = tokio::time::timeout(std::time::Duration::from_secs(15), tokio_tungstenite::connect_async(request))
        .await
        .map_err(|_| "ChatGPT live voice didn't answer in time.".to_string())?
        .map_err(|e| format!("Couldn't join the live voice call: {e}"))?;
    let (mut sink, mut stream) = ws.split();
    let (tx, mut rx) = mpsc::unbounded_channel::<Message>();
    *app.state::<ChatgptVoice>().socket.lock().unwrap() = Some(tx);
    tauri::async_runtime::spawn(async move {
        while let Some(m) = rx.recv().await {
            let close = matches!(m, Message::Close(_));
            if sink.send(m).await.is_err() || close {
                break;
            }
        }
        let _ = sink.close().await;
    });
    tauri::async_runtime::spawn(async move {
        let mut reason = "closed".to_string();
        while let Some(m) = stream.next().await {
            match m {
                Ok(Message::Text(t)) => {
                    let _ = app.emit("chatgpt-voice://event", t.as_str());
                }
                Ok(Message::Close(frame)) => {
                    reason = frame.map(|f| f.reason.to_string()).filter(|r| !r.is_empty()).unwrap_or(reason);
                    break;
                }
                Ok(_) => {}
                Err(e) => {
                    reason = e.to_string();
                    break;
                }
            }
        }
        *app.state::<ChatgptVoice>().socket.lock().unwrap() = None;
        let _ = app.emit("chatgpt-voice://closed", reason);
    });
    Ok(())
}

/// Sends one message on the control socket.
#[tauri::command]
pub fn chatgpt_voice_send(app: AppHandle, message: Value) -> Result<(), String> {
    let socket = app.state::<ChatgptVoice>().socket.lock().unwrap().clone().ok_or("the live voice call has ended")?;
    socket.send(Message::Text(message.to_string().into())).map_err(|_| "the live voice call has ended".to_string())
}

/// Ends the call.
#[tauri::command]
pub fn chatgpt_voice_close(app: AppHandle) {
    let voice = app.state::<ChatgptVoice>();
    if let Some(socket) = voice.socket.lock().unwrap().take() {
        let _ = socket.send(Message::Text(json!({ "type": "session.close" }).to_string().into()));
        let _ = socket.send(Message::Close(None));
    }
    *voice.call.lock().unwrap() = None;
}

/// The Codex CLI's configured model, for its Responses endpoint.
fn codex_model() -> Option<String> {
    let config = std::fs::read_to_string(codex_home()?.join("config.toml")).ok()?;
    config.lines().find_map(|l| {
        let (k, v) = l.split_once('=')?;
        (k.trim() == "model").then(|| v.trim().trim_matches('"').to_string())
    })
}

/// Opens a streaming Responses call: on winfer's ChatGPT sign-in, else on the Codex login.
fn responses_stream(mut body: Value) -> Result<Box<dyn std::io::Read + Send>, String> {
    if crate::chatgpt::signed_in() {
        if body["model"].as_str().unwrap_or("").is_empty() {
            body["model"] = json!(crate::chatgpt::DEFAULT_MODEL);
        }
        return Ok(Box::new(crate::chatgpt::open_stream(&body)?));
    }
    let login = codex_login().ok_or("Live voice needs Sign in with ChatGPT (Settings → AI) or the Codex CLI's login to run requests.")?;
    if body["model"].as_str().unwrap_or("").is_empty() {
        body["model"] = json!(codex_model().unwrap_or_else(|| crate::chatgpt::DEFAULT_MODEL.into()));
    }
    let mut res = with_login(crate::chatgpt::http().post(CODEX_RESPONSES_URL), &login)
        .header("Accept", "text/event-stream")
        .header("OpenAI-Beta", "responses=experimental")
        .send_json(&body)
        .map_err(|e| format!("couldn't reach ChatGPT: {e}"))?;
    let status = res.status().as_u16();
    if !(200..300).contains(&status) {
        let json: Value = res.body_mut().read_json().unwrap_or(Value::Null);
        return Err(crate::chatgpt::api_error(status, &json).1);
    }
    Ok(Box::new(res.into_body().into_reader()))
}

/// Output items are replayed without server ids (nothing is stored server-side).
fn replayable(mut item: Value) -> Value {
    if let Some(o) = item.as_object_mut() {
        o.remove("id");
        o.remove("status");
    }
    item
}

/// The output items of one streamed Responses answer.
fn read_output(reader: impl std::io::Read) -> Result<Vec<Value>, String> {
    let mut out = Vec::new();
    for line in BufReader::new(reader).lines() {
        let line = line.map_err(|_| "the connection to OpenAI dropped".to_string())?;
        let Some(data) = line.strip_prefix("data:") else { continue };
        let Ok(ev) = serde_json::from_str::<Value>(data.trim()) else { continue };
        match ev["type"].as_str().unwrap_or("") {
            "response.output_item.done" => out.push(replayable(ev["item"].clone())),
            "response.completed" | "response.incomplete" => return Ok(out),
            "response.failed" | "error" => {
                let error = if ev["response"]["error"].is_object() { &ev["response"]["error"] } else { &ev["error"] };
                return Err(crate::chatgpt::api_error(200, &json!({ "error": error })).1);
            }
            _ => {}
        }
    }
    Err("OpenAI's answer stopped before it finished.".into())
}

/// One model turn with tools for a handed-over voice request: returns its output items
/// (function calls to run, or the answer message).
#[tauri::command]
pub async fn chatgpt_tool_turn(model: String, instructions: String, input: Vec<Value>, tools: Vec<Value>) -> Result<Vec<Value>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let mut body = json!({
            "model": model,
            "instructions": instructions,
            "input": input,
            "store": false,
            "stream": true,
            "include": ["reasoning.encrypted_content"],
            "reasoning": { "effort": "low" },
        });
        if !tools.is_empty() {
            body["tools"] = json!(tools);
            body["tool_choice"] = json!("auto");
            body["parallel_tool_calls"] = json!(true);
        }
        read_output(responses_stream(body)?)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Whether live voice can run on a ChatGPT plan (the Codex CLI is logged in), for Settings.
#[tauri::command]
pub fn chatgpt_voice_ready() -> bool {
    codex_login().is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_delegates_to_the_client_with_a_known_voice() {
        let s = session("Be brief.", "Juniper");
        assert_eq!((s["model"].as_str(), s["audio"]["output"]["voice"].as_str()), (Some(MODEL), Some("juniper")));
        assert_eq!(s["delegation"]["type"], "client");
        assert_eq!(session("x", "alloy")["audio"]["output"]["voice"], DEFAULT_VOICE);
    }

    #[test]
    fn call_ids_come_from_the_location() {
        assert_eq!(call_id_from_location("/v1/realtime/calls/rtc_abc123?x=1").as_deref(), Some("rtc_abc123"));
        assert_eq!(call_id_from_location("https://chatgpt.com/backend-api/codex/realtime/calls/0f8fad5b-d9cb-469f-a165-70867728950e").as_deref(), Some("0f8fad5b-d9cb-469f-a165-70867728950e"));
        assert_eq!(call_id_from_location("/v1/realtime/calls/"), None);
    }

    #[test]
    fn reads_the_account_from_a_token() {
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(br#"{"https://api.openai.com/auth":{"chatgpt_account_id":"acc-1"},"exp":1}"#);
        assert_eq!(account_of(&format!("h.{payload}.s")).as_deref(), Some("acc-1"));
        assert_eq!(account_of("nope"), None);
    }

    #[test]
    fn collects_output_items_without_ids() {
        let sse = "data: {\"type\":\"response.output_item.done\",\"item\":{\"id\":\"fc_1\",\"status\":\"completed\",\"type\":\"function_call\",\"name\":\"list_panes\",\"call_id\":\"c1\",\"arguments\":\"{}\"}}\n\ndata: {\"type\":\"response.completed\",\"response\":{}}\n";
        let items = read_output(sse.as_bytes()).unwrap();
        assert_eq!(items, vec![json!({ "type": "function_call", "name": "list_panes", "call_id": "c1", "arguments": "{}" })]);
    }
}
