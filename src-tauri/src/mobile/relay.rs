//! One phone's connection to the relay: a WebSocket winfer dials out to (so no port has to be
//! open), reconnecting with backoff. Frames are sealed and opened here; the rest of winfer
//! sends and receives plain JSON.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde::Serialize;
use serde_json::Value;
use tokio::sync::{mpsc, watch};
use tokio_tungstenite::tungstenite::Message;

use super::crypto::Keys;

const PING_EVERY: Duration = Duration::from_secs(30);
const MAX_BACKOFF_SECS: u64 = 60;

#[derive(Debug, Default, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnState {
    /// Connected to the relay.
    pub connected: bool,
    /// Phones in the room right now.
    pub phones: u32,
    pub error: Option<String>,
}

pub struct Conn {
    tx: mpsc::UnboundedSender<String>,
    stop: watch::Sender<bool>,
    pub state: Arc<Mutex<ConnState>>,
}

impl Conn {
    /// Queues a message for the phone; dropped when no phone is listening.
    pub fn send(&self, msg: &Value) {
        if self.state.lock().unwrap().phones > 0 {
            let _ = self.tx.send(msg.to_string());
        }
    }

    pub fn stop(&self) {
        let _ = self.stop.send(true);
    }
}

impl Drop for Conn {
    fn drop(&mut self) {
        self.stop();
    }
}

/// `wss://host/r/<room>?role=desktop&auth=<token>` from the relay's https URL.
pub fn ws_url(base: &str, room: &str, auth: &str) -> String {
    let base = base.trim().trim_end_matches('/');
    let base = if let Some(rest) = base.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = base.strip_prefix("http://") {
        format!("ws://{rest}")
    } else {
        base.to_string()
    };
    format!("{base}/r/{room}?role=desktop&auth={auth}")
}

/// Connects in the background until stopped. `on_message` gets the phone's decrypted
/// messages, `on_change` is called when the connection or the phones in the room change.
pub fn spawn(
    base: String,
    room: String,
    keys: Arc<Keys>,
    on_message: impl Fn(Value) + Send + Sync + 'static,
    on_change: impl Fn() + Send + Sync + 'static,
) -> Conn {
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    let (stop, mut stopped) = watch::channel(false);
    let state = Arc::new(Mutex::new(ConnState::default()));
    let shared = state.clone();
    let set = move |f: &dyn Fn(&mut ConnState)| {
        f(&mut shared.lock().unwrap());
        on_change();
    };
    tauri::async_runtime::spawn(async move {
        let url = ws_url(&base, &room, &keys.auth_token);
        let mut backoff = 1;
        loop {
            if *stopped.borrow() {
                break;
            }
            // What was queued while offline is stale; the phone asks again when it reconnects.
            while rx.try_recv().is_ok() {}
            let connect = tokio::select! {
                r = tokio_tungstenite::connect_async(url.as_str()) => r,
                _ = stopped.changed() => break,
            };
            match connect {
                Ok((ws, _)) => {
                    backoff = 1;
                    set(&|s| {
                        s.connected = true;
                        s.error = None;
                    });
                    let (mut sink, mut stream) = ws.split();
                    let mut ping = tokio::time::interval(PING_EVERY);
                    ping.tick().await;
                    let reason = loop {
                        tokio::select! {
                            frame = stream.next() => match frame {
                                Some(Ok(Message::Text(text))) => {
                                    let text = text.as_str();
                                    if text == "pong" {
                                    } else if text.starts_with('{') {
                                        // The relay's presence report.
                                        if let Ok(v) = serde_json::from_str::<Value>(text)
                                            && v["t"] == "presence"
                                        {
                                            let phones = v["phones"].as_u64().unwrap_or(0) as u32;
                                            set(&|s| s.phones = phones);
                                        }
                                    } else if let Some(msg) = keys.open(text).and_then(|p| serde_json::from_slice::<Value>(&p).ok()) {
                                        on_message(msg);
                                    }
                                }
                                Some(Ok(Message::Close(_))) | None => break None,
                                Some(Ok(_)) => {}
                                Some(Err(e)) => break Some(e.to_string()),
                            },
                            out = rx.recv() => {
                                let Some(out) = out else { break None };
                                if let Err(e) = sink.send(Message::text(keys.seal(out.as_bytes()))).await {
                                    break Some(e.to_string());
                                }
                            }
                            _ = ping.tick() => {
                                if let Err(e) = sink.send(Message::text("ping")).await {
                                    break Some(e.to_string());
                                }
                            }
                            _ = stopped.changed() => {
                                let _ = sink.send(Message::Close(None)).await;
                                break None;
                            }
                        }
                    };
                    set(&|s| {
                        s.connected = false;
                        s.phones = 0;
                        s.error = reason.clone();
                    });
                }
                Err(e) => {
                    let error = match e {
                        tokio_tungstenite::tungstenite::Error::Http(r) => format!("relay answered {}", r.status()),
                        e => e.to_string(),
                    };
                    set(&|s| s.error = Some(error.clone()));
                }
            }
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(backoff)) => {}
                _ = stopped.changed() => break,
            }
            backoff = (backoff * 2).min(MAX_BACKOFF_SECS);
        }
    });
    Conn { tx, stop, state }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relay_urls() {
        assert_eq!(ws_url("https://relay.example.dev/", "R", "A"), "wss://relay.example.dev/r/R?role=desktop&auth=A");
        assert_eq!(ws_url("http://localhost:8787", "R", "A"), "ws://localhost:8787/r/R?role=desktop&auth=A");
    }
}

#[cfg(test)]
mod tls_smoke {
    /// wss works with the bundled TLS stack (needs network): `cargo test -- --ignored wss_connects`.
    #[test]
    #[ignore]
    fn wss_connects() {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        rt.block_on(async {
            let (ws, _) = tokio_tungstenite::connect_async("wss://echo.websocket.org").await.unwrap();
            drop(ws);
        });
    }
}
