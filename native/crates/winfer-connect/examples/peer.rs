//! A bare Connect peer for trying things by hand: `cargo run -p winfer-connect --example peer --
//! <data dir> <relay url> [host|join <link>] [name]`. `host` prints a pair link and confirms the code by
//! itself; `join` joins one. Then it echoes every message back and prints what it gets.

use serde_json::json;
use std::sync::Arc;
use winfer_connect::pairing::Kind;
use winfer_connect::peers::Persist;
use winfer_connect::store::SecretStore;
use winfer_connect::{Connect, Event, PairStatus};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (dir, relay, mode) = (&args[1], &args[2], args.get(3).map(String::as_str).unwrap_or("host"));
    let name = args.iter().position(|a| a == "--name").and_then(|i| args.get(i + 1)).cloned().unwrap_or_else(|| "test-peer".into());
    let echo: Arc<std::sync::OnceLock<Connect>> = Arc::default();
    let sink = echo.clone();
    let connect = Connect::new(
        Persist::new(std::path::Path::new(dir), SecretStore::file(&std::path::Path::new(dir).join("secrets.json"), "peer")),
        Kind::Server,
        Arc::new(move |e| match e {
            Event::Message { peer, msg } => {
                println!("[msg from {peer}] {msg}");
                if let Some(c) = sink.get() {
                    c.send(&peer, json!({"t": "echo", "msg": msg}));
                }
            }
            Event::PeerUp(p) => println!("[up] {p}"),
            Event::PeerDown(p) => println!("[down] {p}"),
            Event::Changed => {}
        }),
    );
    let _ = echo.set(connect.clone());
    connect.rename(&name);
    connect.set_relay_url(relay).unwrap();
    connect.start();
    match mode {
        "host" => println!("[link] {}", connect.pair_create_link()),
        "join" => connect.pair_join_link(&args[4], true).unwrap(),
        _ => {}
    }
    let mut last = None;
    loop {
        let status = connect.status();
        if status.pairing != last {
            println!("[pairing] {:?}", status.pairing);
            if let Some(PairStatus::Code { confirmed: false, .. }) = &status.pairing {
                connect.pair_confirm();
            }
            last = status.pairing;
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
}
