//! A paired machine's phones, emulators and simulators in this machine's device panel (the host
//! side is `devices.rs`). `peer` None is this machine: its calls go straight to `devices.rs`, so
//! the panel treats both alike. Requests are `device.call {op, id?, ...}`, answered with an ack;
//! frames come back as `device.frame {streamId, codec, key, data}`, a failed stream as
//! `device.end {streamId, error}`.

use crate::devices::{self, Link, Msg, Out};
use base64::Engine as _;
use parking_lot::Mutex;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

const TIMEOUT: Duration = Duration::from_secs(30);
/// What this machine's own panel is, as a client of `devices.rs`.
const LOCAL: &str = "local";

pub type Sink = Arc<dyn Fn(Msg) + Send + Sync>;
type Answer = async_channel::Sender<Result<Value, String>>;

#[derive(Default)]
struct Shared {
    seq: u64,
    /// Calls waiting on an answer: (peer, answer by, where it goes).
    waiting: HashMap<String, (String, Instant, Answer)>,
    /// Streams watched: id → (peer, where frames go).
    sinks: HashMap<String, (Option<String>, Sink)>,
    sweeping: bool,
}

static SHARED: LazyLock<Mutex<Shared>> = LazyLock::new(Default::default);

/// A stream id unique in this process.
pub fn new_stream_id() -> String {
    let mut s = SHARED.lock();
    s.seq += 1;
    format!("s{}-{}", std::process::id(), s.seq)
}

fn local_out() -> Out {
    Arc::new(deliver)
}

fn message(op: &str, mut args: Value) -> Value {
    if !args.is_object() {
        args = json!({});
    }
    args["t"] = json!("device.call");
    args["op"] = json!(op);
    args
}

/// Calls `op` on `peer`'s devices (None: this machine's) and waits for its answer.
pub async fn call(peer: Option<&str>, op: &str, args: Value) -> Result<Value, String> {
    let msg = message(op, args);
    let Some(peer) = peer else {
        return crate::backend::background(move || devices::call(LOCAL, &msg, local_out(), Link::Local)).await.map_err(|e| e.to_string())?;
    };
    let (tx, rx) = async_channel::bounded(1);
    let rid = {
        let mut s = SHARED.lock();
        s.seq += 1;
        let rid = format!("d{}", s.seq);
        s.waiting.insert(rid.clone(), (peer.to_string(), Instant::now() + TIMEOUT, tx));
        if !std::mem::replace(&mut s.sweeping, true) {
            std::thread::spawn(sweep);
        }
        rid
    };
    let mut msg = msg;
    msg["id"] = json!(rid);
    crate::connect::send(peer, msg);
    rx.recv().await.map_err(|_| "no answer".to_string())?
}

/// Like `call`, without an answer: input and frame acks.
pub fn fire(peer: Option<&str>, op: &str, args: Value) {
    let msg = message(op, args);
    match peer {
        Some(peer) => crate::connect::send(peer, msg),
        None => {
            let _ = devices::call(LOCAL, &msg, local_out(), Link::Local);
        }
    }
}

/// Calls that waited too long fail.
fn sweep() {
    loop {
        std::thread::sleep(Duration::from_secs(1));
        let late: Vec<Answer> = {
            let mut s = SHARED.lock();
            let ids: Vec<String> = s.waiting.iter().filter(|(_, (_, by, _))| Instant::now() > *by).map(|(id, _)| id.clone()).collect();
            ids.iter().filter_map(|id| s.waiting.remove(id)).map(|(_, _, tx)| tx).collect()
        };
        for tx in late {
            let _ = tx.try_send(Err("The machine didn't answer (it may need a newer winfer).".into()));
        }
    }
}

/// Where the frames of stream `id` (on `peer`, None: this machine) go.
pub fn on_frames(id: &str, peer: Option<&str>, sink: impl Fn(Msg) + Send + Sync + 'static) {
    SHARED.lock().sinks.insert(id.to_string(), (peer.map(str::to_string), Arc::new(sink)));
}

pub fn drop_frames(id: &str) {
    SHARED.lock().sinks.remove(id);
}

fn deliver(stream: &str, msg: Msg) {
    let sink = SHARED.lock().sinks.get(stream).map(|(_, s)| s.clone());
    if let Some(sink) = sink {
        sink(msg);
    }
}

/// A message from `peer` meant for the device mirror (either side): handled here (true).
pub fn intercept(peer: &str, msg: &Value) -> bool {
    match msg["t"].as_str().unwrap_or_default() {
        "device.call" => {
            devices::on_request(peer, msg);
            true
        }
        "device.frame" => {
            let data = msg["data"].as_str().and_then(|d| base64::engine::general_purpose::STANDARD.decode(d).ok());
            let codec = match msg["codec"].as_str() {
                Some("h264") => "h264",
                Some("png") => "png",
                _ => "jpeg",
            };
            if let (Some(stream), Some(data)) = (msg["streamId"].as_str(), data) {
                deliver(stream, Msg::Frame { codec, data, key: msg["key"] == true });
            }
            true
        }
        "device.end" => {
            if let Some(stream) = msg["streamId"].as_str() {
                deliver(stream, Msg::End(msg["error"].as_str().unwrap_or("The mirror stopped.").to_string()));
            }
            true
        }
        "ack" => {
            let Some(id) = msg["id"].as_str().filter(|id| id.starts_with('d')) else { return false };
            let Some((_, _, tx)) = SHARED.lock().waiting.remove(id) else { return false };
            let _ = tx.try_send(if msg["ok"] == true { Ok(msg["value"].clone()) } else { Err(msg["error"].as_str().unwrap_or("failed").to_string()) });
            true
        }
        _ => false,
    }
}

/// `peer`'s link went down: what it watched here stops, what this machine watched there ends.
pub fn peer_down(peer: &str) {
    devices::drop_client(peer);
    let (sinks, calls) = {
        let mut s = SHARED.lock();
        let sinks: Vec<Sink> = s.sinks.values().filter(|(p, _)| p.as_deref() == Some(peer)).map(|(_, sink)| sink.clone()).collect();
        let ids: Vec<String> = s.waiting.iter().filter(|(_, (p, _, _))| p == peer).map(|(id, _)| id.clone()).collect();
        let calls: Vec<_> = ids.iter().filter_map(|id| s.waiting.remove(id)).map(|(_, _, tx)| tx).collect();
        (sinks, calls)
    };
    let name = crate::connect::peer_name(peer).unwrap_or_else(|| "the machine".into());
    for sink in sinks {
        sink(Msg::End(format!("Lost the link to {name}.")));
    }
    for tx in calls {
        let _ = tx.try_send(Err(format!("Lost the link to {name}.")));
    }
}
