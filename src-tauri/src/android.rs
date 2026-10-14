//! Android device mirroring over adb, shown in the side panel next to the browser.
//!
//! Video: `adb exec-out screenrecord --output-format=h264 -` streams the screen as H.264
//! (Annex B), forwarded raw to the webview, which decodes it with WebCodecs. screenrecord stops
//! after 3 minutes, so the stream is restarted until the view stops it. Input goes back through
//! `adb shell input`. `screencap` is the fallback where the stream can't be decoded.

use std::collections::HashMap;
use std::io::Read;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use base64::Engine;
use serde::{Deserialize, Serialize};
use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::State;

use crate::env::{login_env, no_window};

/// The longest side of the mirrored video; keeps the stream light and decoding fast.
const MAX_SIDE: u32 = 1280;

/// adb from PATH, else the Android SDK's platform-tools (ANDROID_HOME, or the default SDK folder).
fn sdk_dir() -> Option<PathBuf> {
    for var in ["ANDROID_HOME", "ANDROID_SDK_ROOT"] {
        if let Some(dir) = login_env().get(var).filter(|d| !d.is_empty()) {
            return Some(PathBuf::from(dir));
        }
    }
    let home = dirs::home_dir()?;
    let default = if cfg!(target_os = "macos") {
        home.join("Library/Android/sdk")
    } else if cfg!(windows) {
        dirs::data_local_dir()?.join("Android").join("Sdk")
    } else {
        home.join("Android/Sdk")
    };
    default.exists().then_some(default)
}

fn tool(name: &str, sub: &str) -> Option<PathBuf> {
    crate::env::find_command(name).or_else(|| {
        let exe = if cfg!(windows) { format!("{name}.exe") } else { name.to_string() };
        Some(sdk_dir()?.join(sub).join(exe)).filter(|p| p.exists())
    })
}

fn adb_path() -> Result<PathBuf, String> {
    tool("adb", "platform-tools").ok_or_else(|| "adb isn't installed. Install Android Studio's platform-tools (or `brew install android-platform-tools`).".into())
}

fn adb(serial: Option<&str>) -> Result<Command, String> {
    let mut cmd = Command::new(adb_path()?);
    no_window(&mut cmd).envs(login_env());
    if let Some(serial) = serial {
        cmd.args(["-s", serial]);
    }
    Ok(cmd)
}

fn run(mut cmd: Command) -> Result<Vec<u8>, String> {
    let out = cmd.stdin(Stdio::null()).output().map_err(|e| format!("couldn't run adb: {e}"))?;
    if out.status.success() {
        Ok(out.stdout)
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Device {
    pub serial: String,
    /// "device" when usable; "unauthorized", "offline", … otherwise.
    pub state: String,
    pub model: String,
    pub emulator: bool,
}

/// Parses `adb devices -l`.
fn parse_devices(text: &str) -> Vec<Device> {
    text.lines()
        .skip_while(|l| !l.starts_with("List of devices"))
        .skip(1)
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let serial = parts.next()?.to_string();
            let state = parts.next()?.to_string();
            let fields: HashMap<&str, &str> = parts.filter_map(|p| p.split_once(':')).collect();
            let model = fields.get("model").map(|m| m.replace('_', " ")).unwrap_or_else(|| serial.clone());
            Some(Device { emulator: serial.starts_with("emulator-"), serial, state, model })
        })
        .collect()
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AndroidStatus {
    adb: bool,
    devices: Vec<Device>,
    /// Emulators (AVDs) that can be started.
    avds: Vec<String>,
}

#[tauri::command]
pub async fn android_status() -> Result<AndroidStatus, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let Ok(mut cmd) = adb(None) else {
            return Ok(AndroidStatus { adb: false, devices: Vec::new(), avds: avds() });
        };
        cmd.args(["devices", "-l"]);
        let devices = parse_devices(&String::from_utf8_lossy(&run(cmd)?));
        Ok(AndroidStatus { adb: true, devices, avds: avds() })
    })
    .await
    .map_err(|e| e.to_string())?
}

fn avds() -> Vec<String> {
    let Some(emulator) = tool("emulator", "emulator") else {
        return Vec::new();
    };
    let mut cmd = Command::new(emulator);
    no_window(&mut cmd).arg("-list-avds");
    run(cmd)
        .map(|out| {
            String::from_utf8_lossy(&out)
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty() && !l.starts_with("INFO"))
                .filter(|name| avd_exists(name))
                .map(String::from)
                .collect()
        })
        .unwrap_or_default()
}

/// `-list-avds` also lists emulators whose folder was deleted (only `<name>.ini` is left);
/// those can't start, so they aren't offered.
fn avd_exists(name: &str) -> bool {
    let avd_home = login_env()
        .get("ANDROID_AVD_HOME")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|h| h.join(".android").join("avd")));
    let Some(avd_home) = avd_home else { return true };
    let Ok(ini) = std::fs::read_to_string(avd_home.join(format!("{name}.ini"))) else {
        return true;
    };
    ini.lines()
        .find_map(|l| l.strip_prefix("path="))
        .is_none_or(|path| PathBuf::from(path.trim()).join("config.ini").exists())
}

/// Starts an emulator in its own window; it shows up in the device list once booted.
#[tauri::command]
pub fn android_start_avd(name: String) -> Result<(), String> {
    let emulator = tool("emulator", "emulator").ok_or("The Android emulator isn't installed.")?;
    let mut cmd = Command::new(emulator);
    no_window(&mut cmd)
        .args(["-avd", &name])
        .envs(login_env())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    cmd.spawn().map(|_| ()).map_err(|e| format!("couldn't start the emulator: {e}"))
}

/// Physical screen size, from `wm size` ("Physical size: 1080x2400", maybe an override).
fn screen_size(serial: &str) -> Result<(u32, u32), String> {
    let mut cmd = adb(Some(serial))?;
    cmd.args(["shell", "wm", "size"]);
    let text = String::from_utf8_lossy(&run(cmd)?).into_owned();
    parse_size(&text).ok_or_else(|| format!("couldn't read the screen size: {text}"))
}

fn parse_size(text: &str) -> Option<(u32, u32)> {
    // An override (set with `wm size`) is what's actually shown.
    let line = text.lines().find(|l| l.starts_with("Override size")).or_else(|| text.lines().find(|l| l.starts_with("Physical size")))?;
    let (w, h) = line.split(':').nth(1)?.trim().split_once('x')?;
    Some((w.trim().parse().ok()?, h.trim().parse().ok()?))
}

/// Video size: the screen scaled so its longest side is at most MAX_SIDE, even numbers (H.264).
fn video_size((w, h): (u32, u32)) -> (u32, u32) {
    let scale = (MAX_SIDE as f64 / w.max(h) as f64).min(1.0);
    let even = |v: f64| ((v as u32) / 2 * 2).max(2);
    (even(w as f64 * scale), even(h as f64 * scale))
}

struct Mirror {
    stop: Arc<AtomicBool>,
    child: Arc<Mutex<Option<Child>>>,
}

#[derive(Default)]
pub struct AndroidManager {
    mirrors: Mutex<HashMap<String, Mirror>>,
}

impl AndroidManager {
    pub fn shutdown(&self) {
        for (_, mirror) in self.mirrors.lock().unwrap().drain() {
            stop_mirror(&mirror);
        }
    }
}

fn stop_mirror(mirror: &Mirror) {
    mirror.stop.store(true, Ordering::SeqCst);
    if let Some(child) = mirror.child.lock().unwrap().as_mut() {
        let _ = child.kill();
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MirrorInfo {
    /// The device's screen, for mapping taps.
    width: u32,
    height: u32,
    /// The video's size.
    video_width: u32,
    video_height: u32,
}

/// Streams the device's screen as H.264 to `on_data` until `android_mirror_stop`.
#[tauri::command]
pub fn android_mirror_start(manager: State<'_, AndroidManager>, serial: String, on_data: Channel) -> Result<MirrorInfo, String> {
    let screen = screen_size(&serial)?;
    let (vw, vh) = video_size(screen);
    if let Some(old) = manager.mirrors.lock().unwrap().remove(&serial) {
        stop_mirror(&old);
    }
    let stop = Arc::new(AtomicBool::new(false));
    let child: Arc<Mutex<Option<Child>>> = Arc::default();
    manager.mirrors.lock().unwrap().insert(serial.clone(), Mirror { stop: stop.clone(), child: child.clone() });

    std::thread::spawn(move || {
        while !stop.load(Ordering::SeqCst) {
            let Ok(mut cmd) = adb(Some(&serial)) else { break };
            cmd.args([
                "exec-out",
                "screenrecord",
                "--output-format=h264",
                "--bit-rate",
                "6000000",
                "--size",
                &format!("{vw}x{vh}"),
                "-",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
            let Ok(mut process) = cmd.spawn() else { break };
            let Some(mut stdout) = process.stdout.take() else { break };
            *child.lock().unwrap() = Some(process);
            let started = std::time::Instant::now();
            let mut buf = vec![0u8; 64 * 1024];
            loop {
                match stdout.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if on_data.send(InvokeResponseBody::Raw(buf[..n].to_vec())).is_err() {
                            stop.store(true, Ordering::SeqCst);
                            break;
                        }
                    }
                }
            }
            if let Some(mut process) = child.lock().unwrap().take() {
                let _ = process.kill();
                let _ = process.wait();
            }
            // screenrecord ends after its time limit; one that dies right away (unsupported,
            // device gone) isn't restarted in a tight loop.
            if started.elapsed() < std::time::Duration::from_secs(2) {
                std::thread::sleep(std::time::Duration::from_secs(2));
            }
        }
    });
    Ok(MirrorInfo { width: screen.0, height: screen.1, video_width: vw, video_height: vh })
}

#[tauri::command]
pub fn android_mirror_stop(manager: State<'_, AndroidManager>, serial: String) {
    if let Some(mirror) = manager.mirrors.lock().unwrap().remove(&serial) {
        stop_mirror(&mirror);
    }
}

/// A PNG screenshot as a data URL (fallback where H.264 can't be decoded).
#[tauri::command]
pub async fn android_screenshot(serial: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let mut cmd = adb(Some(&serial))?;
        cmd.args(["exec-out", "screencap", "-p"]);
        let png = run(cmd)?;
        Ok(format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(png)))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Input {
    Tap { x: u32, y: u32 },
    Swipe { x1: u32, y1: u32, x2: u32, y2: u32, ms: u32 },
    Text { text: String },
    /// Android keycode, e.g. 4 = back, 3 = home, 187 = recents, 66 = enter, 67 = delete.
    Key { code: u32 },
}

/// `input text` takes one shell word: spaces become %s and the rest is single-quoted.
fn input_text_arg(text: &str) -> String {
    let escaped = text.replace('%', "\\%").replace(' ', "%s");
    format!("'{}'", escaped.replace('\'', "'\\''"))
}

fn input_args(input: &Input) -> Vec<String> {
    let mut args = vec!["shell".to_string(), "input".to_string()];
    match input {
        Input::Tap { x, y } => args.extend(["tap".into(), x.to_string(), y.to_string()]),
        Input::Swipe { x1, y1, x2, y2, ms } => args.extend(["swipe".into(), x1.to_string(), y1.to_string(), x2.to_string(), y2.to_string(), ms.to_string()]),
        Input::Text { text } => args.extend(["text".into(), input_text_arg(text)]),
        Input::Key { code } => args.extend(["keyevent".into(), code.to_string()]),
    }
    args
}

#[tauri::command]
pub async fn android_input(serial: String, input: Input) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let mut cmd = adb(Some(&serial))?;
        cmd.args(input_args(&input));
        run(cmd).map(|_| ())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_adb_devices() {
        let out = "List of devices attached\nemulator-5554          device product:sdk_gphone64_arm64 model:sdk_gphone64_arm64 device:emu64a transport_id:1\nR58M12ABCDE            unauthorized usb:1-1 transport_id:2\n\n";
        let devices = parse_devices(out);
        assert_eq!(devices.len(), 2);
        assert_eq!(devices[0], Device { serial: "emulator-5554".into(), state: "device".into(), model: "sdk gphone64 arm64".into(), emulator: true });
        assert_eq!((devices[1].state.as_str(), devices[1].emulator), ("unauthorized", false));
        assert!(parse_devices("List of devices attached\n\n").is_empty());
    }

    #[test]
    fn reads_screen_and_video_sizes() {
        assert_eq!(parse_size("Physical size: 1080x2400\n"), Some((1080, 2400)));
        assert_eq!(parse_size("Physical size: 1080x2400\nOverride size: 720x1600\n"), Some((720, 1600)));
        assert_eq!(video_size((1080, 2400)), (576, 1280));
        assert_eq!(video_size((720, 1280)), (720, 1280));
        assert_eq!(video_size((2560, 1600)), (1280, 800));
    }

    #[test]
    fn quotes_text_for_input() {
        assert_eq!(input_text_arg("hello world"), "'hello%sworld'");
        assert_eq!(input_text_arg("it's 100%"), "'it'\\''s%s100\\%'");
        assert_eq!(input_args(&Input::Key { code: 4 }), ["shell", "input", "keyevent", "4"]);
    }
}
