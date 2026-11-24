//! Android devices and emulators over adb, mirrored in the side panel (port of
//! `features/android`): live H.264 from the scrcpy server (with live touch input, see scrcpy.rs),
//! else from `adb screenrecord` with `adb shell input`, decoded with openh264; else screenshots.

use super::scrcpy::{self, Touch};
use crate::backend;
use crate::model::AppState;
use crate::theme::{self, Colors};
use base64::Engine as _;
use gpui_kit::assets::IconName;
use gpui_kit::component::Icon;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use openh264::decoder::Decoder;
use openh264::formats::YUVSource;
use parking_lot::Mutex;
use serde_json::{Value, json};
use smallvec::smallvec;
use std::sync::Arc;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};
use tauri::Manager;
use tauri::ipc::{Channel, InvokeResponseBody};
use winfer_backend::android;

const KEY_BACK: u32 = 4;
const KEY_HOME: u32 = 3;
const KEY_RECENTS: u32 = 187;
const KEY_POWER: u32 = 26;

/// Android keycode for a key without text.
fn keycode(key: &str) -> Option<u32> {
    Some(match key {
        "enter" => 66,
        "backspace" => 67,
        "up" => 19,
        "down" => 20,
        "left" => 21,
        "right" => 22,
        "tab" => 61,
        "escape" => KEY_BACK,
        _ => return None,
    })
}

/// Offsets of start codes (00 00 01, possibly preceded by another 00) in `buf`.
pub fn start_codes(buf: &[u8]) -> Vec<usize> {
    let mut out = Vec::new();
    let mut i = 0;
    while i + 2 < buf.len() {
        if buf[i] == 0 && buf[i + 1] == 0 && buf[i + 2] == 1 {
            out.push(if i > 0 && buf[i - 1] == 0 { i - 1 } else { i });
            i += 3;
        } else {
            i += 1;
        }
    }
    out
}

/// Where a point lands on a picture fitted (contain) into `area`, as a fraction of it (0..1).
pub fn to_unit(at: (f32, f32), area: (f32, f32, f32, f32), video: (f32, f32)) -> (f32, f32) {
    let (ax, ay, aw, ah) = area;
    let scale = (aw / video.0).min(ah / video.1);
    let (w, h) = (video.0 * scale, video.1 * scale);
    let (left, top) = (ax + (aw - w) / 2.0, ay + (ah - h) / 2.0);
    (((at.0 - left) / w).clamp(0.0, 1.0), ((at.1 - top) / h).clamp(0.0, 1.0))
}

/// Where a point lands on the device, for a picture fitted (contain) into `area`.
pub fn to_device(at: (f32, f32), area: (f32, f32, f32, f32), video: (f32, f32), device: (f32, f32)) -> (u32, u32) {
    let (x, y) = to_unit(at, area, video);
    ((x * device.0).round() as u32, (y * device.1).round() as u32)
}

/// How the mirror runs, best first; each falls back to the next when it can't.
#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Scrcpy,
    Stream,
    Screenshots,
}

struct Mirror {
    serial: String,
    info: Option<(f32, f32, f32, f32)>,
    mode: Mode,
    error: Option<String>,
    frames: usize,
    typed: String,
    /// Set once the scrcpy server runs (Mode::Scrcpy); dropping it stops the server.
    session: Option<Arc<scrcpy::Session>>,
    _pump: Task<()>,
}

pub struct AndroidPanel {
    state: Entity<AppState>,
    visible: bool,
    status: Option<Value>,
    starting: Option<String>,
    error: Option<String>,
    mirror: Option<Mirror>,
    restart: u64,
    latest: Arc<Mutex<Option<Arc<RenderImage>>>>,
    shown: Option<Arc<RenderImage>>,
    area: Bounds<Pixels>,
    press: Option<(u32, u32, Instant)>,
    /// A finger is down on the scrcpy session.
    touching: bool,
    focus: FocusHandle,
    _poll: Task<()>,
}

fn render_image(mut rgba: Vec<u8>, w: u32, h: u32) -> Option<Arc<RenderImage>> {
    for p in rgba.chunks_exact_mut(4) {
        p.swap(0, 2);
    }
    let buffer = ::image::RgbaImage::from_raw(w, h, rgba)?;
    Some(Arc::new(RenderImage::new(smallvec![::image::Frame::new(buffer)])))
}

/// Decodes NAL units or whole frames into `latest`; false once the stream can't be decoded.
fn frame_decoder(latest: Arc<Mutex<Option<Arc<RenderImage>>>>, frames: async_channel::Sender<Result<(), String>>) -> Option<impl FnMut(&[u8]) -> bool> {
    let mut decoder = match Decoder::new() {
        Ok(d) => d,
        Err(e) => {
            let _ = frames.try_send(Err(e.to_string()));
            return None;
        }
    };
    let mut errors = 0;
    Some(move |data: &[u8]| -> bool {
        match decoder.decode(data) {
            Ok(Some(yuv)) => {
                errors = 0;
                let (w, h) = yuv.dimensions();
                let mut rgba = vec![0; w * h * 4];
                yuv.write_rgba8(&mut rgba);
                if let Some(image) = render_image(rgba, w as u32, h as u32) {
                    super::capture::offer_image(super::capture::Source::Android, &image);
                    *latest.lock() = Some(image);
                    let _ = frames.try_send(Ok(()));
                }
                true
            }
            Ok(None) => true,
            Err(e) => {
                errors += 1;
                if errors > 30 {
                    let _ = frames.try_send(Err(e.to_string()));
                    return false;
                }
                true
            }
        }
    })
}

/// Whole frames from the scrcpy server, decoded as they come.
fn decode_frames(rx: mpsc::Receiver<Vec<u8>>, latest: Arc<Mutex<Option<Arc<RenderImage>>>>, frames: async_channel::Sender<Result<(), String>>) {
    let Some(mut feed) = frame_decoder(latest, frames.clone()) else { return };
    while let Ok(frame) = rx.recv() {
        if !feed(&frame) {
            return;
        }
    }
    let _ = frames.try_send(Err("the scrcpy stream ended".into()));
}

/// Feeds complete NAL units of the raw screenrecord stream to the decoder.
fn decode_loop(rx: mpsc::Receiver<Vec<u8>>, latest: Arc<Mutex<Option<Arc<RenderImage>>>>, frames: async_channel::Sender<Result<(), String>>) {
    let Some(mut feed) = frame_decoder(latest, frames) else { return };
    let mut pending: Vec<u8> = Vec::new();
    loop {
        match rx.recv_timeout(Duration::from_millis(150)) {
            Ok(chunk) => {
                pending.extend_from_slice(&chunk);
                let starts = start_codes(&pending);
                if starts.len() < 2 {
                    continue;
                }
                // Every NAL but the last is complete; the last one may still be arriving.
                for pair in starts.windows(2) {
                    if !feed(&pending[pair[0]..pair[1]]) {
                        return;
                    }
                }
                pending.drain(..*starts.last().unwrap());
            }
            // The encoder only sends frames when the screen changes: the newest NAL may be the last for a while.
            Err(RecvTimeoutError::Timeout) => {
                if pending.len() > 4 && !feed(&pending) {
                    return;
                }
                pending.clear();
            }
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

/// Redraws for each decoded frame (several queued ones at once: only the newest is shown) until
/// the stream fails, then falls back to `next`.
async fn pump_frames(this: WeakEntity<AndroidPanel>, cx: &mut AsyncApp, frame_rx: async_channel::Receiver<Result<(), String>>, next: Mode) {
    while let Ok(first) = frame_rx.recv().await {
        let mut failed = first.err();
        let mut count = 1;
        while failed.is_none()
            && let Ok(more) = frame_rx.try_recv()
        {
            failed = more.err();
            count += 1;
        }
        let done = this.update(cx, |this, cx| {
            if let Some(e) = failed {
                eprintln!("android: mirror stream failed, falling back: {e}");
                this.fall_back(next, cx);
                return true;
            }
            if let Some(m) = this.mirror.as_mut() {
                m.frames += count;
            }
            cx.notify();
            false
        });
        if !matches!(done, Ok(false)) {
            break;
        }
    }
}

fn decode_data_url(url: &str) -> Option<(Arc<RenderImage>, u32, u32)> {
    let encoded = url.split_once(',').map_or(url, |(_, b)| b);
    let bytes = base64::engine::general_purpose::STANDARD.decode(encoded).ok()?;
    super::capture::offer_encoded(super::capture::Source::Android, &bytes);
    let rgba = ::image::load_from_memory(&bytes).ok()?.into_rgba8();
    let (w, h) = rgba.dimensions();
    Some((render_image(rgba.into_raw(), w, h)?, w, h))
}

impl AndroidPanel {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let poll = cx.spawn(async move |this, cx| {
            loop {
                let visible = this.update(cx, |this, _| this.visible).unwrap_or(false);
                if visible {
                    let status = tauri::async_runtime::spawn(android::android_status()).await.map_err(|e| e.to_string()).and_then(|r| r).and_then(|s| serde_json::to_value(s).map_err(|e| e.to_string()));
                    if this.update(cx, |this, cx| this.on_status(status, cx)).is_err() {
                        break;
                    }
                }
                cx.background_executor().timer(Duration::from_secs(3)).await;
            }
        });
        Self { state, visible: false, status: None, starting: None, error: None, mirror: None, restart: 0, latest: Default::default(), shown: None, area: Bounds::default(), press: None, touching: false, focus: cx.focus_handle(), _poll: poll }
    }

    /// Shown or hidden with the side panel's Android tab: the mirror only runs while on screen.
    pub fn set_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        if self.visible == visible {
            return;
        }
        self.visible = visible;
        if visible {
            self.sync_device(cx);
        } else {
            self.stop_mirror();
        }
    }

    fn on_status(&mut self, status: Result<Value, String>, cx: &mut Context<Self>) {
        match status {
            Ok(s) => self.status = Some(s),
            Err(e) => self.error = Some(e),
        }
        self.sync_device(cx);
        cx.notify();
    }

    fn ready(&self) -> Vec<Value> {
        self.status.iter().flat_map(|s| s["devices"].as_array().into_iter().flatten()).filter(|d| d["state"] == "device").cloned().collect()
    }

    fn current(&self, cx: &App) -> Option<Value> {
        let ready = self.ready();
        let saved = self.state.read(cx).rest.get("androidSerial").and_then(Value::as_str).map(str::to_string);
        ready.iter().find(|d| d["serial"].as_str() == saved.as_deref()).or(ready.first()).cloned()
    }

    fn sync_device(&mut self, cx: &mut Context<Self>) {
        if !self.visible {
            return;
        }
        let serial = self.current(cx).and_then(|d| d["serial"].as_str().map(str::to_string));
        if self.mirror.as_ref().map(|m| &m.serial) == serial.as_ref() {
            return;
        }
        self.stop_mirror();
        if let Some(serial) = serial {
            self.start_mirror(serial, Mode::Scrcpy, cx);
        }
    }

    fn stop_mirror(&mut self) {
        if let Some(m) = self.mirror.take()
            && m.mode == Mode::Stream
        {
            let serial = m.serial;
            backend::background(move || android::android_mirror_stop(backend::app().state(), serial));
        }
        *self.latest.lock() = None;
        self.touching = false;
    }

    fn start_mirror(&mut self, serial: String, mode: Mode, cx: &mut Context<Self>) {
        self.restart += 1;
        let pump = match mode {
            Mode::Scrcpy => self.scrcpy_task(serial.clone(), cx),
            Mode::Stream => self.stream_task(serial.clone(), cx),
            Mode::Screenshots => self.screenshot_task(serial.clone(), cx),
        };
        self.mirror = Some(Mirror { serial, info: None, mode, error: None, frames: 0, typed: String::new(), session: None, _pump: pump });
        cx.notify();
    }

    fn stream_task(&mut self, serial: String, cx: &mut Context<Self>) -> Task<()> {
        let (bytes, rx) = mpsc::channel::<Vec<u8>>();
        let (frames, frame_rx) = async_channel::unbounded();
        let latest = self.latest.clone();
        std::thread::spawn(move || decode_loop(rx, latest, frames));
        let channel: Channel = Channel::new(move |body| {
            if let InvokeResponseBody::Raw(data) = body {
                let _ = bytes.send(data);
            }
            Ok(())
        });
        cx.spawn(async move |this, cx| {
            let start = serial.clone();
            let info = backend::background(move || android::android_mirror_start(backend::app().state(), start, channel).map(|i| serde_json::to_value(i).unwrap_or_default())).await.unwrap_or_else(|e| Err(e.to_string()));
            let ok = this.update(cx, |this, cx| {
                if let Some(m) = this.mirror.as_mut() {
                    match &info {
                        Ok(i) => m.info = Some((i["width"].as_f64().unwrap_or(0.0) as f32, i["height"].as_f64().unwrap_or(0.0) as f32, i["videoWidth"].as_f64().unwrap_or(0.0) as f32, i["videoHeight"].as_f64().unwrap_or(0.0) as f32)),
                        Err(e) => m.error = Some(e.clone()),
                    }
                }
                cx.notify();
                info.is_ok()
            });
            if !matches!(ok, Ok(true)) {
                return;
            }
            pump_frames(this, cx, frame_rx, Mode::Screenshots).await;
        })
    }

    fn scrcpy_task(&mut self, serial: String, cx: &mut Context<Self>) -> Task<()> {
        let (frames, frame_rx) = async_channel::unbounded();
        let latest = self.latest.clone();
        cx.spawn(async move |this, cx| {
            let (tx, rx) = mpsc::channel::<Vec<u8>>();
            let start = serial.clone();
            let session = backend::background(move || scrcpy::Session::start(&start, tx)).await.unwrap_or_else(|e| Err(e.to_string()));
            let session = match session {
                Ok(s) => Arc::new(s),
                // No scrcpy (offline, old device, …): the screenrecord stream instead.
                Err(e) => {
                    eprintln!("android: scrcpy didn't start, using screenrecord: {e}");
                    let _ = this.update(cx, |this, cx| this.fall_back(Mode::Stream, cx));
                    return;
                }
            };
            std::thread::spawn(move || decode_frames(rx, latest, frames));
            let ok = this.update(cx, |this, cx| {
                let Some(m) = this.mirror.as_mut().filter(|m| m.serial == serial) else { return false };
                let (w, h) = session.size();
                m.info = Some((w as f32, h as f32, w as f32, h as f32));
                m.session = Some(session);
                cx.notify();
                true
            });
            if matches!(ok, Ok(true)) {
                pump_frames(this, cx, frame_rx, Mode::Stream).await;
            }
        })
    }

    /// The current mirror can't go on: restart it in the next mode.
    fn fall_back(&mut self, next: Mode, cx: &mut Context<Self>) {
        let serial = self.mirror.as_ref().map(|m| m.serial.clone());
        self.stop_mirror();
        if let Some(s) = serial {
            self.start_mirror(s, next, cx);
        }
    }

    fn screenshot_task(&mut self, serial: String, cx: &mut Context<Self>) -> Task<()> {
        cx.spawn(async move |this, cx| {
            loop {
                let shot = tauri::async_runtime::spawn(android::android_screenshot(serial.clone())).await.map_err(|e| e.to_string()).and_then(|r| r);
                let decoded = shot.map(|url| decode_data_url(&url));
                let alive = this.update(cx, |this, cx| {
                    if let Some(m) = this.mirror.as_mut() {
                        match decoded {
                            Ok(Some((image, w, h))) => {
                                *this.latest.lock() = Some(image);
                                m.info.get_or_insert((w as f32, h as f32, w as f32, h as f32));
                                m.frames = 2;
                            }
                            Ok(None) => m.error = Some("Couldn't read the screenshot".into()),
                            Err(e) => m.error = Some(e),
                        }
                    }
                    cx.notify();
                });
                if alive.is_err() {
                    break;
                }
                cx.background_executor().timer(Duration::from_millis(500)).await;
            }
        })
    }

    fn session(&self) -> Option<&Arc<scrcpy::Session>> {
        self.mirror.as_ref()?.session.as_ref()
    }

    /// A point on the picture as a fraction of it, for the scrcpy session (whose video size
    /// changes when the device rotates).
    fn unit_point(&self, at: Point<Pixels>) -> Option<(f32, f32)> {
        let (vw, vh) = self.session()?.size();
        let b = self.area;
        Some(to_unit((at.x / px(1.0), at.y / px(1.0)), (b.origin.x / px(1.0), b.origin.y / px(1.0), b.size.width / px(1.0), b.size.height / px(1.0)), (vw as f32, vh as f32)))
    }

    fn touch(&mut self, touch: Touch, at: Point<Pixels>) -> bool {
        let (Some(session), Some(p)) = (self.session(), self.unit_point(at)) else { return false };
        session.touch(touch, p);
        self.touching = touch != Touch::Up;
        true
    }

    fn send(&self, input: Value) {
        let Some(serial) = self.mirror.as_ref().map(|m| m.serial.clone()) else { return };
        tauri::async_runtime::spawn(async move {
            if let Ok(input) = serde_json::from_value(input) {
                let _ = android::android_input(serial, input).await;
            }
        });
    }

    fn key(&self, code: u32) {
        match self.session() {
            Some(s) => s.key(code),
            None => self.send(json!({ "kind": "key", "code": code })),
        }
    }

    fn device_point(&self, at: Point<Pixels>) -> Option<(u32, u32)> {
        let (w, h, vw, vh) = self.mirror.as_ref()?.info?;
        let b = self.area;
        Some(to_device((at.x / px(1.0), at.y / px(1.0)), (b.origin.x / px(1.0), b.origin.y / px(1.0), b.size.width / px(1.0), b.size.height / px(1.0)), (vw, vh), (w, h)))
    }

    fn flush_text(&mut self) {
        if let Some(text) = self.mirror.as_mut().map(|m| std::mem::take(&mut m.typed)).filter(|t| !t.is_empty()) {
            self.send(json!({ "kind": "text", "text": text }));
        }
    }

    fn on_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let k = &event.keystroke;
        if k.modifiers.platform || k.modifiers.control {
            return;
        }
        if let Some(text) = k.key_char.clone().filter(|t| !t.chars().any(char::is_control)) {
            if let Some(s) = self.session() {
                s.text(&text);
                return;
            }
            if let Some(m) = self.mirror.as_mut() {
                m.typed.push_str(&text);
            }
            // Printable keys are batched into `input text`.
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(Duration::from_millis(120)).await;
                let _ = this.update(cx, |this, _| this.flush_text());
            })
            .detach();
        } else if let Some(code) = keycode(&k.key) {
            self.flush_text();
            self.key(code);
        }
    }

    fn start_avd(&mut self, name: String, cx: &mut Context<Self>) {
        self.starting = Some(name.clone());
        self.error = None;
        if let Err(e) = android::android_start_avd(name) {
            self.error = Some(e);
        }
        // Booting takes a while; the device list picks it up when it's ready.
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_secs(30)).await;
            let _ = this.update(cx, |this, cx| {
                this.starting = None;
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn render_empty(&self, c: Colors, cx: &mut Context<Self>) -> Div {
        let mut body = div().flex_1().flex().flex_col().items_center().justify_center().gap_3().p_6().text_center().text_xs().text_color(c.muted);
        let status = self.status.clone().unwrap_or_default();
        if status["adb"] != true {
            return body.child("adb isn't installed. Install Android Studio's platform-tools (macOS: brew install android-platform-tools).");
        }
        body = body.child("Connect a phone with USB debugging on, or start an emulator.");
        for d in status["devices"].as_array().into_iter().flatten().filter(|d| d["state"] != "device") {
            let state = d["state"].as_str().unwrap_or_default();
            body = body.child(div().text_color(c.warning).child(format!("{}: {}", d["serial"].as_str().unwrap_or_default(), if state == "unauthorized" { "allow USB debugging on the phone" } else { state })));
        }
        let mut list = div().w(px(240.0)).flex().flex_col().gap_1();
        for (i, avd) in status["avds"].as_array().into_iter().flatten().filter_map(Value::as_str).enumerate() {
            let name = avd.to_string();
            let booting = self.starting.as_deref() == Some(avd);
            let free = self.starting.is_none();
            list = list.child(div().id(("avd", i)).flex().items_center().gap_2().rounded_md().border_1().border_color(c.edge).px_2p5().py_1p5().text_color(c.fg).when(free, |d| d.cursor_pointer().hover(move |d| d.bg(c.hover))).when(!free, |d| d.opacity(0.5))
                .on_click(cx.listener(move |this, _, _, cx| if this.starting.is_none() { this.start_avd(name.clone(), cx) }))
                .child(Icon::new(IconName::Smartphone).size(px(12.0)))
                .child(div().flex_1().truncate().text_left().child(avd.replace('_', " ")))
                .child(div().text_size(px(10.0)).text_color(c.subtle).child(if booting { "booting…" } else { "Start" })));
        }
        body.child(list).when_some(self.error.clone(), |d, e| d.child(div().text_color(c.danger).child(e)))
    }

    fn render_mirror(&mut self, device: &Value, c: Colors, window: &mut Window, cx: &mut Context<Self>) -> Div {
        if let Some(next) = self.latest.lock().take() {
            if let Some(old) = self.shown.replace(next) {
                cx.drop_image(old, Some(window));
            }
        }
        let model = device["model"].as_str().unwrap_or_default().to_string();
        let (frames, error, mode) = self.mirror.as_ref().map_or((0, None, Mode::Stream), |m| (m.frames, m.error.clone(), m.mode));
        let entity = cx.entity();
        let stage = div().id("android-stage").relative().flex_1().min_h_0().overflow_hidden().bg(c.app).track_focus(&self.focus)
            .on_key_down(cx.listener(|this, e: &KeyDownEvent, _, cx| this.on_key(e, cx)))
            .on_mouse_down(MouseButton::Left, cx.listener(|this, e: &MouseDownEvent, window, cx| {
                this.focus.focus(window, cx);
                if !this.touch(Touch::Down, e.position) {
                    this.press = this.device_point(e.position).map(|(x, y)| (x, y, Instant::now()));
                }
            }))
            // With scrcpy the finger follows the pointer live; screenrecord mode replays the drag on release.
            .on_mouse_move(cx.listener(|this, e: &MouseMoveEvent, _, _| {
                if this.touching && e.pressed_button == Some(MouseButton::Left) {
                    this.touch(Touch::Move, e.position);
                }
            }))
            .on_mouse_up_out(MouseButton::Left, cx.listener(|this, e: &MouseUpEvent, _, _| {
                if this.touching {
                    this.touch(Touch::Up, e.position);
                }
            }))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, e: &MouseUpEvent, _, _| {
                if this.touching {
                    this.touch(Touch::Up, e.position);
                    return;
                }
                let (Some((x1, y1, at)), Some((x2, y2))) = (this.press.take(), this.device_point(e.position)) else { return };
                let ms = at.elapsed().as_millis() as u64;
                let moved = ((x2 as f64 - x1 as f64).powi(2) + (y2 as f64 - y1 as f64).powi(2)).sqrt();
                if moved < 12.0 && ms < 400 {
                    this.send(json!({ "kind": "tap", "x": x1, "y": y1 }));
                } else {
                    this.send(json!({ "kind": "swipe", "x1": x1, "y1": y1, "x2": x2, "y2": y2, "ms": ms.max(60) }));
                }
            }))
            .on_scroll(cx.listener(|this, e: &ScrollEvent, _, _| {
                if let (Some(s), Some(p)) = (this.session(), this.unit_point(e.position)) {
                    // Wheel notches; a trackpad's pixels as fractions of one.
                    let d = e.delta.pixel_delta(px(20.0));
                    s.scroll(p, -(d.x / px(20.0)), d.y / px(20.0));
                    return;
                }
                // Scroll by swiping from the pointer.
                let Some((x, y)) = this.device_point(e.position) else { return };
                let dy = (e.delta.pixel_delta(px(20.0)).y / px(1.0) * 3.0).clamp(-600.0, 600.0);
                this.send(json!({ "kind": "swipe", "x1": x, "y1": y, "x2": x, "y2": (y as f32 + dy).max(0.0) as u32, "ms": 120 }));
            }));
        // The picture lives in an absolutely positioned box inset from the stage, so its size never
        // depends on the image's own pixel size and it can't push the panel taller than its parent.
        // The measured `area` is this same box, which keeps pointer mapping aligned with the picture.
        let area = self.area;
        let mut fit = div().absolute().top(px(12.0)).left(px(12.0)).right(px(12.0)).bottom(px(12.0)).flex().items_center().justify_center()
            .child(canvas(move |bounds, _, cx| entity.update(cx, |this, cx| if this.area != bounds {
                this.area = bounds;
                cx.notify();
            }), |_, _, _, _| {}).absolute().size_full());
        if let Some(image) = self.shown.clone().filter(|_| frames > 0) {
            // Sized explicitly: gpui's img adds the picture's aspect ratio to its style, which with a
            // percentage size lets it grow to the picture's own pixel size and spill out of the box.
            let video = image.size(0);
            let ratio = (video.width.0 as f32 / video.height.0.max(1) as f32).max(0.01);
            let (aw, ah) = (area.size.width / px(1.0), area.size.height / px(1.0));
            let width = aw.min(ah * ratio).max(0.0).floor();
            fit = fit.child(img(ImageSource::Render(image)).w(px(width)).h(px((width / ratio).floor())).object_fit(ObjectFit::Fill));
        } else {
            fit = fit.child(div().size_full().flex().flex_col().items_center().justify_center().gap_2().p_6().text_center().text_xs().text_color(c.subtle).child(match error {
                Some(e) => div().flex().flex_col().items_center().gap_2().child(div().text_color(c.danger).child(e)).child(div().id("android-retry").rounded_md().border_1().border_color(c.edge).px_2().py_1().text_color(c.muted).cursor_pointer().on_click(cx.listener(|this, _, _, cx| this.reconnect(cx))).child("Try again")),
                None => div().child(format!("Connecting to {model}… (the picture appears when the screen changes)")),
            }));
        }
        let stage = stage.child(fit);
        let button = move |id: &'static str, icon: IconName| div().id(id).rounded_sm().p_1p5().text_color(c.subtle).cursor_pointer().hover(move |d| d.bg(c.hover).text_color(c.fg)).child(Icon::new(icon).size(px(13.0)));
        div().flex_1().min_h_0().overflow_hidden().flex().flex_col().child(stage).child(div().flex().flex_shrink_0().items_center().justify_center().gap_1().py_1p5()
            .child(button("android-back", IconName::ChevronLeft).on_click(cx.listener(|this, _, _, _| this.key(KEY_BACK))))
            .child(button("android-home", IconName::Circle).on_click(cx.listener(|this, _, _, _| this.key(KEY_HOME))))
            .child(button("android-recents", IconName::Square).on_click(cx.listener(|this, _, _, _| this.key(KEY_RECENTS))))
            .child(div().mx_1().h_4().w(px(1.0)).bg(c.border))
            .child(button("android-power", IconName::Power).on_click(cx.listener(|this, _, _, _| this.key(KEY_POWER))))
            .child(button("android-reconnect", IconName::RefreshCw).on_click(cx.listener(|this, _, _, cx| this.reconnect(cx))))
            .child(div().ml_2().text_size(px(10.0)).text_color(c.subtle).child(match mode {
                Mode::Scrcpy => "live",
                Mode::Stream => "live (slow input)",
                Mode::Screenshots => "screenshots",
            })))
    }

    fn reconnect(&mut self, cx: &mut Context<Self>) {
        let Some(serial) = self.mirror.as_ref().map(|m| m.serial.clone()) else { return };
        self.stop_mirror();
        self.start_mirror(serial, Mode::Scrcpy, cx);
    }
}

impl Render for AndroidPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = theme::colors(&self.state.read(cx).theme);
        if self.status.is_none() {
            return div().size_full().flex().items_center().justify_center().text_xs().text_color(c.subtle).child("Looking for devices…");
        }
        let ready = self.ready();
        let current = self.current(cx);
        let mut top = div().flex().h(px(36.0)).flex_shrink_0().items_center().gap_2().px_2().text_xs().child(Icon::new(IconName::Smartphone).size(px(13.0)).text_color(c.subtle));
        if ready.is_empty() {
            top = top.child(div().flex_1().text_color(c.muted).child("No device"));
        } else {
            let mut row = div().flex_1().min_w_0().flex().gap_1().overflow_hidden();
            for (i, d) in ready.iter().enumerate() {
                let serial = d["serial"].as_str().unwrap_or_default().to_string();
                let on = current.as_ref().is_some_and(|x| x["serial"] == d["serial"]);
                let label = format!("{}{} · {serial}", d["model"].as_str().unwrap_or_default(), if d["emulator"] == true { " (emulator)" } else { "" });
                row = row.child(div().id(("device", i)).truncate().rounded_md().border_1().px_1p5().py_0p5().cursor_pointer().when(on, |d| d.border_color(c.accent).text_color(c.fg)).when(!on, |d| d.border_color(c.edge).text_color(c.muted))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let serial = serial.clone();
                        this.state.update(cx, |s, cx| {
                            s.rest.insert("androidSerial".into(), Value::String(serial));
                            cx.notify();
                        });
                        this.sync_device(cx);
                    }))
                    .child(label));
            }
            top = top.child(row);
        }
        let body = match current {
            Some(d) => self.render_mirror(&d, c, window, cx),
            None => self.render_empty(c, cx),
        };
        div().size_full().flex().flex_col().child(top).child(body)
    }
}

#[cfg(test)]
mod tests {
    use super::{keycode, start_codes, to_device};

    #[test]
    fn start_codes_include_the_leading_zero() {
        assert_eq!(start_codes(&[0, 0, 0, 1, 9, 0, 0, 1, 8]), vec![0, 5]);
        assert!(start_codes(&[1, 2, 3]).is_empty());
    }

    #[test]
    fn points_map_through_the_fitted_picture() {
        // A 100×200 picture letterboxed in a 200×200 area: 50px bars left and right.
        assert_eq!(to_device((100.0, 100.0), (0.0, 0.0, 200.0, 200.0), (100.0, 200.0), (1080.0, 2400.0)), (540, 1200));
        assert_eq!(to_device((0.0, 0.0), (0.0, 0.0, 200.0, 200.0), (100.0, 200.0), (1080.0, 2400.0)), (0, 0));
    }

    #[test]
    fn escape_is_back() {
        assert_eq!(keycode("escape"), Some(4));
        assert_eq!(keycode("f1"), None);
    }
}
