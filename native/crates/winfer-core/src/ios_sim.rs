//! iOS Simulators on this Mac, for the device mirror (`devices.rs`): listed and booted with
//! `xcrun simctl`. Live mirroring attaches directly to CoreSimulator's `IOSurface` framebuffer
//! (hardware H.264 via VideoToolbox, up to 60 fps) and injects live touches, buttons and keys
//! through SimulatorKit's IndigoHID (`ios_sim_macos.m`), falling back to `simctl` JPEG screenshots
//! and AXe (github.com/cameroncooke/AXe) or idb (fbidb.io) when native frameworks aren't available.

use crate::scrcpy::{Frame, Options, Touch};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc::Sender;
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

fn developer_dir() -> Option<String> {
    static DIR: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    DIR.get_or_init(|| {
        if !cfg!(target_os = "macos") {
            return None;
        }
        let mut cmd = Command::new("xcode-select");
        no_window(&mut cmd).arg("-p").stdin(Stdio::null());
        run(cmd)
            .ok()
            .map(|b| String::from_utf8_lossy(&b).trim().to_string())
            .filter(|s| !s.is_empty())
    })
    .clone()
}

/// Whether Xcode's tools are installed: without them `xcrun` offers to install them, in a dialog.
fn developer_tools() -> bool {
    developer_dir().is_some()
}

/// Whether native SimulatorKit HID input is available on this Mac.
pub fn native_input_available() -> bool {
    #[cfg(target_os = "macos")]
    {
        if let Some(dir) = developer_dir()
            && let Ok(c) = std::ffi::CString::new(dir)
        {
            return unsafe { ffi::winfer_ios_sim_native_available(c.as_ptr()) != 0 };
        }
        false
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
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

/// `{simctl, input, devices: [{udid, name, state, runtime, booted}]}`. Blocking.
pub fn status() -> Value {
    if !cfg!(target_os = "macos") || !developer_tools() {
        return json!({"simctl": false, "devices": []});
    }
    let mut cmd = xcrun();
    cmd.args(["simctl", "list", "devices", "available", "-j"]);
    let input = if native_input_available() {
        Some("native")
    } else {
        input_tool().map(|t| t.name())
    };
    match run(cmd).ok().and_then(|out| serde_json::from_slice::<Value>(&out).ok()) {
        Some(list) => json!({"simctl": true, "input": input, "devices": parse_devices(&list)}),
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

/// Maps an ASCII printable character to `(USB HID Keyboard usage, shift)`.
pub fn ascii_hid(c: char) -> Option<(u32, bool)> {
    Some(match c {
        'a'..='z' => (4 + (c as u32 - 'a' as u32), false),
        'A'..='Z' => (4 + (c as u32 - 'A' as u32), true),
        '1'..='9' => (30 + (c as u32 - '1' as u32), false),
        '0' => (39, false),
        '!' => (30, true),
        '@' => (31, true),
        '#' => (32, true),
        '$' => (33, true),
        '%' => (34, true),
        '^' => (35, true),
        '&' => (36, true),
        '*' => (37, true),
        '(' => (38, true),
        ')' => (39, true),
        '\n' | '\r' => (40, false),
        '\t' => (43, false),
        ' ' => (44, false),
        '-' => (45, false),
        '_' => (45, true),
        '=' => (46, false),
        '+' => (46, true),
        '[' => (47, false),
        '{' => (47, true),
        ']' => (48, false),
        '}' => (48, true),
        '\\' => (49, false),
        '|' => (49, true),
        ';' => (51, false),
        ':' => (51, true),
        '\'' => (52, false),
        '"' => (52, true),
        '`' => (53, false),
        '~' => (53, true),
        ',' => (54, false),
        '<' => (54, true),
        '.' => (55, false),
        '>' => (55, true),
        '/' => (56, false),
        '?' => (56, true),
        _ => return None,
    })
}

/// Copies `text` to the simulator's pasteboard via `xcrun simctl pbcopy`.
fn pbcopy(udid: &str, text: &str) -> Result<(), String> {
    use std::io::Write;
    let mut cmd = xcrun();
    cmd.args(["simctl", "pbcopy", udid]).stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| format!("couldn't run simctl pbcopy: {e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(text.as_bytes()).map_err(|e| e.to_string())?;
    }
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

#[cfg(target_os = "macos")]
mod ffi {
    use std::ffi::{c_char, c_void};

    pub type FrameCb = extern "C" fn(ctx: *mut c_void, data: *const u8, len: usize, is_key: u8, width: u32, height: u32);
    pub type EndCb = extern "C" fn(ctx: *mut c_void, error: *const c_char);

    unsafe extern "C" {
        pub fn winfer_ios_sim_native_available(developer_dir: *const c_char) -> i32;
        pub fn winfer_ios_sim_start(
            developer_dir: *const c_char,
            udid: *const c_char,
            max_side: u32,
            max_fps: u32,
            bit_rate: u32,
            on_frame: FrameCb,
            on_end: EndCb,
            ctx: *mut c_void,
            out_width: *mut u32,
            out_height: *mut u32,
            out_has_hid: *mut u8,
            err_buf: *mut c_char,
            err_cap: usize,
        ) -> *mut c_void;
        pub fn winfer_ios_sim_set_profile(handle: *mut c_void, max_fps: u32, bit_rate: u32);
        pub fn winfer_ios_sim_force_keyframe(handle: *mut c_void);
        pub fn winfer_ios_sim_touch(handle: *mut c_void, action: u8, x: f64, y: f64) -> i32;
        pub fn winfer_ios_sim_button(handle: *mut c_void, button: u8) -> i32;
        pub fn winfer_ios_sim_key(handle: *mut c_void, usage: u32, action: u8) -> i32;
        pub fn winfer_ios_sim_stop(handle: *mut c_void);
    }
}

#[cfg(target_os = "macos")]
struct CallbackCtx {
    frames: Sender<Frame>,
    size: std::sync::Arc<parking_lot::Mutex<(u32, u32)>>,
}

#[cfg(target_os = "macos")]
extern "C" fn on_native_frame(ctx: *mut std::ffi::c_void, data: *const u8, len: usize, is_key: u8, width: u32, height: u32) {
    if ctx.is_null() || data.is_null() || len == 0 {
        return;
    }
    let ctx = unsafe { &*(ctx as *const CallbackCtx) };
    *ctx.size.lock() = (width, height);
    let bytes = unsafe { std::slice::from_raw_parts(data, len) }.to_vec();
    let _ = ctx.frames.send(Frame { data: bytes, key: is_key != 0 });
}

#[cfg(target_os = "macos")]
extern "C" fn on_native_end(_ctx: *mut std::ffi::c_void, _error: *const std::ffi::c_char) {}

/// A live hardware-encoded H.264 + IndigoHID session to a booted macOS iOS Simulator.
/// Dropping it unregisters all CoreSimulator callbacks and invalidates VideoToolbox cleanly.
pub struct LiveSession {
    #[cfg(target_os = "macos")]
    handle: *mut std::ffi::c_void,
    #[cfg(target_os = "macos")]
    ctx: *mut CallbackCtx,
    #[cfg(target_os = "macos")]
    udid: String,
    #[cfg(target_os = "macos")]
    size: std::sync::Arc<parking_lot::Mutex<(u32, u32)>>,
    #[cfg(target_os = "macos")]
    has_hid: bool,
}

#[cfg(target_os = "macos")]
unsafe impl Send for LiveSession {}
#[cfg(target_os = "macos")]
unsafe impl Sync for LiveSession {}

impl LiveSession {
    pub fn start(udid: &str, options: Options, frames: Sender<Frame>) -> Result<LiveSession, String> {
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (udid, options, frames);
            Err("iOS Simulators require macOS".into())
        }
        #[cfg(target_os = "macos")]
        {
            let dir = developer_dir().ok_or_else(|| "Xcode developer tools aren't installed".to_string())?;
            let c_dir = std::ffi::CString::new(dir).map_err(|e| e.to_string())?;
            let c_udid = std::ffi::CString::new(udid).map_err(|e| e.to_string())?;
            let size = std::sync::Arc::new(parking_lot::Mutex::new((0u32, 0u32)));
            let ctx = Box::into_raw(Box::new(CallbackCtx { frames, size: size.clone() }));
            let mut w = 0u32;
            let mut h = 0u32;
            let mut has_hid = 0u8;
            let mut err_buf = [0 as std::ffi::c_char; 512];

            let handle = unsafe {
                ffi::winfer_ios_sim_start(
                    c_dir.as_ptr(),
                    c_udid.as_ptr(),
                    options.max_side,
                    options.max_fps,
                    options.bit_rate,
                    on_native_frame,
                    on_native_end,
                    ctx as *mut std::ffi::c_void,
                    &mut w,
                    &mut h,
                    &mut has_hid,
                    err_buf.as_mut_ptr(),
                    err_buf.len(),
                )
            };
            if handle.is_null() {
                unsafe { drop(Box::from_raw(ctx)) };
                let msg = unsafe { std::ffi::CStr::from_ptr(err_buf.as_ptr()) }.to_string_lossy().into_owned();
                return Err(if msg.is_empty() { "couldn't start live iOS simulator stream".into() } else { msg });
            }
            *size.lock() = (w, h);
            Ok(LiveSession { handle, ctx, udid: udid.to_string(), size, has_hid: has_hid != 0 })
        }
    }

    pub fn size(&self) -> (u32, u32) {
        #[cfg(target_os = "macos")]
        {
            *self.size.lock()
        }
        #[cfg(not(target_os = "macos"))]
        {
            (0, 0)
        }
    }

    pub fn has_hid(&self) -> bool {
        #[cfg(target_os = "macos")]
        {
            self.has_hid
        }
        #[cfg(not(target_os = "macos"))]
        {
            false
        }
    }

    pub fn set_profile(&self, options: Options) {
        #[cfg(target_os = "macos")]
        unsafe {
            ffi::winfer_ios_sim_set_profile(self.handle, options.max_fps, options.bit_rate);
        }
        #[cfg(not(target_os = "macos"))]
        let _ = options;
    }

    pub fn reset_video(&self) {
        #[cfg(target_os = "macos")]
        unsafe {
            ffi::winfer_ios_sim_force_keyframe(self.handle);
        }
    }

    pub fn touch(&self, touch: Touch, at: (f32, f32)) -> bool {
        #[cfg(target_os = "macos")]
        {
            let action = match touch {
                Touch::Down => 0,
                Touch::Move => 1,
                Touch::Up => 2,
            };
            unsafe { ffi::winfer_ios_sim_touch(self.handle, action, at.0 as f64, at.1 as f64) != 0 }
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (touch, at);
            false
        }
    }

    pub fn button(&self, button: &str) -> bool {
        #[cfg(target_os = "macos")]
        {
            let code = match button {
                "home" => 0,
                "lock" | "power" => 1,
                _ => return false,
            };
            unsafe { ffi::winfer_ios_sim_button(self.handle, code) != 0 }
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = button;
            false
        }
    }

    pub fn key(&self, usage: u32) -> bool {
        #[cfg(target_os = "macos")]
        unsafe {
            ffi::winfer_ios_sim_key(self.handle, usage, 2) != 0
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = usage;
            false
        }
    }

    /// Types text via HID keyboard usages for ASCII (instant, <1ms) and pasteboard + Cmd+V for Unicode.
    pub fn text(&self, text: &str) -> bool {
        #[cfg(target_os = "macos")]
        {
            if !self.has_hid || text.is_empty() {
                return false;
            }
            const LEFT_SHIFT: u32 = 0xE1;
            const LEFT_GUI: u32 = 0xE3;
            const KEY_V: u32 = 25;
            if text.chars().all(|c| ascii_hid(c).is_some()) {
                for ch in text.chars() {
                    let Some((usage, shift)) = ascii_hid(ch) else { continue };
                    unsafe {
                        if shift {
                            ffi::winfer_ios_sim_key(self.handle, LEFT_SHIFT, 0);
                        }
                        ffi::winfer_ios_sim_key(self.handle, usage, 2);
                        if shift {
                            ffi::winfer_ios_sim_key(self.handle, LEFT_SHIFT, 1);
                        }
                    }
                }
                return true;
            }
            if pbcopy(&self.udid, text).is_ok() {
                unsafe {
                    ffi::winfer_ios_sim_key(self.handle, LEFT_GUI, 0);
                    ffi::winfer_ios_sim_key(self.handle, KEY_V, 2);
                    ffi::winfer_ios_sim_key(self.handle, LEFT_GUI, 1);
                }
                return true;
            }
            false
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = text;
            false
        }
    }
}

#[cfg(target_os = "macos")]
impl Drop for LiveSession {
    fn drop(&mut self) {
        // Blocks until CoreSimulator callbacks and VideoToolbox have drained, so freeing `ctx` is safe.
        unsafe {
            ffi::winfer_ios_sim_stop(self.handle);
            drop(Box::from_raw(self.ctx));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_ascii_to_usb_hid() {
        assert_eq!(ascii_hid('a'), Some((4, false)));
        assert_eq!(ascii_hid('Z'), Some((29, true)));
        assert_eq!(ascii_hid('0'), Some((39, false)));
        assert_eq!(ascii_hid('!'), Some((30, true)));
        assert_eq!(ascii_hid(' '), Some((44, false)));
        assert_eq!(ascii_hid('ä'), None);
    }

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

    /// Against the first booted iOS Simulator:
    /// `cargo test -p winfer-core ios_sim::tests::live_session -- --ignored --nocapture`.
    #[test]
    #[ignore]
    #[cfg(target_os = "macos")]
    fn live_session() {
        use openh264::formats::YUVSource;
        use std::time::{Duration, Instant};

        let list = status();
        let udid = list["devices"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|d| d["booted"] == true)
            .and_then(|d| d["udid"].as_str())
            .expect("no booted iOS simulator")
            .to_string();

        // Baseline: one old simctl JPEG screenshot.
        let t_shot = Instant::now();
        let shot = screenshot(&udid).expect("simctl screenshot failed");
        let shot_ms = t_shot.elapsed().as_millis();
        println!("baseline simctl screenshot: {shot_ms} ms, {} bytes", shot.len());

        let (tx, rx) = std::sync::mpsc::channel();
        let t0 = Instant::now();
        let session = LiveSession::start(&udid, Options::default(), tx).expect("LiveSession::start failed");
        println!("live started in {:?}, size {:?}, has_hid={}", t0.elapsed(), session.size(), session.has_hid());
        assert!(session.has_hid(), "expected native IndigoHID");

        let first = rx.recv_timeout(Duration::from_secs(3)).expect("no initial keyframe");
        println!("first keyframe after {:?}: {} bytes, key={}", t0.elapsed(), first.data.len(), first.key);
        assert!(first.key);

        let mut decoder = openh264::decoder::Decoder::new().expect("openh264 init");
        let (mut dec, mut frames, mut bytes) = (Duration::ZERO, 0u32, 0usize);
        let mut decode_one = |f: &Frame| -> bool {
            let t = Instant::now();
            match decoder.decode(&f.data) {
                Ok(Some(yuv)) => {
                    dec += t.elapsed();
                    let (w, h) = yuv.dimensions();
                    assert!(w > 0 && h > 0);
                    frames += 1;
                    bytes += f.data.len();
                    true
                }
                _ => false,
            }
        };
        assert!(decode_one(&first), "first keyframe failed to decode in openh264");

        // Drain any idle frames, then perform a 60Hz live swipe and measure input-to-frame latency + FPS.
        while rx.recv_timeout(Duration::from_millis(150)).is_ok() {}
        let mut latencies = Vec::new();
        let swipe_start = Instant::now();
        let mut swipe_frames = 0u32;
        for step in 0..=20 {
            let x = 0.85 - 0.70 * (step as f32 / 20.0);
            let t = Instant::now();
            let kind = if step == 0 { Touch::Down } else if step == 20 { Touch::Up } else { Touch::Move };
            assert!(session.touch(kind, (x, 0.5)));
            if step == 1 {
                if let Ok(f) = rx.recv_timeout(Duration::from_secs(1)) {
                    latencies.push(t.elapsed().as_millis());
                    if decode_one(&f) {
                        swipe_frames += 1;
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(16));
            while let Ok(f) = rx.try_recv() {
                if decode_one(&f) {
                    swipe_frames += 1;
                }
            }
        }
        let settle_until = Instant::now() + Duration::from_millis(800);
        while Instant::now() < settle_until {
            let rem = settle_until.saturating_duration_since(Instant::now());
            match rx.recv_timeout(rem.min(Duration::from_millis(200))) {
                Ok(f) => {
                    if decode_one(&f) {
                        swipe_frames += 1;
                    }
                }
                Err(_) => break,
            }
        }
        let swipe_elapsed = swipe_start.elapsed();
        let avg_bytes = bytes / (frames as usize).max(1);
        let avg_dec_us = (dec / frames.max(1)).as_micros();
        println!(
            "live swipe: {swipe_frames} decoded frames in {:?} (input->frame {latencies:?} ms), avg frame {avg_bytes} bytes, avg openh264 decode {avg_dec_us} us",
            swipe_elapsed
        );
        assert!(session.button("home"));
        assert!(swipe_frames >= 15, "expected smooth animation frames, got {swipe_frames}");
        drop(session);
    }
}
