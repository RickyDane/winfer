//! The Connect service: settings, identity, peers, their links and pairing sessions. The app (or
//! `winferd`) owns one and gets [`Event`]s on a callback; everything here runs on its own tokio
//! runtime, so callers never block on the network.

use crate::crypto::now_ms;
use crate::link::{self, Link, LinkEvent, LinkState};
use crate::pairing::{Kind, Me, PairError, PairLink, Paired, Pairing, Side};
use crate::peers::{Config, Peer, Persist};
use crate::keys::{LinkKeys, RoomKeys};
use crate::lan::{self, Discovery, Incoming, Nearby, Want};
use crate::transport::{self, TEvent, Transport};
use crate::wire::{self, Frame};
use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

#[derive(Debug, Clone)]
pub enum Event {
    /// Status changed (peers, links, pairing): re-read [`Connect::status`].
    Changed,
    PeerUp(String),
    PeerDown(String),
    /// This machine forgot a peer: unpaired here, or the peer revoked the pairing.
    Unpaired(String),
    Message { peer: String, msg: Value },
}

#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PeerView {
    pub peer_id: String,
    pub name: String,
    pub kind: Kind,
    pub created_at: u64,
    pub last_seen: Option<u64>,
    pub notify: bool,
    pub link: LinkState,
}

/// An unpaired instance on this network.
#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct NearbyView {
    pub id: String,
    pub name: String,
    pub kind: Kind,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum PairStatus {
    /// Waiting for the other side (a link host waits for it to be pasted).
    Waiting { side: &'static str, link: Option<String>, expires_at: Option<u64> },
    /// Both sides show this code; waiting for the user (and the other side) to confirm.
    /// `incoming`: the other side asked (a nearby instance clicked Pair).
    Code { code: String, peer_name: String, peer_kind: Kind, confirmed: bool, incoming: bool },
    Done { peer_name: String },
    Failed { error: String },
}

#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub enabled: bool,
    pub discoverable: bool,
    pub instance_id: String,
    pub name: String,
    pub relay_url: String,
    pub peers: Vec<PeerView>,
    pub pairing: Option<PairStatus>,
    /// Unpaired instances on this network.
    pub nearby: Vec<NearbyView>,
    /// The LAN port, while listening.
    pub lan_port: Option<u16>,
    pub lan_error: Option<String>,
}

struct PairSession {
    id: u64,
    pairing: Pairing,
    transport: Option<Transport>,
    status: PairStatus,
    /// Confirms by itself once the code is in (a server joining with a link).
    auto_confirm: bool,
    relay_url: String,
    /// Asked by the other side over the LAN.
    incoming: bool,
}

#[derive(Default)]
struct Lan {
    port: Option<u16>,
    discovery: Option<Discovery>,
    error: Option<String>,
    /// Bumped on every start and stop (never reset), so a stale listener's connections are dropped.
    generation: u64,
    listener: Option<tokio::task::AbortHandle>,
}

pub type OnEvent = Arc<dyn Fn(Event) + Send + Sync>;

struct Inner {
    rt: tokio::runtime::Runtime,
    persist: Persist,
    kind: Kind,
    config: Mutex<Config>,
    peers: Mutex<Vec<Peer>>,
    links: Mutex<HashMap<String, Link>>,
    pairing: Mutex<Option<PairSession>>,
    pair_seq: Mutex<u64>,
    lan: Mutex<Lan>,
    /// Held while writing the peers, so saves land in order.
    saving: Mutex<()>,
    on: OnEvent,
}

/// Cheap to clone; all clones are the same service.
#[derive(Clone)]
pub struct Connect(Arc<Inner>);

impl Connect {
    pub fn new(persist: Persist, kind: Kind, on: OnEvent) -> Connect {
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).thread_name("winfer-connect").enable_all().build().expect("tokio runtime");
        let config = persist.config();
        Connect(Arc::new(Inner { rt, persist, kind, config: Mutex::new(config), peers: Mutex::default(), links: Mutex::default(), pairing: Mutex::default(), pair_seq: Mutex::new(0), lan: Mutex::default(), saving: Mutex::default(), on }))
    }

    pub fn runtime(&self) -> tokio::runtime::Handle {
        self.0.rt.handle().clone()
    }

    pub fn me(&self) -> Me {
        Persist::me(&self.0.config.lock().unwrap(), self.0.kind)
    }

    pub fn config(&self) -> Config {
        self.0.config.lock().unwrap().clone()
    }

    fn changed(&self) {
        (self.0.on)(Event::Changed);
    }

    /// Reads the peers (may wait on a keychain prompt) and connects to them when enabled. Call once.
    pub fn start(&self) {
        let loaded = self.0.persist.peers(&self.0.config.lock().unwrap().clone());
        {
            let mut peers = self.0.peers.lock().unwrap();
            if peers.is_empty() {
                *peers = loaded;
            }
        }
        self.changed();
        self.connect_all();
        self.start_lan();
    }

    fn connect_all(&self) {
        if !self.0.config.lock().unwrap().enabled {
            return;
        }
        let peers = self.0.peers.lock().unwrap().clone();
        for peer in &peers {
            self.connect(peer);
        }
    }

    fn connect(&self, peer: &Peer) {
        let Some(keys) = peer.keys() else { return };
        let me = self.me();
        let relay_url = peer.relay_url.clone().unwrap_or_else(|| self.0.config.lock().unwrap().relay_url());
        let (tx, rx) = mpsc::unbounded_channel();
        let relay = transport::relay(self.0.rt.handle(), &relay_url, &keys.room, &me.id, tx);
        let this = Arc::downgrade(&self.0);
        let on: link::OnLink = Arc::new(move |ev| {
            if let Some(inner) = this.upgrade() {
                Connect(inner).on_link(ev);
            }
        });
        let link = link::spawn(self.0.rt.handle(), me.id, peer.peer_id.clone(), keys, (relay, rx), on);
        self.0.links.lock().unwrap().insert(peer.peer_id.clone(), link);
    }

    fn on_link(&self, ev: LinkEvent) {
        match ev {
            LinkEvent::Up { peer } => {
                self.touch(&peer);
                (self.0.on)(Event::PeerUp(peer));
                self.changed();
            }
            LinkEvent::Down { peer } => {
                (self.0.on)(Event::PeerDown(peer));
                self.changed();
            }
            LinkEvent::Message { peer, msg } => (self.0.on)(Event::Message { peer, msg }),
            LinkEvent::Revoked { peer } => {
                self.forget(&peer, false);
            }
            LinkEvent::Changed { .. } => self.changed(),
        }
    }

    fn touch(&self, peer_id: &str) {
        {
            let mut peers = self.0.peers.lock().unwrap();
            let Some(p) = peers.iter_mut().find(|p| p.peer_id == peer_id) else { return };
            p.last_seen = Some(now_ms());
        }
        self.save();
    }

    /// Writes the peers off the calling thread (the keychain may prompt). Each write takes the
    /// list as it is then, one at a time, so an older snapshot never lands after a newer one.
    fn save(&self) {
        let this = self.clone();
        std::thread::spawn(move || {
            let _saving = this.0.saving.lock().unwrap();
            let peers = this.peers();
            let mut config = this.0.config.lock().unwrap().clone();
            let _ = this.0.persist.save_peers(&mut config, &peers);
            this.0.config.lock().unwrap().has_peers = config.has_peers;
        });
    }

    fn save_config(&self) {
        let config = self.0.config.lock().unwrap().clone();
        let _ = config.save(&self.0.persist.config_path);
    }

    pub fn status(&self) -> Status {
        let config = self.0.config.lock().unwrap().clone();
        let links = self.0.links.lock().unwrap();
        let peers = self
            .0
            .peers
            .lock()
            .unwrap()
            .iter()
            .map(|p| PeerView {
                peer_id: p.peer_id.clone(),
                name: p.name.clone(),
                kind: p.kind,
                created_at: p.created_at,
                last_seen: p.last_seen,
                notify: p.notify,
                link: links.get(&p.peer_id).map(Link::state).unwrap_or_default(),
            })
            .collect();
        let paired: Vec<String> = self.0.peers.lock().unwrap().iter().map(|p| p.peer_id.clone()).collect();
        let (nearby, lan_port, lan_error) = {
            let lan = self.0.lan.lock().unwrap();
            let nearby = lan.discovery.as_ref().map(Discovery::list).unwrap_or_default();
            (
                nearby.into_iter().filter(|n| !paired.contains(&n.id)).map(|n| NearbyView { id: n.id, name: n.name, kind: n.kind }).collect(),
                lan.port,
                lan.error.clone(),
            )
        };
        Status {
            nearby,
            lan_port,
            lan_error,
            enabled: config.enabled,
            discoverable: config.discoverable,
            instance_id: config.instance_id.clone(),
            name: Persist::me(&config, self.0.kind).name,
            relay_url: config.relay_url(),
            peers,
            pairing: self.0.pairing.lock().unwrap().as_ref().map(|s| s.status.clone()),
        }
    }

    pub fn peers(&self) -> Vec<Peer> {
        self.0.peers.lock().unwrap().clone()
    }

    pub fn is_up(&self, peer: &str) -> bool {
        self.0.links.lock().unwrap().get(peer).is_some_and(Link::is_up)
    }

    /// Sends to one peer; dropped when its link is down.
    pub fn send(&self, peer: &str, msg: Value) {
        if let Some(link) = self.0.links.lock().unwrap().get(peer) {
            link.send(msg);
        }
    }

    /// Hands a LAN connection to a paired peer's link.
    pub fn add_lan(&self, peer: &str, transport: Transport, events: mpsc::UnboundedReceiver<TEvent>) -> bool {
        match self.0.links.lock().unwrap().get(peer) {
            Some(link) => {
                link.add_lan(transport, events);
                true
            }
            None => false,
        }
    }

    pub fn set_enabled(&self, enabled: bool) {
        self.0.config.lock().unwrap().enabled = enabled;
        self.save_config();
        if enabled {
            if self.0.links.lock().unwrap().is_empty() {
                self.connect_all();
            }
            self.start_lan();
        } else {
            self.0.links.lock().unwrap().clear();
            let mut lan = self.0.lan.lock().unwrap();
            if let Some(listener) = lan.listener.take() {
                listener.abort();
            }
            *lan = Lan { generation: lan.generation + 1, ..Lan::default() };
        }
        self.changed();
    }

    pub fn set_discoverable(&self, on: bool) {
        self.0.config.lock().unwrap().discoverable = on;
        self.save_config();
        self.advertise();
        self.changed();
    }

    pub fn rename(&self, name: &str) {
        let name = name.trim();
        self.0.config.lock().unwrap().name = (!name.is_empty()).then(|| name.chars().take(64).collect());
        self.save_config();
        self.advertise();
        self.changed();
    }

    /// A new relay URL reconnects every link.
    pub fn set_relay_url(&self, url: &str) -> Result<(), String> {
        let url = url.trim();
        if !url.is_empty() && !(url.starts_with("https://") || url.starts_with("http://")) {
            return Err("The relay URL must start with https://".into());
        }
        let old = self.0.config.lock().unwrap().relay_url();
        self.0.config.lock().unwrap().relay_url = (!url.is_empty()).then(|| url.trim_end_matches('/').to_string());
        self.save_config();
        // Existing pairings stay on the relay they met on (the other side didn't change); new
        // pairings use the new one.
        let new = self.0.config.lock().unwrap().relay_url();
        let peers = {
            let mut peers = self.0.peers.lock().unwrap();
            for p in peers.iter_mut().filter(|p| p.relay_url.is_none()) {
                p.relay_url = Some(old.clone());
            }
            for p in peers.iter_mut().filter(|p| p.relay_url.as_deref() == Some(new.as_str())) {
                p.relay_url = None;
            }
            peers.clone()
        };
        if !peers.is_empty() {
            self.save();
        }
        self.0.links.lock().unwrap().clear();
        self.connect_all();
        self.changed();
        Ok(())
    }

    pub fn set_notify(&self, peer_id: &str, notify: bool) {
        if let Some(p) = self.0.peers.lock().unwrap().iter_mut().find(|p| p.peer_id == peer_id) {
            p.notify = notify;
        }
        self.save();
        self.changed();
    }

    pub fn rename_peer(&self, peer_id: &str, name: &str) {
        if let Some(p) = self.0.peers.lock().unwrap().iter_mut().find(|p| p.peer_id == peer_id) {
            p.name = name.trim().chars().take(64).collect();
        }
        self.save();
        self.changed();
    }

    /// Unpairs: tells the peer, wipes the relay room, forgets the key.
    pub fn revoke(&self, peer_id: &str) {
        self.send(peer_id, serde_json::json!({"t": "bye"}));
        self.forget(peer_id, true);
    }

    fn forget(&self, peer_id: &str, wipe_room: bool) {
        let peer = {
            let mut peers = self.0.peers.lock().unwrap();
            let Some(i) = peers.iter().position(|p| p.peer_id == peer_id) else { return };
            peers.remove(i)
        };
        self.save();
        let link = self.0.links.lock().unwrap().remove(peer_id);
        let relay_url = peer.relay_url.clone().unwrap_or_else(|| self.0.config.lock().unwrap().relay_url());
        // Give the `bye` a moment on the wire before the link and room go.
        self.0.rt.spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            drop(link);
            if wipe_room && let Some(keys) = peer.keys() {
                let _ = tokio::task::spawn_blocking(move || transport::delete_room(&relay_url, &keys.room)).await;
            }
        });
        (self.0.on)(Event::Unpaired(peer_id.to_string()));
        (self.0.on)(Event::PeerDown(peer_id.to_string()));
        self.changed();
    }

    // ---- pairing ----

    fn next_pair_id(&self) -> u64 {
        let mut seq = self.0.pair_seq.lock().unwrap();
        *seq += 1;
        *seq
    }

    /// Starts a link pairing as the host; returns the link to hand to the other instance.
    pub fn pair_create_link(&self) -> String {
        self.pair_create_link_with(false)
    }

    /// Like [`Self::pair_create_link`]; `auto_confirm`: confirm the code by itself (the link's key
    /// already proves the other side, so this is for an agent that handed the link on itself).
    pub fn pair_create_link_with(&self, auto_confirm: bool) -> String {
        let me = self.me();
        let relay_url = self.0.config.lock().unwrap().relay_url();
        let (link, key) = PairLink::new(&me, &relay_url, now_ms());
        let url = link.to_url();
        let status = PairStatus::Waiting { side: "host", link: Some(url.clone()), expires_at: Some(link.x) };
        self.start_link_pairing(Side::Host, me, key, &relay_url, status, auto_confirm, link.x);
        url
    }

    /// Joins a pairing from a pasted link. `auto_confirm`: confirm without asking (servers).
    pub fn pair_join_link(&self, text: &str, auto_confirm: bool) -> Result<(), String> {
        let link = PairLink::parse(text).ok_or("That isn't a winfer pair link.")?;
        if now_ms() > link.x {
            return Err("This pair link has expired. Make a new one.".into());
        }
        let me = self.me();
        if link.i == me.id {
            return Err("That's this winfer's own pair link.".into());
        }
        if self.0.peers.lock().unwrap().iter().any(|p| p.peer_id == link.i) {
            return Err(format!("Already paired with {}.", link.n));
        }
        let key = link.key().ok_or("That isn't a winfer pair link.")?;
        let status = PairStatus::Waiting { side: "joiner", link: None, expires_at: Some(link.x) };
        self.start_link_pairing(Side::Joiner, me, key, &link.u, status, auto_confirm, link.x);
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn start_link_pairing(&self, side: Side, me: Me, key: [u8; 32], relay_url: &str, status: PairStatus, auto_confirm: bool, expires: u64) {
        let keys = LinkKeys::derive(&key);
        let (tx, rx) = mpsc::unbounded_channel();
        let transport = transport::relay(self.0.rt.handle(), relay_url, &keys.room, &me.id, tx);
        let pairing = Pairing::new(side, me, Some(key));
        self.run_pairing(pairing, transport, rx, status, auto_confirm, relay_url, Some(keys.room), expires, false);
    }

    /// Runs a pairing session on a transport until it's done, failed, cancelled or expired.
    #[allow(clippy::too_many_arguments)]
    fn run_pairing(&self, pairing: Pairing, transport: Transport, mut rx: mpsc::UnboundedReceiver<TEvent>, status: PairStatus, auto_confirm: bool, relay_url: &str, room: Option<RoomKeys>, expires: u64, incoming: bool) {
        let id = self.next_pair_id();
        *self.0.pairing.lock().unwrap() = Some(PairSession { id, pairing, transport: Some(transport), status, auto_confirm, relay_url: relay_url.to_string(), incoming });
        self.changed();
        let this = self.clone();
        let relay_url = relay_url.to_string();
        self.0.rt.spawn(async move {
            let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(expires.saturating_sub(now_ms()));
            loop {
                let ev = match tokio::time::timeout_at(deadline, rx.recv()).await {
                    Ok(ev) => ev,
                    Err(_) => {
                        this.pair_fail(id, "The pairing expired.");
                        break;
                    }
                };
                let Some(ev) = ev else { break };
                if let TEvent::Closed { .. } = ev
                    && room.is_none()
                {
                    // A LAN pairing's socket went away.
                    this.pair_fail(id, "The other machine went away.");
                    break;
                }
                if !this.on_pair_event(id, ev) {
                    break;
                }
            }
            // The one-time room has done its job.
            if let Some(room) = room {
                let _ = tokio::task::spawn_blocking(move || transport::delete_room(&relay_url, &room)).await;
            }
        });
    }

    /// Returns false when the session is over.
    fn on_pair_event(&self, id: u64, ev: TEvent) -> bool {
        let mut guard = self.0.pairing.lock().unwrap();
        let Some(session) = guard.as_mut().filter(|s| s.id == id) else { return false };
        let result: Result<crate::pairing::Out, PairError> = match ev {
            TEvent::Presence(2) => Ok(session.pairing.start()),
            TEvent::Frame(text) => match wire::parse(&text) {
                Some(Frame::Handshake(msg)) => session.pairing.on_message(&msg),
                _ => Ok(Default::default()),
            },
            _ => Ok(Default::default()),
        };
        let mut out = match result {
            Ok(out) => out,
            Err(e) => {
                let error = match e {
                    PairError::Cancelled(_) => "The other side cancelled.".to_string(),
                    PairError::BadLink => "The pair link didn't match.".to_string(),
                    PairError::BadConfirm => "The codes didn't match.".to_string(),
                    PairError::BadCommit => "The other side changed its key mid-pairing.".to_string(),
                    _ => format!("Pairing failed ({e:?})."),
                };
                drop(guard);
                self.pair_fail(id, &error);
                return false;
            }
        };
        if out.code.is_some()
            && session.auto_confirm
            && let Ok(c) = session.pairing.confirm()
        {
            out.send.extend(c.send);
            out.paired = out.paired.or(c.paired);
        }
        let Some(transport) = session.transport.as_ref() else { return false };
        for msg in &out.send {
            transport.send(wire::handshake(msg));
        }
        if let Some(code) = out.code
            && let Some((_, name, kind)) = session.pairing.peer()
        {
            session.status = PairStatus::Code { code, peer_name: name.to_string(), peer_kind: kind, confirmed: session.auto_confirm, incoming: session.incoming };
        }
        let paired = out.paired;
        drop(guard);
        if let Some(paired) = paired {
            self.pair_done(id, paired);
            return false;
        }
        self.changed();
        true
    }

    /// The user checked the code.
    pub fn pair_confirm(&self) {
        let paired = {
            let mut guard = self.0.pairing.lock().unwrap();
            let Some(session) = guard.as_mut() else { return };
            let Ok(out) = session.pairing.confirm() else { return };
            if let Some(t) = session.transport.as_ref() {
                for msg in &out.send {
                    t.send(wire::handshake(msg));
                }
            }
            if let PairStatus::Code { confirmed, .. } = &mut session.status {
                *confirmed = true;
            }
            out.paired.map(|p| (session.id, p))
        };
        match paired {
            Some((id, p)) => self.pair_done(id, p),
            None => self.changed(),
        }
    }

    pub fn pair_cancel(&self) {
        let session = self.0.pairing.lock().unwrap().take();
        if let Some(session) = session
            && let Some(t) = session.transport
        {
            t.send(wire::handshake(&Pairing::cancel("cancelled")));
            // Let the cancel go out before the transport closes.
            self.0.rt.spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                drop(t);
            });
        }
        self.changed();
    }

    /// Clears a finished or failed pairing from the status.
    pub fn pair_dismiss(&self) {
        let mut guard = self.0.pairing.lock().unwrap();
        if guard.as_ref().is_some_and(|s| matches!(s.status, PairStatus::Done { .. } | PairStatus::Failed { .. })) {
            *guard = None;
        }
        drop(guard);
        self.changed();
    }

    fn pair_fail(&self, id: u64, error: &str) {
        if let Some(session) = self.0.pairing.lock().unwrap().as_mut().filter(|s| s.id == id) {
            session.status = PairStatus::Failed { error: error.into() };
            session.transport = None;
        }
        self.changed();
    }

    fn pair_done(&self, id: u64, paired: Paired) {
        let (transport, relay_url) = {
            let mut guard = self.0.pairing.lock().unwrap();
            let Some(session) = guard.as_mut().filter(|s| s.id == id) else { return };
            session.status = PairStatus::Done { peer_name: paired.name.clone() };
            (session.transport.take(), session.relay_url.clone())
        };
        // The last confirmation needs to reach the other side before the room closes.
        self.0.rt.spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            drop(transport);
        });
        let own = self.0.config.lock().unwrap().relay_url();
        self.add_peer(paired, (relay_url != own).then_some(relay_url));
    }

    /// Stores a finished pairing and connects to it (also used by LAN pairing). `relay_url`: the
    /// pairing's relay when it isn't this instance's own.
    pub fn add_peer(&self, paired: Paired, relay_url: Option<String>) {
        let mut peer = Peer::from_paired(&paired, now_ms());
        peer.relay_url = relay_url;
        {
            let mut peers = self.0.peers.lock().unwrap();
            peers.retain(|p| p.peer_id != peer.peer_id);
            peers.push(peer.clone());
        }
        self.save();
        self.0.links.lock().unwrap().remove(&peer.peer_id);
        if self.0.config.lock().unwrap().enabled {
            self.connect(&peer);
        }
        self.changed();
    }
    // ---- LAN ----

    /// Listens and starts mDNS (when enabled); safe to call again.
    fn start_lan(&self) {
        if !self.0.config.lock().unwrap().enabled {
            return;
        }
        let generation = {
            let mut lan = self.0.lan.lock().unwrap();
            if lan.port.is_some() {
                return;
            }
            lan.generation += 1;
            lan.generation
        };
        let this = self.clone();
        let rt = self.0.rt.handle().clone();
        self.0.rt.spawn(async move {
            let weak = Arc::downgrade(&this.0);
            let on_incoming: Arc<dyn Fn(Incoming) + Send + Sync> = Arc::new(move |inc| {
                if let Some(inner) = weak.upgrade() {
                    Connect(inner).on_incoming(inc, generation);
                }
            });
            let (port, listener) = match lan::listen(rt, on_incoming).await {
                Ok(l) => l,
                Err(e) => {
                    this.0.lan.lock().unwrap().error = Some(format!("can't listen: {e}"));
                    this.changed();
                    return;
                }
            };
            let weak = Arc::downgrade(&this.0);
            let on_nearby: Arc<dyn Fn(&Nearby) + Send + Sync> = Arc::new(move |n| {
                if let Some(inner) = weak.upgrade() {
                    Connect(inner).on_nearby(n);
                }
            });
            let discovery = Discovery::start(this.me().id, on_nearby);
            {
                let mut lan = this.0.lan.lock().unwrap();
                if lan.generation != generation {
                    listener.abort();
                    return;
                }
                lan.port = Some(port);
                lan.listener = Some(listener);
                match discovery {
                    Ok(d) => lan.discovery = Some(d),
                    Err(e) => lan.error = Some(format!("mDNS: {e}")),
                }
            }
            this.advertise();
            this.changed();
            // Paired instances that were already announced before we listened, and retries.
            let mut tick = tokio::time::interval(std::time::Duration::from_secs(20));
            loop {
                tick.tick().await;
                if this.0.lan.lock().unwrap().generation != generation {
                    break;
                }
                for peer in this.peers() {
                    this.maybe_dial(&peer.peer_id);
                }
            }
        });
    }

    fn advertise(&self) {
        let config = self.0.config.lock().unwrap().clone();
        let me = Persist::me(&config, self.0.kind);
        let lan = self.0.lan.lock().unwrap();
        let Some(discovery) = lan.discovery.as_ref() else { return };
        if config.enabled && config.discoverable && let Some(port) = lan.port {
            let _ = discovery.advertise(&me.id, &me.name, self.0.kind, port);
        } else {
            discovery.withdraw();
        }
    }

    fn on_nearby(&self, n: &Nearby) {
        self.maybe_dial(&n.id);
        self.changed();
    }

    /// Opens a LAN connection to a paired instance that's on this network and not on the LAN yet.
    /// The one with the smaller id dials (or whoever isn't advertising, as nobody can find it).
    fn maybe_dial(&self, peer_id: &str) {
        let me = self.me();
        let discoverable = self.0.config.lock().unwrap().discoverable;
        if me.id.as_str() > peer_id && discoverable {
            return;
        }
        let Some(target) = self.0.lan.lock().unwrap().discovery.as_ref().and_then(|d| d.get(peer_id)) else { return };
        let lan_up = self.0.links.lock().unwrap().get(peer_id).map(|l| l.state().lan);
        if lan_up != Some(false) {
            return; // not paired, or already on the LAN
        }
        let this = self.clone();
        let peer = peer_id.to_string();
        self.0.rt.spawn(async move {
            for addr in target.addrs {
                if let Ok((transport, events)) = lan::dial(&this.0.rt.handle().clone(), addr, &me.id, Want::Link).await {
                    this.add_lan(&peer, transport, events);
                    this.changed();
                    return;
                }
            }
        });
    }

    fn on_incoming(&self, inc: Incoming, generation: u64) {
        if self.0.lan.lock().unwrap().generation != generation || !self.0.config.lock().unwrap().enabled {
            return;
        }
        match inc.want {
            Want::Link => {
                // Only paired instances get a link; the handshake then proves it.
                self.add_lan(&inc.from, inc.transport, inc.events);
                self.changed();
            }
            Want::Pair => {
                let config = self.0.config.lock().unwrap().clone();
                let busy = self.0.pairing.lock().unwrap().as_ref().is_some_and(|s| !matches!(s.status, PairStatus::Done { .. } | PairStatus::Failed { .. }));
                if !config.discoverable || busy {
                    inc.transport.send(wire::handshake(&Pairing::cancel(if busy { "busy" } else { "not discoverable" })));
                    let t = inc.transport;
                    self.0.rt.spawn(async move {
                        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                        drop(t);
                    });
                    return;
                }
                let me = Persist::me(&config, self.0.kind);
                let pairing = Pairing::new(Side::Host, me, None);
                let status = PairStatus::Waiting { side: "host", link: None, expires_at: None };
                self.run_pairing(pairing, inc.transport, inc.events, status, false, &config.relay_url(), None, now_ms() + 2 * 60 * 1000, true);
            }
        }
    }

    /// Asks a nearby instance to pair; both then show a code.
    pub fn pair_nearby(&self, id: &str) -> Result<(), String> {
        let target = self.0.lan.lock().unwrap().discovery.as_ref().and_then(|d| d.get(id)).ok_or("That machine isn't on this network anymore.")?;
        self.pair_addrs(target.name, target.addrs)
    }

    /// Asks the instance at `host:port` to pair (when mDNS can't see it, e.g. in a container).
    pub fn pair_address(&self, address: &str) -> Result<(), String> {
        use std::net::ToSocketAddrs;
        let addrs: Vec<std::net::SocketAddr> = address.trim().to_socket_addrs().map_err(|e| format!("{address}: {e}"))?.collect();
        if addrs.is_empty() {
            return Err(format!("{address}: no address"));
        }
        self.pair_addrs(address.trim().to_string(), addrs)
    }

    fn pair_addrs(&self, name: String, addrs: Vec<std::net::SocketAddr>) -> Result<(), String> {
        if !self.0.config.lock().unwrap().enabled {
            return Err("Connect is off.".into());
        }
        let target = Nearby { id: String::new(), name, kind: Kind::Desktop, addrs, seen: std::time::Instant::now() };
        let me = self.me();
        let this = self.clone();
        let status = PairStatus::Waiting { side: "joiner", link: None, expires_at: Some(now_ms() + 2 * 60 * 1000) };
        // Show "reaching…" at once; the session starts when the connection is up.
        let placeholder = self.next_pair_id();
        *self.0.pairing.lock().unwrap() = Some(PairSession { id: placeholder, pairing: Pairing::new(Side::Joiner, me.clone(), None), transport: None, status: status.clone(), auto_confirm: false, relay_url: String::new(), incoming: false });
        self.changed();
        self.0.rt.spawn(async move {
            for addr in &target.addrs {
                if let Ok((transport, events)) = lan::dial(&this.0.rt.handle().clone(), *addr, &me.id, Want::Pair).await {
                    if this.0.pairing.lock().unwrap().as_ref().is_some_and(|s| s.id == placeholder) {
                        let relay = this.0.config.lock().unwrap().relay_url();
                        this.run_pairing(Pairing::new(Side::Joiner, me, None), transport, events, status, false, &relay, None, now_ms() + 2 * 60 * 1000, false);
                    }
                    return;
                }
            }
            this.pair_fail(placeholder, &format!("Couldn't reach {}.", target.name));
        });
        Ok(())
    }
}
