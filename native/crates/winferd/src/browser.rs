//! The server's browser, shown on a paired desktop (docs/connect/protocol.md, "Browser on a
//! server"): each workspace has the headless Chromium its agents drive (`browser/` backend); a
//! desktop's browser panel lists its tabs, watches one as a screencast and types and clicks in it.
//!
//! Requests are `browser.call {op, workspaceId, ...}`, answered with an ack (`value` holding what
//! the op returns). Frames go to the desktop that attached as `browser.frame {attachId, data}`
//! (JPEG, base64), each acknowledged with `frameDone` before the next is sent, so the frame rate
//! follows what the link carries. The backend's `browser://…` events go to every paired desktop
//! as `browser.event {event, payload}`.

use crate::host::{Host, send_to};
use base64::Engine as _;
use parking_lot::Mutex;
use serde_json::{Value, json};
use std::collections::HashMap;
use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::Manager;
use winfer_backend::browser::{self, BrowserManager};
use winfer_core::{backend, connect};

/// Screencasts being watched: attach id → (desktop, workspace).
static ATTACHED: Mutex<Option<HashMap<String, (String, String)>>> = Mutex::new(None);

fn watched(workspace: &str) -> Vec<String> {
    ATTACHED.lock().as_ref().map(|a| a.values().filter(|(_, w)| w == workspace).map(|(c, _)| c.clone()).collect()).unwrap_or_default()
}

/// Forwards the backend's browser events to the paired desktops (a tab's console only to the
/// desktop watching it).
pub fn start(host: &'static Host) {
    backend::app().listen_any(move |event, payload| {
        if !event.starts_with("browser://") {
            return;
        }
        if std::env::var_os("WINFER_CONNECT_TRACE").is_some() {
            eprintln!("[browser] {event} → {} desktops", connect::subscribers().len());
        }
        let mut payload = payload.clone();
        if let Some(ws) = payload["workspaceId"].as_str().map(str::to_string) {
            payload["workspace"] = json!(host.with_state(|s| s.workspaces.iter().find(|w| w.id == ws).map(|w| w.name.clone())));
        }
        let msg = json!({"t": "browser.event", "event": event, "payload": payload});
        if event == "browser://console" {
            let ws = payload["workspaceId"].as_str().unwrap_or_default();
            for client in watched(ws) {
                send_to(&client, msg.clone());
            }
        } else {
            connect::broadcast(&msg);
        }
    });
}

/// The desktop's link went down: its screencasts end.
pub fn drop_client(client: &str) {
    let gone: Vec<(String, String)> = {
        let mut attached = ATTACHED.lock();
        let a = attached.get_or_insert_with(HashMap::new);
        let ids: Vec<String> = a.iter().filter(|(_, (c, _))| c == client).map(|(id, _)| id.clone()).collect();
        ids.into_iter().filter_map(|id| a.remove(&id).map(|(_, ws)| (id, ws))).collect()
    };
    for (id, ws) in gone {
        tauri::async_runtime::spawn(async move {
            let _ = browser::browser_detach(backend::app().state::<BrowserManager>(), ws, id).await;
        });
    }
}

fn s(msg: &Value, key: &str) -> String {
    msg[key].as_str().unwrap_or_default().to_string()
}

/// Input events, applied one after another (a press must not overtake the move before it).
static INPUT: std::sync::OnceLock<async_channel::Sender<(String, Value)>> = std::sync::OnceLock::new();

/// Answers a `browser.call` from `client` (a paired desktop).
pub fn on_request(host: &'static Host, client: &str, msg: &Value) {
    let (client, msg) = (client.to_string(), msg.clone());
    if msg["op"] == "input" {
        let queue = INPUT.get_or_init(|| {
            let (tx, rx) = async_channel::unbounded::<(String, Value)>();
            tauri::async_runtime::spawn(async move {
                while let Ok((client, msg)) = rx.recv().await {
                    let _ = call(host, &client, &msg).await;
                }
            });
            tx
        });
        let _ = queue.try_send((client, msg));
        return;
    }
    tauri::async_runtime::spawn(async move {
        let result = call(host, &client, &msg).await;
        // Input and frame acks aren't answered.
        if msg["id"].is_null() {
            return;
        }
        let mut answer = match result {
            Ok(value) => json!({"t": "ack", "ok": true, "value": value}),
            Err(e) => json!({"t": "ack", "ok": false, "error": e}),
        };
        answer["id"] = msg["id"].clone();
        send_to(&client, answer);
    });
}

async fn call(host: &'static Host, client: &str, msg: &Value) -> Result<Value, String> {
    let app = backend::app();
    let manager = || app.state::<BrowserManager>();
    let op = msg["op"].as_str().unwrap_or_default();
    if op == "list" {
        let names: HashMap<String, String> = host.with_state(|s| s.workspaces.iter().map(|w| (w.id.clone(), w.name.clone())).collect());
        let open = manager().open_tabs();
        return Ok(Value::Array(open.into_iter().map(|(id, tabs)| json!({"workspaceId": id, "workspace": names.get(&id), "tabs": tabs})).collect()));
    }
    if op == "status" {
        return browser::browser_status(app.clone()).map(|s| json!(s)).map_err(|e| e.to_string());
    }
    if op == "install" {
        return browser::browser_install(app.clone()).await.map(|e| json!(e)).map_err(|e| e.to_string());
    }
    // Only this server's workspaces (each gets a browser profile of its own).
    let ws = s(msg, "workspaceId");
    if !host.with_state(|s| s.workspaces.iter().any(|w| w.id == ws)) {
        return Err("That workspace isn't on this server.".into());
    }
    let tab = s(msg, "tab");
    let f = |key: &str| msg[key].as_f64().unwrap_or(0.0);
    let done = |r: Result<(), String>| r.map(|_| Value::Null);
    match op {
        "open" => browser::browser_open(app.clone(), manager(), ws).await.map(|t| json!(t)).map_err(|e| e.to_string()),
        "newTab" => browser::browser_new_tab(app.clone(), manager(), ws, s(msg, "url")).await.map(Value::String).map_err(|e| e.to_string()),
        "closeTab" => done(browser::browser_close_tab(app.clone(), manager(), ws, tab).await.map_err(|e| e.to_string())),
        "navigate" => done(browser::browser_navigate(app.clone(), manager(), ws, tab, s(msg, "url")).await.map_err(|e| e.to_string())),
        "history" => done(browser::browser_history(app.clone(), manager(), ws, tab, msg["delta"].as_i64().unwrap_or(0)).await.map_err(|e| e.to_string())),
        "input" => done(browser::browser_input(app.clone(), manager(), ws, tab, s(msg, "method"), msg["params"].clone()).await.map_err(|e| e.to_string())),
        "resize" => {
            let viewport = serde_json::from_value(msg["viewport"].clone()).map_err(|e| e.to_string())?;
            done(browser::browser_resize(app.clone(), manager(), ws, tab, viewport).await.map_err(|e| e.to_string()))
        }
        "attach" => {
            let viewport = serde_json::from_value(msg["viewport"].clone()).map_err(|e| e.to_string())?;
            let attach = s(msg, "attachId");
            ATTACHED.lock().get_or_insert_with(HashMap::new).insert(attach.clone(), (client.to_string(), ws.clone()));
            let (to, id) = (client.to_string(), attach.clone());
            let channel = Channel::new(move |body| {
                if let InvokeResponseBody::Raw(bytes) = body {
                    let data = base64::engine::general_purpose::STANDARD.encode(bytes);
                    send_to(&to, json!({"t": "browser.frame", "attachId": id, "data": data}));
                }
                Ok(())
            });
            done(browser::browser_attach(app.clone(), manager(), ws, tab, viewport, attach, channel).await.map_err(|e| e.to_string()))
        }
        "frameDone" => done(browser::browser_frame_done(manager(), ws, s(msg, "attachId")).await.map_err(|e| e.to_string())),
        "detach" => {
            let attach = s(msg, "attachId");
            if let Some(a) = ATTACHED.lock().as_mut() {
                a.remove(&attach);
            }
            done(browser::browser_detach(manager(), ws, attach).await.map_err(|e| e.to_string()))
        }
        "certs" => done(browser::browser_allow_invalid_certificates(app.clone(), manager(), ws, msg["allow"] == true).await.map_err(|e| e.to_string())),
        "consoleClear" => done(browser::inspect::browser_console_clear(app.clone(), manager(), ws, tab).await.map_err(|e| e.to_string())),
        "selectProbe" => browser::browser_select_probe(app.clone(), manager(), ws, tab, f("x"), f("y")).await.map(|v| v.unwrap_or_default()).map_err(|e| e.to_string()),
        "selectPick" => done(browser::browser_select_pick(app.clone(), manager(), ws, tab, msg["index"].as_i64()).await.map_err(|e| e.to_string())),
        "selection" => browser::browser_selection(app.clone(), manager(), ws, tab).await.map(Value::String).map_err(|e| e.to_string()),
        "imageAt" => browser::browser_image_at(app.clone(), manager(), ws, tab, f("x"), f("y"), msg["withData"] == true).await.map(|v| v.unwrap_or_default()).map_err(|e| e.to_string()),
        "pickStart" => done(browser::inspect::browser_pick_start(app.clone(), manager(), ws, tab).await.map_err(|e| e.to_string())),
        "pickCancel" => done(browser::inspect::browser_pick_cancel(app.clone(), manager(), ws, tab).await.map_err(|e| e.to_string())),
        "pickHover" => browser::inspect::browser_pick_hover(app.clone(), manager(), ws, tab, f("x"), f("y")).await.map(|v| json!(v)).map_err(|e| e.to_string()),
        "pasteImage" => done(browser::browser_paste_image(app.clone(), manager(), ws, tab, s(msg, "data"), s(msg, "mime")).await.map_err(|e| e.to_string())),
        _ => Err(format!("A winfer server's browser can't {op} yet.")),
    }
}
