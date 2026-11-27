#![allow(clippy::result_large_err)]
//! Two Connect services pair and talk through an in-process stand-in for the relay
//! (`relay/src/index.ts`, peer rooms only).

use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};
use winfer_connect::pairing::Kind;
use winfer_connect::peers::Persist;
use winfer_connect::store::SecretStore;
use winfer_connect::{Connect, Event, PairStatus};

type Rooms = Arc<Mutex<HashMap<String, Vec<(u64, mpsc::UnboundedSender<Message>)>>>>;

fn presence(rooms: &Rooms, room: &str) {
    let rooms = rooms.lock().unwrap();
    let sockets = rooms.get(room).cloned().unwrap_or_default();
    let frame = json!({"t": "presence", "peers": sockets.len()}).to_string();
    for (_, tx) in sockets {
        let _ = tx.send(Message::text(frame.clone()));
    }
}

/// Starts the fake relay; returns its http:// URL.
fn fake_relay(rt: &tokio::runtime::Runtime) -> String {
    let listener = rt.block_on(tokio::net::TcpListener::bind("127.0.0.1:0")).unwrap();
    let addr = listener.local_addr().unwrap();
    let rooms: Rooms = Arc::default();
    rt.spawn(async move {
        let mut seq = 0u64;
        loop {
            let Ok((stream, _)) = listener.accept().await else { break };
            seq += 1;
            let id = seq;
            let rooms = rooms.clone();
            tokio::spawn(async move {
                let path = Arc::new(Mutex::new(String::new()));
                let p = path.clone();
                let Ok(ws) = tokio_tungstenite::accept_hdr_async(stream, move |req: &Request, res: Response| {
                    *p.lock().unwrap() = req.uri().path().to_string();
                    Ok(res)
                })
                .await
                else {
                    return;
                };
                let room = path.lock().unwrap().trim_start_matches("/r/").to_string();
                let (tx, mut rx) = mpsc::unbounded_channel::<Message>();
                rooms.lock().unwrap().entry(room.clone()).or_default().push((id, tx));
                presence(&rooms, &room);
                let (mut sink, mut stream) = ws.split();
                loop {
                    tokio::select! {
                        out = rx.recv() => match out {
                            Some(m) => { if sink.send(m).await.is_err() { break; } }
                            None => break,
                        },
                        msg = stream.next() => match msg {
                            Some(Ok(Message::Text(t))) => {
                                if t.as_str() == "ping" { let _ = sink.send(Message::text("pong")).await; continue; }
                                if t.as_str().starts_with('{') { continue; }
                                let peers = rooms.lock().unwrap().get(&room).cloned().unwrap_or_default();
                                for (pid, ptx) in peers { if pid != id { let _ = ptx.send(Message::Text(t.clone())); } }
                            }
                            Some(Ok(_)) => {}
                            _ => break,
                        }
                    }
                }
                if let Some(v) = rooms.lock().unwrap().get_mut(&room) {
                    v.retain(|(pid, _)| *pid != id);
                }
                presence(&rooms, &room);
            });
        }
    });
    format!("http://{addr}")
}

struct Node {
    connect: Connect,
    events: Arc<Mutex<Vec<Event>>>,
}

fn node(name: &str, kind: Kind, relay: &str) -> Node {
    let dir = std::env::temp_dir().join(format!("wc-e2e-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let events: Arc<Mutex<Vec<Event>>> = Arc::default();
    let sink = events.clone();
    let connect = Connect::new(Persist::new(&dir, SecretStore::memory()), kind, Arc::new(move |e| sink.lock().unwrap().push(e)));
    connect.rename(name);
    connect.set_relay_url(relay).unwrap();
    // Off by default on a desktop (on for a server); these tests use it.
    assert_eq!(connect.config().enabled, kind == Kind::Server);
    connect.set_enabled(true);
    Node { connect, events }
}

fn wait<T>(what: &str, mut f: impl FnMut() -> Option<T>) -> T {
    let start = Instant::now();
    loop {
        if let Some(v) = f() {
            return v;
        }
        assert!(start.elapsed() < Duration::from_secs(15), "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn code(n: &Node) -> Option<String> {
    match n.connect.status().pairing {
        Some(PairStatus::Code { code, .. }) => Some(code),
        _ => None,
    }
}

#[test]
fn pair_by_link_talk_and_revoke() {
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
    let relay = fake_relay(&rt);
    let desk = node("desk", Kind::Desktop, &relay);
    let server = node("server", Kind::Server, &relay);

    let link = desk.connect.pair_create_link();
    server.connect.pair_join_link(&link, true).unwrap();

    let (a, b) = (wait("desk code", || code(&desk)), wait("server code", || code(&server)));
    assert_eq!(a, b, "both screens show the same code");
    desk.connect.pair_confirm();

    let peers = wait("pairing stored on both", || {
        let (d, s) = (desk.connect.peers(), server.connect.peers());
        (d.len() == 1 && s.len() == 1).then_some((d, s))
    });
    assert_eq!(peers.0[0].name, "server");
    assert_eq!(peers.0[0].kind, Kind::Server);
    assert_eq!(peers.1[0].name, "desk");
    let (server_id, desk_id) = (peers.0[0].peer_id.clone(), peers.1[0].peer_id.clone());

    wait("links up", || (desk.connect.is_up(&server_id) && server.connect.is_up(&desk_id)).then_some(()));
    assert_eq!(desk.connect.status().peers[0].link.via, Some("relay"));

    desk.connect.send(&server_id, json!({"t": "hello", "x": 1}));
    let big = "y".repeat(1_500_000);
    server.connect.send(&desk_id, json!({"t": "big", "s": big}));
    wait("server gets hello", || {
        server.events.lock().unwrap().iter().any(|e| matches!(e, Event::Message { msg, .. } if msg["t"] == "hello")).then_some(())
    });
    wait("desk gets the chunked message", || {
        desk.events.lock().unwrap().iter().any(|e| matches!(e, Event::Message { msg, .. } if msg["s"].as_str().map(str::len) == Some(1_500_000))).then_some(())
    });

    server.connect.revoke(&desk_id);
    assert!(server.connect.peers().is_empty());
    wait("desk drops the pairing after bye", || desk.connect.peers().is_empty().then_some(()));
}

fn port(n: &Node) -> u16 {
    wait("LAN listener", || n.connect.status().lan_port)
}

#[test]
fn lan_pairing_and_link_switching() {
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
    let relay = fake_relay(&rt);
    let a = node("lan-a", Kind::Desktop, &relay);
    let b = node("lan-b", Kind::Desktop, &relay);
    a.connect.start();
    b.connect.start();
    let b_port = port(&b);

    // A asks B over the LAN; B gets an incoming request with the same code.
    a.connect.pair_address(&format!("127.0.0.1:{b_port}")).unwrap();
    let (ca, cb) = (wait("a code", || code(&a)), wait("b code", || code(&b)));
    assert_eq!(ca, cb);
    assert!(matches!(b.connect.status().pairing, Some(PairStatus::Code { incoming: true, .. })));
    a.connect.pair_confirm();
    b.connect.pair_confirm();
    let (a_id, b_id) = wait("paired", || {
        let (pa, pb) = (a.connect.peers(), b.connect.peers());
        (pa.len() == 1 && pb.len() == 1).then(|| (pb[0].peer_id.clone(), pa[0].peer_id.clone()))
    });

    // Up on the relay first; then a LAN connection takes over.
    wait("relay link", || (a.connect.is_up(&b_id) && b.connect.is_up(&a_id)).then_some(()));
    let rt2 = rt.handle().clone();
    let (t, ev) = rt.block_on(winfer_connect::lan::dial(&rt2, format!("127.0.0.1:{b_port}").parse().unwrap(), &a_id, winfer_connect::lan::Want::Link)).unwrap();
    assert!(a.connect.add_lan(&b_id, t, ev));
    wait("both on the LAN", || {
        let (sa, sb) = (a.connect.status(), b.connect.status());
        (sa.peers[0].link.via == Some("lan") && sb.peers[0].link.via == Some("lan")).then_some(())
    });
    a.connect.send(&b_id, json!({"t": "over-lan"}));
    wait("message over the LAN", || {
        b.events.lock().unwrap().iter().any(|e| matches!(e, Event::Message { msg, .. } if msg["t"] == "over-lan")).then_some(())
    });

    // Turning Connect off on B closes its LAN side; A falls back to... nothing (B is gone entirely).
    b.connect.set_enabled(false);
    wait("A notices", || (!a.connect.is_up(&b_id)).then_some(()));
    b.connect.set_enabled(true);
    wait("back on the relay", || a.connect.is_up(&b_id).then_some(()));
}

#[test]
fn a_stranger_cant_open_a_link_over_the_lan() {
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
    let relay = fake_relay(&rt);
    let b = node("lan-c", Kind::Desktop, &relay);
    b.connect.start();
    let b_port = port(&b);
    let rt2 = rt.handle().clone();
    // Not paired: the hello is accepted, but nothing listens for it and no link appears.
    let (t, _ev) = rt.block_on(winfer_connect::lan::dial(&rt2, format!("127.0.0.1:{b_port}").parse().unwrap(), &"Z".repeat(22), winfer_connect::lan::Want::Link)).unwrap();
    drop(t);
    std::thread::sleep(Duration::from_millis(200));
    assert!(b.connect.status().peers.is_empty());
}

#[test]
fn turning_connect_off_closes_the_lan_port() {
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
    let relay = fake_relay(&rt);
    let b = node("lan-off", Kind::Desktop, &relay);
    b.connect.start();
    let first = port(&b);
    b.connect.set_enabled(false);
    b.connect.set_enabled(true);
    let second = port(&b);
    assert_ne!(first, second);
    wait("old port closed", || std::net::TcpStream::connect(("127.0.0.1", first)).is_err().then_some(()));
    assert!(std::net::TcpStream::connect(("127.0.0.1", second)).is_ok());
}

#[test]
fn wrong_or_expired_links_are_refused() {
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
    let relay = fake_relay(&rt);
    let a = node("a", Kind::Desktop, &relay);
    assert!(a.connect.pair_join_link("hello", false).is_err());
    let own = a.connect.pair_create_link();
    assert!(a.connect.pair_join_link(&own, false).unwrap_err().contains("own"));
}
