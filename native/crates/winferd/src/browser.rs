//! The server's browser, shown on a paired desktop (docs/connect/protocol.md, "Browser on a
//! server"): each workspace has the headless Chromium its agents drive (`browser/` backend); a
//! desktop's browser panel lists its tabs, watches one as a screencast and types and clicks in it.
//!
//! Requests are `browser.call {op, workspaceId, ...}`, answered with an ack (`value` holding what
//! the op returns). Frames go to the desktop that attached as `browser.frame {attachId, data}`
//! (JPEG, base64), each acknowledged with `frameDone` before the next is sent, so the frame rate
//! follows what the link carries; so does the JPEG quality (`Adapt`). The backend's `browser://…` events go to every paired desktop
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
    if let Some(a) = ADAPT.lock().as_mut() {
        for (id, _) in &gone {
            a.remove(id);
        }
    }
    for (id, ws) in gone {
        tauri::async_runtime::spawn(async move {
            let _ = browser::browser_detach(backend::app().state::<BrowserManager>(), ws, id).await;
        });
    }
}

/// Picture quality per screencast, following the link: frames that take long from here to the
/// desktop's `frameDone` (a slow relay, a busy desktop) get lighter; quick ones heavier.
struct Adapt {
    quality: u8,
    max: u8,
    sent: std::collections::VecDeque<std::time::Instant>,
    /// Smoothed send → frameDone time, ms.
    ms: f64,
    since_change: u32,
}

static ADAPT: Mutex<Option<HashMap<String, Adapt>>> = Mutex::new(None);

/// Where a screencast starts: full quality on the LAN; on the relay by its round trip.
fn start_quality(relay_rtt: Option<Option<u32>>) -> (u8, u8) {
    match relay_rtt {
        None => (80, 80),
        Some(Some(rtt)) if rtt < 80 => (65, 75),
        Some(Some(rtt)) if rtt < 200 => (55, 75),
        Some(Some(_)) => (40, 75),
        // Not measured yet: the middle, and the frame times take it from there.
        Some(None) => (55, 75),
    }
}

/// The next quality, if it should change: down after a few slow frames, up after many quick ones.
fn next_quality(quality: u8, max: u8, ms: f64, since_change: u32) -> Option<u8> {
    if since_change >= 8 && ms > 400.0 && quality > 30 {
        Some(quality.saturating_sub(15).max(30))
    } else if since_change >= 40 && ms < 120.0 && quality < max {
        Some((quality + 10).min(max))
    } else {
        None
    }
}

/// A frame of `attach` went out.
fn frame_sent(attach: &str) {
    if let Some(a) = ADAPT.lock().as_mut().and_then(|m| m.get_mut(attach)) {
        a.sent.push_back(std::time::Instant::now());
    }
}

/// The desktop drew a frame of `attach`: the quality to restart at, if it should change.
fn frame_done(attach: &str) -> Option<u8> {
    let mut adapt = ADAPT.lock();
    let a = adapt.as_mut()?.get_mut(attach)?;
    let at = a.sent.pop_front()?;
    let ms = at.elapsed().as_secs_f64() * 1000.0;
    a.ms = if a.since_change == 0 { ms } else { a.ms * 0.8 + ms * 0.2 };
    a.since_change += 1;
    let next = next_quality(a.quality, a.max, a.ms, a.since_change)?;
    if std::env::var_os("WINFER_CONNECT_TRACE").is_some() {
        eprintln!("[browser] {attach}: frames take {:.0} ms, quality {} → {next}", a.ms, a.quality);
    }
    a.quality = next;
    a.sent.clear();
    a.since_change = 0;
    Some(next)
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
            // Over the relay (not the LAN), lighter frames, then whatever the link carries.
            let relay_rtt = winfer_core::connect::get().and_then(|c| c.status().peers.into_iter().find(|p| p.peer_id == client && p.link.via == Some("relay")).map(|p| p.link.rtt_ms));
            let (quality, max) = start_quality(relay_rtt);
            if quality != 80 {
                browser::set_screencast_quality(&attach, quality);
            }
            ADAPT.lock().get_or_insert_with(HashMap::new).insert(attach.clone(), Adapt { quality, max, sent: Default::default(), ms: 0.0, since_change: 0 });
            ATTACHED.lock().get_or_insert_with(HashMap::new).insert(attach.clone(), (client.to_string(), ws.clone()));
            let (to, id) = (client.to_string(), attach.clone());
            let channel = Channel::new(move |body| {
                if let InvokeResponseBody::Raw(bytes) = body {
                    frame_sent(&id);
                    let data = base64::engine::general_purpose::STANDARD.encode(bytes);
                    send_to(&to, json!({"t": "browser.frame", "attachId": id, "data": data}));
                }
                Ok(())
            });
            done(browser::browser_attach(app.clone(), manager(), ws, tab, viewport, attach, channel).await.map_err(|e| e.to_string()))
        }
        "frameDone" => {
            let attach = s(msg, "attachId");
            if let Some(quality) = frame_done(&attach) {
                let _ = browser::set_live_quality(&manager(), &ws, &attach, quality).await;
            }
            done(browser::browser_frame_done(manager(), ws, attach).await.map_err(|e| e.to_string()))
        }
        "detach" => {
            let attach = s(msg, "attachId");
            if let Some(a) = ATTACHED.lock().as_mut() {
                a.remove(&attach);
            }
            if let Some(a) = ADAPT.lock().as_mut() {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quality_starts_by_link_and_follows_frame_times() {
        assert_eq!(start_quality(None), (80, 80));
        assert_eq!(start_quality(Some(Some(40))), (65, 75));
        assert_eq!(start_quality(Some(Some(150))), (55, 75));
        assert_eq!(start_quality(Some(Some(400))), (40, 75));
        assert_eq!(start_quality(Some(None)), (55, 75));
        // Slow frames: down by 15, not below 30, and only after a few.
        assert_eq!(next_quality(55, 75, 600.0, 3), None);
        assert_eq!(next_quality(55, 75, 600.0, 8), Some(40));
        assert_eq!(next_quality(40, 75, 600.0, 8), Some(30));
        assert_eq!(next_quality(30, 75, 900.0, 50), None);
        // Quick frames: up by 10 after many, up to the link's cap.
        assert_eq!(next_quality(55, 75, 80.0, 20), None);
        assert_eq!(next_quality(55, 75, 80.0, 40), Some(65));
        assert_eq!(next_quality(70, 75, 80.0, 40), Some(75));
        assert_eq!(next_quality(75, 75, 80.0, 40), None);
        assert_eq!(next_quality(55, 75, 250.0, 100), None);
    }
}
