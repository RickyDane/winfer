//! Android devices and emulators over adb, mirrored in the side panel (port of
//! `features/android`): live H.264 from the scrcpy server (with live touch input, see scrcpy.rs),
//! else from `adb screenrecord` with `adb shell input`, decoded with openh264; else screenshots.
//!
//! Also the phones, emulators and iOS Simulators of paired machines (Winfer Connect), and this
//! Mac's simulators: those go through `devices.rs` on the machine they're on (`Hosted`).

use crate::backend;
use crate::model::AppState;
use crate::theme::{self, Colors};
use base64::Engine as _;
use gpui_kit::assets::IconName;
use gpui_kit::component::Icon;
use gpui_kit::component::input::{Escape as InputEscape, Input, InputEvent, InputState, MoveDown, MoveUp};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use openh264::decoder::Decoder;
use openh264::formats::YUVSource;
use parking_lot::Mutex;
use serde_json::{Value, json};
use smallvec::smallvec;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};
use tauri::Manager;
use tauri::ipc::{Channel, InvokeResponseBody};
use winfer_backend::android;
use winfer_core::connect_devices;
use winfer_core::devices::Msg;
use winfer_core::scrcpy::{self, Touch};

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

/// What a simulator gets for an Android keycode a phone sends: a button or a key; None for ones
/// it has no equivalent of (back, recent apps, volume).
fn simulator_key(code: u32) -> Option<Value> {
    match code {
        KEY_HOME => Some(json!({"kind": "button", "button": "home"})),
        KEY_POWER => Some(json!({"kind": "button", "button": "power"})),
        _ => ["enter", "backspace", "tab", "up", "down", "left", "right"].into_iter().find(|k| keycode(k) == Some(code)).map(|k| json!({"kind": "key", "key": k})),
    }
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

/// A device in the panel's list.
#[derive(Clone, Debug, PartialEq)]
enum Pick {
    /// This machine's adb device, mirrored here (`Mirror`).
    Local(String),
    /// A device mirrored through `devices.rs`: a paired machine's (`machine`), or a simulator here.
    Hosted { machine: Option<String>, platform: &'static str, id: String, name: String },
}

impl Pick {
    fn id(&self) -> &str {
        match self {
            Pick::Local(serial) => serial,
            Pick::Hosted { id, .. } => id,
        }
    }
}

/// A device the selected machine offers: its name, and what and where it is.
#[derive(Clone)]
struct Device {
    pick: Pick,
    name: String,
    detail: String,
}

/// The panel's two pickers.
#[derive(Clone, Copy, PartialEq)]
enum Combo {
    Machine,
    Device,
}

/// What a picker's row selects.
#[derive(Clone)]
enum Target {
    Machine(Option<String>),
    Device(Pick),
}

/// A picker's row; `detail` is its second line (empty: none), `on` the one selected now.
struct Choice {
    target: Target,
    icon: Icon,
    name: String,
    detail: String,
    on: bool,
}

/// The picker that is open.
struct OpenCombo {
    which: Combo,
    search: Entity<InputState>,
    /// The row Enter picks, moved by the arrow keys and the pointer.
    cursor: usize,
    scroll: ScrollHandle,
    /// Had the keyboard before, and gets it back.
    before: Option<FocusHandle>,
    _events: Subscription,
}

/// Whether every word of `query` is somewhere in `text`, in any order and case.
fn matches_query(text: &str, query: &str) -> bool {
    let text = text.to_lowercase();
    query.to_lowercase().split_whitespace().all(|word| text.contains(word))
}

/// A stream from `devices.rs` (another machine's, or this Mac's simulator).
struct Hosted {
    machine: Option<String>,
    platform: &'static str,
    id: String,
    stream: String,
    /// The attach answer: `{codec, live, input, inputTool}`.
    info: Option<Value>,
    error: Option<String>,
    frames: usize,
    typed: String,
    _pump: Task<()>,
}

impl Hosted {
    fn live(&self) -> bool {
        self.info.as_ref().is_some_and(|i| i["live"] == true)
    }

    fn input(&self) -> bool {
        self.info.as_ref().is_some_and(|i| i["input"] == true)
    }
}

impl Drop for Hosted {
    fn drop(&mut self) {
        connect_devices::drop_frames(&self.stream);
        connect_devices::fire(self.machine.as_deref(), "detach", json!({"streamId": self.stream}));
    }
}

/// Decodes a hosted stream's frames (H.264, or JPEG/PNG screenshots) into `latest`, acknowledging
/// each so the next comes. When multiple H.264 frames are already queued in a burst, intermediate
/// frames still update the decoder's reference state while skipping BGRA conversion & upload.
fn hosted_decode(rx: mpsc::Receiver<Msg>, latest: Arc<Mutex<Option<Arc<RenderImage>>>>, frames: async_channel::Sender<Result<(), String>>, ack: impl Fn()) {
    let Some(mut h264) = frame_decoder(latest.clone(), frames.clone()) else { return };
    let mut pending: Option<Msg> = None;
    loop {
        let Some(msg) = pending.take().or_else(|| rx.recv().ok()) else { return };
        match msg {
            Msg::End(e) => {
                let _ = frames.try_send(Err(e));
                return;
            }
            Msg::Frame { codec: "h264", data, .. } => {
                let next = rx.try_recv().ok();
                let more_h264 = matches!(&next, Some(Msg::Frame { codec: "h264", .. }));
                pending = next;
                let ok = h264(&data, !more_h264);
                ack();
                if !ok {
                    return;
                }
            }
            Msg::Frame { data, .. } => {
                super::capture::offer_encoded(super::capture::Source::Android, &data);
                let image = ::image::load_from_memory(&data).ok().map(|i| i.into_rgba8()).and_then(|rgba| {
                    let (w, h) = rgba.dimensions();
                    render_image(rgba.into_raw(), w, h)
                });
                if let Some(image) = image {
                    *latest.lock() = Some(image);
                    let _ = frames.try_send(Ok(()));
                }
                ack();
            }
        }
    }
}

/// The paired machines that are online: (peer id, name).
fn machines() -> Vec<(String, String)> {
    crate::connect::get().map(|c| c.status().peers.into_iter().filter(|p| p.link.via.is_some()).map(|p| (p.peer_id, p.name)).collect()).unwrap_or_default()
}

/// A machine's icon: a server's, else by what computer it said it is, else (an older winfer, or
/// offline) by what its name says.
fn machine_icon(form: Option<&str>, name: &str, server: bool) -> Icon {
    if server {
        return Icon::new(IconName::Server);
    }
    match form.or_else(|| winfer_core::machine::mac_form(name)) {
        Some("laptop") => Icon::new(IconName::Laptop),
        Some("mac-studio") => Icon::empty().path("devices/mac-studio.svg"),
        Some("mac-mini") => Icon::empty().path("devices/mac-mini.svg"),
        Some("mac-pro") => Icon::new(IconName::PcCase),
        _ => Icon::new(IconName::Monitor),
    }
}

pub struct AndroidPanel {
    state: Entity<AppState>,
    visible: bool,
    status: Option<Value>,
    starting: Option<String>,
    error: Option<String>,
    mirror: Option<Mirror>,
    /// Whose devices show: None for this machine's, else a paired machine's (peer id).
    machine: Option<String>,
    /// `devices.rs` lists by machine (this machine's: only its simulators).
    lists: HashMap<Option<String>, Result<Value, String>>,
    hosted: Option<Hosted>,
    /// Machines whose device list is being read.
    listing: HashSet<Option<String>>,
    /// What computer each machine is (None: this one), for its icon; no value while asking or
    /// when it can't tell.
    forms: HashMap<Option<String>, Option<String>>,
    restart: u64,
    latest: Arc<Mutex<Option<Arc<RenderImage>>>>,
    shown: Option<Arc<RenderImage>>,
    area: Bounds<Pixels>,
    press: Option<(u32, u32, Instant)>,
    /// A finger is down on the scrcpy session.
    touching: bool,
    /// Last live `Touch::Move` sent (throttles 120Hz trackpads).
    last_move: Option<Instant>,
    /// Accumulated hosted scroll delta waiting for the next 60Hz flush.
    scroll_pending: Option<((f32, f32), f32, f32)>,
    last_scroll: Option<Instant>,
    /// A phone shows the mirror (mobile_panel.rs): it runs while the panel or a phone shows it.
    phone: bool,
    /// Where the phone's finger went down, for a tap or swipe without scrcpy.
    phone_press: Option<(f32, f32, Instant)>,
    focus: FocusHandle,
    combo: Option<OpenCombo>,
    /// A click outside just closed this picker: that click on its button must not reopen it.
    dismissed: Option<(Combo, Instant)>,
    _poll: Task<()>,
}

fn render_bgra(bgra: Vec<u8>, w: u32, h: u32) -> Option<Arc<RenderImage>> {
    let buffer = ::image::RgbaImage::from_raw(w, h, bgra)?;
    Some(Arc::new(RenderImage::new(smallvec![::image::Frame::new(buffer)])))
}

fn render_image(mut rgba: Vec<u8>, w: u32, h: u32) -> Option<Arc<RenderImage>> {
    for p in rgba.chunks_exact_mut(4) {
        p.swap(0, 2);
    }
    render_bgra(rgba, w, h)
}

/// Converts YUV420 directly to BGRA8 in one pass (avoiding zero-fill + a second R/B swap pass).
fn yuv420_to_bgra(yuv: &impl YUVSource) -> Vec<u8> {
    let (w, h) = yuv.dimensions();
    let (y_stride, u_stride, v_stride) = yuv.strides();
    let (y_plane, u_plane, v_plane) = (yuv.y(), yuv.u(), yuv.v());
    let mut out = Vec::with_capacity(w * h * 4);
    for row in 0..h {
        let y_row = &y_plane[row * y_stride..row * y_stride + w];
        let u_row = &u_plane[(row >> 1) * u_stride..];
        let v_row = &v_plane[(row >> 1) * v_stride..];
        let mut x = 0;
        while x + 1 < w {
            let cx = x >> 1;
            let d = u_row[cx] as i32 - 128;
            let e = v_row[cx] as i32 - 128;
            let r_off = 409 * e + 128;
            let g_off = -100 * d - 208 * e + 128;
            let b_off = 516 * d + 128;

            let c0 = 298 * (y_row[x] as i32 - 16);
            let b0 = ((c0 + b_off) >> 8).clamp(0, 255) as u8;
            let g0 = ((c0 + g_off) >> 8).clamp(0, 255) as u8;
            let r0 = ((c0 + r_off) >> 8).clamp(0, 255) as u8;

            let c1 = 298 * (y_row[x + 1] as i32 - 16);
            let b1 = ((c1 + b_off) >> 8).clamp(0, 255) as u8;
            let g1 = ((c1 + g_off) >> 8).clamp(0, 255) as u8;
            let r1 = ((c1 + r_off) >> 8).clamp(0, 255) as u8;

            out.extend_from_slice(&[b0, g0, r0, 255, b1, g1, r1, 255]);
            x += 2;
        }
        if x < w {
            let cx = x >> 1;
            let c = 298 * (y_row[x] as i32 - 16);
            let d = u_row[cx] as i32 - 128;
            let e = v_row[cx] as i32 - 128;
            let b = ((c + 516 * d + 128) >> 8).clamp(0, 255) as u8;
            let g = ((c - 100 * d - 208 * e + 128) >> 8).clamp(0, 255) as u8;
            let r = ((c + 409 * e + 128) >> 8).clamp(0, 255) as u8;
            out.extend_from_slice(&[b, g, r, 255]);
        }
    }
    out
}

/// Decodes NAL units or whole frames into `latest`; false once the stream can't be decoded.
/// When `render` is false, the H.264 decoder state advances without spending CPU on YUV → BGRA.
fn frame_decoder(latest: Arc<Mutex<Option<Arc<RenderImage>>>>, frames: async_channel::Sender<Result<(), String>>) -> Option<impl FnMut(&[u8], bool) -> bool> {
    let mut decoder = match Decoder::new() {
        Ok(d) => d,
        Err(e) => {
            let _ = frames.try_send(Err(e.to_string()));
            return None;
        }
    };
    let mut errors = 0;
    Some(move |data: &[u8], render: bool| -> bool {
        match decoder.decode(data) {
            Ok(Some(yuv)) => {
                errors = 0;
                if render {
                    let (w, h) = yuv.dimensions();
                    let bgra = yuv420_to_bgra(&yuv);
                    if let Some(image) = render_bgra(bgra, w as u32, h as u32) {
                        super::capture::offer_image(super::capture::Source::Android, &image);
                        *latest.lock() = Some(image);
                        let _ = frames.try_send(Ok(()));
                    }
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

/// Whole frames from the scrcpy server, decoded as they come; queued bursts only render the newest.
fn decode_frames(rx: mpsc::Receiver<scrcpy::Frame>, latest: Arc<Mutex<Option<Arc<RenderImage>>>>, frames: async_channel::Sender<Result<(), String>>) {
    let Some(mut feed) = frame_decoder(latest, frames.clone()) else { return };
    let mut pending: Option<scrcpy::Frame> = None;
    loop {
        let Some(frame) = pending.take().or_else(|| rx.recv().ok()) else { break };
        let next = rx.try_recv().ok();
        let has_more = next.is_some();
        pending = next;
        if !feed(&frame.data, !has_more) {
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
                let last_idx = starts.len() - 2;
                for (i, pair) in starts.windows(2).enumerate() {
                    if !feed(&pending[pair[0]..pair[1]], i == last_idx) {
                        return;
                    }
                }
                pending.drain(..*starts.last().unwrap());
            }
            // The encoder only sends frames when the screen changes: the newest NAL may be the last for a while.
            Err(RecvTimeoutError::Timeout) => {
                if pending.len() > 4 && !feed(&pending, true) {
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
                let Ok((visible, machine, phone)) = this.update(cx, |this, _| (this.live(), this.machine.clone(), this.phone)) else { break };
                if visible && this.update(cx, |this, cx| { this.ask_forms(cx); this.refresh(machine.clone(), cx) }).is_err() {
                    break;
                }
                // A phone shows this machine's devices, even while the panel shows a paired one's.
                if phone && machine.is_some() && this.update(cx, |this, cx| this.refresh(None, cx)).is_err() {
                    break;
                }
                cx.background_executor().timer(Duration::from_secs(3)).await;
            }
        });
        let machine = state.read(cx).rest.get("deviceMachine").and_then(Value::as_str).map(str::to_string);
        Self { state, visible: false, status: None, starting: None, error: None, mirror: None, machine, lists: HashMap::new(), hosted: None, listing: HashSet::new(), forms: HashMap::new(), restart: 0, latest: Default::default(), shown: None, area: Bounds::default(), press: None, touching: false, last_move: None, scroll_pending: None, last_scroll: None, phone: false, phone_press: None, focus: cx.focus_handle(), combo: None, dismissed: None, _poll: poll }
    }

    /// Reads the device list of `machine` (None: this machine's adb devices and simulators).
    fn refresh(&mut self, machine: Option<String>, cx: &mut Context<Self>) {
        if !self.listing.insert(machine.clone()) {
            return;
        }
        cx.spawn(async move |this, cx| {
            match machine {
                None => {
                    let status = tauri::async_runtime::spawn(android::android_status()).await.map_err(|e| e.to_string()).and_then(|r| r).and_then(|s| serde_json::to_value(s).map_err(|e| e.to_string()));
                    let ios = if cfg!(target_os = "macos") { Some(backend::background(winfer_core::ios_sim::status).await.unwrap_or_default()) } else { None };
                    let _ = this.update(cx, |this, cx| {
                        this.listing.remove(&None);
                        if let Some(ios) = ios {
                            this.lists.insert(None, Ok(json!({"ios": ios})));
                        }
                        this.on_status(status, cx);
                    });
                }
                Some(peer) => {
                    let up = crate::connect::get().is_some_and(|c| c.is_up(&peer));
                    let list = if up { connect_devices::call(Some(&peer), "list", json!({})).await } else { Err(format!("{} is offline.", crate::connect::peer_name(&peer).unwrap_or_else(|| "That machine".into()))) };
                    let _ = this.update(cx, |this, cx| {
                        this.listing.remove(&Some(peer.clone()));
                        this.lists.insert(Some(peer), list);
                        this.sync_device(cx);
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    /// Asks this machine and each online one what computer it is.
    fn ask_forms(&mut self, cx: &mut Context<Self>) {
        let online: Vec<Option<String>> = std::iter::once(None).chain(machines().into_iter().map(|(id, _)| Some(id))).collect();
        // One that couldn't tell is asked again once it reconnects (it may have updated).
        self.forms.retain(|machine, form| form.is_some() || online.contains(machine));
        for machine in online {
            if self.forms.contains_key(&machine) {
                continue;
            }
            self.forms.insert(machine.clone(), None);
            cx.spawn(async move |this, cx| {
                let answer = connect_devices::call(machine.as_deref(), "host", json!({})).await;
                let form = answer.ok().and_then(|v| v["form"].as_str().map(str::to_string));
                let _ = this.update(cx, |this, cx| {
                    this.forms.insert(machine, form);
                    cx.notify();
                });
            })
            .detach();
        }
    }

    /// Shows `machine`'s devices (None: this machine's).
    fn pick_machine(&mut self, machine: Option<String>, cx: &mut Context<Self>) {
        if self.machine == machine {
            return;
        }
        self.machine = machine.clone();
        self.starting = None;
        self.state.update(cx, |s, cx| {
            s.rest.insert("deviceMachine".into(), machine.clone().map_or(Value::Null, Value::String));
            cx.notify();
        });
        self.sync_device(cx);
        self.refresh(machine, cx);
        cx.notify();
    }

    fn pick(&mut self, pick: &Pick, cx: &mut Context<Self>) {
        let (machine, id) = (self.machine.clone(), pick.id().to_string());
        self.state.update(cx, |s, cx| {
            if let Pick::Local(serial) = pick {
                s.rest.insert("androidSerial".into(), Value::String(serial.clone()));
            }
            s.rest.insert("devicePick".into(), json!({"machine": machine, "id": id}));
            cx.notify();
        });
        self.sync_device(cx);
    }

    /// The selected machine's devices that can be mirrored.
    fn picks(&self) -> Vec<Device> {
        let mut out = Vec::new();
        let list = self.lists.get(&self.machine).and_then(|l| l.as_ref().ok());
        let android: Vec<Value> = match &self.machine {
            None => self.ready(),
            Some(_) => list.iter().flat_map(|l| l["android"]["devices"].as_array().into_iter().flatten()).filter(|d| d["state"] == "device").cloned().collect(),
        };
        for d in android {
            let serial = d["serial"].as_str().unwrap_or_default().to_string();
            let model = d["model"].as_str().unwrap_or_default().to_string();
            let detail = if d["emulator"] == true { format!("Emulator · {serial}") } else { serial.clone() };
            let name = if model.is_empty() { serial.clone() } else { model.clone() };
            let pick = match &self.machine {
                None => Pick::Local(serial),
                Some(m) => Pick::Hosted { machine: Some(m.clone()), platform: "android", id: serial, name: model },
            };
            out.push(Device { pick, name, detail });
        }
        for d in list.iter().flat_map(|l| l["ios"]["devices"].as_array().into_iter().flatten()).filter(|d| d["booted"] == true) {
            let name = d["name"].as_str().unwrap_or_default().to_string();
            let detail = format!("Simulator · {}", d["runtime"].as_str().unwrap_or("iOS"));
            let pick = Pick::Hosted { machine: self.machine.clone(), platform: "ios", id: d["udid"].as_str().unwrap_or_default().to_string(), name: name.clone() };
            out.push(Device { pick, name, detail });
        }
        out
    }

    /// Shown or hidden with the side panel's Android tab: the mirror only runs while on screen.
    pub fn set_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        let was = self.live();
        self.visible = visible;
        if visible {
            self.ask_forms(cx);
        } else {
            self.combo = None;
        }
        self.live_changed(was, cx);
    }

    /// Shown on the panel or on a phone.
    fn live(&self) -> bool {
        self.visible || self.phone
    }

    fn live_changed(&mut self, was: bool, cx: &mut Context<Self>) {
        if self.live() == was {
            return;
        }
        if self.live() {
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

    /// The device shown: the one picked last on this machine, else the first.
    fn current(&self, cx: &App) -> Option<Device> {
        let picks = self.picks();
        let rest = &self.state.read(cx).rest;
        let saved = rest.get("devicePick").filter(|p| p["machine"].as_str() == self.machine.as_deref()).and_then(|p| p["id"].as_str());
        let saved = saved.or(if self.machine.is_none() { rest.get("androidSerial").and_then(Value::as_str) } else { None });
        picks.iter().find(|d| Some(d.pick.id()) == saved).or(picks.first()).cloned()
    }

    fn sync_device(&mut self, cx: &mut Context<Self>) {
        if !self.live() {
            return;
        }
        match self.current(cx).map(|d| d.pick) {
            Some(Pick::Local(serial)) => {
                if self.hosted.is_none() && self.mirror.as_ref().map(|m| &m.serial) == Some(&serial) {
                    return;
                }
                self.stop_mirror();
                self.start_mirror(serial, Mode::Scrcpy, cx);
            }
            Some(Pick::Hosted { machine, platform, id, name }) => {
                if self.hosted.as_ref().is_some_and(|h| h.machine == machine && h.id == id) {
                    return;
                }
                self.stop_mirror();
                self.start_hosted(machine, platform, id, name, cx);
            }
            None => self.stop_mirror(),
        }
    }

    fn start_hosted(&mut self, machine: Option<String>, platform: &'static str, id: String, name: String, cx: &mut Context<Self>) {
        let stream = connect_devices::new_stream_id();
        let (frames, frame_rx) = async_channel::unbounded();
        let (tx, rx) = mpsc::channel::<Msg>();
        connect_devices::on_frames(&stream, machine.as_deref(), move |m| {
            let _ = tx.send(m);
        });
        let (latest, ack_machine, ack_stream) = (self.latest.clone(), machine.clone(), stream.clone());
        std::thread::spawn(move || hosted_decode(rx, latest, frames, move || connect_devices::fire(ack_machine.as_deref(), "frameDone", json!({"streamId": ack_stream}))));
        let (m, sid, device) = (machine.clone(), stream.clone(), id.clone());
        let pump = cx.spawn(async move |this, cx| {
            let info = connect_devices::call(m.as_deref(), "attach", json!({"streamId": sid, "platform": platform, "device": device, "name": name})).await;
            let ok = this.update(cx, |this, cx| {
                let Some(h) = this.hosted.as_mut().filter(|h| h.stream == sid) else { return false };
                match &info {
                    Ok(v) => h.info = Some(v.clone()),
                    Err(e) => h.error = Some(e.clone()),
                }
                cx.notify();
                info.is_ok()
            });
            if !matches!(ok, Ok(true)) {
                return;
            }
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
                    let Some(h) = this.hosted.as_mut().filter(|h| h.stream == sid) else { return true };
                    h.frames += count;
                    if let Some(e) = failed.take() {
                        h.error = Some(e);
                    }
                    cx.notify();
                    h.error.is_some()
                });
                if !matches!(done, Ok(false)) {
                    break;
                }
            }
        });
        self.hosted = Some(Hosted { machine, platform, id, stream, info: None, error: None, frames: 0, typed: String::new(), _pump: pump });
        cx.notify();
    }

    fn stop_mirror(&mut self) {
        self.hosted = None;
        if let Some(m) = self.mirror.take()
            && m.mode == Mode::Stream
        {
            let serial = m.serial;
            backend::background(move || android::android_mirror_stop(backend::app().state(), serial));
        }
        *self.latest.lock() = None;
        self.touching = false;
        self.last_move = None;
        self.scroll_pending = None;
        self.last_scroll = None;
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
            let (tx, rx) = mpsc::channel::<scrcpy::Frame>();
            let start = serial.clone();
            let session = backend::background(move || scrcpy::Session::start(&start, scrcpy::Options::default(), tx)).await.unwrap_or_else(|e| Err(e.to_string()));
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

    /// A point on the shown picture as a fraction of it (hosted streams, whose size can change).
    fn picture_point(&self, at: Point<Pixels>) -> Option<(f32, f32)> {
        let video = self.shown.as_ref()?.size(0);
        let b = self.area;
        Some(to_unit((at.x / px(1.0), at.y / px(1.0)), (b.origin.x / px(1.0), b.origin.y / px(1.0), b.size.width / px(1.0), b.size.height / px(1.0)), (video.width.0 as f32, video.height.0 as f32)))
    }

    /// Input for the hosted stream (`kind` and its fields), when it takes input.
    fn hosted_input(&self, input: Value) {
        let Some(h) = self.hosted.as_ref().filter(|h| h.input()) else { return };
        let mut input = input;
        input["streamId"] = json!(h.stream);
        connect_devices::fire(h.machine.as_deref(), "input", input);
    }

    fn touch(&mut self, touch: Touch, at: Point<Pixels>) -> bool {
        if let Some(h) = self.hosted.as_ref() {
            // Screenshot mode only needs where the finger went down and lifted.
            if touch == Touch::Move && !h.live() {
                return true;
            }
            if touch == Touch::Move && self.last_move.is_some_and(|t| t.elapsed() < Duration::from_millis(12)) {
                return true;
            }
            let Some((x, y)) = self.picture_point(at) else { return false };
            let action = match touch {
                Touch::Down => "down",
                Touch::Move => "move",
                Touch::Up => "up",
            };
            self.hosted_input(json!({"kind": "touch", "action": action, "x": x, "y": y}));
            self.touching = touch != Touch::Up;
            self.last_move = if touch == Touch::Up { None } else { Some(Instant::now()) };
            return true;
        }
        if touch == Touch::Move && self.last_move.is_some_and(|t| t.elapsed() < Duration::from_millis(8)) {
            return true;
        }
        let (Some(session), Some(p)) = (self.session(), self.unit_point(at)) else { return false };
        session.touch(touch, p);
        self.touching = touch != Touch::Up;
        self.last_move = if touch == Touch::Up { None } else { Some(Instant::now()) };
        true
    }

    fn flush_hosted_scroll(&mut self) {
        if let Some(((x, y), h, v)) = self.scroll_pending.take() {
            self.last_scroll = Some(Instant::now());
            self.hosted_input(json!({"kind": "scroll", "x": x, "y": y, "h": h, "v": v}));
        }
    }

    fn send(&self, input: Value) {
        let Some(serial) = self.mirror.as_ref().map(|m| m.serial.clone()) else { return };
        tauri::async_runtime::spawn(async move {
            if let Ok(input) = serde_json::from_value(input) {
                let _ = android::android_input(serial, input).await;
            }
        });
    }

    /// A hardware button: back, home, recents or power.
    fn button(&self, name: &str, code: u32) {
        if self.hosted.is_some() {
            return self.hosted_input(json!({"kind": "button", "button": name}));
        }
        self.key(code);
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
        if let Some(text) = self.hosted.as_mut().map(|h| std::mem::take(&mut h.typed)).filter(|t| !t.is_empty()) {
            return self.hosted_input(json!({"kind": "text", "text": text}));
        }
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
            if let Some(h) = self.hosted.as_mut() {
                if h.live() {
                    self.hosted_input(json!({"kind": "text", "text": text}));
                    return;
                }
                h.typed.push_str(&text);
            } else if let Some(s) = self.session() {
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
            if self.hosted.is_some() {
                self.hosted_input(json!({"kind": "key", "key": k.key}));
            } else {
                self.key(code);
            }
        }
    }

    // ---- the phone (mobile_panel.rs) ----

    pub fn phone_watching(&self) -> bool {
        self.phone
    }

    /// A phone starts or stops showing the mirror.
    pub fn set_phone(&mut self, on: bool, cx: &mut Context<Self>) {
        let was = self.live();
        self.phone = on;
        self.live_changed(was, cx);
        // Its devices are this machine's: list them now, whatever the panel shows.
        if on && (self.machine.is_some() || self.status.is_none()) {
            self.refresh(None, cx);
        }
        cx.notify();
    }

    /// This Mac's simulator, when that's what's mirrored (a phone never reaches a paired machine's).
    fn phone_simulator(&self) -> Option<&Hosted> {
        self.hosted.as_ref().filter(|h| h.machine.is_none())
    }

    /// What a phone shows: adb, the devices, emulators and simulators to start, and the device
    /// mirrored and how.
    pub fn phone_state(&self, cx: &App) -> Value {
        let status = self.status.clone().unwrap_or(Value::Null);
        let ios = self.lists.get(&None).and_then(|l| l.as_ref().ok()).map(|l| l["ios"].clone());
        let mirror = match self.phone_simulator() {
            // `input` is false once the stream says it takes none (no native input, AXe or idb).
            Some(h) => Some(json!({ "serial": h.id, "mode": if h.live() { "live" } else { "screenshots" }, "error": h.error, "showing": h.frames > 0, "live": h.live(), "input": h.info.is_none() || h.input() })),
            None => self.mirror.as_ref().map(|m| {
                let mode = match m.mode {
                    Mode::Scrcpy => "scrcpy",
                    Mode::Stream => "stream",
                    Mode::Screenshots => "screenshots",
                };
                json!({ "serial": m.serial, "mode": mode, "error": m.error, "showing": m.frames > 0, "live": m.session.is_some() })
            }),
        };
        json!({
            "loaded": self.status.is_some() || ios.is_some(),
            "adb": status["adb"],
            "devices": status["devices"],
            "avds": status["avds"],
            "ios": ios,
            "current": self.current(cx).and_then(|d| match d.pick {
                Pick::Local(serial) => Some(serial),
                Pick::Hosted { machine: None, id, .. } => Some(id),
                Pick::Hosted { .. } => None,
            }),
            "starting": self.starting,
            "error": self.error,
            "mirror": mirror,
        })
    }

    /// The phone works with this machine's devices (`id`: an adb serial or a simulator's udid), so
    /// it switches the panel back to them.
    pub fn phone_select(&mut self, id: String, cx: &mut Context<Self>) {
        self.pick_machine(None, cx);
        // Picking a simulator whose stream stopped connects it again.
        if self.phone_simulator().is_some_and(|h| h.id == id && h.error.is_some()) {
            self.stop_mirror();
        }
        let pick = self.picks().into_iter().map(|d| d.pick).find(|p| p.id() == id).unwrap_or(Pick::Local(id));
        self.pick(&pick, cx);
    }

    pub fn phone_start_avd(&mut self, name: String, cx: &mut Context<Self>) {
        self.pick_machine(None, cx);
        self.boot("android", name, cx);
    }

    pub fn phone_start_simulator(&mut self, udid: String, cx: &mut Context<Self>) {
        self.pick_machine(None, cx);
        self.boot("ios", udid, cx);
    }

    /// A finger on the phone's picture, as a fraction of the screen (0..1 both ways).
    pub fn phone_touch(&mut self, touch: Touch, at: (f32, f32)) {
        let at = (at.0.clamp(0.0, 1.0), at.1.clamp(0.0, 1.0));
        if self.phone_simulator().is_some() {
            let action = match touch {
                Touch::Down => "down",
                Touch::Move => "move",
                Touch::Up => "up",
            };
            return self.hosted_input(json!({ "kind": "touch", "action": action, "x": at.0, "y": at.1 }));
        }
        if let Some(s) = self.session() {
            s.touch(touch, at);
            return;
        }
        // Without scrcpy, adb input: a tap, or a swipe when the finger moved.
        match touch {
            Touch::Down => self.phone_press = Some((at.0, at.1, Instant::now())),
            Touch::Move => {}
            Touch::Up => {
                let Some((x0, y0, t0)) = self.phone_press.take() else { return };
                let Some((w, h, _, _)) = self.mirror.as_ref().and_then(|m| m.info) else { return };
                let p = |x: f32, y: f32| ((x * w).round() as u32, (y * h).round() as u32);
                let (a, b) = (p(x0, y0), p(at.0, at.1));
                if (at.0 - x0).abs() < 0.02 && (at.1 - y0).abs() < 0.02 {
                    self.send(json!({ "kind": "tap", "x": a.0, "y": a.1 }));
                } else {
                    self.send(json!({ "kind": "swipe", "x1": a.0, "y1": a.1, "x2": b.0, "y2": b.1, "ms": t0.elapsed().as_millis().clamp(80, 2000) as u32 }));
                }
            }
        }
    }

    /// Wheel notches at a point (scrcpy and simulators; adb has swipes for that).
    pub fn phone_scroll(&self, at: (f32, f32), v: f32) {
        if self.phone_simulator().is_some() {
            return self.hosted_input(json!({ "kind": "scroll", "x": at.0, "y": at.1, "h": 0.0, "v": v }));
        }
        if let Some(s) = self.session() {
            s.scroll(at, 0.0, v);
        }
    }

    pub fn phone_key(&self, code: u32) {
        if self.phone_simulator().is_some() {
            if let Some(input) = simulator_key(code) {
                self.hosted_input(input);
            }
            return;
        }
        self.key(code);
    }

    pub fn phone_text(&self, text: &str) {
        if self.phone_simulator().is_some() {
            return self.hosted_input(json!({ "kind": "text", "text": text }));
        }
        match self.session() {
            Some(s) => s.text(text),
            None => self.send(json!({ "kind": "text", "text": text })),
        }
    }

    /// Starts an emulator (`platform` "android", by AVD name) or boots a simulator ("ios", by
    /// udid) on the machine shown.
    fn boot(&mut self, platform: &'static str, name: String, cx: &mut Context<Self>) {
        self.starting = Some(name.clone());
        self.error = None;
        if platform == "android" && self.machine.is_none() {
            if let Err(e) = android::android_start_avd(name) {
                self.error = Some(e);
            }
        } else {
            let machine = self.machine.clone();
            cx.spawn(async move |this, cx| {
                let booted = connect_devices::call(machine.as_deref(), "boot", json!({"platform": platform, "device": name})).await;
                let _ = this.update(cx, |this, cx| {
                    if let Err(e) = booted {
                        this.error = Some(e);
                        this.starting = None;
                    }
                    this.refresh(machine, cx);
                    cx.notify();
                });
            })
            .detach();
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
        let list = self.lists.get(&self.machine);
        let android = match &self.machine {
            None => self.status.clone().unwrap_or_default(),
            Some(m) => match list {
                None => return body.child(format!("Asking {} for its devices…", crate::connect::peer_name(m).unwrap_or_else(|| "the machine".into()))),
                Some(Err(e)) => return body.child(div().text_color(c.danger).child(e.clone())),
                Some(Ok(l)) => l["android"].clone(),
            },
        };
        let ios = list.and_then(|l| l.as_ref().ok()).map(|l| l["ios"].clone()).unwrap_or_default();
        let here = self.machine.is_none();
        if android["adb"] != true && ios["simctl"] != true {
            return body.child(if here { "adb isn't installed. Install Android Studio's platform-tools (macOS: brew install android-platform-tools)." } else { "That machine has neither adb (Android) nor Xcode's simulators." });
        }
        body = body.child(if here { "Connect a phone with USB debugging on, or start an emulator or simulator." } else { "Start an emulator or simulator there, or connect a phone to it." });
        if android["adb"] != true {
            body = body.child(div().text_color(c.subtle).child(if here { "For Android, install Android Studio's platform-tools (macOS: brew install android-platform-tools)." } else { "For Android, install adb there (Android platform-tools)." }));
        }
        for d in android["devices"].as_array().into_iter().flatten().filter(|d| d["state"] != "device") {
            let state = d["state"].as_str().unwrap_or_default();
            body = body.child(div().text_color(c.warning).child(format!("{}: {}", d["serial"].as_str().unwrap_or_default(), if state == "unauthorized" { "allow USB debugging on the phone" } else { state })));
        }
        let free = self.starting.is_none();
        let row = |i: usize, id: &'static str, label: String, key: String, platform: &'static str| {
            let booting = self.starting.as_deref() == Some(key.as_str());
            div().id((id, i)).flex().items_center().gap_2().rounded_md().border_1().border_color(c.edge).px_2p5().py_1p5().text_color(c.fg).when(free, |d| d.cursor_pointer().hover(move |d| d.bg(c.hover))).when(!free, |d| d.opacity(0.5))
                .on_click(cx.listener(move |this, _, _, cx| if this.starting.is_none() { this.boot(platform, key.clone(), cx) }))
                .child(Icon::new(IconName::Smartphone).size(px(12.0)))
                .child(div().flex_1().truncate().text_left().child(label))
                .child(div().text_size(px(10.0)).text_color(c.subtle).child(if booting { "booting…" } else { "Start" }))
        };
        let mut avds = div().w(px(260.0)).flex().flex_col().gap_1();
        for (i, avd) in android["avds"].as_array().into_iter().flatten().filter_map(Value::as_str).enumerate() {
            avds = avds.child(row(i, "avd", avd.replace('_', " "), avd.to_string(), "android"));
        }
        let mut sims = div().w(px(260.0)).flex().flex_col().gap_1();
        let off: Vec<&Value> = ios["devices"].as_array().into_iter().flatten().filter(|d| d["booted"] != true).collect();
        for (i, d) in off.iter().enumerate() {
            let label = format!("{} · {}", d["name"].as_str().unwrap_or_default(), d["runtime"].as_str().unwrap_or("iOS"));
            sims = sims.child(row(i, "sim", label, d["udid"].as_str().unwrap_or_default().to_string(), "ios"));
        }
        body.child(avds)
            .when(!off.is_empty(), |b| b.child(div().text_color(c.subtle).child("iOS Simulators")).child(div().id("sims").max_h(px(240.0)).overflow_y_scroll().child(sims)))
            .when_some(self.error.clone(), |d, e| d.child(div().text_color(c.danger).child(e)))
    }

    fn render_mirror(&mut self, label: String, c: Colors, window: &mut Window, cx: &mut Context<Self>) -> Div {
        if let Some(next) = self.latest.lock().take() {
            if let Some(old) = self.shown.replace(next) {
                cx.drop_image(old, Some(window));
            }
        }
        let model = label;
        let (frames, error, mode) = match &self.hosted {
            Some(h) => {
                let how = if h.info.is_none() { "connecting" } else if h.live() { "live" } else { "screenshots" };
                let label = match &h.machine {
                    Some(m) => format!("{how} · on {}", crate::connect::peer_name(m).unwrap_or_else(|| "another machine".into())),
                    None => how.to_string(),
                };
                (h.frames, h.error.clone(), label)
            }
            None => self.mirror.as_ref().map_or((0, None, "live".to_string()), |m| {
                let how = match m.mode {
                    Mode::Scrcpy => "live",
                    Mode::Stream => "live (slow input)",
                    Mode::Screenshots => "screenshots",
                };
                (m.frames, m.error.clone(), how.to_string())
            }),
        };
        let ios = self.hosted.as_ref().is_some_and(|h| h.platform == "ios");
        // A simulator without AXe or idb can only be watched.
        let view_only = self.hosted.as_ref().is_some_and(|h| h.info.is_some() && !h.input());
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
            .on_scroll(cx.listener(|this, e: &ScrollEvent, _, cx| {
                if this.hosted.is_some() {
                    let d = e.delta.pixel_delta(px(20.0));
                    if let Some((x, y)) = this.picture_point(e.position) {
                        let (dh, dv) = (-(d.x / px(20.0)), d.y / px(20.0));
                        let was_none = this.scroll_pending.is_none();
                        let entry = this.scroll_pending.get_or_insert(((x, y), 0.0, 0.0));
                        entry.0 = (x, y);
                        entry.1 += dh;
                        entry.2 += dv;
                        let big = entry.1.abs() + entry.2.abs() >= 0.35;
                        let due = this.last_scroll.is_none_or(|t| t.elapsed() >= Duration::from_millis(12));
                        if due || big {
                            this.flush_hosted_scroll();
                        } else if was_none {
                            cx.spawn(async move |this, cx| {
                                cx.background_executor().timer(Duration::from_millis(16)).await;
                                let _ = this.update(cx, |this, _| this.flush_hosted_scroll());
                            })
                            .detach();
                        }
                    }
                    return;
                }
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
        div().flex_1().min_h_0().overflow_hidden().flex().flex_col().child(stage)
            .when(view_only, |d| d.child(div().flex_shrink_0().px_3().pt_1p5().text_center().text_size(px(10.0)).text_color(c.warning).child(format!("View only: install AXe on {} to tap and type (brew install cameroncooke/axe/axe).", if self.machine.is_none() { "this Mac" } else { "that Mac" }))))
            .child(div().flex().flex_shrink_0().items_center().justify_center().gap_1().py_1p5()
            .when(!ios, |d| d.child(button("android-back", IconName::ChevronLeft).on_click(cx.listener(|this, _, _, _| this.button("back", KEY_BACK)))))
            .child(button("android-home", IconName::Circle).on_click(cx.listener(|this, _, _, _| this.button("home", KEY_HOME))))
            .when(!ios, |d| d.child(button("android-recents", IconName::Square).on_click(cx.listener(|this, _, _, _| this.button("recents", KEY_RECENTS)))))
            .child(div().mx_1().h_4().w(px(1.0)).bg(c.border))
            .child(button("android-power", IconName::Power).on_click(cx.listener(|this, _, _, _| this.button("power", KEY_POWER))))
            .child(button("android-reconnect", IconName::RefreshCw).on_click(cx.listener(|this, _, _, cx| this.reconnect(cx))))
            .child(div().ml_2().text_size(px(10.0)).text_color(c.subtle).child(mode)))
    }

    /// This machine and the paired ones that are online, plus the selected one if it isn't.
    fn machine_choices(&self) -> Vec<Choice> {
        let online = machines();
        let offline = self.machine.clone().filter(|m| !online.iter().any(|(id, _)| id == m)).map(|m| {
            let name = format!("{} (offline)", crate::connect::peer_name(&m).unwrap_or_else(|| "machine".into()));
            (Some(m), name)
        });
        let servers: HashSet<String> = crate::connect::get().map(|connect| connect.peers().into_iter().filter(|p| p.kind == winfer_connect::pairing::Kind::Server).map(|p| p.peer_id).collect()).unwrap_or_default();
        std::iter::once((None, "This machine".to_string()))
            .chain(online.into_iter().map(|(id, name)| (Some(id), name)))
            .chain(offline)
            .map(|(machine, name)| {
                let form = self.forms.get(&machine).and_then(Option::as_deref);
                // This machine's name isn't its computer's: only what it said picks its icon.
                let icon = machine_icon(form, if machine.is_some() { &name } else { "" }, machine.as_ref().is_some_and(|m| servers.contains(m)));
                Choice { on: self.machine == machine, target: Target::Machine(machine), icon, name, detail: String::new() }
            })
            .collect()
    }

    fn device_choices(&self, cx: &App) -> Vec<Choice> {
        let current = self.current(cx).map(|d| d.pick);
        self.picks().into_iter().map(|d| Choice { on: current.as_ref() == Some(&d.pick), target: Target::Device(d.pick), icon: Icon::new(IconName::Smartphone), name: d.name, detail: d.detail }).collect()
    }

    /// A picker's rows that match its search.
    fn found(&self, which: Combo, query: &str, cx: &App) -> Vec<Choice> {
        let all = match which {
            Combo::Machine => self.machine_choices(),
            Combo::Device => self.device_choices(cx),
        };
        all.into_iter().filter(|o| matches_query(&format!("{} {}", o.name, o.detail), query)).collect()
    }

    fn toggle_combo(&mut self, which: Combo, window: &mut Window, cx: &mut Context<Self>) {
        if self.dismissed.take().is_some_and(|(w, at)| w == which && at.elapsed() < Duration::from_millis(300)) {
            return;
        }
        if self.combo.as_ref().is_some_and(|o| o.which == which) {
            return self.close_combo(window, cx);
        }
        let hint = match which {
            Combo::Machine => "Search machines",
            Combo::Device => "Search devices",
        };
        let search = cx.new(|cx| InputState::new(window, cx).placeholder(hint));
        let events = cx.subscribe_in(&search, window, |this, _, event: &InputEvent, window, cx| match event {
            InputEvent::Change => this.reset_cursor(cx),
            InputEvent::PressEnter { .. } => this.choose_cursor(window, cx),
            _ => {}
        });
        let before = self.combo.take().and_then(|o| o.before).or_else(|| window.focused(cx));
        search.read(cx).focus_handle(cx).focus(window, cx);
        self.combo = Some(OpenCombo { which, search, cursor: 0, scroll: ScrollHandle::new(), before, _events: events });
        self.reset_cursor(cx);
    }

    fn close_combo(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(open) = self.combo.take() else { return };
        if let Some(before) = open.before {
            before.focus(window, cx);
        }
        cx.notify();
    }

    /// Puts the cursor on the selected row, or on the first match once something is searched.
    fn reset_cursor(&mut self, cx: &mut Context<Self>) {
        let Some((which, query)) = self.combo.as_ref().map(|o| (o.which, o.search.read(cx).value().to_string())) else { return };
        let at = if query.trim().is_empty() { self.found(which, "", cx).iter().position(|o| o.on).unwrap_or(0) } else { 0 };
        if let Some(open) = self.combo.as_mut() {
            open.cursor = at;
            open.scroll.scroll_to_item(at);
        }
        cx.notify();
    }

    fn move_cursor(&mut self, down: bool, cx: &mut Context<Self>) {
        let Some((which, query)) = self.combo.as_ref().map(|o| (o.which, o.search.read(cx).value().to_string())) else { return };
        let last = self.found(which, &query, cx).len().saturating_sub(1);
        if let Some(open) = self.combo.as_mut() {
            open.cursor = if down { (open.cursor + 1).min(last) } else { open.cursor.min(last).saturating_sub(1) };
            open.scroll.scroll_to_item(open.cursor);
        }
        cx.notify();
    }

    fn choose_cursor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((which, query, cursor)) = self.combo.as_ref().map(|o| (o.which, o.search.read(cx).value().to_string(), o.cursor)) else { return };
        let mut found = self.found(which, &query, cx);
        if found.is_empty() {
            return;
        }
        let target = found.swap_remove(cursor.min(found.len() - 1)).target;
        self.choose(target, window, cx);
    }

    fn choose(&mut self, target: Target, window: &mut Window, cx: &mut Context<Self>) {
        self.close_combo(window, cx);
        match target {
            Target::Machine(machine) => self.pick_machine(machine, cx),
            Target::Device(pick) => self.pick(&pick, cx),
        }
        cx.notify();
    }

    /// A picker: a button naming what's selected; open, a search field over the matching rows.
    fn render_combo(&self, which: Combo, choices: Vec<Choice>, c: Colors, cx: &mut Context<Self>) -> Div {
        let key = which as usize;
        let open = self.combo.as_ref().filter(|o| o.which == which);
        let (icon, name) = choices.into_iter().find(|o| o.on).map_or_else(|| (Icon::new(IconName::Smartphone), String::new()), |o| (o.icon, o.name));
        let border = if open.is_some() { Hsla { a: 0.45, ..c.accent } } else { c.edge };
        let trigger = div().id(("device-combo", key)).w_full().h(px(28.0)).flex().items_center().gap_1p5().rounded_md().border_1().border_color(border).bg(c.raised).px_2().text_color(c.fg).cursor_pointer().hover(move |d| d.bg(c.hover))
            .on_click(cx.listener(move |this, _, window, cx| this.toggle_combo(which, window, cx)))
            .child(icon.size(px(12.0)).flex_shrink_0().text_color(c.muted))
            .child(div().flex_1().min_w_0().truncate().child(name))
            .child(Icon::new(IconName::ChevronDown).size(px(12.0)).flex_shrink_0().text_color(c.subtle));
        let combo = div().relative().flex_1().min_w_0().child(trigger);
        let Some(open) = open else { return combo };
        let query = open.search.read(cx).value().to_string();
        let found = self.found(which, &query, cx);
        let cursor = open.cursor.min(found.len().saturating_sub(1));
        let mut list = div().id(("device-combo-list", key)).max_h(px(260.0)).overflow_y_scroll().track_scroll(&open.scroll).flex().flex_col().p_1();
        if found.is_empty() {
            let what = if which == Combo::Machine { "machine" } else { "device" };
            list = list.child(div().px_2().py_3().text_center().text_color(c.subtle).child(format!("No {what} matches “{}”", query.trim())));
        }
        for (i, choice) in found.into_iter().enumerate() {
            let Choice { target, icon, name, detail, on } = choice;
            list = list.child(div().id(("device-combo-option", i)).flex().flex_shrink_0().items_center().gap_2().rounded_md().px_2().py_1p5().cursor_pointer()
                .when(i == cursor, |d| d.bg(c.hover))
                .on_mouse_move(cx.listener(move |this, _, _, cx| {
                    if let Some(o) = this.combo.as_mut().filter(|o| o.cursor != i) {
                        o.cursor = i;
                        cx.notify();
                    }
                }))
                .on_click(cx.listener(move |this, _, window, cx| this.choose(target.clone(), window, cx)))
                .child(icon.size(px(13.0)).flex_shrink_0().text_color(if on { c.accent } else { c.muted }))
                .child(div().flex_1().min_w_0().flex().flex_col()
                    .child(div().truncate().text_color(c.fg).when(on, |d| d.font_weight(FontWeight::MEDIUM)).child(name))
                    .when(!detail.is_empty(), |d| d.child(div().truncate().text_size(px(10.0)).text_color(c.subtle).child(detail))))
                .when(on, |d| d.child(Icon::new(IconName::Check).size(px(13.0)).flex_shrink_0().text_color(c.accent))));
        }
        let search = div().flex().items_center().gap_2().h(px(34.0)).px_2p5().border_b_1().border_color(c.edge)
            .child(Icon::new(IconName::Search).size(px(13.0)).flex_shrink_0().text_color(c.subtle))
            .child(div().flex_1().min_w_0().child(Input::new(&open.search).appearance(false).text_size(px(12.0))))
            .when(!query.is_empty(), |d| d.child(div().id(("device-combo-clear", key)).rounded_sm().p_0p5().text_color(c.subtle).cursor_pointer().hover(move |d| d.bg(c.hover).text_color(c.fg))
                .on_click(cx.listener(|this, _, window, cx| {
                    let Some(search) = this.combo.as_ref().map(|o| o.search.clone()) else { return };
                    search.update(cx, |s, cx| s.set_value("", window, cx));
                    this.reset_cursor(cx);
                }))
                .child(Icon::new(IconName::X).size(px(12.0)))));
        let panel = div().id(("device-combo-pop", key)).w(px(280.0)).flex().flex_col().overflow_hidden().rounded_lg().border_1().border_color(c.edge).bg(c.raised).shadow_lg().text_xs()
            .occlude()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down_out(cx.listener(move |this, _, window, cx| {
                this.dismissed = Some((which, Instant::now()));
                this.close_combo(window, cx);
            }))
            .capture_action(cx.listener(|this, _: &MoveDown, _, cx| {
                cx.stop_propagation();
                this.move_cursor(true, cx);
            }))
            .capture_action(cx.listener(|this, _: &MoveUp, _, cx| {
                cx.stop_propagation();
                this.move_cursor(false, cx);
            }))
            .capture_action(cx.listener(|this, _: &InputEscape, window, cx| {
                cx.stop_propagation();
                this.close_combo(window, cx);
            }))
            .child(search)
            .child(list);
        combo.child(deferred(anchored().snap_to_window_with_margin(px(8.0)).child(div().pt_1().child(panel))).with_priority(10))
    }

    fn reconnect(&mut self, cx: &mut Context<Self>) {
        if self.hosted.take().is_some() {
            *self.latest.lock() = None;
            self.touching = false;
            return self.sync_device(cx);
        }
        let Some(serial) = self.mirror.as_ref().map(|m| m.serial.clone()) else { return };
        self.stop_mirror();
        self.start_mirror(serial, Mode::Scrcpy, cx);
    }
}

impl Render for AndroidPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = theme::colors(&self.state.read(cx).theme);
        let machines = self.machine_choices();
        let current = self.current(cx);
        // A picker whose button went away closes with it.
        if current.is_none() && self.combo.as_ref().is_some_and(|o| o.which == Combo::Device) {
            self.combo = None;
        }
        let mut top = div().flex().h(px(40.0)).flex_shrink_0().items_center().gap_2().px_2().text_xs();
        // Whose devices: this machine or a paired one (only offered while there is a choice).
        let switchable = machines.len() > 1;
        if switchable {
            top = top.child(self.render_combo(Combo::Machine, machines, c, cx));
        }
        if self.machine.is_none() && self.status.is_none() {
            return div().size_full().flex().flex_col().when(switchable, |d| d.child(top)).child(div().flex_1().flex().items_center().justify_center().text_xs().text_color(c.subtle).child("Looking for devices…"));
        }
        top = match current {
            Some(_) => {
                let devices = self.device_choices(cx);
                top.child(self.render_combo(Combo::Device, devices, c, cx))
            }
            None => top.child(div().flex_1().min_w_0().text_color(c.muted).child("No device")),
        };
        let body = match current {
            Some(d) => self.render_mirror(d.name, c, window, cx),
            None => self.render_empty(c, cx),
        };
        div().size_full().flex().flex_col().child(top).child(body)
    }
}

#[cfg(test)]
mod tests {
    use super::{keycode, matches_query, simulator_key, start_codes, to_device};

    #[gpui_kit::test]
    fn a_phone_sees_and_picks_this_macs_simulators(cx: &mut gpui_kit::TestAppContext) {
        use super::AndroidPanel;
        use crate::model::AppState;
        use gpui_kit::AppContext;
        use serde_json::json;

        cx.update(gpui_kit::init);
        let (panel, vcx) = cx.add_window_view(|_, cx| {
            let app = cx.new(|_| AppState::new());
            let mut panel = AndroidPanel::new(app, cx);
            panel.status = Some(json!({"adb": true, "devices": [{"serial": "a1", "model": "Pixel 9", "state": "device"}], "avds": []}));
            let sim = |udid: &str, booted: bool| json!({"udid": udid, "name": udid, "runtime": "iOS 18.2", "booted": booted});
            panel.lists.insert(None, Ok(json!({"ios": {"simctl": true, "devices": [sim("S1", true), sim("S2", false)]}})));
            panel
        });
        vcx.run_until_parked();
        let state = vcx.read(|cx| panel.read(cx).phone_state(cx));
        assert_eq!(state["current"], "a1");
        assert_eq!(state["ios"]["devices"][1]["udid"], "S2");

        vcx.update(|_, cx| panel.update(cx, |p, cx| p.phone_select("S1".into(), cx)));
        assert_eq!(vcx.read(|cx| panel.read(cx).phone_state(cx))["current"], "S1");
        vcx.update(|_, cx| panel.update(cx, |p, cx| p.phone_select("a1".into(), cx)));
        assert_eq!(vcx.read(|cx| panel.read(cx).phone_state(cx))["current"], "a1");
    }

    #[test]
    fn a_phones_keys_on_a_simulator() {
        use serde_json::json;

        assert_eq!(simulator_key(3), Some(json!({"kind": "button", "button": "home"})));
        assert_eq!(simulator_key(26), Some(json!({"kind": "button", "button": "power"})));
        assert_eq!(simulator_key(67), Some(json!({"kind": "key", "key": "backspace"})));
        assert_eq!(simulator_key(19), Some(json!({"kind": "key", "key": "up"})));
        // Back, recent apps and volume have no equivalent.
        for code in [4, 187, 24, 25] {
            assert_eq!(simulator_key(code), None);
        }
    }

    #[gpui_kit::test]
    fn the_device_picker_searches_and_picks(cx: &mut gpui_kit::TestAppContext) {
        use super::{AndroidPanel, Combo};
        use crate::model::AppState;
        use gpui_kit::{AppContext, Modifiers, point, px};
        use serde_json::json;

        cx.update(gpui_kit::init);
        let (panel, vcx) = cx.add_window_view(|_, cx| {
            let app = cx.new(|_| AppState::new());
            let mut panel = AndroidPanel::new(app, cx);
            let device = |serial: &str, model: &str| json!({"serial": serial, "model": model, "state": "device"});
            panel.status = Some(json!({"devices": [device("a1", "Pixel 9"), device("b2", "Galaxy S25"), device("c3", "Galaxy Tab")]}));
            panel
        });
        let open = |vcx: &mut gpui_kit::VisualTestContext| {
            vcx.update(|window, cx| {
                panel.update(cx, |p, cx| p.toggle_combo(Combo::Device, window, cx));
                let _ = window.draw(cx);
            });
            vcx.run_until_parked();
        };
        let current = |vcx: &mut gpui_kit::VisualTestContext| vcx.read(|cx| panel.read(cx).current(cx).map(|d| d.name));
        vcx.run_until_parked();

        // Typing filters; Enter picks the row the arrows are on.
        open(vcx);
        vcx.simulate_input("galaxy");
        vcx.simulate_keystrokes("down down enter");
        vcx.run_until_parked();
        assert_eq!(current(vcx).as_deref(), Some("Galaxy Tab"));
        assert!(vcx.read(|cx| panel.read(cx).combo.is_none()));

        // It opens on the selected row; Escape leaves the selection alone.
        open(vcx);
        assert_eq!(vcx.read(|cx| panel.read(cx).combo.as_ref().map(|o| o.cursor)), Some(2));
        vcx.simulate_keystrokes("up escape");
        vcx.run_until_parked();
        assert!(vcx.read(|cx| panel.read(cx).combo.is_none()));
        assert_eq!(current(vcx).as_deref(), Some("Galaxy Tab"));

        // The button opens and closes it, and a row picks on click.
        let click = |vcx: &mut gpui_kit::VisualTestContext, y: f32| {
            vcx.simulate_click(point(px(60.0), px(y)), Modifiers::default());
            vcx.update(|window, cx| {
                let _ = window.draw(cx);
            });
            vcx.run_until_parked();
        };
        let is_open = |vcx: &mut gpui_kit::VisualTestContext| vcx.read(|cx| panel.read(cx).combo.is_some());
        click(vcx, 20.0);
        assert!(is_open(vcx));
        click(vcx, 20.0);
        assert!(!is_open(vcx));
        vcx.update(|_, cx| panel.update(cx, |p, _| p.dismissed = None));
        click(vcx, 20.0);
        click(vcx, 95.0);
        assert!(!is_open(vcx));
        assert_eq!(current(vcx).as_deref(), Some("Pixel 9"));
    }

    #[test]
    fn search_matches_words_in_any_order() {
        assert!(matches_query("iPhone 18 Pro Simulator · iOS 27.2", "sim iphone"));
        assert!(matches_query("Mac Studio von Ricky", "  "));
        assert!(!matches_query("dane-server", "dane studio"));
    }

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

    #[test]
    fn yuv420_to_bgra_matches_bt601() {
        use openh264::formats::{RgbSliceU8, YUVBuffer};
        // Solid red (255, 0, 0), green (0, 255, 0), blue (0, 0, 255), white (255, 255, 255) in 4x2 blocks.
        let rgb = [
            255, 0, 0, 255, 0, 0, 0, 0, 255, 0, 0, 255,
            255, 0, 0, 255, 0, 0, 0, 0, 255, 0, 0, 255,
        ];
        let yuv = YUVBuffer::from_rgb_source(RgbSliceU8::new(&rgb, (4, 2)));
        let bgra = super::yuv420_to_bgra(&yuv);
        assert_eq!(bgra.len(), 4 * 2 * 4);
        // Pixel (0,0) is red -> BGRA = [low B, low G, high R, 255].
        assert!(bgra[2] > 230 && bgra[0] < 30 && bgra[1] < 30 && bgra[3] == 255);
        // Pixel (2,0) is blue -> BGRA = [high B, low G, low R, 255].
        assert!(bgra[8] > 230 && bgra[9] < 30 && bgra[10] < 30 && bgra[11] == 255);
    }
}
