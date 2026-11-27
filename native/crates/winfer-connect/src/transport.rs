//! Transports carry text frames (see `wire`) between two instances. The relay one reconnects
//! by itself; LAN ones are made per connection (`lan`).

use crate::keys::RoomKeys;
use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use std::time::Duration;
use tokio::sync::{mpsc, watch};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;

pub const RELAY_PING: Duration = Duration::from_secs(30);
const MAX_BACKOFF_SECS: u64 = 60;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Via {
    Relay,
    Lan,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TEvent {
    /// Connected (to the relay, or to the other instance on the LAN).
    Open,
    /// The relay's count of instances in the room (LAN connections are always 2).
    Presence(u32),
    Frame(String),
    Closed { code: Option<u16>, error: Option<String> },
}

/// Sends frames on a transport; dropping it (or `stop`) closes it.
pub struct Transport {
    pub via: Via,
    out: mpsc::UnboundedSender<String>,
    stop: watch::Sender<bool>,
}

impl Transport {
    pub fn new(via: Via, out: mpsc::UnboundedSender<String>, stop: watch::Sender<bool>) -> Transport {
        Transport { via, out, stop }
    }

    pub fn send(&self, frame: String) -> bool {
        self.out.send(frame).is_ok()
    }

    pub fn stop(&self) {
        let _ = self.stop.send(true);
    }
}

impl Drop for Transport {
    fn drop(&mut self) {
        self.stop();
    }
}

pub fn ws_base(base: &str) -> String {
    let base = base.trim().trim_end_matches('/');
    if let Some(rest) = base.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = base.strip_prefix("http://") {
        format!("ws://{rest}")
    } else {
        base.to_string()
    }
}

pub fn relay_url(base: &str, room: &RoomKeys, inst: &str) -> String {
    format!("{}/r/{}?role=peer&inst={inst}&auth={}", ws_base(base), room.room, room.relay_auth)
}

/// `DELETE`s a relay room (revoke), closing it with 4001 for whoever is in it.
pub fn delete_room(base: &str, room: &RoomKeys) -> Result<(), String> {
    let url = format!("{}/r/{}?auth={}", base.trim().trim_end_matches('/'), room.room, room.relay_auth);
    match ureq::delete(&url).call() {
        Ok(_) | Err(ureq::Error::StatusCode(403)) => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}

/// Keeps a relay connection up until stopped, reporting through `events`.
pub fn relay(rt: &tokio::runtime::Handle, base: &str, room: &RoomKeys, inst: &str, events: mpsc::UnboundedSender<TEvent>) -> Transport {
    let (out, mut rx) = mpsc::unbounded_channel::<String>();
    let (stop, mut stopped) = watch::channel(false);
    let url = relay_url(base, room, inst);
    rt.spawn(async move {
        let mut backoff = 1;
        loop {
            if *stopped.borrow() {
                break;
            }
            // Frames queued while offline belong to a handshake that's gone.
            while rx.try_recv().is_ok() {}
            let connect = tokio::select! {
                r = tokio_tungstenite::connect_async(url.as_str()) => r,
                _ = stopped.changed() => break,
            };
            let mut code = None;
            let error = match connect {
                Ok((ws, _)) => {
                    backoff = 1;
                    let _ = events.send(TEvent::Open);
                    let (mut sink, mut stream) = ws.split();
                    let mut ping = tokio::time::interval(RELAY_PING);
                    ping.tick().await;
                    loop {
                        tokio::select! {
                            frame = stream.next() => match frame {
                                Some(Ok(Message::Text(text))) => {
                                    let text = text.as_str();
                                    if text == "pong" {
                                    } else if text.starts_with('{') {
                                        if let Ok(v) = serde_json::from_str::<Value>(text) && v["t"] == "presence" {
                                            let _ = events.send(TEvent::Presence(v["peers"].as_u64().unwrap_or(0) as u32));
                                        }
                                    } else {
                                        let _ = events.send(TEvent::Frame(text.to_string()));
                                    }
                                }
                                Some(Ok(Message::Close(frame))) => {
                                    code = frame.map(|f: CloseFrame| u16::from(f.code));
                                    break None;
                                }
                                None => break None,
                                Some(Ok(_)) => {}
                                Some(Err(e)) => break Some(e.to_string()),
                            },
                            out = rx.recv() => {
                                let Some(out) = out else { break None };
                                if let Err(e) = sink.feed(Message::text(out)).await {
                                    break Some(e.to_string());
                                }
                                let mut err = None;
                                for _ in 0..16 {
                                    match rx.try_recv() {
                                        Ok(more) => {
                                            if let Err(e) = sink.feed(Message::text(more)).await {
                                                err = Some(e.to_string());
                                                break;
                                            }
                                        }
                                        Err(_) => break,
                                    }
                                }
                                if let Some(e) = err {
                                    break Some(e);
                                }
                                if let Err(e) = sink.flush().await {
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
                                return;
                            }
                        }
                    }
                }
                Err(tokio_tungstenite::tungstenite::Error::Http(r)) => Some(format!("relay answered {}", r.status())),
                Err(e) => Some(e.to_string()),
            };
            let _ = events.send(TEvent::Closed { code, error });
            // Revoked rooms (4001) stay closed; a replaced socket (4000) means this instance
            // connected again elsewhere, which the newer connection handles.
            if matches!(code, Some(4001) | Some(4000)) {
                break;
            }
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(backoff)) => {}
                _ = stopped.changed() => break,
            }
            backoff = (backoff * 2).min(MAX_BACKOFF_SECS);
        }
    });
    Transport::new(Via::Relay, out, stop)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relay_urls() {
        let room = RoomKeys { room: "R".into(), relay_auth: "A".into() };
        assert_eq!(relay_url("https://relay.example.dev/", &room, "I"), "wss://relay.example.dev/r/R?role=peer&inst=I&auth=A");
        assert_eq!(relay_url("http://localhost:8787", &room, "I"), "ws://localhost:8787/r/R?role=peer&inst=I&auth=A");
    }
}
