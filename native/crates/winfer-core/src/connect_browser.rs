//! A paired server's browser on this desktop (the server's side is `winferd/src/browser.rs`).
//! Its workspaces' browsers show in the browser panel under the ids `‹peer›|‹workspace›`: their
//! `browser.event`s come back as the local `browser://…` events with those ids, their screencast
//! frames go to the panel's frame sink, and the panel's calls go out as `browser.call`.

use base64::Engine as _;
use parking_lot::Mutex;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;
use std::time::{Duration, Instant};
use tauri::Emitter;

const TIMEOUT: Duration = Duration::from_secs(30);

type Sink = std::sync::Arc<dyn Fn(Vec<u8>) + Send + Sync>;

#[derive(Default)]
struct Shared {
    seq: u64,
    waiting: HashMap<String, (Instant, async_channel::Sender<Result<Value, String>>)>,
    frames: HashMap<String, Sink>,
    /// Remote workspaces with a browser seen, by peer.
    seen: HashMap<String, HashSet<String>>,
    sweeping: bool,
}

static SHARED: LazyLock<Mutex<Shared>> = LazyLock::new(Default::default);

/// The browser panel's id for a server's workspace.
pub fn remote_id(peer: &str, workspace: &str) -> String {
    format!("{peer}|{workspace}")
}

/// `(peer, workspace)` of a remote browser id; None for this machine's workspaces.
pub fn split(id: &str) -> Option<(&str, &str)> {
    id.split_once('|')
}

/// Calls `op` on the browser of the remote workspace `id` and waits for its answer.
pub async fn call(id: &str, op: &str, mut args: Value) -> Result<Value, String> {
    let (peer, ws) = split(id).ok_or("not a server's browser")?;
    let (tx, rx) = async_channel::bounded(1);
    let rid = {
        let mut s = SHARED.lock();
        s.seq += 1;
        let rid = format!("b{}", s.seq);
        s.waiting.insert(rid.clone(), (Instant::now(), tx));
        if !std::mem::replace(&mut s.sweeping, true) {
            std::thread::spawn(sweep);
        }
        rid
    };
    if !args.is_object() {
        args = json!({});
    }
    args["t"] = json!("browser.call");
    args["op"] = json!(op);
    args["workspaceId"] = json!(ws);
    args["id"] = json!(rid);
    crate::connect::send(peer, args);
    rx.recv().await.map_err(|_| "no answer".to_string())?
}

/// Like `call`, without waiting for (or getting) an answer: input and frame acks.
pub fn fire(id: &str, op: &str, mut args: Value) {
    let Some((peer, ws)) = split(id) else { return };
    if !args.is_object() {
        args = json!({});
    }
    args["t"] = json!("browser.call");
    args["op"] = json!(op);
    args["workspaceId"] = json!(ws);
    crate::connect::send(peer, args);
}

/// Calls that waited too long fail.
fn sweep() {
    loop {
        std::thread::sleep(Duration::from_secs(1));
        let late: Vec<async_channel::Sender<Result<Value, String>>> = {
            let mut s = SHARED.lock();
            let ids: Vec<String> = s.waiting.iter().filter(|(_, (at, _))| at.elapsed() > TIMEOUT).map(|(id, _)| id.clone()).collect();
            ids.iter().filter_map(|id| s.waiting.remove(id)).map(|(_, tx)| tx).collect()
        };
        for tx in late {
            let _ = tx.try_send(Err("The server didn't answer.".into()));
        }
    }
}

/// Where the frames of the screencast `attach_id` go (JPEG bytes).
pub fn on_frames(attach_id: &str, sink: impl Fn(Vec<u8>) + Send + Sync + 'static) {
    SHARED.lock().frames.insert(attach_id.to_string(), std::sync::Arc::new(sink));
}

pub fn drop_frames(attach_id: &str) {
    SHARED.lock().frames.remove(attach_id);
}

/// A message from `peer` meant for this module: handled here (true) instead of going to the UI.
pub fn intercept(app: &tauri::AppHandle, peer: &str, msg: &Value) -> bool {
    match msg["t"].as_str().unwrap_or_default() {
        "ack" => {
            let Some(id) = msg["id"].as_str().filter(|id| id.starts_with('b')) else { return false };
            let Some((_, tx)) = SHARED.lock().waiting.remove(id) else { return false };
            let _ = tx.try_send(if msg["ok"] == true { Ok(msg["value"].clone()) } else { Err(msg["error"].as_str().unwrap_or("failed").to_string()) });
            true
        }
        "browser.frame" => {
            let bytes = msg["data"].as_str().and_then(|d| base64::engine::general_purpose::STANDARD.decode(d).ok());
            let sink = msg["attachId"].as_str().and_then(|id| SHARED.lock().frames.get(id).cloned());
            if let (Some(sink), Some(bytes)) = (sink, bytes) {
                sink(bytes);
            }
            true
        }
        "browser.event" => {
            let (Some(event), mut payload) = (msg["event"].as_str(), msg["payload"].clone()) else { return true };
            if !event.starts_with("browser://") {
                return true;
            }
            if let Some(ws) = payload.as_str().map(str::to_string) {
                payload = json!(remote_id(peer, &ws));
            } else if let Some(ws) = payload["workspaceId"].as_str().map(str::to_string) {
                SHARED.lock().seen.entry(peer.to_string()).or_default().insert(ws.clone());
                payload["workspaceId"] = json!(remote_id(peer, &ws));
                payload["workspaceName"] = json!(name(peer, payload["workspace"].as_str()));
            }
            let _ = app.emit(event, payload);
            true
        }
        _ => false,
    }
}

/// "‹machine› · ‹workspace›".
pub fn name(peer: &str, workspace: Option<&str>) -> String {
    let machine = crate::connect::peer_name(peer).unwrap_or_else(|| "server".into());
    match workspace.filter(|w| !w.is_empty()) {
        Some(w) => format!("{machine} · {w}"),
        None => machine,
    }
}

/// A server's link came up: its browsers that are open already show.
pub fn peer_up(app: &tauri::AppHandle, peer: &str) {
    let is_server = crate::connect::get().is_some_and(|c| c.peers().iter().any(|p| p.peer_id == peer && p.kind == winfer_connect::pairing::Kind::Server));
    if !is_server {
        return;
    }
    let (app, peer) = (app.clone(), peer.to_string());
    tauri::async_runtime::spawn(async move {
        let Ok(list) = call(&remote_id(&peer, ""), "list", json!({})).await else { return };
        for b in list.as_array().into_iter().flatten() {
            let Some(ws) = b["workspaceId"].as_str() else { continue };
            SHARED.lock().seen.entry(peer.clone()).or_default().insert(ws.to_string());
            let _ = app.emit("browser://tabs", json!({"workspaceId": remote_id(&peer, ws), "workspaceName": name(&peer, b["workspace"].as_str()), "tabs": b["tabs"]}));
        }
    });
}

/// A server's link went down: its browsers leave the panel, and calls waiting on it fail.
pub fn peer_down(app: &tauri::AppHandle, peer: &str) {
    let seen = SHARED.lock().seen.remove(peer).unwrap_or_default();
    for ws in seen {
        let id = remote_id(peer, &ws);
        let _ = app.emit("browser://disconnected", json!(id));
        let _ = app.emit("browser://tabs", json!({"workspaceId": id, "tabs": []}));
    }
}
