//! Minimal Chrome DevTools Protocol client over a WebSocket (flattened sessions).

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot, watch};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;

use super::log;

/// How often an idle connection is pinged, so a dead socket is noticed (and replaced) before
/// the panel needs it rather than when a click fails.
const PING_EVERY: Duration = Duration::from_secs(15);

type Pending = Mutex<HashMap<u64, oneshot::Sender<Result<Value, String>>>>;

#[derive(Debug, Clone)]
pub struct Event {
    pub method: String,
    pub params: Value,
    pub session_id: Option<String>,
}

pub struct Cdp {
    out: mpsc::UnboundedSender<Message>,
    pending: Arc<Pending>,
    next_id: AtomicU64,
    closed: Arc<watch::Sender<bool>>,
}

impl Cdp {
    pub async fn connect(
        ws_url: &str,
        on_event: impl Fn(Event) + Send + Sync + 'static,
    ) -> Result<Arc<Self>, String> {
        // Chrome sends each message as one frame, and some (big DOM or network payloads) exceed
        // tungstenite's default limits, which would end the whole connection.
        let config = WebSocketConfig::default()
            .max_message_size(None)
            .max_frame_size(None);
        let (ws, _) = tokio_tungstenite::connect_async_with_config(ws_url, Some(config), false)
            .await
            .map_err(|e| format!("CDP connect failed: {e}"))?;
        let (mut sink, mut stream) = ws.split();
        let (out, mut rx) = mpsc::unbounded_channel::<Message>();
        let pending: Arc<Pending> = Arc::default();
        let closed = Arc::new(watch::Sender::new(false));

        let writer_closed = closed.clone();
        tauri::async_runtime::spawn(async move {
            let mut ping = tokio::time::interval(PING_EVERY);
            ping.tick().await;
            loop {
                let msg = tokio::select! {
                    msg = rx.recv() => match msg {
                        Some(msg) => msg,
                        None => break,
                    },
                    _ = ping.tick() => Message::Ping(Default::default()),
                };
                if let Err(err) = sink.send(msg).await {
                    log::warning!("CDP send failed: {err}");
                    break;
                }
            }
            writer_closed.send_replace(true);
            let _ = sink.close().await;
        });

        let reader_pending = pending.clone();
        let reader_closed = closed.clone();
        tauri::async_runtime::spawn(async move {
            let reason = loop {
                let msg = match stream.next().await {
                    Some(Ok(msg)) => msg,
                    Some(Err(err)) => break err.to_string(),
                    None => break "socket ended".to_string(),
                };
                if let Message::Close(frame) = &msg {
                    break format!("closed by browser ({frame:?})");
                }
                let Message::Text(text) = msg else { continue };
                let Ok(value) = serde_json::from_str::<Value>(text.as_str()) else {
                    continue;
                };
                if let Some(id) = value.get("id").and_then(Value::as_u64) {
                    let result = match value.get("error") {
                        Some(err) => Err(err
                            .get("message")
                            .and_then(Value::as_str)
                            .unwrap_or("CDP error")
                            .to_string()),
                        None => Ok(value.get("result").cloned().unwrap_or(Value::Null)),
                    };
                    if let Some(tx) = reader_pending.lock().unwrap().remove(&id) {
                        let _ = tx.send(result);
                    }
                } else if let Some(method) = value.get("method").and_then(Value::as_str) {
                    on_event(Event {
                        method: method.to_string(),
                        params: value.get("params").cloned().unwrap_or(Value::Null),
                        session_id: value
                            .get("sessionId")
                            .and_then(Value::as_str)
                            .map(String::from),
                    });
                }
            };
            reader_closed.send_replace(true);
            log::warning!("CDP connection lost: {reason}");
            // Connection closed: fail everything still waiting.
            for (_, tx) in reader_pending.lock().unwrap().drain() {
                let _ = tx.send(Err("browser connection closed".into()));
            }
        });

        Ok(Arc::new(Self {
            out,
            pending,
            next_id: AtomicU64::new(1),
            closed,
        }))
    }

    /// The socket is gone; calls on this connection can only fail.
    pub fn is_closed(&self) -> bool {
        *self.closed.borrow() || self.out.is_closed()
    }

    /// Resolves once the socket is gone.
    pub async fn closed(&self) {
        let _ = self.closed.subscribe().wait_for(|closed| *closed).await;
    }

    pub async fn call(
        &self,
        method: &str,
        params: Value,
        session_id: Option<&str>,
    ) -> Result<Value, String> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let mut msg = json!({ "id": id, "method": method, "params": params });
        if let Some(session) = session_id {
            msg["sessionId"] = json!(session);
        }
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(id, tx);
        self.out
            .send(Message::text(msg.to_string()))
            .map_err(|_| "browser connection closed".to_string())?;
        match tokio::time::timeout(Duration::from_secs(30), rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err("browser connection closed".into()),
            Err(_) => {
                self.pending.lock().unwrap().remove(&id);
                Err(format!("{method} timed out"))
            }
        }
    }

    /// Fire-and-forget (used for screencast acks from the event loop).
    pub fn send(&self, method: &str, params: Value, session_id: Option<&str>) {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let mut msg = json!({ "id": id, "method": method, "params": params });
        if let Some(session) = session_id {
            msg["sessionId"] = json!(session);
        }
        let _ = self.out.send(Message::text(msg.to_string()));
    }
}
