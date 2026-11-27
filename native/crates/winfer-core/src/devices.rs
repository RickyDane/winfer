//! This machine's phones, emulators and simulators, mirrored on a paired machine (or in this
//! machine's own device panel): the host side of `device.call` (docs/connect/protocol.md,
//! "Devices"). Android goes through scrcpy (H.264 with live touch) or, failing that, adb
//! screenshots and `adb shell input`; iOS Simulators through CoreSimulator IOSurface +
//! VideoToolbox H.264 with live IndigoHID touch/keys (`ios_sim.rs`), falling back to `simctl`
//! screenshots and AXe or idb. Input comes in as fractions of the screen, so the viewer needn't
//! know its size.
//!
//! Each frame is acknowledged with `frameDone` once drawn. With too many H.264 frames on the way
//! (a slow link), the rest are dropped until the link catches up, then the encoder restarts at a
//! keyframe; a screenshot waits for the last one's ack.

use crate::{ios_sim, scrcpy};
use base64::Engine as _;
use parking_lot::{Condvar, Mutex, RwLock};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, LazyLock, Weak};
use std::time::{Duration, Instant};
use winfer_backend::android;

/// H.264 frames on the way before the next ones are dropped (default / LAN).
const WINDOW_LAN: usize = 8;
/// Tighter in-flight window on the relay to keep latency low.
const WINDOW_RELAY: usize = 4;
/// Frames not acknowledged for this long count as lost.
const ACK_TIMEOUT: Duration = Duration::from_secs(3);

pub enum Msg {
    Frame { codec: &'static str, data: Vec<u8>, key: bool },
    End(String),
}

/// Where a stream's frames go: `(stream id, message)`.
pub type Out = Arc<dyn Fn(&str, Msg) + Send + Sync>;

/// How the viewer is reached, which sets how heavy the video is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Link {
    Local,
    Lan,
    Relay,
}

fn options(link: Link) -> scrcpy::Options {
    match link {
        Link::Local => scrcpy::Options::default(),
        Link::Lan => scrcpy::Options { bit_rate: 8_000_000, max_fps: 60, max_side: 1280 },
        Link::Relay => scrcpy::Options { bit_rate: 2_000_000, max_fps: 30, max_side: 1024 },
    }
}

fn ios_options(link: Link) -> scrcpy::Options {
    match link {
        Link::Local => scrcpy::Options { bit_rate: 8_000_000, max_fps: 60, max_side: 1280 },
        Link::Lan => scrcpy::Options { bit_rate: 8_000_000, max_fps: 60, max_side: 1280 },
        Link::Relay => scrcpy::Options { bit_rate: 2_000_000, max_fps: 30, max_side: 960 },
    }
}

fn window_for(link: Link) -> usize {
    match link {
        Link::Local | Link::Lan => WINDOW_LAN,
        Link::Relay => WINDOW_RELAY,
    }
}

/// Frames sent and not acknowledged yet.
struct Flow {
    n: AtomicUsize,
    window: AtomicUsize,
    since: Mutex<Option<Instant>>,
    ack: Condvar,
}

impl Default for Flow {
    fn default() -> Self {
        Self::new(WINDOW_LAN)
    }
}

impl Flow {
    fn new(window: usize) -> Self {
        Self { n: AtomicUsize::new(0), window: AtomicUsize::new(window.max(1)), since: Mutex::new(None), ack: Condvar::new() }
    }

    fn set_window(&self, window: usize) {
        self.window.store(window.max(1), Ordering::SeqCst);
        self.ack.notify_all();
    }

    fn window(&self) -> usize {
        self.window.load(Ordering::SeqCst)
    }

    fn sent(&self) {
        if self.n.fetch_add(1, Ordering::SeqCst) == 0 {
            *self.since.lock() = Some(Instant::now());
        }
    }

    fn done(&self) {
        let _ = self.n.try_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1));
        *self.since.lock() = Some(Instant::now());
        self.ack.notify_all();
    }

    fn on_the_way(&self) -> usize {
        let n = self.n.load(Ordering::SeqCst);
        if n > 0 && self.since.lock().is_some_and(|t| t.elapsed() > ACK_TIMEOUT) {
            self.n.store(0, Ordering::SeqCst);
            return 0;
        }
        n
    }

    fn wait_idle(&self, stop: &AtomicBool) {
        let mut guard = self.since.lock();
        while self.n.load(Ordering::SeqCst) > 0 && !stop.load(Ordering::SeqCst) {
            if guard.is_some_and(|t| t.elapsed() > ACK_TIMEOUT) {
                self.n.store(0, Ordering::SeqCst);
                break;
            }
            self.ack.wait_for(&mut guard, Duration::from_millis(50));
        }
    }

    fn wake(&self) {
        self.ack.notify_all();
    }
}

/// A live Android scrcpy stream whose encoder session can be swapped seamlessly when the
/// underlying link switches between Relay and LAN.
struct AndroidLive {
    device: String,
    stream: String,
    out: Out,
    stop: Arc<AtomicBool>,
    flow: Arc<Flow>,
    link: Mutex<Link>,
    session: RwLock<Arc<scrcpy::Session>>,
    generation: AtomicU64,
}

impl AndroidLive {
    fn session(&self) -> Arc<scrcpy::Session> {
        self.session.read().clone()
    }
}

struct Stream {
    client: String,
    stop: Arc<AtomicBool>,
    flow: Arc<Flow>,
    input: mpsc::Sender<Value>,
    /// The scrcpy server runs while this is held.
    android: Option<Arc<AndroidLive>>,
    /// The live native iOS Simulator session runs while this is held.
    ios: Option<Arc<ios_sim::LiveSession>>,
}

impl Drop for Stream {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        self.flow.wake();
    }
}

static STREAMS: LazyLock<Mutex<HashMap<String, Stream>>> = LazyLock::new(Default::default);

fn key(client: &str, stream: &str) -> String {
    format!("{client}\n{stream}")
}

fn s(msg: &Value, key: &str) -> String {
    msg[key].as_str().unwrap_or_default().to_string()
}

/// Whether an emulator started here should have no window.
fn headless() -> bool {
    crate::backend::profile().server || (cfg!(target_os = "linux") && std::env::var_os("DISPLAY").is_none() && std::env::var_os("WAYLAND_DISPLAY").is_none())
}

/// `{android: {adb, devices, avds}, ios: {simctl, input, devices}}`. Blocking.
pub fn list() -> Value {
    let android = tauri::async_runtime::block_on(android::android_status()).ok().and_then(|s| serde_json::to_value(s).ok()).unwrap_or_else(|| json!({"adb": false, "devices": [], "avds": []}));
    json!({"android": android, "ios": ios_sim::status()})
}

/// Answers one `device.call` (without `t`) from `client`; frames of streams it attaches go to
/// `out`. Blocking.
pub fn call(client: &str, msg: &Value, out: Out, link: Link) -> Result<Value, String> {
    let stream = s(msg, "streamId");
    match msg["op"].as_str().unwrap_or_default() {
        "list" => Ok(list()),
        "host" => Ok(json!({"form": crate::machine::form()})),
        "boot" => {
            let device = s(msg, "device");
            match msg["platform"].as_str() {
                Some("ios") => ios_sim::boot(&device),
                _ => android::start_avd(&device, headless()),
            }
            .map(|_| Value::Null)
        }
        "attach" => attach(client, &stream, msg, out, link),
        "detach" => {
            let gone = STREAMS.lock().remove(&key(client, &stream));
            drop(gone);
            Ok(Value::Null)
        }
        "frameDone" => {
            if let Some(st) = STREAMS.lock().get(&key(client, &stream)) {
                st.flow.done();
            }
            Ok(Value::Null)
        }
        "input" => {
            if let Some(st) = STREAMS.lock().get(&key(client, &stream)) {
                let _ = st.input.send(msg.clone());
            }
            Ok(Value::Null)
        }
        op => Err(format!("This machine's device mirror can't {op} yet.")),
    }
}

/// Ends the streams `client` watches (its link went down).
pub fn drop_client(client: &str) {
    let gone: Vec<Stream> = {
        let mut streams = STREAMS.lock();
        let keys: Vec<String> = streams.iter().filter(|(_, st)| st.client == client).map(|(k, _)| k.clone()).collect();
        keys.iter().filter_map(|k| streams.remove(k)).collect()
    };
    drop(gone);
}

fn peer_link(peer: &str) -> Link {
    match crate::connect::get().and_then(|c| c.status().peers.into_iter().find(|p| p.peer_id == peer)).map(|p| p.link.via == Some("relay")) {
        Some(true) => Link::Relay,
        _ => Link::Lan,
    }
}

/// The transport to `peer` changed (e.g. upgraded from Relay to LAN): adapt active live streams in place.
pub fn link_changed(peer: &str) {
    let link = peer_link(peer);
    let win = window_for(link);
    let profile = ios_options(link);
    let mut upgrade_android: Vec<Arc<AndroidLive>> = Vec::new();
    {
        let streams = STREAMS.lock();
        for st in streams.values().filter(|st| st.client == peer) {
            st.flow.set_window(win);
            if let Some(ios) = &st.ios {
                ios.set_profile(profile);
                ios.reset_video();
            }
            if let Some(android) = &st.android {
                let mut cur = android.link.lock();
                if *cur != link {
                    *cur = link;
                    upgrade_android.push(android.clone());
                }
            }
        }
    }
    for live in upgrade_android {
        std::thread::spawn(move || upgrade_android_link(live, link));
    }
}

/// Restarts scrcpy with the new link profile (e.g. 30 fps Relay → 60 fps LAN) and swaps it into
/// `live` seamlessly on the same `streamId` without dropping the viewer.
fn upgrade_android_link(live: Arc<AndroidLive>, link: Link) {
    if live.stop.load(Ordering::SeqCst) {
        return;
    }
    let (tx, rx) = mpsc::channel();
    let Ok(next) = scrcpy::Session::start(&live.device, options(link), tx) else { return };
    if live.stop.load(Ordering::SeqCst) || *live.link.lock() != link {
        return;
    }
    let next = Arc::new(next);
    let generation = live.generation.fetch_add(1, Ordering::SeqCst) + 1;
    let old = std::mem::replace(&mut *live.session.write(), next);
    let (reset, stop, flow, out, stream) = (
        VideoReset::Scrcpy(Arc::downgrade(&live)),
        live.stop.clone(),
        live.flow.clone(),
        live.out.clone(),
        live.stream.clone(),
    );
    let check = Some((live.clone(), generation));
    std::thread::spawn(move || forward(&stream, rx, reset, &stop, &flow, &out, check));
    drop(old);
}

/// A `device.call` from paired machine `peer`, answered with an ack when it has an id.
pub fn on_request(peer: &str, msg: &Value) {
    let (peer, msg) = (peer.to_string(), msg.clone());
    let out = peer_out(peer.clone());
    let link = peer_link(&peer);
    // Input and acks are quick and must stay in order: on the event thread.
    if matches!(msg["op"].as_str(), Some("input" | "frameDone" | "detach")) {
        let _ = call(&peer, &msg, out, link);
        return;
    }
    std::thread::spawn(move || {
        let result = call(&peer, &msg, out, link);
        if msg["id"].is_null() {
            return;
        }
        let mut answer = match result {
            Ok(value) => json!({"t": "ack", "ok": true, "value": value}),
            Err(e) => json!({"t": "ack", "ok": false, "error": e}),
        };
        answer["id"] = msg["id"].clone();
        crate::connect::send(&peer, answer);
    });
}

fn peer_out(peer: String) -> Out {
    Arc::new(move |stream, msg| {
        let m = match msg {
            Msg::Frame { codec, data, key } => json!({"t": "device.frame", "streamId": stream, "codec": codec, "key": key, "data": base64::engine::general_purpose::STANDARD.encode(data)}),
            Msg::End(error) => json!({"t": "device.end", "streamId": stream, "error": error}),
        };
        crate::connect::send(&peer, m);
    })
}

/// What input goes to.
enum Target {
    Scrcpy(Weak<AndroidLive>),
    IosLive { session: Weak<ios_sim::LiveSession>, udid: String, name: String, tool: Option<ios_sim::Tool> },
    Adb { serial: String, size: Arc<Mutex<(u32, u32)>> },
    Sim { udid: String, name: String, tool: Option<ios_sim::Tool>, size: Arc<Mutex<(u32, u32)>> },
}

enum VideoReset {
    Scrcpy(Weak<AndroidLive>),
    Ios(Weak<ios_sim::LiveSession>),
}

impl VideoReset {
    fn reset(&self) {
        match self {
            VideoReset::Scrcpy(s) => {
                if let Some(s) = s.upgrade() {
                    s.session().reset_video();
                }
            }
            VideoReset::Ios(s) => {
                if let Some(s) = s.upgrade() {
                    s.reset_video();
                }
            }
        }
    }
}

fn attach(client: &str, stream: &str, msg: &Value, out: Out, link: Link) -> Result<Value, String> {
    if stream.is_empty() {
        return Err("no streamId".into());
    }
    let device = s(msg, "device");
    let stop = Arc::new(AtomicBool::new(false));
    let flow = Arc::new(Flow::new(window_for(link)));
    let size = Arc::new(Mutex::new((0, 0)));
    let (target, android_live, ios_session, value) = match msg["platform"].as_str() {
        Some("ios") => {
            let tool = ios_sim::input_tool();
            let (tx, rx) = mpsc::channel();
            match ios_sim::LiveSession::start(&device, ios_options(link), tx) {
                Ok(live) => {
                    let live = Arc::new(live);
                    let (w, h) = live.size();
                    let has_hid = live.has_hid();
                    let has_input = has_hid || tool.is_some();
                    let input_tool_name = if has_hid { Some("native") } else { tool.as_ref().map(|t| t.name()) };
                    let (reset, stop2, flow2, out2, stream2) = (VideoReset::Ios(Arc::downgrade(&live)), stop.clone(), flow.clone(), out.clone(), stream.to_string());
                    std::thread::spawn(move || forward(&stream2, rx, reset, &stop2, &flow2, &out2, None));
                    let value = json!({"codec": "h264", "width": w, "height": h, "live": true, "input": has_input, "inputTool": input_tool_name});
                    (Target::IosLive { session: Arc::downgrade(&live), udid: device, name: s(msg, "name"), tool }, None, Some(live), value)
                }
                Err(e) => {
                    eprintln!("devices: live iOS simulator stream unavailable for {device}, using screenshots: {e}");
                    let (udid, size2, stop2, flow2, out2, stream2) = (device.clone(), size.clone(), stop.clone(), flow.clone(), out.clone(), stream.to_string());
                    std::thread::spawn(move || shots(&stream2, &stop2, &flow2, &size2, &out2, "jpeg", Duration::from_millis(250), || ios_sim::screenshot(&udid).map(|b| { let d = ios_sim::jpeg_size(&b).unwrap_or_default(); (b, d) })));
                    let value = json!({"codec": "jpeg", "live": false, "input": tool.is_some(), "inputTool": tool.as_ref().map(|t| t.name())});
                    (Target::Sim { udid: device, name: s(msg, "name"), tool, size }, None, None, value)
                }
            }
        }
        _ => {
            let (tx, rx) = mpsc::channel();
            match scrcpy::Session::start(&device, options(link), tx) {
                Ok(session) => {
                    let session = Arc::new(session);
                    let (w, h) = session.size();
                    let live = Arc::new(AndroidLive {
                        device: device.clone(),
                        stream: stream.to_string(),
                        out: out.clone(),
                        stop: stop.clone(),
                        flow: flow.clone(),
                        link: Mutex::new(link),
                        session: RwLock::new(session),
                        generation: AtomicU64::new(1),
                    });
                    let (reset, stop2, flow2, out2, stream2) = (VideoReset::Scrcpy(Arc::downgrade(&live)), stop.clone(), flow.clone(), out.clone(), stream.to_string());
                    let check = Some((live.clone(), 1));
                    std::thread::spawn(move || forward(&stream2, rx, reset, &stop2, &flow2, &out2, check));
                    (Target::Scrcpy(Arc::downgrade(&live)), Some(live), None, json!({"codec": "h264", "width": w, "height": h, "live": true, "input": true}))
                }
                Err(e) => {
                    eprintln!("devices: scrcpy didn't start on {device}, using screenshots: {e}");
                    let (serial, size2, stop2, flow2, out2, stream2) = (device.clone(), size.clone(), stop.clone(), flow.clone(), out.clone(), stream.to_string());
                    std::thread::spawn(move || shots(&stream2, &stop2, &flow2, &size2, &out2, "png", Duration::from_millis(350), || adb_screenshot(&serial)));
                    (Target::Adb { serial: device, size }, None, None, json!({"codec": "png", "live": false, "input": true}))
                }
            }
        }
    };
    let (input, rx) = mpsc::channel();
    let stop2 = stop.clone();
    std::thread::spawn(move || input_loop(rx, target, &stop2));
    let old = STREAMS.lock().insert(key(client, stream), Stream { client: client.to_string(), stop, flow, input, android: android_live, ios: ios_session });
    drop(old);
    Ok(value)
}

fn adb_screenshot(serial: &str) -> Result<(Vec<u8>, (u32, u32)), String> {
    let mut cmd = android::adb(Some(serial))?;
    cmd.args(["exec-out", "screencap", "-p"]);
    let png = android::run(cmd)?;
    let size = png_size(&png).ok_or("Couldn't read the screenshot")?;
    Ok((png, size))
}

fn png_size(b: &[u8]) -> Option<(u32, u32)> {
    if b.len() < 24 || &b[1..4] != b"PNG" {
        return None;
    }
    Some((u32::from_be_bytes(b[16..20].try_into().ok()?), u32::from_be_bytes(b[20..24].try_into().ok()?)))
}

/// Sends live H.264 frames on, dropping them while too many are on the way.
fn forward(
    stream: &str,
    rx: mpsc::Receiver<scrcpy::Frame>,
    video_reset: VideoReset,
    stop: &AtomicBool,
    flow: &Flow,
    out: &Out,
    gen_check: Option<(Arc<AndroidLive>, u64)>,
) {
    let superseded = || gen_check.as_ref().is_some_and(|(live, g)| live.generation.load(Ordering::SeqCst) != *g);
    let (mut dropping, mut reset) = (false, false);
    loop {
        let wait = if dropping && !reset { Duration::from_millis(15) } else { Duration::from_millis(100) };
        let frame = match rx.recv_timeout(wait) {
            Ok(f) => Some(f),
            Err(RecvTimeoutError::Timeout) => None,
            Err(RecvTimeoutError::Disconnected) => {
                if !stop.load(Ordering::SeqCst) && !superseded() {
                    out(stream, Msg::End("The device's video ended.".into()));
                }
                return;
            }
        };
        if stop.load(Ordering::SeqCst) || superseded() {
            return;
        }
        let window = flow.window();
        let on_the_way = flow.on_the_way();
        // Caught up after dropping: a keyframe to start from.
        if dropping && !reset && on_the_way <= window / 2 {
            video_reset.reset();
            reset = true;
        }
        let Some(frame) = frame else { continue };
        match decide(dropping, reset, on_the_way, window, frame.key) {
            Pass::Send => dropping = false,
            Pass::Drop => continue,
            Pass::StartDropping => {
                (dropping, reset) = (true, false);
                continue;
            }
        }
        flow.sent();
        out(stream, Msg::Frame { codec: "h264", data: frame.data, key: frame.key });
    }
}

#[derive(Debug, PartialEq)]
enum Pass {
    Send,
    Drop,
    StartDropping,
}

/// What happens to a frame: while dropping, only the keyframe after the reset goes; otherwise
/// a frame goes unless the window is full.
fn decide(dropping: bool, reset: bool, on_the_way: usize, window: usize, key: bool) -> Pass {
    if dropping {
        if key && reset { Pass::Send } else { Pass::Drop }
    } else if on_the_way >= window {
        Pass::StartDropping
    } else {
        Pass::Send
    }
}

/// Screenshots, each once the last one was drawn, unchanged ones skipped.
#[allow(clippy::too_many_arguments)]
fn shots(stream: &str, stop: &AtomicBool, flow: &Flow, size: &Mutex<(u32, u32)>, out: &Out, codec: &'static str, every: Duration, grab: impl Fn() -> Result<(Vec<u8>, (u32, u32)), String>) {
    let (mut last, mut fails) = (None, 0);
    while !stop.load(Ordering::SeqCst) {
        flow.wait_idle(stop);
        if stop.load(Ordering::SeqCst) {
            return;
        }
        let at = Instant::now();
        match grab() {
            Ok((data, dims)) => {
                fails = 0;
                *size.lock() = dims;
                let mut h = DefaultHasher::new();
                data.hash(&mut h);
                let h = h.finish();
                if last != Some(h) && !stop.load(Ordering::SeqCst) {
                    last = Some(h);
                    flow.sent();
                    out(stream, Msg::Frame { codec, data, key: true });
                }
            }
            Err(e) => {
                fails += 1;
                if fails >= 5 {
                    if !stop.load(Ordering::SeqCst) {
                        out(stream, Msg::End(e));
                    }
                    return;
                }
            }
        }
        std::thread::sleep(every.saturating_sub(at.elapsed()));
    }
}

/// A gesture made of screenshot-mode touches, in pixels.
#[derive(Debug, PartialEq)]
enum Gesture {
    Tap((f32, f32)),
    Swipe { from: (f32, f32), to: (f32, f32), ms: u32 },
}

/// A press from `from` to `to` (fractions of the screen) that took `ms`: a tap when short and still.
fn gesture(from: (f32, f32), to: (f32, f32), ms: u32, (w, h): (u32, u32)) -> Gesture {
    let px = |p: (f32, f32)| (p.0.clamp(0.0, 1.0) * w as f32, p.1.clamp(0.0, 1.0) * h as f32);
    let (a, b) = (px(from), px(to));
    let moved = ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt();
    if moved < w.max(h) as f32 * 0.01 && ms < 400 { Gesture::Tap(a) } else { Gesture::Swipe { from: a, to: b, ms: ms.max(60) } }
}

/// A scroll by wheel notches (positive: right / up) as a swipe from `at`.
fn scroll_swipe(at: (f32, f32), h: f32, v: f32) -> (f32, f32) {
    ((at.0 - (h * 0.06).clamp(-0.4, 0.4)).clamp(0.0, 1.0), (at.1 + (v * 0.06).clamp(-0.4, 0.4)).clamp(0.0, 1.0))
}

fn android_key(key: &str) -> Option<u32> {
    Some(match key {
        "enter" => 66,
        "backspace" => 67,
        "up" => 19,
        "down" => 20,
        "left" => 21,
        "right" => 22,
        "tab" => 61,
        "escape" | "back" => 4,
        "home" => 3,
        "recents" => 187,
        "power" => 26,
        _ => return None,
    })
}

fn point(msg: &Value) -> (f32, f32) {
    (msg["x"].as_f64().unwrap_or(0.0) as f32, msg["y"].as_f64().unwrap_or(0.0) as f32)
}

/// Applies input in order; consecutive scrolls are merged (each is a command in screenshot mode,
/// or a continuous live touch drag for live iOS simulators).
fn input_loop(rx: mpsc::Receiver<Value>, target: Target, stop: &AtomicBool) {
    // Where the finger went down, where it is, since when.
    type Press = ((f32, f32), (f32, f32), Instant);
    let mut press: Option<Press> = None;
    let mut next = None;
    // Active virtual finger for live trackpad/wheel scrolling on iOS Simulator.
    let mut scroll_finger: Option<(f32, f32)> = None;
    const SCROLL_IDLE: Duration = Duration::from_millis(95);
    loop {
        let msg = if let Some(queued) = next.take() {
            queued
        } else if let Some(pos) = scroll_finger {
            match rx.recv_timeout(SCROLL_IDLE) {
                Ok(m) => m,
                Err(RecvTimeoutError::Timeout) => {
                    if let Target::IosLive { session, .. } = &target
                        && let Some(s) = session.upgrade()
                    {
                        s.touch(scrcpy::Touch::Up, pos);
                    }
                    scroll_finger = None;
                    continue;
                }
                Err(RecvTimeoutError::Disconnected) => return,
            }
        } else {
            let Ok(m) = rx.recv() else { return };
            m
        };
        if stop.load(Ordering::SeqCst) {
            return;
        }
        let kind = msg["kind"].as_str().unwrap_or_default();
        if let Target::Scrcpy(live) = &target {
            let Some(live) = live.upgrade() else { return };
            let session = live.session();
            match kind {
                "touch" => {
                    let touch = match msg["action"].as_str() {
                        Some("down") => scrcpy::Touch::Down,
                        Some("up") => scrcpy::Touch::Up,
                        _ => scrcpy::Touch::Move,
                    };
                    let mut at = point(&msg);
                    if touch == scrcpy::Touch::Move {
                        while let Ok(more) = rx.try_recv() {
                            if more["kind"] == "touch" && more["action"].as_str().is_none_or(|a| a == "move") {
                                at = point(&more);
                            } else {
                                next = Some(more);
                                break;
                            }
                        }
                    }
                    session.touch(touch, at);
                }
                "scroll" => {
                    let (mut at, mut h, mut v) = (point(&msg), msg["h"].as_f64().unwrap_or(0.0) as f32, msg["v"].as_f64().unwrap_or(0.0) as f32);
                    while let Ok(more) = rx.try_recv() {
                        if more["kind"] != "scroll" {
                            next = Some(more);
                            break;
                        }
                        at = point(&more);
                        h += more["h"].as_f64().unwrap_or(0.0) as f32;
                        v += more["v"].as_f64().unwrap_or(0.0) as f32;
                    }
                    session.scroll(at, h, v);
                }
                "text" => session.text(msg["text"].as_str().unwrap_or_default()),
                "key" | "button" => {
                    if let Some(code) = android_key(msg[kind].as_str().unwrap_or_default()) {
                        session.key(code);
                    }
                }
                _ => {}
            }
            continue;
        }
        if let Target::IosLive { session, udid, name, tool } = &target {
            let Some(session) = session.upgrade() else { return };
            if session.has_hid() {
                // Any non-scroll input immediately releases a virtual scroll finger.
                if kind != "scroll"
                    && let Some(pos) = scroll_finger.take()
                {
                    session.touch(scrcpy::Touch::Up, pos);
                }
                match kind {
                    "touch" => {
                        let touch = match msg["action"].as_str() {
                            Some("down") => scrcpy::Touch::Down,
                            Some("up") => scrcpy::Touch::Up,
                            _ => scrcpy::Touch::Move,
                        };
                        let mut at = point(&msg);
                        if touch == scrcpy::Touch::Move {
                            while let Ok(more) = rx.try_recv() {
                                if more["kind"] == "touch" && more["action"].as_str().is_none_or(|a| a == "move") {
                                    at = point(&more);
                                } else {
                                    next = Some(more);
                                    break;
                                }
                            }
                        }
                        session.touch(touch, at);
                    }
                    "scroll" => {
                        // Coalesce any burst of queued scroll ticks into one smooth drag step.
                        let (at, mut h, mut v) = (point(&msg), msg["h"].as_f64().unwrap_or(0.0) as f32, msg["v"].as_f64().unwrap_or(0.0) as f32);
                        while let Ok(more) = rx.try_recv() {
                            if more["kind"] != "scroll" {
                                next = Some(more);
                                break;
                            }
                            h += more["h"].as_f64().unwrap_or(0.0) as f32;
                            v += more["v"].as_f64().unwrap_or(0.0) as f32;
                        }
                        let dx = (-h * 0.035).clamp(-0.35, 0.35);
                        let dy = (v * 0.035).clamp(-0.35, 0.35);
                        if dx.abs() > 0.0005 || dy.abs() > 0.0005 {
                            let cur = match scroll_finger {
                                Some(p) => p,
                                None => {
                                    // Keep start away from system edge gesture bands.
                                    let start = (at.0.clamp(0.12, 0.88), at.1.clamp(0.12, 0.88));
                                    session.touch(scrcpy::Touch::Down, start);
                                    start
                                }
                            };
                            let mut to = (cur.0 + dx, cur.1 + dy);
                            // If nearing an edge on a long continuous scroll, re-anchor at center.
                            if to.0 < 0.08 || to.0 > 0.92 || to.1 < 0.08 || to.1 > 0.92 {
                                session.touch(scrcpy::Touch::Up, cur);
                                let anchor = (at.0.clamp(0.2, 0.8), at.1.clamp(0.2, 0.8));
                                session.touch(scrcpy::Touch::Down, anchor);
                                to = ((anchor.0 + dx).clamp(0.08, 0.92), (anchor.1 + dy).clamp(0.08, 0.92));
                            }
                            session.touch(scrcpy::Touch::Move, to);
                            scroll_finger = Some(to);
                        }
                    }
                    "text" => {
                        let text = s(&msg, "text");
                        if !session.text(&text)
                            && let Some(tool) = tool
                        {
                            let _ = ios_sim::act(tool, udid, &ios_sim::Action::Text(text));
                        }
                    }
                    "key" => {
                        if let Some(code) = ios_sim::hid_key(msg["key"].as_str().unwrap_or_default()) {
                            session.key(code);
                        }
                    }
                    "button" => {
                        if let Some(btn) = msg["button"].as_str() {
                            session.button(btn);
                        }
                    }
                    _ => {}
                }
                continue;
            }
            // Live video without native HID: fall back to AXe/idb gestures below.
            let Some(tool) = tool else { continue };
            let gest = match kind {
                "touch" => match msg["action"].as_str() {
                    Some("down") => {
                        press = Some((point(&msg), point(&msg), Instant::now()));
                        None
                    }
                    Some("up") => press.take().map(|(from, _, at)| (from, point(&msg), at.elapsed().as_millis() as u32)),
                    _ => {
                        if let Some(p) = press.as_mut() {
                            p.1 = point(&msg);
                        }
                        None
                    }
                },
                "scroll" => {
                    let (at, mut h, mut v) = (point(&msg), msg["h"].as_f64().unwrap_or(0.0) as f32, msg["v"].as_f64().unwrap_or(0.0) as f32);
                    while let Ok(more) = rx.try_recv() {
                        if more["kind"] != "scroll" {
                            next = Some(more);
                            break;
                        }
                        h += more["h"].as_f64().unwrap_or(0.0) as f32;
                        v += more["v"].as_f64().unwrap_or(0.0) as f32;
                    }
                    Some((at, scroll_swipe(at, h, v), 150))
                }
                _ => None,
            };
            let (w, h) = session.size();
            let scale = ios_sim::scale(name, (w, h));
            let pt = |p: (f32, f32)| (p.0 / scale, p.1 / scale);
            let action = match (kind, gest) {
                (_, Some((from, to, ms))) => Some(match gesture(from, to, ms, (w, h)) {
                    Gesture::Tap(p) => ios_sim::Action::Tap(pt(p).0, pt(p).1),
                    Gesture::Swipe { from, to, ms } => ios_sim::Action::Swipe { from: pt(from), to: pt(to), secs: ms as f32 / 1000.0 },
                }),
                ("text", _) => Some(ios_sim::Action::Text(s(&msg, "text"))),
                ("key", _) => ios_sim::hid_key(msg["key"].as_str().unwrap_or_default()).map(ios_sim::Action::Key),
                ("button", _) => match msg["button"].as_str() {
                    Some("home") => Some(ios_sim::Action::Button("home")),
                    Some("power") => Some(ios_sim::Action::Button("lock")),
                    _ => None,
                },
                _ => None,
            };
            if let Some(a) = action
                && let Err(e) = ios_sim::act(tool, udid, &a)
            {
                eprintln!("devices: input failed: {e}");
            }
            continue;
        }
        // Screenshot mode: touches make gestures, run when the finger lifts.
        let gest = match kind {
            "touch" => match msg["action"].as_str() {
                Some("down") => {
                    press = Some((point(&msg), point(&msg), Instant::now()));
                    None
                }
                Some("up") => press.take().map(|(from, _, at)| (from, point(&msg), at.elapsed().as_millis() as u32)),
                _ => {
                    if let Some(p) = press.as_mut() {
                        p.1 = point(&msg);
                    }
                    None
                }
            },
            "scroll" => {
                let (at, mut h, mut v) = (point(&msg), msg["h"].as_f64().unwrap_or(0.0) as f32, msg["v"].as_f64().unwrap_or(0.0) as f32);
                while let Ok(more) = rx.try_recv() {
                    if more["kind"] != "scroll" {
                        next = Some(more);
                        break;
                    }
                    h += more["h"].as_f64().unwrap_or(0.0) as f32;
                    v += more["v"].as_f64().unwrap_or(0.0) as f32;
                }
                Some((at, scroll_swipe(at, h, v), 150))
            }
            _ => None,
        };
        let result = match &target {
            Target::Scrcpy(_) | Target::IosLive { .. } => Ok(()),
            Target::Adb { serial, size } => {
                let input = match (kind, gest) {
                    (_, Some((from, to, ms))) => match gesture(from, to, ms, *size.lock()) {
                        Gesture::Tap((x, y)) => Some(android::Input::Tap { x: x as u32, y: y as u32 }),
                        Gesture::Swipe { from, to, ms } => Some(android::Input::Swipe { x1: from.0 as u32, y1: from.1 as u32, x2: to.0 as u32, y2: to.1 as u32, ms }),
                    },
                    ("text", _) => Some(android::Input::Text { text: s(&msg, "text") }),
                    ("key" | "button", _) => android_key(msg[kind].as_str().unwrap_or_default()).map(|code| android::Input::Key { code }),
                    _ => None,
                };
                match input {
                    Some(i) => tauri::async_runtime::block_on(android::android_input(serial.clone(), i)),
                    None => Ok(()),
                }
            }
            Target::Sim { udid, name, tool, size } => {
                let Some(tool) = tool else { continue };
                // Points, from the screenshot's pixels.
                let (w, h) = *size.lock();
                let scale = ios_sim::scale(name, (w, h));
                let pt = |p: (f32, f32)| (p.0 / scale, p.1 / scale);
                let action = match (kind, gest) {
                    (_, Some((from, to, ms))) => Some(match gesture(from, to, ms, (w, h)) {
                        Gesture::Tap(p) => ios_sim::Action::Tap(pt(p).0, pt(p).1),
                        Gesture::Swipe { from, to, ms } => ios_sim::Action::Swipe { from: pt(from), to: pt(to), secs: ms as f32 / 1000.0 },
                    }),
                    ("text", _) => Some(ios_sim::Action::Text(s(&msg, "text"))),
                    ("key", _) => ios_sim::hid_key(msg["key"].as_str().unwrap_or_default()).map(ios_sim::Action::Key),
                    ("button", _) => match msg["button"].as_str() {
                        Some("home") => Some(ios_sim::Action::Button("home")),
                        Some("power") => Some(ios_sim::Action::Button("lock")),
                        _ => None,
                    },
                    _ => None,
                };
                match action {
                    Some(a) => ios_sim::act(tool, udid, &a),
                    None => Ok(()),
                }
            }
        };
        if let Err(e) = result {
            eprintln!("devices: input failed: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drops_frames_while_the_link_is_behind() {
        assert_eq!(decide(false, false, 0, WINDOW_LAN, false), Pass::Send);
        assert_eq!(decide(false, false, WINDOW_LAN, WINDOW_LAN, true), Pass::StartDropping);
        // Dropping: nothing until the keyframe that follows the reset.
        assert_eq!(decide(true, false, 0, WINDOW_LAN, true), Pass::Drop);
        assert_eq!(decide(true, true, 0, WINDOW_LAN, false), Pass::Drop);
        assert_eq!(decide(true, true, 0, WINDOW_LAN, true), Pass::Send);
    }

    #[test]
    fn acks_settle_the_flow() {
        let flow = Flow::default();
        flow.sent();
        flow.sent();
        assert_eq!(flow.on_the_way(), 2);
        flow.done();
        flow.done();
        flow.done();
        assert_eq!(flow.on_the_way(), 0);
    }

    #[test]
    fn touches_make_taps_and_swipes() {
        let size = (1000, 2000);
        assert_eq!(gesture((0.5, 0.5), (0.505, 0.5), 100, size), Gesture::Tap((500.0, 1000.0)));
        assert_eq!(gesture((0.5, 0.8), (0.5, 0.2), 300, size), Gesture::Swipe { from: (500.0, 1600.0), to: (500.0, 400.0), ms: 300 });
        // A long press is a swipe in place (held), not a tap.
        assert!(matches!(gesture((0.5, 0.5), (0.5, 0.5), 900, size), Gesture::Swipe { ms: 900, .. }));
        // Scrolling up drags the content down.
        let to = scroll_swipe((0.5, 0.5), 0.0, 2.0);
        assert!(to.1 > 0.5 && to.0 == 0.5);
    }

    #[test]
    fn reads_png_size() {
        let mut png = vec![0x89, b'P', b'N', b'G', 13, 10, 26, 10, 0, 0, 0, 13, b'I', b'H', b'D', b'R'];
        png.extend(1080u32.to_be_bytes());
        png.extend(2400u32.to_be_bytes());
        assert_eq!(png_size(&png), Some((1080, 2400)));
        assert_eq!(png_size(b"nope"), None);
    }
}
