//! One logical connection to a paired instance over the best transport it has: the relay is kept
//! up as the fallback, a LAN connection is preferred while it's there. Each transport runs its own
//! handshake. When the transport carrying traffic changes, `LinkEvent::Up` fires again so the
//! layers above resubscribe (mirrors, terminals and chats re-snapshot; nothing is replayed).

use crate::handshake::{Handshake, Role};
use crate::keys::PairKeys;
use crate::transport::{TEvent, Transport, Via};
use crate::wire::{self, Channel, Frame};
use serde::Serialize;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

const HS_RETRY: Duration = Duration::from_secs(5);

/// `WINFER_CONNECT_TRACE=1` prints the link's steps to stderr.
fn trace(text: impl FnOnce() -> String) {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if *ON.get_or_init(|| std::env::var_os("WINFER_CONNECT_TRACE").is_some()) {
        eprintln!("[link] {}", text());
    }
}

fn ping_every(via: Via) -> Duration {
    match via {
        Via::Lan => Duration::from_secs(5),
        Via::Relay => Duration::from_secs(30),
    }
}

#[derive(Clone, Debug, Default, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LinkState {
    /// The transport carrying traffic; None while the link is down.
    pub via: Option<&'static str>,
    pub relay_connected: bool,
    pub lan: bool,
    pub rtt_ms: Option<u32>,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub enum LinkEvent {
    Up { peer: String },
    Down { peer: String },
    Message { peer: String, msg: Value },
    /// The relay room was deleted, or the peer said `bye`: the pairing is gone.
    Revoked { peer: String },
    /// State (transport, RTT, errors) changed.
    Changed { peer: String },
}

pub type OnLink = Arc<dyn Fn(LinkEvent) + Send + Sync>;

enum Cmd {
    Send(Value),
    AddLan(Transport, mpsc::UnboundedReceiver<TEvent>),
    Stop,
}

pub struct Link {
    pub peer: String,
    cmds: mpsc::UnboundedSender<Cmd>,
    pub state: Arc<Mutex<LinkState>>,
}

impl Link {
    /// Messages sent while the link is down are dropped; callers resend on `Up`.
    pub fn send(&self, msg: Value) {
        let _ = self.cmds.send(Cmd::Send(msg));
    }

    /// Hands over a LAN connection to this peer (dialed or accepted).
    pub fn add_lan(&self, transport: Transport, events: mpsc::UnboundedReceiver<TEvent>) {
        let _ = self.cmds.send(Cmd::AddLan(transport, events));
    }

    pub fn is_up(&self) -> bool {
        self.state.lock().unwrap().via.is_some()
    }

    pub fn state(&self) -> LinkState {
        self.state.lock().unwrap().clone()
    }
}

impl Drop for Link {
    fn drop(&mut self) {
        let _ = self.cmds.send(Cmd::Stop);
    }
}

struct Path {
    via: Via,
    transport: Transport,
    hs: Handshake,
    channel: Option<Channel>,
    present: bool,
    hs_sent: Option<Instant>,
    ping_sent: Option<(u64, Instant)>,
    last_ping: Instant,
    missed: u32,
}

impl Path {
    fn new(via: Via, transport: Transport, me: &str, peer: &str, keys: PairKeys) -> Path {
        Path { via, transport, hs: Handshake::new(me, peer, keys), channel: None, present: false, hs_sent: None, ping_sent: None, last_ping: Instant::now(), missed: 0 }
    }

    fn start(&mut self) {
        trace(|| format!("{:?} start ({:?})", self.via, self.hs.role));
        self.channel = None;
        self.missed = 0;
        self.ping_sent = None;
        let hello = self.hs.start();
        self.hs_sent = Some(Instant::now());
        self.transport.send(wire::handshake(&hello));
    }

    fn send(&mut self, msg: &Value) -> bool {
        let Some(channel) = self.channel.as_mut() else { return false };
        for frame in channel.seal(msg.to_string().as_bytes()) {
            if !self.transport.send(frame) {
                return false;
            }
        }
        true
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

/// Starts the link task: `relay` is the always-on relay transport and its events.
pub fn spawn(rt: &tokio::runtime::Handle, me: String, peer: String, keys: PairKeys, relay: (Transport, mpsc::UnboundedReceiver<TEvent>), on: OnLink) -> Link {
    let (cmds, mut cmd_rx) = mpsc::unbounded_channel();
    let state = Arc::new(Mutex::new(LinkState::default()));
    let shared = state.clone();
    let peer_id = peer.clone();
    rt.spawn(async move {
        let (relay_t, mut relay_rx) = relay;
        let mut relay = Path::new(Via::Relay, relay_t, &me, &peer, keys.clone());
        let mut lan: Option<(Path, mpsc::UnboundedReceiver<TEvent>)> = None;
        let mut active: Option<Via> = None;
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        // The state last reported, so data frames and ticks don't each fire Changed.
        let mut reported = LinkState::default();

        // Which ready path carries traffic; fires Up/Down/Changed as that changes.
        let settle = |relay: &Path, lan: &Option<(Path, mpsc::UnboundedReceiver<TEvent>)>, active: &mut Option<Via>, reported: &mut LinkState| {
            let best = if lan.as_ref().is_some_and(|(p, _)| p.channel.is_some()) {
                Some(Via::Lan)
            } else if relay.channel.is_some() {
                Some(Via::Relay)
            } else {
                None
            };
            let now = {
                let mut s = shared.lock().unwrap();
                s.via = best.map(|v| if v == Via::Lan { "lan" } else { "relay" });
                s.lan = lan.as_ref().is_some_and(|(p, _)| p.channel.is_some());
                s.clone()
            };
            if best != *active {
                trace(|| format!("active {:?} -> {best:?}", *active));
                *active = best;
                on(if best.is_some() { LinkEvent::Up { peer: peer.clone() } } else { LinkEvent::Down { peer: peer.clone() } });
            }
            if now != *reported {
                *reported = now;
                on(LinkEvent::Changed { peer: peer.clone() });
            }
        };

        // Handles one transport event on a path; returns false when the pairing is revoked.
        let handle = |path: &mut Path, ev: TEvent, shared: &Mutex<LinkState>| -> Result<(), ()> {
            match ev {
                TEvent::Open => {
                    if path.via == Via::Relay {
                        shared.lock().unwrap().relay_connected = true;
                    }
                }
                TEvent::Presence(n) => {
                    let present = n >= 2;
                    if present && !path.present {
                        path.present = true;
                        path.start();
                    } else if !present {
                        path.present = false;
                        path.channel = None;
                    }
                }
                TEvent::Closed { code, error } => {
                    trace(|| format!("{:?} closed {code:?} {error:?}", path.via));
                    path.present = false;
                    path.channel = None;
                    if path.via == Via::Relay {
                        let mut s = shared.lock().unwrap();
                        s.relay_connected = false;
                        s.error = error;
                    }
                    if code == Some(4001) {
                        return Err(());
                    }
                }
                TEvent::Frame(text) => match wire::parse(&text) {
                    Some(Frame::Handshake(msg)) => {
                        trace(|| format!("{:?} got {}", path.via, msg["t"]));
                        let step = path.hs.on_message(&msg);
                        match step {
                        Ok(step) => {
                            if let Some(reply) = step.reply {
                                path.transport.send(wire::handshake(&reply));
                            }
                            if msg["t"] == "hs0" {
                                // The responder lost its side: whatever channel we had is over.
                                path.channel = None;
                                path.hs_sent = Some(Instant::now());
                            } else if step.channel.is_some() {
                                path.channel = step.channel;
                                path.hs_sent = None;
                                path.missed = 0;
                                shared.lock().unwrap().error = None;
                            } else if path.hs.role == Role::Responder {
                                // Answered a new hs1: the old channel is over.
                                path.channel = None;
                            }
                        }
                        Err(e) => {
                            trace(|| format!("{:?} handshake error {e:?}", path.via));
                            shared.lock().unwrap().error = Some(format!("handshake: {e:?}"));
                        }
                        }
                    }
                    Some(Frame::Data(sealed)) => {
                        let Some(channel) = path.channel.as_mut() else {
                            trace(|| format!("{:?} data without a channel, dropped", path.via));
                            return Ok(());
                        };
                        match channel.open(&sealed) {
                            Ok(Some(bytes)) => {
                                let Ok(msg) = serde_json::from_slice::<Value>(&bytes) else { return Ok(()) };
                                match msg["t"].as_str() {
                                    Some("_ping") => {
                                        path.send(&json!({"t": "_pong", "ts": msg["ts"]}));
                                    }
                                    Some("_pong") => {
                                        if let Some((ts, at)) = path.ping_sent.take()
                                            && msg["ts"].as_u64() == Some(ts)
                                        {
                                            path.missed = 0;
                                            shared.lock().unwrap().rtt_ms = Some(at.elapsed().as_millis() as u32);
                                        }
                                    }
                                    Some("bye") => return Err(()),
                                    _ => on(LinkEvent::Message { peer: peer.clone(), msg }),
                                }
                            }
                            Ok(None) => {}
                            Err(e) => {
                                trace(|| format!("{:?} open failed {e:?}", path.via));
                                // Out of step: start over.
                                path.channel = None;
                                if path.present {
                                    path.start();
                                }
                            }
                        }
                    }
                    None => {}
                },
            }
            Ok(())
        };

        let revoked = loop {
            let lan_ev = async {
                match lan.as_mut() {
                    Some((_, rx)) => rx.recv().await,
                    None => std::future::pending().await,
                }
            };
            tokio::select! {
                ev = relay_rx.recv() => {
                    let Some(ev) = ev else { break false };
                    if handle(&mut relay, ev, &shared).is_err() { break true; }
                }
                ev = lan_ev => {
                    match ev {
                        Some(ev) => {
                            let (path, _) = lan.as_mut().expect("lan path");
                            if handle(path, ev, &shared).is_err() { break true; }
                            if !path.present && path.channel.is_none() && path.hs_sent.is_none() {
                                lan = None;
                            }
                        }
                        None => lan = None,
                    }
                }
                cmd = cmd_rx.recv() => match cmd {
                    Some(Cmd::Send(msg)) => {
                        trace(|| format!("send {} via {active:?}", msg["t"]));
                        let sent = match active {
                            Some(Via::Lan) => lan.as_mut().is_some_and(|(p, _)| p.send(&msg)),
                            Some(Via::Relay) => relay.send(&msg),
                            None => false,
                        };
                        if !sent && msg["t"] == "bye" {
                            // Best effort on whatever is up.
                            relay.send(&msg);
                        }
                    }
                    Some(Cmd::AddLan(transport, rx)) => {
                        trace(|| format!("add LAN (had one: {})", lan.is_some()));
                        // A working LAN path stays: anyone on the network can say hello with a
                        // paired id, and only the handshake would prove it. A dead one goes after
                        // three silent pings, and the other side dials again.
                        if lan.as_ref().is_some_and(|(p, _)| p.channel.is_some()) {
                            transport.stop();
                            continue;
                        }
                        let mut path = Path::new(Via::Lan, transport, &me, &peer, keys.clone());
                        path.present = true;
                        path.start();
                        lan = Some((path, rx));
                    }
                    Some(Cmd::Stop) | None => {
                        relay.transport.stop();
                        if let Some((p, _)) = &lan { p.transport.stop(); }
                        return;
                    }
                },
                _ = tick.tick() => {
                    let mut paths: Vec<&mut Path> = vec![&mut relay];
                    if let Some((p, _)) = lan.as_mut() { paths.push(p); }
                    for path in paths {
                        if path.present && path.channel.is_none() && path.hs_sent.is_none_or(|t| t.elapsed() > HS_RETRY) {
                            path.start();
                        }
                        if path.channel.is_some() && path.last_ping.elapsed() >= ping_every(path.via) {
                            path.last_ping = Instant::now();
                            if path.ping_sent.is_some() {
                                path.missed += 1;
                            }
                            if path.missed >= 3 {
                                // Silent for three pings: this path is dead.
                                path.channel = None;
                                if path.via == Via::Lan { path.present = false; path.transport.stop(); }
                                continue;
                            }
                            let ts = now_ms();
                            path.ping_sent = Some((ts, Instant::now()));
                            path.send(&json!({"t": "_ping", "ts": ts}));
                        }
                    }
                    if lan.as_ref().is_some_and(|(p, _)| !p.present && p.channel.is_none()) {
                        lan = None;
                    }
                }
            }
            settle(&relay, &lan, &mut active, &mut reported);
        };
        shared.lock().unwrap().via = None;
        if revoked {
            on(LinkEvent::Revoked { peer: peer.clone() });
        } else if active.is_some() {
            on(LinkEvent::Down { peer: peer.clone() });
        }
    });
    Link { peer: peer_id, cmds, state }
}
