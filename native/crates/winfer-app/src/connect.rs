//! Winfer Connect in the desktop app: the service (`winfer-connect`) started with the backend, its
//! events carried to the UI as `connect://changed`, `connect://peer` and `connect://message`.

use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::sync::{Arc, Mutex, OnceLock};
use tauri::Emitter;
use winfer_connect::pairing::Kind;
use winfer_connect::peers::Persist;
use winfer_connect::store::SecretStore;
use winfer_connect::{Connect, Event};

static CONNECT: OnceLock<Connect> = OnceLock::new();
/// Peers that asked for this instance's panes (`panes.get`): they get its status pushes.
static SUBSCRIBERS: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());

/// The service, once the backend has started it.
pub fn get() -> Option<&'static Connect> {
    CONNECT.get()
}

pub fn start(app: tauri::AppHandle, data_dir: &std::path::Path) {
    let store = SecretStore::keychain(&format!("{}.connect", crate::backend::identifier()));
    let on = Arc::new(move |event: Event| {
        if std::env::var_os("WINFER_CONNECT_TRACE").is_some() && !matches!(event, Event::Changed) {
            eprintln!("[connect] {event:?}");
        }
        let _ = match event {
            Event::Changed => app.emit("connect://changed", ()),
            Event::PeerUp(peer) => app.emit("connect://peer", json!({"peer": peer, "up": true})),
            Event::PeerDown(peer) => {
                crate::connect_term::drop_peer(&peer);
                unsubscribe(&peer);
                app.emit("connect://peer", json!({"peer": peer, "up": false}))
            }
            Event::Message { peer, msg } => app.emit("connect://message", json!({"peer": peer, "msg": msg})),
        };
    });
    let connect = Connect::new(Persist::new(data_dir, store), Kind::Desktop, on);
    if CONNECT.set(connect.clone()).is_ok() {
        // Reading the peers may ask for the keychain: off the startup path.
        std::thread::spawn(move || {
            connect.start();
            // Development aid: join a pair link at launch, confirming by itself.
            #[cfg(debug_assertions)]
            if let Ok(link) = std::env::var("WINFER_NATIVE_CONNECT_JOIN") {
                let auto = std::env::var_os("WINFER_NATIVE_CONNECT_CONFIRM").is_some();
                if let Err(e) = connect.pair_join_link(&link, auto) {
                    eprintln!("[connect] join: {e}");
                }
            }
            // Development aid: make a pair link and leave it in a file for another instance.
            #[cfg(debug_assertions)]
            if let Ok(path) = std::env::var("WINFER_NATIVE_CONNECT_LINK_FILE") {
                let _ = std::fs::write(path, connect.pair_create_link());
            }
            // Development aid: confirm every pairing code by itself.
            #[cfg(debug_assertions)]
            if std::env::var_os("WINFER_NATIVE_CONNECT_CONFIRM").is_some() {
                let c = connect.clone();
                std::thread::spawn(move || {
                    loop {
                        std::thread::sleep(std::time::Duration::from_millis(300));
                        if let Some(winfer_connect::PairStatus::Code { confirmed: false, .. }) = c.status().pairing {
                            c.pair_confirm();
                        }
                    }
                });
            }
            // Development aid: ask the first instance seen on the network to pair.
            #[cfg(debug_assertions)]
            if std::env::var_os("WINFER_NATIVE_CONNECT_PAIR_NEARBY").is_some() {
                for _ in 0..30 {
                    std::thread::sleep(std::time::Duration::from_secs(1));
                    if let Some(n) = connect.status().nearby.first() {
                        if let Err(e) = connect.pair_nearby(&n.id) {
                            eprintln!("[connect] pair nearby: {e}");
                        }
                        break;
                    }
                }
            }
        });
    }
}

/// Sends to one peer (dropped while its link is down).
pub fn send(peer: &str, msg: Value) {
    if let Some(connect) = get() {
        connect.send(peer, msg);
    }
}

pub fn subscribe(peer: &str) {
    SUBSCRIBERS.lock().unwrap().insert(peer.to_string());
}

pub fn unsubscribe(peer: &str) {
    SUBSCRIBERS.lock().unwrap().remove(peer);
}

/// What the mobile bridge tells every phone also goes to the peers watching this instance.
pub fn broadcast(msg: &Value) {
    let peers: Vec<String> = SUBSCRIBERS.lock().unwrap().iter().cloned().collect();
    for peer in peers {
        send(&peer, msg.clone());
    }
}

/// Mobile-protocol device ids for peers: `connect:<peer id>`.
pub fn device(peer: &str) -> String {
    format!("connect:{peer}")
}

pub fn peer_of(device: &str) -> Option<&str> {
    device.strip_prefix("connect:")
}

/// A peer's name, for "from ‹machine›" in the activity log.
pub fn peer_name(peer: &str) -> Option<String> {
    get()?.peers().into_iter().find(|p| p.peer_id == peer).map(|p| p.name)
}
