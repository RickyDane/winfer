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
/// What a phone gets of it (its relay frames are at most 1 MiB, sealed and base64'd twice).
const PHONE_RING: usize = 256 << 10;

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
    tap_except(pane, bytes, chat, None);
}

/// `tap`, but not live to `except` (a peer that already shows these bytes).
pub fn tap_except(pane: &str, bytes: &[u8], chat: bool, except: Option<&str>) {
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
    for peer in t.watchers.iter().filter(|p| Some(p.as_str()) != except) {
        crate::connect::send(peer, msg.clone());
    }
}

/// The messages in chat commands (`send`/`steer` JSON lines) as the chat's `user` lines. A chat
/// process doesn't echo what it's sent, so the host feeds these to everyone showing the chat but
/// the one that sent them (it shows them already).
pub fn user_echo(data: &str) -> String {
    let mut out = String::new();
    for line in data.lines().map(str::trim).filter(|l| l.starts_with('{')) {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else { continue };
        if !matches!(v["type"].as_str(), Some("send" | "steer")) {
            continue;
        }
        let text = v["text"].as_str().unwrap_or_default().trim();
        let files = v["files"].as_array().into_iter().flatten().filter_map(|f| f["name"].as_str());
        let shown = crate::code::state::shown_text(text, v["skill"].as_str(), files);
        let images: Vec<String> = v["images"].as_array().into_iter().flatten()
            .filter_map(|i| Some(format!("data:{};base64,{}", i["mime"].as_str()?, i["data"].as_str()?)))
            .collect();
        if shown.is_empty() && images.is_empty() {
            continue;
        }
        out.push_str(&json!({"type": "user", "text": shown, "images": images}).to_string());
        out.push('\n');
    }
    out
}

/// A peer attaches: `term.snapshot` with what it missed, then the live output. Sent under the
/// lock `tap` takes, so no chunk comes before the snapshot or twice.
pub fn attach(pane: &str, peer: &str, chat: bool, provider: &str) {
    let mut host = HOST.lock().unwrap();
    let t = host.entry(pane.to_string()).or_default();
    t.watchers.insert(peer.to_string());
    let snapshot = past_for(peer, t.ring.iter().copied().collect());
    crate::connect::send(peer, json!({"t": "term.snapshot", "paneId": pane, "data": b64(&snapshot), "chat": chat, "provider": provider}));
}

/// What `peer` gets of a pane's past: a phone's relay carries frames up to 1 MiB (base64'd and
/// sealed), so it gets less, from the start of a line.
fn past_for(peer: &str, mut ring: Vec<u8>) -> Vec<u8> {
    if crate::connect::is_phone(peer) && ring.len() > PHONE_RING {
        let mut cut = ring.len() - PHONE_RING;
        while cut < ring.len() && ring[cut - 1] != b'\n' {
            cut += 1;
        }
        ring.drain(..cut);
    }
    ring
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

    fn ring(pane: &str) -> Vec<u8> {
        HOST.lock().unwrap().get(pane).map(|t| t.ring.iter().copied().collect()).unwrap_or_default()
    }

    #[test]
    fn rings_keep_the_tail_and_chats_restart_on_a_line() {
        let pane = "ring-test";
        tap(pane, &vec![b'x'; TERM_RING + 10], false);
        assert_eq!(ring(pane).len(), TERM_RING);
        let chat = "ring-chat";
        let line = b"{\"t\":\"text\"}\n";
        for _ in 0..(CHAT_RING / line.len() + 5) {
            tap(chat, line, true);
        }
        let snap = ring(chat);
        assert!(snap.len() <= CHAT_RING);
        assert!(snap.starts_with(b"{\"t\""));
    }

    #[test]
    fn chat_commands_echo_as_user_lines() {
        let data = concat!(
            r#"{"type":"send","text":"fix it","skill":"review","files":[{"name":"a.txt","data":"eA=="}],"images":[{"mime":"image/png","data":"aQ=="}]}"#, "\n",
            r#"{"type":"interrupt"}"#, "\n",
            r#"{"type":"steer","text":"also tests"}"#, "\n",
        );
        let echo = user_echo(data);
        let lines: Vec<serde_json::Value> = echo.lines().map(|l| serde_json::from_str(l).unwrap()).collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0], json!({"type": "user", "text": "/review fix it\n[File: a.txt]", "images": ["data:image/png;base64,aQ=="]}));
        assert_eq!(lines[1]["text"], "also tests");
        assert!(matches!(crate::code::protocol::parse_line(echo.lines().next().unwrap()), Some(crate::code::protocol::Ev::User { images, .. }) if images.len() == 1));
        assert_eq!(user_echo("ls\r"), "");
    }

    #[test]
    fn phones_get_a_shorter_past_from_a_line() {
        let ring: Vec<u8> = b"0123456789\n".iter().copied().cycle().take(PHONE_RING * 2).collect();
        assert_eq!(past_for("ABCDEFGHIJKLMNOPQRSTUV", ring.clone()).len(), ring.len());
        let phone = past_for(&crate::connect::phone_peer("dev"), ring);
        assert!(phone.len() <= PHONE_RING && phone.len() > PHONE_RING - 11);
        assert!(phone.starts_with(b"0123"));
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
