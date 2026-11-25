//! Phones paired with this server directly (the winfer PWA, docs/mobile/protocol.md): they follow and
//! answer its agents and get Web Push when one finishes or asks, with no desktop in between.
//! `winferd mobile pair|list|revoke`, or `mobile.pair` from a paired desktop (it shows the QR code).

use serde_json::{Value, json};
use std::path::PathBuf;
use std::time::{Duration, Instant};
use winfer_backend::mobile;
use winfer_core::backend;

fn dir() -> PathBuf {
    let dir = backend::data_dir().join("control");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// A new pairing: `{deviceId, link, qrSvg, expiresAt}`.
pub fn start_pairing() -> Result<Value, String> {
    let pairing = mobile::mobile_pair_start()?;
    serde_json::to_value(pairing).map_err(|e| e.to_string())
}

fn devices() -> Vec<Value> {
    mobile::mobile_status().ok().and_then(|s| serde_json::to_value(s).ok()).and_then(|v| v["devices"].as_array().cloned()).unwrap_or_default()
}

/// The daemon's part (each second): pairings and revocations asked for by the CLI, and the
/// phones for `winferd status`.
pub fn tick() {
    let dir = dir();
    if std::fs::remove_file(dir.join("mobile.pair")).is_ok() {
        let answer = start_pairing().unwrap_or_else(|e| json!({"error": e}));
        let _ = std::fs::write(dir.join("mobile.json"), answer.to_string());
    }
    if let Ok(which) = std::fs::read_to_string(dir.join("mobile.revoke")) {
        let _ = std::fs::remove_file(dir.join("mobile.revoke"));
        let _ = std::fs::write(dir.join("mobile.revoked.json"), revoke(which.trim()).to_string());
    }
    let _ = std::fs::write(dir.join("phones.json"), Value::Array(devices()).to_string());
}

fn revoke(which: &str) -> Value {
    let found = devices().into_iter().find(|d| d["deviceId"] == which || d["name"].as_str().is_some_and(|n| n.eq_ignore_ascii_case(which)));
    match found {
        Some(d) => match mobile::mobile_revoke(d["deviceId"].as_str().unwrap_or_default().to_string()) {
            Ok(()) => json!({"ok": true, "name": d["name"]}),
            Err(e) => json!({"ok": false, "error": e}),
        },
        None => json!({"ok": false, "error": format!("no phone named {which}")}),
    }
}

/// The link as a QR code for a terminal (two rows of modules per line).
fn terminal_qr(link: &str) -> String {
    use qrcode::render::unicode::Dense1x2;
    qrcode::QrCode::new(link.as_bytes()).map(|q| q.render::<Dense1x2>().dark_color(Dense1x2::Light).light_color(Dense1x2::Dark).quiet_zone(true).build()).unwrap_or_default()
}

fn wait_for<T>(timeout: Duration, mut next: impl FnMut() -> Option<T>) -> Option<T> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if let Some(v) = next() {
            return Some(v);
        }
        std::thread::sleep(Duration::from_millis(300));
    }
    None
}

/// Shows the pairing and waits for the phone to use it; `phones` reads the paired phones.
fn show_and_follow(pairing: &Value, phones: impl Fn() -> Vec<Value>) -> i32 {
    if let Some(e) = pairing["error"].as_str() {
        eprintln!("Couldn't make a pairing: {e}");
        return 1;
    }
    let link = pairing["link"].as_str().unwrap_or_default();
    let device = pairing["deviceId"].as_str().unwrap_or_default().to_string();
    println!("{}", terminal_qr(link));
    println!("Scan this with your phone's camera, or open this link on the phone:\n{link}\n");
    println!("Waiting for the phone (the link works for 10 minutes)…");
    match wait_for(Duration::from_secs(10 * 60), || phones().into_iter().find(|d| d["deviceId"] == device.as_str() && d["paired"] == true)) {
        Some(d) => {
            println!("Paired with {}. It gets a notification when an agent here finishes or needs you.", d["name"].as_str().unwrap_or("the phone"));
            0
        }
        None => {
            eprintln!("The phone didn't pair in time.");
            1
        }
    }
}

fn read_json(name: &str) -> Option<Value> {
    std::fs::read_to_string(dir().join(name)).ok().and_then(|t| serde_json::from_str(&t).ok())
}

/// `winferd mobile pair|list|revoke <name>`.
pub fn cli(args: &[String]) -> i32 {
    let running = backend::acquire_instance_lock();
    let daemon = running.is_none();
    match args.first().map(String::as_str) {
        Some("pair") if daemon => {
            let _ = std::fs::remove_file(dir().join("mobile.json"));
            let _ = std::fs::write(dir().join("mobile.pair"), "");
            let Some(pairing) = wait_for(Duration::from_secs(10), || read_json("mobile.json")) else {
                eprintln!("winferd didn't answer");
                return 1;
            };
            let _ = std::fs::remove_file(dir().join("mobile.json"));
            show_and_follow(&pairing, || read_json("phones.json").and_then(|v| v.as_array().cloned()).unwrap_or_default())
        }
        Some("pair") => {
            if let Err(e) = backend::start() {
                eprintln!("winferd failed to start: {e}");
                return 1;
            }
            name_bridge();
            let pairing = start_pairing().unwrap_or_else(|e| json!({"error": e}));
            let code = show_and_follow(&pairing, devices);
            // Let the last messages go out.
            std::thread::sleep(Duration::from_millis(500));
            code
        }
        Some("list") => {
            let phones = if daemon {
                read_json("phones.json").and_then(|v| v.as_array().cloned()).unwrap_or_default()
            } else {
                if let Err(e) = backend::start() {
                    eprintln!("winferd failed to start: {e}");
                    return 1;
                }
                std::thread::sleep(Duration::from_millis(300));
                devices()
            };
            let paired: Vec<&Value> = phones.iter().filter(|d| d["paired"] == true).collect();
            if paired.is_empty() {
                println!("No phones paired (`winferd mobile pair`).");
            }
            for d in paired {
                println!("  {}{}", d["name"].as_str().unwrap_or("phone"), if d["push"] == true { " · notifications on" } else { "" });
            }
            0
        }
        Some("revoke") if args.len() > 1 => {
            let which = args[1..].join(" ");
            let result = if daemon {
                let _ = std::fs::remove_file(dir().join("mobile.revoked.json"));
                let _ = std::fs::write(dir().join("mobile.revoke"), &which);
                wait_for(Duration::from_secs(10), || read_json("mobile.revoked.json")).unwrap_or_else(|| json!({"ok": false, "error": "winferd didn't answer"}))
            } else {
                if let Err(e) = backend::start() {
                    eprintln!("winferd failed to start: {e}");
                    return 1;
                }
                std::thread::sleep(Duration::from_millis(300));
                let r = revoke(&which);
                std::thread::sleep(Duration::from_secs(2));
                r
            };
            if result["ok"] == true {
                println!("Unpaired {}.", result["name"].as_str().unwrap_or(&which));
                0
            } else {
                eprintln!("{}", result["error"].as_str().unwrap_or("failed"));
                1
            }
        }
        _ => {
            eprintln!("usage: winferd mobile pair|list|revoke <name>");
            2
        }
    }
}

/// Phones see this server under its winfer name, and get every finish and question (no one
/// sits at a server).
pub fn name_bridge() {
    mobile::set_headless();
    if let Some(connect) = winfer_core::connect::get() {
        mobile::set_name(&connect.status().name);
    }
}
