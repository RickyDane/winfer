//! iOS Simulators on this Mac, for the device mirror (`devices.rs`): listed and booted with
//! `xcrun simctl`, shown as JPEG screenshots (`simctl io … screenshot`), and driven with AXe
//! (github.com/cameroncooke/AXe) or idb (fbidb.io) when one is installed: simctl itself has no
//! way to tap or type.

use serde_json::{Value, json};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use winfer_backend::env::{find_command, login_env, no_window};

/// What sends touches and keys to a simulator.
#[derive(Clone, Debug, PartialEq)]
pub enum Tool {
    Axe(PathBuf),
    Idb(PathBuf),
}

impl Tool {
    pub fn name(&self) -> &'static str {
        match self {
            Tool::Axe(_) => "axe",
            Tool::Idb(_) => "idb",
        }
    }
}

pub fn input_tool() -> Option<Tool> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    let find = |name: &str| find_command(name).or_else(|| ["/opt/homebrew/bin", "/usr/local/bin"].iter().map(|d| PathBuf::from(d).join(name)).find(|p| p.exists()));
    find("axe").map(Tool::Axe).or_else(|| find("idb").map(Tool::Idb))
}

fn xcrun() -> Command {
    let mut cmd = Command::new("xcrun");
    no_window(&mut cmd).envs(login_env()).stdin(Stdio::null());
    cmd
}

fn run(mut cmd: Command) -> Result<Vec<u8>, String> {
    let out = cmd.output().map_err(|e| format!("couldn't run {:?}: {e}", cmd.get_program()))?;
    if out.status.success() {
        Ok(out.stdout)
    } else {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        Err(if err.is_empty() { format!("{:?} failed", cmd.get_program()) } else { err })
    }
}

/// "com.apple.CoreSimulator.SimRuntime.iOS-18-2" → "iOS 18.2".
fn runtime_name(key: &str) -> String {
    let short = key.rsplit('.').next().unwrap_or(key);
    match short.split_once('-') {
        Some((os, version)) => format!("{os} {}", version.replace('-', ".")),
        None => short.to_string(),
    }
}

/// The iPhone and iPad simulators in `simctl list devices available -j`.
fn parse_devices(json: &Value) -> Vec<Value> {
    let mut out = Vec::new();
    for (runtime, devices) in json["devices"].as_object().into_iter().flatten() {
        let name = runtime_name(runtime);
        if !name.starts_with("iOS") {
            continue;
        }
        for d in devices.as_array().into_iter().flatten().filter(|d| d["isAvailable"] != false) {
            out.push(json!({"udid": d["udid"], "name": d["name"], "state": d["state"], "runtime": name, "booted": d["state"] == "Booted"}));
        }
    }
    // Booted first, then by name.
    out.sort_by(|a, b| (b["booted"] == true).cmp(&(a["booted"] == true)).then_with(|| a["name"].as_str().cmp(&b["name"].as_str())));
    out
}

/// Whether Xcode's tools are installed: without them `xcrun` offers to install them, in a dialog.
fn developer_tools() -> bool {
    static FOUND: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *FOUND.get_or_init(|| {
        let mut cmd = Command::new("xcode-select");
        no_window(&mut cmd).arg("-p").stdin(Stdio::null());
        run(cmd).is_ok()
    })
}

/// `{simctl, input, devices: [{udid, name, state, runtime, booted}]}`. Blocking.
pub fn status() -> Value {
    if !cfg!(target_os = "macos") || !developer_tools() {
        return json!({"simctl": false, "devices": []});
    }
    let mut cmd = xcrun();
    cmd.args(["simctl", "list", "devices", "available", "-j"]);
    match run(cmd).ok().and_then(|out| serde_json::from_slice::<Value>(&out).ok()) {
        Some(list) => json!({"simctl": true, "input": input_tool().map(|t| t.name()), "devices": parse_devices(&list)}),
        None => json!({"simctl": false, "devices": []}),
    }
}

/// Boots a simulator (headless: the Simulator app isn't needed for the mirror).
pub fn boot(udid: &str) -> Result<(), String> {
    let mut cmd = xcrun();
    cmd.args(["simctl", "boot", udid]);
    match run(cmd) {
        Ok(_) => Ok(()),
        Err(e) if e.contains("current state: Booted") => Ok(()),
        Err(e) => Err(e),
    }
}

/// The screen as JPEG. Blocking (a few hundred ms).
pub fn screenshot(udid: &str) -> Result<Vec<u8>, String> {
    let path = std::env::temp_dir().join(format!("winfer-sim-{}-{}.jpg", udid.chars().filter(char::is_ascii_alphanumeric).collect::<String>(), rand::random::<u32>()));
    let mut cmd = xcrun();
    cmd.args(["simctl", "io", udid, "screenshot", "--type=jpeg"]).arg(&path).stdout(Stdio::null());
    let shot = run(cmd).and_then(|_| std::fs::read(&path).map_err(|e| e.to_string()));
    let _ = std::fs::remove_file(&path);
    shot
}

/// Width and height of a JPEG, from its first frame header.
pub fn jpeg_size(b: &[u8]) -> Option<(u32, u32)> {
    let mut i = 2;
    while i + 9 < b.len() {
        if b[i] != 0xff {
            return None;
        }
        let marker = b[i + 1];
        let len = u16::from_be_bytes([b[i + 2], b[i + 3]]) as usize;
        // SOF0..SOF15, except DHT (c4), JPG (c8) and DAC (cc).
        if (0xc0..=0xcf).contains(&marker) && !matches!(marker, 0xc4 | 0xc8 | 0xcc) {
            let h = u16::from_be_bytes([b[i + 5], b[i + 6]]) as u32;
            let w = u16::from_be_bytes([b[i + 7], b[i + 8]]) as u32;
            return Some((w, h));
        }
        i += 2 + len;
    }
    None
}

/// Pixels per point: iPads and the smaller iPhones are @2x, the rest @3x.
pub fn scale(name: &str, (w, h): (u32, u32)) -> f32 {
    if name.contains("iPad") || w.min(h) < 1000 { 2.0 } else { 3.0 }
}

/// USB HID keyboard usage for a key without text (what AXe and idb take).
pub fn hid_key(key: &str) -> Option<u32> {
    Some(match key {
        "enter" => 40,
        "escape" => 41,
        "backspace" => 42,
        "tab" => 43,
        "right" => 79,
        "left" => 80,
        "down" => 81,
        "up" => 82,
        _ => return None,
    })
}

/// One action on a simulator, in points.
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    Tap(f32, f32),
    Swipe { from: (f32, f32), to: (f32, f32), secs: f32 },
    Text(String),
    Key(u32),
    /// "home" or "lock".
    Button(&'static str),
}

fn n(v: f32) -> String {
    format!("{}", (v * 10.0).round() / 10.0)
}

/// The tool's command line for `action`.
pub fn args(tool: &Tool, udid: &str, action: &Action) -> Vec<String> {
    let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    let mut out = match (tool, action) {
        (Tool::Axe(_), Action::Tap(x, y)) => s(&["tap", "-x", &n(*x), "-y", &n(*y)]),
        (Tool::Axe(_), Action::Swipe { from, to, secs }) => s(&["swipe", "--start-x", &n(from.0), "--start-y", &n(from.1), "--end-x", &n(to.0), "--end-y", &n(to.1), "--duration", &n(*secs)]),
        (Tool::Axe(_), Action::Text(t)) => s(&["type", t]),
        (Tool::Axe(_), Action::Key(k)) => s(&["key", &k.to_string()]),
        (Tool::Axe(_), Action::Button(b)) => s(&["button", b]),
        (Tool::Idb(_), Action::Tap(x, y)) => s(&["ui", "tap", &n(*x), &n(*y)]),
        (Tool::Idb(_), Action::Swipe { from, to, secs }) => s(&["ui", "swipe", &n(from.0), &n(from.1), &n(to.0), &n(to.1), "--duration", &n(*secs)]),
        (Tool::Idb(_), Action::Text(t)) => s(&["ui", "text", t]),
        (Tool::Idb(_), Action::Key(k)) => s(&["ui", "key", &k.to_string()]),
        (Tool::Idb(_), Action::Button(b)) => s(&["ui", "button", &b.to_uppercase()]),
    };
    out.extend(s(&["--udid", udid]));
    out
}

/// Runs `action` on the simulator. Blocking.
pub fn act(tool: &Tool, udid: &str, action: &Action) -> Result<(), String> {
    let (Tool::Axe(path) | Tool::Idb(path)) = tool;
    let mut cmd = Command::new(path);
    no_window(&mut cmd).envs(login_env()).stdin(Stdio::null()).args(args(tool, udid, action));
    run(cmd).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_ios_simulators_booted_first() {
        let list = json!({"devices": {
            "com.apple.CoreSimulator.SimRuntime.watchOS-11-0": [{"udid": "W", "name": "Apple Watch", "state": "Shutdown", "isAvailable": true}],
            "com.apple.CoreSimulator.SimRuntime.iOS-18-2": [
                {"udid": "A", "name": "iPhone 16", "state": "Shutdown", "isAvailable": true},
                {"udid": "B", "name": "iPad Air", "state": "Booted", "isAvailable": true},
                {"udid": "C", "name": "iPhone X", "state": "Shutdown", "isAvailable": false}
            ]
        }});
        let devices = parse_devices(&list);
        assert_eq!(devices.iter().map(|d| d["udid"].as_str().unwrap()).collect::<Vec<_>>(), ["B", "A"]);
        assert_eq!(devices[0]["runtime"], "iOS 18.2");
    }

    #[test]
    fn reads_jpeg_size() {
        // SOI, an APP0 segment, then SOF0 with height 2556 and width 1179.
        let mut b = vec![0xff, 0xd8, 0xff, 0xe0, 0, 4, 0, 0];
        b.extend([0xff, 0xc0, 0, 17, 8, 0x09, 0xfc, 0x04, 0x9b, 3, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(jpeg_size(&b), Some((1179, 2556)));
        assert_eq!(jpeg_size(b"not a jpeg"), None);
    }

    #[test]
    fn points_per_pixel() {
        assert_eq!(scale("iPhone 16 Pro", (1206, 2622)), 3.0);
        assert_eq!(scale("iPhone SE (3rd generation)", (750, 1334)), 2.0);
        assert_eq!(scale("iPad Pro 13-inch (M4)", (2064, 2752)), 2.0);
    }

    #[test]
    fn command_lines() {
        let axe = Tool::Axe("axe".into());
        let idb = Tool::Idb("idb".into());
        assert_eq!(args(&axe, "U", &Action::Tap(10.0, 20.55)), ["tap", "-x", "10", "-y", "20.6", "--udid", "U"]);
        assert_eq!(args(&idb, "U", &Action::Button("home")), ["ui", "button", "HOME", "--udid", "U"]);
        assert_eq!(args(&axe, "U", &Action::Swipe { from: (1.0, 2.0), to: (3.0, 4.0), secs: 0.3 })[..2], ["swipe", "--start-x"]);
        assert_eq!(hid_key("enter"), Some(40));
    }
}
