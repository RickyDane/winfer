//! `winferd pair|status|unpair`: talk to the running daemon through files in its data folder
//! (`control/`), or do the job in this process when no daemon runs.

use serde_json::{Value, json};
use std::path::PathBuf;
use std::time::{Duration, Instant};
use winfer_connect::Connect;
use winfer_core::backend;

fn dir() -> PathBuf {
    let dir = backend::data_dir().join("control");
    let _ = std::fs::create_dir_all(&dir);
    // Pair links and phone links pass through here: this user's only.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
    }
    dir
}

/// The daemon joins this pair link on its next tick.
pub fn request_pair(link: &str) {
    let _ = std::fs::remove_file(dir().join("pair.json"));
    let _ = std::fs::write(dir().join("pair.link"), link.trim());
}

fn field<'a>(v: &'a Value, camel: &str, snake: &str) -> &'a str {
    v[camel].as_str().or_else(|| v[snake].as_str()).unwrap_or_default()
}

/// The daemon's chores each second: requests from the CLI, and what it should know.
pub fn tick(connect: &Connect, host: &crate::host::Host) {
    let dir = dir();
    if let Ok(tag) = std::fs::read_to_string(dir.join("update")) {
        let _ = std::fs::remove_file(dir.join("update"));
        let tag = tag.trim();
        let result = host.update((!tag.is_empty()).then_some(tag));
        let _ = std::fs::write(dir.join("update.json"), match &result {
            Ok(v) => json!({"ok": true, "version": v}),
            Err(e) => json!({"ok": false, "error": e}),
        }.to_string());
        if result.is_ok() {
            crate::update::restart(&crate::update::exe().unwrap_or_default());
        }
    }
    if let Ok(link) = std::fs::read_to_string(dir.join("pair.link")) {
        let _ = std::fs::remove_file(dir.join("pair.link"));
        // The link's one-time key authenticates both sides: confirm the code by ourselves.
        if let Err(e) = connect.pair_join_link(&link, true) {
            let _ = std::fs::write(dir.join("pair.json"), json!({"state": "failed", "error": e}).to_string());
            eprintln!("pairing failed: {e}");
        }
    }
    if let Ok(name) = std::fs::read_to_string(dir.join("unpair")) {
        let _ = std::fs::remove_file(dir.join("unpair"));
        let result = match find_peer(connect, name.trim()) {
            Some((id, name)) => {
                connect.revoke(&id);
                eprintln!("unpaired {name}");
                json!({"ok": true, "name": name})
            }
            None => json!({"ok": false, "error": format!("no paired machine named {}", name.trim())}),
        };
        let _ = std::fs::write(dir.join("unpair.json"), result.to_string());
    }
    let status = connect.status();
    if let Some(pairing) = status.pairing.as_ref().and_then(|p| serde_json::to_value(p).ok()) {
        let text = pairing.to_string();
        if std::fs::read_to_string(dir.join("pair.json")).ok().as_deref() != Some(&text) {
            log_pairing(&pairing);
            let _ = std::fs::write(dir.join("pair.json"), text);
        }
    }
    let _ = std::fs::write(dir.join("status.json"), json!({"pid": std::process::id(), "at": now_secs(), "connect": status}).to_string());
}

fn log_pairing(p: &Value) {
    match p["state"].as_str().unwrap_or_default() {
        "code" => eprintln!("pairing with {}: code {}", field(p, "peerName", "peer_name"), p["code"].as_str().unwrap_or_default()),
        "done" => eprintln!("paired with {}", field(p, "peerName", "peer_name")),
        "failed" => eprintln!("pairing failed: {}", p["error"].as_str().unwrap_or_default()),
        _ => {}
    }
}

fn find_peer(connect: &Connect, name: &str) -> Option<(String, String)> {
    connect.peers().into_iter().find(|p| p.name.eq_ignore_ascii_case(name) || p.peer_id == name).map(|p| (p.peer_id, p.name))
}

fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// Follows a pairing until it's done: prints the code, returns the exit code.
fn follow(next: impl Fn() -> Option<Value>) -> i32 {
    let deadline = Instant::now() + Duration::from_secs(11 * 60);
    let mut shown = String::new();
    while Instant::now() < deadline {
        if let Some(p) = next() {
            match p["state"].as_str().unwrap_or_default() {
                "code" if shown.is_empty() => {
                    shown = p["code"].as_str().unwrap_or_default().to_string();
                    println!("Pairing code: {shown}");
                    println!("Check that winfer on {} shows the same code, and confirm it there.", field(&p, "peerName", "peer_name"));
                }
                "done" => {
                    println!("Paired with {}.", field(&p, "peerName", "peer_name"));
                    return 0;
                }
                "failed" => {
                    eprintln!("Pairing failed: {}", p["error"].as_str().unwrap_or("unknown error"));
                    return 1;
                }
                _ => {}
            }
        }
        std::thread::sleep(Duration::from_millis(300));
    }
    eprintln!("Pairing timed out.");
    1
}

/// `winferd pair <link>`.
pub fn pair(link: &str, name: Option<&str>) -> i32 {
    match backend::acquire_instance_lock() {
        // A daemon runs: it does the pairing.
        None => {
            request_pair(link);
            println!("Pairing through the running winferd…");
            follow(|| std::fs::read_to_string(dir().join("pair.json")).ok().and_then(|t| serde_json::from_str(&t).ok()))
        }
        Some(_lock) => {
            if let Err(e) = backend::start() {
                eprintln!("winferd failed to start: {e}");
                return 1;
            }
            let connect = winfer_core::connect::get().expect("connect started");
            if let Some(name) = name {
                connect.rename(name);
            }
            if let Err(e) = connect.pair_join_link(link, true) {
                eprintln!("Pairing failed: {e}");
                return 1;
            }
            let code = follow(|| connect.status().pairing.and_then(|p| serde_json::to_value(p).ok()));
            // Let the last pairing message go out.
            std::thread::sleep(Duration::from_millis(500));
            code
        }
    }
}

/// `winferd status`.
pub fn status() -> i32 {
    let running = backend::acquire_instance_lock().is_none();
    let saved: Option<Value> = std::fs::read_to_string(dir().join("status.json")).ok().and_then(|t| serde_json::from_str(&t).ok());
    println!("winferd {} · {}", env!("CARGO_PKG_VERSION"), if running { "running" } else { "not running" });
    println!("data: {}", backend::data_dir().display());
    let Some(saved) = saved.filter(|_| running) else { return if running { 0 } else { 3 } };
    let c = &saved["connect"];
    println!("name: {}", c["name"].as_str().unwrap_or_default());
    println!("relay: {}", field(c, "relayUrl", "relay_url"));
    let peers = c["peers"].as_array().cloned().unwrap_or_default();
    if peers.is_empty() {
        println!("paired machines: none (run `winferd pair <link>` with a link from winfer: Settings → Connect)");
    }
    for p in peers {
        let link = &p["link"];
        let up = link["via"].as_str().filter(|v| !v.is_empty() && *v != "none");
        println!("  {} ({}) · {}", p["name"].as_str().unwrap_or_default(), p["kind"].as_str().unwrap_or_default(), up.map_or("offline".to_string(), |v| format!("online via {v}")));
    }
    let phones: Vec<Value> = std::fs::read_to_string(dir().join("phones.json")).ok().and_then(|t| serde_json::from_str::<Vec<Value>>(&t).ok()).unwrap_or_default().into_iter().filter(|d| d["paired"] == true).collect();
    if !phones.is_empty() {
        println!("phones: {}", phones.iter().map(|d| d["name"].as_str().unwrap_or("phone").to_string()).collect::<Vec<_>>().join(", "));
    }
    0
}

/// `winferd unpair <name>`.
pub fn unpair(name: &str) -> i32 {
    match backend::acquire_instance_lock() {
        None => {
            let _ = std::fs::remove_file(dir().join("unpair.json"));
            let _ = std::fs::write(dir().join("unpair"), name);
            for _ in 0..30 {
                std::thread::sleep(Duration::from_millis(200));
                if let Some(v) = std::fs::read_to_string(dir().join("unpair.json")).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()) {
                    let _ = std::fs::remove_file(dir().join("unpair.json"));
                    return if v["ok"] == true {
                        println!("Unpaired {}.", v["name"].as_str().unwrap_or(name));
                        0
                    } else {
                        eprintln!("{}", v["error"].as_str().unwrap_or("failed"));
                        1
                    };
                }
            }
            eprintln!("winferd didn't answer");
            1
        }
        Some(_lock) => {
            if let Err(e) = backend::start() {
                eprintln!("winferd failed to start: {e}");
                return 1;
            }
            let connect = winfer_core::connect::get().expect("connect started");
            // Peers load as the service starts.
            std::thread::sleep(Duration::from_millis(500));
            match find_peer(connect, name) {
                Some((id, name)) => {
                    connect.revoke(&id);
                    std::thread::sleep(Duration::from_secs(2));
                    println!("Unpaired {name}.");
                    0
                }
                None => {
                    eprintln!("no paired machine named {name}");
                    1
                }
            }
        }
    }
}

/// `winferd update [--check] [--version <tag>]`.
pub fn update(check_only: bool, tag: Option<String>) -> i32 {
    use crate::update;
    let latest = match tag.clone().map(Ok).unwrap_or_else(update::check) {
        Ok(t) => t,
        Err(e) if e == update::NO_RELEASE => {
            println!("winferd {} · {e}", update::VERSION);
            return 0;
        }
        Err(e) => {
            eprintln!("Couldn't check for updates: {e}");
            return 1;
        }
    };
    let newer = winfer_core::version::compare_versions(&latest, update::VERSION) == std::cmp::Ordering::Greater;
    if check_only || (!newer && tag.is_none()) {
        println!("winferd {} · newest release {latest}{}", update::VERSION, if newer { " (update available)" } else { " (up to date)" });
        return 0;
    }
    if update::in_docker() {
        eprintln!("{}", update::docker_hint());
        return 1;
    }
    match backend::acquire_instance_lock() {
        // A daemon runs: it updates and restarts itself.
        None => {
            let _ = std::fs::remove_file(dir().join("update.json"));
            let _ = std::fs::write(dir().join("update"), &latest);
            println!("Updating the running winferd to {latest}…");
            for _ in 0..600 {
                std::thread::sleep(Duration::from_millis(500));
                if let Some(v) = std::fs::read_to_string(dir().join("update.json")).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()) {
                    let _ = std::fs::remove_file(dir().join("update.json"));
                    return if v["ok"] == true {
                        println!("Updated to {}; winferd restarted.", v["version"].as_str().unwrap_or(&latest));
                        0
                    } else {
                        eprintln!("Update failed: {}", v["error"].as_str().unwrap_or("unknown error"));
                        1
                    };
                }
            }
            eprintln!("winferd didn't answer");
            1
        }
        Some(_lock) => match update::exe().and_then(|exe| update::install(&latest, &exe)) {
            Ok(v) => {
                println!("Updated to {v}.");
                0
            }
            Err(e) => {
                eprintln!("Update failed: {e}");
                1
            }
        },
    }
}
