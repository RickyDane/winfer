//! The LAN path: each instance listens for WebSocket connections on a port the OS picks and
//! advertises it over mDNS (`_winfer-connect._tcp`, TXT `i` = instance id, `n` = name, `k` =
//! kind, `v` = 1). A connection opens with one clear frame saying who's calling and why:
//! `h.{"t":"lan","v":1,"i":<id>,"want":"link"|"pair"}`. After that it carries the same frames as
//! the relay: the link handshake for a paired instance, the pairing messages for a new one.

use crate::pairing::Kind;
use crate::transport::{TEvent, Transport, Via};
use crate::wire::{self, Frame};
use futures_util::{SinkExt, StreamExt};
use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
use serde_json::json;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::{mpsc, watch};
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;

pub const SERVICE: &str = "_winfer-connect._tcp.local.";
/// How long an instance stays listed after its last mDNS answer.
const NEARBY_TTL: Duration = Duration::from_secs(5 * 60);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Want {
    Link,
    Pair,
}

pub fn hello(me: &str, want: Want) -> String {
    wire::handshake(&json!({"t": "lan", "v": 1, "i": me, "want": if want == Want::Pair { "pair" } else { "link" }}))
}

pub fn parse_hello(frame: &str) -> Option<(String, Want)> {
    let Some(Frame::Handshake(v)) = wire::parse(frame) else { return None };
    if v["t"] != "lan" {
        return None;
    }
    let id = v["i"].as_str().filter(|s| s.len() == 22)?.to_string();
    let want = match v["want"].as_str()? {
        "pair" => Want::Pair,
        "link" => Want::Link,
        _ => return None,
    };
    Some((id, want))
}

/// Runs a WebSocket as a transport: `Open` and `Presence(2)` at once (the other side is right
/// there), then its frames, then `Closed`.
pub fn pump<S>(rt: &tokio::runtime::Handle, ws: WebSocketStream<S>) -> (Transport, mpsc::UnboundedReceiver<TEvent>)
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (events, rx_events) = mpsc::unbounded_channel();
    let (out, mut rx) = mpsc::unbounded_channel::<String>();
    let (stop, mut stopped) = watch::channel(false);
    rt.spawn(async move {
        let _ = events.send(TEvent::Open);
        let _ = events.send(TEvent::Presence(2));
        let (mut sink, mut stream) = ws.split();
        let error = loop {
            tokio::select! {
                frame = stream.next() => match frame {
                    Some(Ok(Message::Text(text))) => { let _ = events.send(TEvent::Frame(text.to_string())); }
                    Some(Ok(Message::Close(_))) | None => break None,
                    Some(Ok(_)) => {}
                    Some(Err(e)) => break Some(e.to_string()),
                },
                out = rx.recv() => {
                    let Some(out) = out else { break None };
                    if let Err(e) = sink.send(Message::text(out)).await { break Some(e.to_string()); }
                }
                _ = stopped.changed() => {
                    let _ = sink.send(Message::Close(None)).await;
                    break None;
                }
            }
        };
        let _ = events.send(TEvent::Closed { code: None, error });
    });
    (Transport::new(Via::Lan, out, stop), rx_events)
}

/// A connection someone opened to this instance, after its hello.
pub struct Incoming {
    pub from: String,
    pub want: Want,
    pub addr: SocketAddr,
    pub transport: Transport,
    pub events: mpsc::UnboundedReceiver<TEvent>,
}

/// Listens on all interfaces; returns the port. Every connection that says hello within 10 s goes
/// to `on_incoming`; anything else is dropped.
/// Returns the port and a handle that stops the listener.
pub async fn listen(rt: tokio::runtime::Handle, on_incoming: Arc<dyn Fn(Incoming) + Send + Sync>) -> std::io::Result<(u16, tokio::task::AbortHandle)> {
    let listener = tokio::net::TcpListener::bind(("0.0.0.0", 0)).await?;
    let port = listener.local_addr()?.port();
    let handle = rt.clone();
    let task = rt.spawn(async move {
        // At most a few handshakes at once, so a flood can't pile up sockets.
        let slots = Arc::new(tokio::sync::Semaphore::new(8));
        loop {
            let Ok((stream, addr)) = listener.accept().await else { break };
            let Ok(permit) = slots.clone().try_acquire_owned() else { continue };
            let on_incoming = on_incoming.clone();
            let handle = handle.clone();
            tokio::spawn(async move {
                let _permit = permit;
                let Ok(Ok(mut ws)) = tokio::time::timeout(Duration::from_secs(10), tokio_tungstenite::accept_async(stream)).await else { return };
                let first = tokio::time::timeout(Duration::from_secs(10), ws.next()).await;
                let Ok(Some(Ok(Message::Text(text)))) = first else { return };
                let Some((from, want)) = parse_hello(text.as_str()) else { return };
                let (transport, events) = pump(&handle, ws);
                on_incoming(Incoming { from, want, addr, transport, events });
            });
        }
    });
    Ok((port, task.abort_handle()))
}

/// Opens a connection to another instance and says hello.
pub async fn dial(rt: &tokio::runtime::Handle, addr: SocketAddr, me: &str, want: Want) -> Result<(Transport, mpsc::UnboundedReceiver<TEvent>), String> {
    let url = format!("ws://{addr}/");
    let (mut ws, _) = tokio::time::timeout(Duration::from_secs(5), tokio_tungstenite::connect_async(url.as_str()))
        .await
        .map_err(|_| "timed out".to_string())?
        .map_err(|e| e.to_string())?;
    ws.send(Message::text(hello(me, want))).await.map_err(|e| e.to_string())?;
    Ok(pump(rt, ws))
}

#[derive(Clone, Debug)]
pub struct Nearby {
    pub id: String,
    pub name: String,
    pub kind: Kind,
    pub addrs: Vec<SocketAddr>,
    pub seen: Instant,
}

/// mDNS: advertising this instance and the instances seen on the network.
pub struct Discovery {
    daemon: ServiceDaemon,
    advertised: Mutex<Option<String>>,
    pub nearby: Arc<Mutex<HashMap<String, Nearby>>>,
}

impl Discovery {
    /// Starts browsing; `on_change` runs (on mDNS's thread) whenever the nearby list changes.
    pub fn start(me: String, on_change: Arc<dyn Fn(&Nearby) + Send + Sync>) -> Result<Discovery, String> {
        let daemon = ServiceDaemon::new().map_err(|e| e.to_string())?;
        let rx = daemon.browse(SERVICE).map_err(|e| e.to_string())?;
        let nearby: Arc<Mutex<HashMap<String, Nearby>>> = Arc::default();
        let list = nearby.clone();
        std::thread::Builder::new()
            .name("connect-mdns".into())
            .spawn(move || {
                while let Ok(event) = rx.recv() {
                    match event {
                        ServiceEvent::ServiceResolved(info) => {
                            let Some(id) = info.get_property_val_str("i").filter(|i| i.len() == 22 && *i != me).map(str::to_string) else { continue };
                            let name = info.get_property_val_str("n").unwrap_or("winfer").chars().take(64).collect();
                            let kind = if info.get_property_val_str("k") == Some("server") { Kind::Server } else { Kind::Desktop };
                            let port = info.get_port();
                            let mut addrs: Vec<SocketAddr> = info.get_addresses_v4().into_iter().map(|ip| SocketAddr::from((ip, port))).collect();
                            addrs.sort();
                            let entry = Nearby { id: id.clone(), name, kind, addrs, seen: Instant::now() };
                            list.lock().unwrap().insert(id, entry.clone());
                            on_change(&entry);
                        }
                        ServiceEvent::ServiceRemoved(_, fullname) => {
                            let id = fullname.split('.').next().unwrap_or_default().to_string();
                            let removed = list.lock().unwrap().remove(&id);
                            if let Some(gone) = removed {
                                on_change(&gone);
                            }
                        }
                        _ => {}
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(Discovery { daemon, advertised: Mutex::new(None), nearby })
    }

    pub fn advertise(&self, me: &str, name: &str, kind: Kind, port: u16) -> Result<(), String> {
        self.withdraw();
        let host = format!("winfer-{}.local.", me[..8].to_ascii_lowercase().replace(['_', '-'], "x"));
        let kind = if kind == Kind::Server { "server" } else { "desktop" };
        let props = [("i", me), ("n", name), ("k", kind), ("v", "1")];
        let info = ServiceInfo::new(SERVICE, me, &host, "", port, &props[..]).map_err(|e| e.to_string())?.enable_addr_auto();
        let fullname = info.get_fullname().to_string();
        self.daemon.register(info).map_err(|e| e.to_string())?;
        *self.advertised.lock().unwrap() = Some(fullname);
        Ok(())
    }

    pub fn withdraw(&self) {
        if let Some(fullname) = self.advertised.lock().unwrap().take() {
            let _ = self.daemon.unregister(&fullname);
        }
    }

    /// Instances heard from lately.
    pub fn list(&self) -> Vec<Nearby> {
        let mut list: Vec<Nearby> = self.nearby.lock().unwrap().values().filter(|n| n.seen.elapsed() < NEARBY_TTL).cloned().collect();
        list.sort_by(|a, b| a.name.cmp(&b.name));
        list
    }

    pub fn get(&self, id: &str) -> Option<Nearby> {
        self.nearby.lock().unwrap().get(id).filter(|n| n.seen.elapsed() < NEARBY_TTL).cloned()
    }
}

impl Drop for Discovery {
    fn drop(&mut self) {
        self.withdraw();
        let _ = self.daemon.shutdown();
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hellos_round_trip() {
        let id = "A".repeat(22);
        assert_eq!(parse_hello(&hello(&id, Want::Pair)), Some((id.clone(), Want::Pair)));
        assert_eq!(parse_hello(&hello(&id, Want::Link)), Some((id, Want::Link)));
        assert_eq!(parse_hello("h.e30"), None);
        assert_eq!(parse_hello("garbage"), None);
    }
}
