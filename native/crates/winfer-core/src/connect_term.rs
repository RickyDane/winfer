//! Winfer Connect terminals (docs/plans/winfer-connect.md, "Remote terminal tiles").
//!
//! Host side: every pane's output is kept in a ring (what a peer gets when it attaches) and
//! copied to the peers attached to it. A peer that attaches or types takes over the pane's size.
//! Client side: a tile showing another machine's pane is a [`terminal::Session`] whose process is
//! over there: its keys and size go out as `term.in` / `term.resize`, its output comes back as
//! `term.out`. winfer code chat panes travel the same way (their process speaks JSON lines).

use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use serde_json::json;
use std::collections::{BTreeSet, HashMap, VecDeque};
use std::sync::{LazyLock, Mutex};

/// What a newly attached peer gets of a terminal's past (bytes).
const TERM_RING: usize = 1 << 20;
/// Chat panes need more: their history is JSON lines.
const CHAT_RING: usize = 8 << 20;

#[derive(Default)]
struct Tapped {
    ring: VecDeque<u8>,
    watchers: BTreeSet<String>,
}

static HOST: LazyLock<Mutex<HashMap<String, Tapped>>> = LazyLock::new(Mutex::default);
/// Panes a peer is sizing (pane → peer).
static SIZED: LazyLock<Mutex<HashMap<String, String>>> = LazyLock::new(Mutex::default);
/// Tiles showing another machine's pane: local pane id → (peer, its pane id).
static REMOTE: LazyLock<Mutex<HashMap<String, (String, String)>>> = LazyLock::new(Mutex::default);
/// Remote tiles whose size the host took back.
static RECLAIMED: LazyLock<Mutex<BTreeSet<String>>> = LazyLock::new(Mutex::default);

pub fn b64(bytes: &[u8]) -> String {
    B64.encode(bytes)
}

pub fn unb64(text: &str) -> Vec<u8> {
    B64.decode(text).unwrap_or_default()
}

// ---- host ----

/// A local pane's output (called for every chunk the process writes).
pub fn tap(pane: &str, bytes: &[u8], chat: bool) {
    if is_remote(pane) {
        return;
    }
    let mut host = HOST.lock().unwrap();
    let t = host.entry(pane.to_string()).or_default();
    t.ring.extend(bytes);
    let cap = if chat { CHAT_RING } else { TERM_RING };
    if t.ring.len() > cap {
        let mut cut = t.ring.len() - cap;
        // Chats restart on a line, so the first replayed line parses.
        if chat {
            while cut < t.ring.len() && t.ring[cut - 1] != b'\n' {
                cut += 1;
            }
        }
        t.ring.drain(..cut);
    }
    if t.watchers.is_empty() {
        return;
    }
    let msg = json!({"t": "term.out", "paneId": pane, "data": b64(bytes)});
    for peer in &t.watchers {
        crate::connect::send(peer, msg.clone());
    }
}

/// A peer attaches: what it missed, and from now on the live output (in order).
pub fn attach(pane: &str, peer: &str) -> Vec<u8> {
    let mut host = HOST.lock().unwrap();
    let t = host.entry(pane.to_string()).or_default();
    t.watchers.insert(peer.to_string());
    t.ring.iter().copied().collect()
}

pub fn detach(pane: &str, peer: &str) {
    if let Some(t) = HOST.lock().unwrap().get_mut(pane) {
        t.watchers.remove(peer);
    }
    let mut sized = SIZED.lock().unwrap();
    if sized.get(pane).is_some_and(|p| p == peer) {
        sized.remove(pane);
    }
}

/// A peer went away: it watches and sizes nothing anymore.
pub fn drop_peer(peer: &str) {
    for t in HOST.lock().unwrap().values_mut() {
        t.watchers.remove(peer);
    }
    SIZED.lock().unwrap().retain(|_, p| p != peer);
}

/// The pane's process exited: tell its watchers.
pub fn exited(pane: &str, code: u32) {
    let watchers = HOST.lock().unwrap().remove(pane).map(|t| t.watchers).unwrap_or_default();
    SIZED.lock().unwrap().remove(pane);
    for peer in watchers {
        crate::connect::send(&peer, json!({"t": "term.exit", "paneId": pane, "code": code}));
    }
}

/// Which peer sizes this pane, if one does (the local tile then leaves the size alone).
pub fn sized_by(pane: &str) -> Option<String> {
    SIZED.lock().unwrap().get(pane).cloned()
}

pub fn set_sized_by(pane: &str, peer: &str) {
    SIZED.lock().unwrap().insert(pane.to_string(), peer.to_string());
}

/// The local tile takes the size back; tells the peer.
pub fn reclaim(pane: &str) {
    if let Some(peer) = SIZED.lock().unwrap().remove(pane) {
        crate::connect::send(&peer, json!({"t": "term.reclaim", "paneId": pane}));
    }
}

// ---- client ----

pub fn register(local: &str, peer: &str, pane: &str) {
    REMOTE.lock().unwrap().insert(local.to_string(), (peer.to_string(), pane.to_string()));
}

pub fn unregister(local: &str) -> Option<(String, String)> {
    RECLAIMED.lock().unwrap().remove(local);
    REMOTE.lock().unwrap().remove(local)
}

pub fn is_remote(local: &str) -> bool {
    REMOTE.lock().unwrap().contains_key(local)
}

pub fn remote_of(local: &str) -> Option<(String, String)> {
    REMOTE.lock().unwrap().get(local).cloned()
}

/// The local tiles showing a peer's pane.
pub fn locals_of(peer: &str, pane: &str) -> Vec<String> {
    REMOTE.lock().unwrap().iter().filter(|(_, (p, r))| p == peer && r == pane).map(|(l, _)| l.clone()).collect()
}

pub fn locals_of_peer(peer: &str) -> Vec<String> {
    REMOTE.lock().unwrap().iter().filter(|(_, (p, _))| p == peer).map(|(l, _)| l.clone()).collect()
}

/// Keys for a remote tile go to its machine; false when the tile isn't remote.
pub fn send_input(local: &str, data: &str) -> bool {
    let Some((peer, pane)) = remote_of(local) else { return false };
    // Typing takes the size back from the host.
    if RECLAIMED.lock().unwrap().remove(local)
        && let Some(session) = crate::terminal::SESSIONS.get(local)
    {
        let size = *session.size.lock();
        crate::connect::send(&peer, json!({"t": "term.resize", "paneId": pane, "cols": size.cols, "rows": size.rows}));
    }
    crate::connect::send(&peer, json!({"t": "term.in", "paneId": pane, "data": b64(data.as_bytes())}));
    true
}

/// A remote tile's new size; false when the tile isn't remote.
pub fn send_resize(local: &str, cols: u16, rows: u16) -> bool {
    let Some((peer, pane)) = remote_of(local) else { return false };
    if !RECLAIMED.lock().unwrap().contains(local) {
        crate::connect::send(&peer, json!({"t": "term.resize", "paneId": pane, "cols": cols, "rows": rows}));
    }
    true
}

pub fn set_reclaimed(local: &str, reclaimed: bool) {
    let mut set = RECLAIMED.lock().unwrap();
    if reclaimed {
        set.insert(local.to_string());
    } else {
        set.remove(local);
    }
}

pub fn is_reclaimed(local: &str) -> bool {
    RECLAIMED.lock().unwrap().contains(local)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rings_keep_the_tail_and_chats_restart_on_a_line() {
        let pane = "ring-test";
        tap(pane, &vec![b'x'; TERM_RING + 10], false);
        assert_eq!(attach(pane, "peer").len(), TERM_RING);
        detach(pane, "peer");
        let chat = "ring-chat";
        let line = b"{\"t\":\"text\"}\n";
        for _ in 0..(CHAT_RING / line.len() + 5) {
            tap(chat, line, true);
        }
        let snap = attach(chat, "peer");
        assert!(snap.len() <= CHAT_RING);
        assert!(snap.starts_with(b"{\"t\""));
    }

    #[test]
    fn remote_tiles_map_both_ways() {
        let local = "p-remote-test".to_string();
        register(&local, "ABCDEFGHIJKLMNOPQRSTUV", "p-1");
        assert!(is_remote(&local));
        assert_eq!(locals_of("ABCDEFGHIJKLMNOPQRSTUV", "p-1"), vec![local.clone()]);
        assert_eq!(unregister(&local).unwrap().1, "p-1");
        assert!(!is_remote(&local));
    }
}
