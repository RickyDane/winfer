//! The browser side panel: tabs from each workspace's headless Chromium, shown through a CDP
//! screencast (port of `src/features/browser/BrowserPanel.tsx` and `BrowserView.tsx`).

use crate::backend;
use crate::model::AppState;
use crate::terminal::SESSIONS;
use crate::theme::{self, Colors};
use gpui_kit::assets::IconName;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{Icon, Sizable};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use parking_lot::Mutex;
use serde::Deserialize;
use serde_json::{Value, json};
use smallvec::smallvec;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tauri::Manager;
use tauri::ipc::{Channel, InvokeResponseBody};
use winfer_backend::browser::{self, BrowserManager};

#[derive(Clone, Debug, Deserialize)]
pub struct Tab {
    pub id: String,
    pub url: String,
    pub title: String,
    /// The page's icon as a data URL ("" until it loaded, or if it has none).
    #[serde(default)]
    pub favicon: String,
}

/// The phone size of the mobile view.
const MOBILE: (u32, u32) = (390, 844);
const MIN_PAGE: u32 = 200;
const MAX_PAGE: u32 = 4000;
/// Space around a sized page for its drag handles.
const HANDLE_ROOM: f32 = 16.0;
/// How long an agent's cursor stays after its last mouse event, and a click's ripple.
const CURSOR_IDLE: Duration = Duration::from_millis(2500);
const AGENT_LINGER: Duration = Duration::from_millis(3500);

#[derive(Clone, Debug, Deserialize)]
pub struct ConsoleEntry {
    pub level: String,
    pub text: String,
    #[serde(default)]
    pub source: Option<String>,
}

/// The agent's mouse in a tab, in page pixels.
#[derive(Clone, Copy)]
struct AgentCursor {
    x: f32,
    y: f32,
    at: Instant,
    pressed: Option<Instant>,
}

/// A page edge being dragged to size the page.
#[derive(Clone, Copy)]
struct SizeDrag {
    x: bool,
    y: bool,
    start: Point<Pixels>,
    from: (u32, u32),
    scale: f32,
}

/// Where the page is painted within the panel: its own size (an agent's, a drag's or the phone's)
/// fitted into the room it has, else all of it.
fn page_rect(bounds: Bounds<Pixels>, own: Option<(u32, u32)>) -> Bounds<Pixels> {
    let Some((w, h)) = own else { return bounds };
    let (bw, bh) = (bounds.size.width / px(1.0), bounds.size.height / px(1.0));
    let (room_w, room_h) = ((bw - HANDLE_ROOM * 2.0).max(1.0), (bh - HANDLE_ROOM * 2.0).max(1.0));
    let scale = (room_w / w as f32).min(room_h / h as f32).min(1.0);
    let (dw, dh) = (w as f32 * scale, h as f32 * scale);
    Bounds::new(point(bounds.origin.x + px((bw - dw) / 2.0), bounds.origin.y + px(HANDLE_ROOM.min((bh - dh) / 2.0))), size(px(dw), px(dh)))
}

/// A favicon as an image: any format the `image` crate reads, with dark icons lightened on a dark panel.
fn favicon_image(data_url: &str, dark: bool) -> Option<Arc<Image>> {
    use base64::Engine;
    let (head, body) = data_url.split_once(',')?;
    let mime = head.strip_prefix("data:")?.split(';').next()?;
    let bytes = if head.contains(";base64") { base64::engine::general_purpose::STANDARD.decode(body).ok()? } else { body.as_bytes().to_vec() };
    if mime == "image/svg+xml" {
        return Some(Arc::new(Image::from_bytes(ImageFormat::Svg, bytes)));
    }
    let mut rgba = image::load_from_memory(&bytes).ok()?.into_rgba8();
    if dark {
        let (mut sum, mut n) = (0.0f32, 0.0f32);
        for p in rgba.pixels().filter(|p| p[3] > 128) {
            sum += (0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32) / 255.0;
            n += 1.0;
        }
        if n > 0.0 && sum / n < 0.25 {
            for p in rgba.pixels_mut() {
                p[0] = 255 - p[0];
                p[1] = 255 - p[1];
                p[2] = 255 - p[2];
            }
        }
    }
    let mut png = std::io::Cursor::new(Vec::new());
    rgba.write_to(&mut png, image::ImageFormat::Png).ok()?;
    Some(Arc::new(Image::from_bytes(ImageFormat::Png, png.into_inner())))
}

fn tab_label(tab: &Tab) -> String {
    if !tab.title.is_empty() && tab.title != tab.url {
        return tab.title.clone();
    }
    if tab.url.is_empty() || tab.url == "about:blank" {
        return "New tab".to_string();
    }
    tab.url.split("://").nth(1).and_then(|r| r.split('/').next()).filter(|h| !h.is_empty()).unwrap_or(&tab.url).to_string()
}

/// Turns what the user typed into a URL: localhost → http, bare domains → https, else a search.
pub fn normalize_url(input: &str) -> String {
    use regex::Regex;
    use std::sync::LazyLock;
    static SCHEME: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^[a-z][a-z0-9+.-]*:").unwrap());
    static HOST_PORT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^(localhost|[\w.-]+):\d+").unwrap());
    static LOCAL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^(localhost|127\.0\.0\.1|0\.0\.0\.0|\[::1\])(:\d+)?(/|$)").unwrap());
    static DOMAIN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[\w-]+(\.[\w-]+)+(:\d+)?(/.*)?$").unwrap());
    let text = input.trim();
    if SCHEME.is_match(text) && !HOST_PORT.is_match(text) {
        return text.to_string();
    }
    if LOCAL.is_match(text) {
        return format!("http://{text}");
    }
    if DOMAIN.is_match(text) && !text.contains(' ') {
        return format!("https://{text}");
    }
    let query: String = text
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'!' | b'~' | b'*' | b'\'' | b'(' | b')' => (b as char).to_string(),
            b => format!("%{b:02X}"),
        })
        .collect();
    format!("https://www.google.com/search?q={query}")
}

/// The open tab already showing `url`'s site, so an agent takes it over instead of duplicating it.
pub fn related_tab<'a>(tabs: &'a [Tab], url: &str) -> Option<&'a Tab> {
    fn parse(s: &str) -> Option<url::Url> {
        url::Url::parse(s).ok().filter(|u| matches!(u.scheme(), "http" | "https" | "file"))
    }
    fn site(u: &url::Url) -> String {
        let host = u.host_str().unwrap_or_default();
        let loopback = matches!(host.to_lowercase().as_str(), "localhost" | "127.0.0.1" | "0.0.0.0" | "[::1]");
        let host = if loopback { "localhost" } else { host.strip_prefix("www.").unwrap_or(host) };
        let scheme = if u.scheme() == "https" && host == "localhost" { "http" } else { u.scheme() };
        format!("{scheme}://{host}:{}", u.port().map_or(String::new(), |p| p.to_string()))
    }
    fn folder(u: &url::Url) -> String {
        u.path().rsplit_once('/').map_or(String::new(), |(a, _)| a.to_string())
    }
    fn segments(u: &url::Url) -> Vec<&str> {
        u.path().split('/').filter(|s| !s.is_empty()).collect()
    }
    let target = parse(url)?;
    let is_file = target.scheme() == "file";
    let mut best: Option<(&Tab, usize)> = None;
    for tab in tabs {
        let Some(u) = parse(&tab.url) else { continue };
        let same = if is_file { u.scheme() == "file" && folder(&u) == folder(&target) } else { u.scheme() != "file" && site(&u) == site(&target) };
        if !same {
            continue;
        }
        let strip = |x: &url::Url| x.as_str().split('#').next().unwrap_or_default().to_string();
        let shared = segments(&u).iter().zip(segments(&target)).take_while(|(a, b)| *a == b).count();
        let score = if strip(&u) == strip(&target) { 1000 } else { 0 } + shared;
        if best.is_none_or(|(_, b)| score >= b) {
            best = Some((tab, score));
        }
    }
    best.map(|(t, _)| t)
}

/// One screencast being shown.
struct Attachment {
    workspace: String,
    tab: String,
    id: String,
    width: u32,
    height: u32,
    scale: f64,
    mobile: bool,
}

/// The frame decoder: keeps only the newest undecoded frame.
#[derive(Default)]
struct Decoder {
    busy: bool,
    pending: Option<Vec<u8>>,
}

type InputMessage = (String, String, String, Value);

fn decode(bytes: &[u8]) -> Option<Arc<RenderImage>> {
    let mut rgba = image::load_from_memory_with_format(bytes, image::ImageFormat::Jpeg).ok()?.into_rgba8();
    for pixel in rgba.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }
    Some(Arc::new(RenderImage::new(smallvec![image::Frame::new(rgba)])))
}

fn frame_done(workspace: String, attach: String) {
    tauri::async_runtime::spawn(async move {
        let _ = browser::browser_frame_done(backend::app().state(), workspace, attach).await;
    });
}

fn run_decoder(decoder: Arc<Mutex<Decoder>>, latest: Arc<Mutex<Option<Arc<RenderImage>>>>, notify: async_channel::Sender<()>, workspace: String, attach: String, first: Vec<u8>) {
    backend::background(move || {
        let mut bytes = first;
        loop {
            super::capture::offer_encoded(super::capture::Source::Browser, &bytes);
            if let Some(image) = decode(&bytes) {
                *latest.lock() = Some(image);
                let _ = notify.try_send(());
            }
            frame_done(workspace.clone(), attach.clone());
            let mut d = decoder.lock();
            match d.pending.take() {
                Some(next) => bytes = next,
                None => {
                    d.busy = false;
                    return;
                }
            }
        }
    });
}

fn input_loop(rx: async_channel::Receiver<InputMessage>) {
    tauri::async_runtime::spawn(async move {
        while let Ok(mut message) = rx.recv().await {
            // Only the latest pointer position matters to a page that's catching up.
            let is_move = |m: &InputMessage| m.2 == "Input.dispatchMouseEvent" && m.3["type"] == "mouseMoved";
            while is_move(&message) {
                match rx.try_recv() {
                    Ok(next) => message = next,
                    Err(_) => break,
                }
            }
            let (workspace, tab, method, params) = message;
            let _ = browser::browser_input(backend::app().clone(), backend::app().state::<BrowserManager>(), workspace, tab, method, params).await;
        }
    });
}

fn cdp_modifiers(m: &Modifiers) -> u32 {
    (m.alt as u32) | ((m.control as u32) << 1) | ((m.platform as u32) << 2) | ((m.shift as u32) << 3)
}

/// CDP `key`, `code` and virtual-key code for a GPUI key name.
fn cdp_key(key: &str, typed: Option<&str>) -> (String, String, u32) {
    let named = |k: &str, code: &str, vk: u32| (k.to_string(), code.to_string(), vk);
    match key {
        "enter" => named("Enter", "Enter", 13),
        "tab" => named("Tab", "Tab", 9),
        "backspace" => named("Backspace", "Backspace", 8),
        "escape" => named("Escape", "Escape", 27),
        "space" => (" ".into(), "Space".into(), 32),
        "pageup" => named("PageUp", "PageUp", 33),
        "pagedown" => named("PageDown", "PageDown", 34),
        "end" => named("End", "End", 35),
        "home" => named("Home", "Home", 36),
        "left" => named("ArrowLeft", "ArrowLeft", 37),
        "up" => named("ArrowUp", "ArrowUp", 38),
        "right" => named("ArrowRight", "ArrowRight", 39),
        "down" => named("ArrowDown", "ArrowDown", 40),
        "delete" => named("Delete", "Delete", 46),
        k => {
            let shown = typed.unwrap_or(k).to_string();
            let c = k.chars().next().unwrap_or(' ');
            let (code, vk) = match c {
                'a'..='z' => (format!("Key{}", c.to_ascii_uppercase()), c.to_ascii_uppercase() as u32),
                '0'..='9' => (format!("Digit{c}"), c as u32),
                ';' => ("Semicolon".into(), 186),
                '=' => ("Equal".into(), 187),
                ',' => ("Comma".into(), 188),
                '-' => ("Minus".into(), 189),
                '.' => ("Period".into(), 190),
                '/' => ("Slash".into(), 191),
                '`' => ("Backquote".into(), 192),
                '[' => ("BracketLeft".into(), 219),
                '\\' => ("Backslash".into(), 220),
                ']' => ("BracketRight".into(), 221),
                '\'' => ("Quote".into(), 222),
                _ => (String::new(), 0),
            };
            (shown, code, vk)
        }
    }
}

pub struct BrowserPanel {
    state: Entity<AppState>,
    tabs: HashMap<String, Vec<Tab>>,
    active_tab: HashMap<String, String>,
    workspace: Option<String>,
    opening: Option<String>,
    message: Option<SharedString>,
    needs_install: bool,
    installing: bool,
    address: Entity<InputState>,
    address_for: Option<(String, String)>,
    focus: FocusHandle,
    latest: Arc<Mutex<Option<Arc<RenderImage>>>>,
    shown: Option<Arc<RenderImage>>,
    attachment: Option<Attachment>,
    bounds: Bounds<Pixels>,
    input: async_channel::Sender<InputMessage>,
    frames: async_channel::Sender<()>,
    resize_task: Option<Task<()>>,
    buttons: u32,
    last_click: u32,
    attach_count: u64,
    /// Console and network entries per tab id.
    console: HashMap<String, Vec<ConsoleEntry>>,
    console_open: bool,
    picking: bool,
    /// Workspaces whose browser skips HTTPS certificate checks (for this browser session).
    invalid_certs: std::collections::HashSet<String>,
    cert_prompt: bool,
    cert_busy: bool,
    notice: Option<(SharedString, Instant)>,
    /// Workspaces in the phone view.
    mobile: HashSet<String>,
    /// Page sizes set by dragging the page's edges or by an agent, per tab id.
    sizes: HashMap<String, (u32, u32)>,
    size_drag: Option<SizeDrag>,
    cursors: HashMap<String, AgentCursor>,
    /// Tabs an agent used lately, by tab id.
    agent_tabs: HashMap<String, Instant>,
    favicons: HashMap<(String, usize, bool), Option<Arc<Image>>>,
    /// Where the page was last painted, and the panel around it.
    panel_bounds: Bounds<Pixels>,
    #[cfg(debug_assertions)]
    dev_url_opened: bool,
    /// A blank replacement tab was requested after the last tab closed; cleared once any tab exists.
    blank_pending: bool,
}

impl BrowserPanel {
    pub fn new(state: Entity<AppState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let (input, input_rx) = async_channel::unbounded();
        input_loop(input_rx);
        let (frames, frames_rx) = async_channel::bounded(1);
        cx.spawn(async move |this, cx| {
            while frames_rx.recv().await.is_ok() {
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
            }
        })
        .detach();
        let address = cx.new(|cx| InputState::new(window, cx).placeholder("Search or enter address"));
        cx.subscribe_in(&address, window, |this, _, event: &InputEvent, _, cx| {
            if matches!(event, InputEvent::PressEnter { .. }) {
                this.submit_address(cx);
            }
        })
        .detach();
        Self {
            state,
            tabs: HashMap::new(),
            active_tab: HashMap::new(),
            workspace: None,
            opening: None,
            message: None,
            needs_install: false,
            installing: false,
            address,
            address_for: None,
            focus: cx.focus_handle(),
            latest: Arc::default(),
            shown: None,
            attachment: None,
            bounds: Bounds::default(),
            input,
            frames,
            resize_task: None,
            buttons: 0,
            last_click: 1,
            attach_count: 0,
            console: HashMap::new(),
            console_open: false,
            picking: false,
            invalid_certs: Default::default(),
            cert_prompt: false,
            cert_busy: false,
            notice: None,
            mobile: HashSet::new(),
            sizes: HashMap::new(),
            size_drag: None,
            cursors: HashMap::new(),
            agent_tabs: HashMap::new(),
            favicons: HashMap::new(),
            panel_bounds: Bounds::default(),
            #[cfg(debug_assertions)]
            dev_url_opened: false,
            blank_pending: false,
        }
    }

    /// A `browser://…` event from the backend.
    pub fn on_event(&mut self, event: &str, payload: &Value, cx: &mut Context<Self>) {
        match event {
            "browser://tabs" => {
                let Some(workspace) = payload["workspaceId"].as_str() else { return };
                let tabs: Vec<Tab> = serde_json::from_value(payload["tabs"].clone()).unwrap_or_default();
                self.set_tabs(workspace.to_string(), tabs, cx);
            }
            "browser://console" => {
                let (Some(tab), Ok(entry)) = (payload["targetId"].as_str(), serde_json::from_value::<ConsoleEntry>(payload["entry"].clone())) else { return };
                let list = self.console.entry(tab.to_string()).or_default();
                list.push(entry);
                let extra = list.len().saturating_sub(200);
                list.drain(..extra);
                if self.console_open {
                    cx.notify();
                }
            }
            "browser://viewport" => {
                let (Some(tab), Some(w), Some(h)) = (payload["targetId"].as_str(), payload["width"].as_u64(), payload["height"].as_u64()) else { return };
                self.sizes.insert(tab.to_string(), (w as u32, h as u32));
                cx.notify();
            }
            "browser://picked" => {
                self.picking = false;
                let line = |k: &str| payload[k].as_str().unwrap_or_default().to_string();
                let mut lines = vec![format!("Element on {}", line("url")), format!("selector: {}", line("selector"))];
                let text = line("text");
                if !text.is_empty() {
                    lines.push(format!("text: \"{}\"", text.split_whitespace().collect::<Vec<_>>().join(" ")));
                }
                lines.push("html:".into());
                lines.push(line("html"));
                if let Some(shot) = payload["screenshot"].as_str() {
                    lines.push(format!("screenshot: {shot}"));
                }
                let what = line("selector");
                self.send_to_agent(format!("{}\n", lines.join("\n")), &what, cx);
            }
            "browser://pick-error" => {
                self.picking = false;
                self.flash(format!("Couldn't read that element: {}", payload.as_str().unwrap_or_default()), cx);
            }
            "browser://agent" => {
                if let Some(tab) = payload["targetId"].as_str() {
                    self.agent_tabs.insert(tab.to_string(), Instant::now());
                    self.later(AGENT_LINGER, cx);
                }
            }
            "browser://agent-navigate" => {
                let (Some(ws), Some(tab)) = (payload["workspaceId"].as_str(), payload["targetId"].as_str()) else { return };
                self.active_tab.insert(ws.to_string(), tab.to_string());
                self.state.update(cx, |s, cx| {
                    if !s.browser_open || s.side_panel != Some(crate::model::SidePanel::Browser) {
                        s.side_panel = Some(crate::model::SidePanel::Browser);
                        s.browser_open = true;
                        cx.notify();
                    }
                });
                cx.notify();
            }
            "browser://agent-pointer" => {
                let (Some(tab), Some(x), Some(y)) = (payload["targetId"].as_str(), payload["x"].as_f64(), payload["y"].as_f64()) else { return };
                let pressed = (payload["kind"] == "mousePressed").then(Instant::now);
                let previous = self.cursors.get(tab).and_then(|c| c.pressed);
                self.cursors.insert(tab.to_string(), AgentCursor { x: x as f32, y: y as f32, at: Instant::now(), pressed: pressed.or(previous) });
                self.later(CURSOR_IDLE, cx);
            }
            "browser://certificate-policy" => {
                if let Some(ws) = payload["workspaceId"].as_str() {
                    if payload["allow"].as_bool() == Some(true) {
                        self.invalid_certs.insert(ws.to_string());
                    } else {
                        self.invalid_certs.remove(ws);
                    }
                    cx.notify();
                }
            }
            "browser://disconnected" => {
                if payload.as_str() == self.workspace.as_deref() {
                    self.detach();
                    cx.notify();
                }
            }
            _ => {}
        }
    }

    fn set_tabs(&mut self, workspace: String, tabs: Vec<Tab>, cx: &mut Context<Self>) {
        let active = self.active_tab.get(&workspace).filter(|id| tabs.iter().any(|t| t.id == **id)).cloned();
        match active.or_else(|| tabs.last().map(|t| t.id.clone())) {
            Some(id) => {
                self.active_tab.insert(workspace.clone(), id);
            }
            None => {
                self.active_tab.remove(&workspace);
            }
        }
        self.tabs.insert(workspace.clone(), tabs);
        // Never leave the browser without a tab: once the last one overall closes, open a blank one.
        if self.tabs.values().any(|t| !t.is_empty()) {
            self.blank_pending = false;
        } else if !std::mem::replace(&mut self.blank_pending, true) {
            tauri::async_runtime::spawn(async move {
                let app = backend::app();
                let _ = browser::browser_new_tab(app.clone(), app.state::<BrowserManager>(), workspace, "about:blank".to_string()).await;
            });
        }
        // Development aid: load this page once the browser is up.
        #[cfg(debug_assertions)]
        if let Some(url) = std::env::var("WINFER_NATIVE_URL").ok().filter(|_| !std::mem::replace(&mut self.dev_url_opened, true)) {
            if std::env::var_os("WINFER_NATIVE_MOBILE").is_some() {
                if let Some(w) = self.workspace.clone() {
                    self.mobile.insert(w);
                }
            }
            self.navigate(url, cx);
        }
        cx.notify();
    }

    /// Repaints once `after` has passed (for things that time out).
    fn later(&self, after: Duration, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(after + Duration::from_millis(50)).await;
            let _ = this.update(cx, |_, cx| cx.notify());
        })
        .detach();
    }

    fn flash(&mut self, text: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.notice = Some((text.into(), Instant::now()));
        self.later(Duration::from_secs(4), cx);
        cx.notify();
    }

    /// Pastes into the agent that should get browser context: the workspace's focused agent pane, else its first.
    fn send_to_agent(&mut self, text: String, what: &str, cx: &mut Context<Self>) {
        let target = {
            let s = self.state.read(cx);
            let Some(ws) = s.active() else { return };
            let agents: Vec<&crate::model::Pane> = ws.panes.iter().filter(|p| p.provider != "shell" && !p.archived).collect();
            agents.iter().find(|p| s.focused_pane_id.as_deref() == Some(&p.id)).or(agents.first()).map(|p| ((*p).id.clone(), (*p).provider.clone(), s.provider_name(&p.provider)))
        };
        let Some((id, provider, name)) = target else {
            return self.flash("Open an agent pane first", cx);
        };
        if let Some(session) = SESSIONS.get(&id) {
            session.paste(&text, &provider);
        }
        self.state.update(cx, |s, cx| {
            s.focus_pane(&id);
            cx.notify();
        });
        self.flash(format!("Sent {what} to {name}"), cx);
    }

    pub fn toggle_pick(&mut self, cx: &mut Context<Self>) {
        let Some((workspace, tab)) = self.active().map(|(w, t)| (w.clone(), t.id.clone())) else { return };
        let start = !self.picking;
        self.picking = start;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = tauri::async_runtime::spawn(async move {
                let app = backend::app();
                if start {
                    browser::inspect::browser_pick_start(app.clone(), app.state::<BrowserManager>(), workspace, tab).await.map_err(|e| e.to_string())
                } else {
                    browser::inspect::browser_pick_cancel(app.clone(), app.state::<BrowserManager>(), workspace, tab).await.map_err(|e| e.to_string())
                }
            })
            .await;
            if let Ok(Err(err)) = result {
                let _ = this.update(cx, |this, cx| {
                    this.picking = false;
                    this.flash(err, cx);
                });
            }
        })
        .detach();
    }

    /// Turns HTTPS certificate checks off or back on for the workspace's browser.
    fn set_invalid_certs(&mut self, allow: bool, cx: &mut Context<Self>) {
        let Some(workspace) = self.workspace.clone() else { return };
        self.cert_busy = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let ws = workspace.clone();
            let result = tauri::async_runtime::spawn(async move {
                let app = backend::app();
                browser::browser_allow_invalid_certificates(app.clone(), app.state::<BrowserManager>(), ws, allow).await.map_err(|e| e.to_string())
            })
            .await;
            let _ = this.update(cx, |this, cx| {
                this.cert_busy = false;
                match result {
                    Ok(Ok(())) => {
                        if allow { this.invalid_certs.insert(workspace) } else { this.invalid_certs.remove(&workspace) };
                        this.cert_prompt = false;
                    }
                    Ok(Err(err)) => this.flash(err, cx),
                    Err(err) => this.flash(err.to_string(), cx),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn clear_console(&mut self, cx: &mut Context<Self>) {
        let Some((workspace, tab)) = self.active().map(|(w, t)| (w.clone(), t.id.clone())) else { return };
        self.console.insert(tab.clone(), Vec::new());
        cx.notify();
        tauri::async_runtime::spawn(async move {
            let app = backend::app();
            let _ = browser::inspect::browser_console_clear(app.clone(), app.state::<BrowserManager>(), workspace, tab).await;
        });
    }

    fn send_console(&mut self, cx: &mut Context<Self>) {
        let Some((_, tab)) = self.active().map(|(w, t)| (w.clone(), t.clone())) else { return };
        let entries: Vec<&ConsoleEntry> = self.console.get(&tab.id).map(|l| l.iter().filter(|e| e.level == "error" || e.level == "warning").collect()).unwrap_or_default();
        if entries.is_empty() {
            return self.flash("No console errors to send", cx);
        }
        let entries: Vec<&ConsoleEntry> = entries.into_iter().rev().take(20).rev().collect();
        let body = entries.iter().map(|e| format!("- [{}] {}{}", e.level, e.text, e.source.as_ref().map_or(String::new(), |s| format!(" ({s})")))).collect::<Vec<_>>().join("\n");
        let n = entries.len();
        let text = format!("Browser console errors on {}:\n{body}\n", if tab.url.is_empty() { "the page" } else { &tab.url });
        self.send_to_agent(text, &format!("{n} console error{}", if n == 1 { "" } else { "s" }), cx);
    }

    fn is_mobile(&self) -> bool {
        self.workspace.as_ref().is_some_and(|w| self.mobile.contains(w))
    }

    /// The page's own size, if it has one: a drag's or an agent's, else the phone's.
    fn own_size(&self) -> Option<(u32, u32)> {
        let (_, tab) = self.active()?;
        self.sizes.get(&tab.id).copied().or_else(|| self.is_mobile().then_some(MOBILE))
    }

    fn set_device(&mut self, mobile: bool, cx: &mut Context<Self>) {
        if let Some((_, tab)) = self.active() {
            let id = tab.id.clone();
            self.sizes.remove(&id);
        }
        if let Some(w) = self.workspace.clone() {
            if mobile { self.mobile.insert(w); } else { self.mobile.remove(&w); }
        }
        cx.notify();
    }

    fn fit_page(&mut self, cx: &mut Context<Self>) {
        if let Some((_, tab)) = self.active() {
            let id = tab.id.clone();
            self.sizes.remove(&id);
        }
        // The phone view with no size of its own goes back to the phone size, desktop fits the panel.
        cx.notify();
    }

    fn favicon(&mut self, tab: &Tab, dark: bool) -> Option<Arc<Image>> {
        if tab.favicon.is_empty() {
            return None;
        }
        let key = (tab.id.clone(), tab.favicon.len(), dark);
        self.favicons.entry(key).or_insert_with(|| favicon_image(&tab.favicon, dark)).clone()
    }

    /// Titles of a workspace's tabs, for voice.
    pub fn tab_labels(&self, workspace: &str) -> Vec<String> {
        self.tabs.get(workspace).map(|t| t.iter().map(tab_label).collect()).unwrap_or_default()
    }

    fn active(&self) -> Option<(&String, &Tab)> {
        let workspace = self.workspace.as_ref()?;
        let id = self.active_tab.get(workspace)?;
        Some((workspace, self.tabs.get(workspace)?.iter().find(|t| t.id == *id)?))
    }

    fn open_workspace(&mut self, workspace: String, cx: &mut Context<Self>) {
        if self.opening.as_ref() == Some(&workspace) || self.tabs.contains_key(&workspace) {
            return;
        }
        self.opening = Some(workspace.clone());
        cx.spawn(async move |this, cx| {
            let target = workspace.clone();
            let opened = tauri::async_runtime::spawn(async move {
                let app = backend::app();
                let status = browser::browser_status(app.clone()).map_err(|e| e.to_string())?;
                let status = serde_json::to_value(status).unwrap_or(Value::Null);
                if status["executable"].is_null() {
                    return Ok::<Option<Value>, String>(None);
                }
                let tabs = browser::browser_open(app.clone(), app.state::<BrowserManager>(), target).await.map_err(|e| e.to_string())?;
                Ok(Some(serde_json::to_value(tabs).unwrap_or(Value::Null)))
            })
            .await;
            let _ = this.update(cx, |this, cx| {
                this.opening = None;
                match opened {
                    Ok(Ok(Some(tabs))) => {
                        this.message = None;
                        this.needs_install = false;
                        this.set_tabs(workspace, serde_json::from_value(tabs).unwrap_or_default(), cx);
                    }
                    Ok(Ok(None)) => {
                        this.needs_install = true;
                        // Development aid: download the browser without clicking.
                        if cfg!(debug_assertions) && std::env::var_os("WINFER_NATIVE_INSTALL_BROWSER").is_some() && !this.installing {
                            this.install(cx);
                        }
                    }
                    Ok(Err(err)) => this.message = Some(err.into()),
                    Err(err) => this.message = Some(err.to_string().into()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn install(&mut self, cx: &mut Context<Self>) {
        self.installing = true;
        self.message = Some("Downloading the browser (about 100 MB)…".into());
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = tauri::async_runtime::spawn(async { browser::browser_install(backend::app().clone()).await.map(|_| ()).map_err(|e| e.to_string()) }).await;
            let _ = this.update(cx, |this, cx| {
                this.installing = false;
                match result {
                    Ok(Ok(())) => {
                        this.needs_install = false;
                        this.message = None;
                    }
                    Ok(Err(err)) => this.message = Some(err.into()),
                    Err(err) => this.message = Some(err.to_string().into()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn detach(&mut self) {
        if let Some(a) = self.attachment.take() {
            tauri::async_runtime::spawn(async move {
                let _ = browser::browser_detach(backend::app().state::<BrowserManager>(), a.workspace, a.id).await;
            });
        }
    }

    fn attach(&mut self, workspace: String, tab: String, width: u32, height: u32, scale: f64, mobile: bool, cx: &mut Context<Self>) {
        self.detach();
        self.attach_count += 1;
        let attach_id = format!("native-{}-{:08x}", self.attach_count, rand::random::<u32>());
        self.attachment = Some(Attachment { workspace: workspace.clone(), tab: tab.clone(), id: attach_id.clone(), width, height, scale, mobile });
        let decoder: Arc<Mutex<Decoder>> = Arc::default();
        let (latest, notify, for_workspace, for_attach) = (self.latest.clone(), self.frames.clone(), workspace.clone(), attach_id.clone());
        let channel: Channel = Channel::new(move |body| {
            if let InvokeResponseBody::Raw(bytes) = body {
                let mut d = decoder.lock();
                if d.busy {
                    if d.pending.replace(bytes).is_some() {
                        frame_done(for_workspace.clone(), for_attach.clone());
                    }
                } else {
                    d.busy = true;
                    drop(d);
                    run_decoder(decoder.clone(), latest.clone(), notify.clone(), for_workspace.clone(), for_attach.clone(), bytes);
                }
            }
            Ok(())
        });
        let viewport = json!({ "width": width, "height": height, "deviceScaleFactor": scale, "mobile": mobile });
        cx.spawn(async move |this, cx| {
            let attached = tauri::async_runtime::spawn(async move {
                let viewport = serde_json::from_value(viewport).map_err(|e: serde_json::Error| e.to_string())?;
                let app = backend::app();
                browser::browser_attach(app.clone(), app.state::<BrowserManager>(), workspace, tab, viewport, attach_id, channel).await.map(|_| ()).map_err(|e| e.to_string())
            })
            .await;
            let failure = match attached {
                Ok(Ok(())) => None,
                Ok(Err(err)) => Some(err),
                Err(err) => Some(err.to_string()),
            };
            if let Some(err) = failure {
                let _ = this.update(cx, |this, cx| {
                    this.message = Some(err.into());
                    cx.notify();
                });
            }
        })
        .detach();
    }

    /// Keeps the screencast on the tab and at the size that's showing.
    fn sync(&mut self, bounds: Bounds<Pixels>, scale: f64, cx: &mut Context<Self>) {
        let own = self.own_size();
        let mobile = self.is_mobile();
        self.panel_bounds = bounds;
        self.bounds = page_rect(bounds, own);
        let (width, height) = own.unwrap_or(((bounds.size.width / px(1.0)).round() as u32, (bounds.size.height / px(1.0)).round() as u32));
        let scale = if mobile { 2.0 } else { scale };
        if width < 2 || height < 2 {
            return;
        }
        let Some((workspace, tab)) = self.active().map(|(w, t)| (w.clone(), t.id.clone())) else {
            self.detach();
            return;
        };
        match &self.attachment {
            Some(a) if a.workspace == workspace && a.tab == tab => {
                if a.width != width || a.height != height || a.scale != scale || a.mobile != mobile {
                    if let Some(a) = self.attachment.as_mut() {
                        (a.width, a.height, a.scale, a.mobile) = (width, height, scale, mobile);
                    }
                    let viewport = json!({ "width": width, "height": height, "deviceScaleFactor": scale, "mobile": mobile });
                    self.resize_task = Some(cx.spawn(async move |_, cx| {
                        cx.background_executor().timer(Duration::from_millis(50)).await;
                        tauri::async_runtime::spawn(async move {
                            let Ok(viewport) = serde_json::from_value(viewport) else { return };
                            let app = backend::app();
                            let _ = browser::browser_resize(app.clone(), app.state::<BrowserManager>(), workspace, tab, viewport).await;
                        });
                    }));
                }
            }
            _ => self.attach(workspace, tab, width, height, scale, mobile, cx),
        }
    }

    fn send(&self, method: &str, params: Value) {
        if let Some((workspace, tab)) = self.active() {
            let _ = self.input.try_send((workspace.clone(), tab.id.clone(), method.to_string(), params));
        }
    }

    fn page_point(&self, position: Point<Pixels>) -> (f32, f32) {
        let relative = position - self.bounds.origin;
        let (x, y) = (relative.x / px(1.0), relative.y / px(1.0));
        match &self.attachment {
            Some(a) if self.bounds.size.width > px(0.0) => (
                x / (self.bounds.size.width / px(1.0)) * a.width as f32,
                y / (self.bounds.size.height / px(1.0)) * a.height as f32,
            ),
            _ => (x, y),
        }
    }

    fn mouse(&mut self, kind: &str, position: Point<Pixels>, button: &str, clicks: u32, modifiers: &Modifiers) {
        let (x, y) = self.page_point(position);
        self.send("Input.dispatchMouseEvent", json!({ "type": kind, "x": x, "y": y, "button": button, "buttons": self.buttons, "clickCount": clicks, "modifiers": cdp_modifiers(modifiers) }));
    }

    fn key(&self, down: bool, stroke: &Keystroke) {
        let m = stroke.modifiers;
        let typed = stroke.key_char.as_deref();
        let (key, code, vk) = cdp_key(&stroke.key, typed);
        let modifiers = cdp_modifiers(&m);
        let base = json!({ "key": key, "code": code, "windowsVirtualKeyCode": vk, "modifiers": modifiers });
        let mut params = base.clone();
        if !down {
            params["type"] = "keyUp".into();
        } else if key == "Enter" {
            params["type"] = "keyDown".into();
            params["text"] = "\r".into();
        } else if key.chars().count() == 1 {
            // Option composes characters on macOS ("@" is ⌥L): sent as Alt, Chrome would treat it
            // as a shortcut and drop the text.
            params["type"] = "keyDown".into();
            params["modifiers"] = (modifiers & !1).into();
            params["text"] = key.clone().into();
        } else {
            params["type"] = "rawKeyDown".into();
        }
        self.send("Input.dispatchKeyEvent", params);
    }

    fn key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let m = event.keystroke.modifiers;
        if self.picking && event.keystroke.key == "escape" {
            self.toggle_pick(cx);
            cx.stop_propagation();
            return;
        }
        if m.platform {
            let k = event.keystroke.key.as_str();
            match k {
                "c" => self.copy_selection(cx),
                "v" => {
                    if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                        self.send("Input.insertText", json!({ "text": text }));
                    }
                }
                "a" | "x" | "z" => {
                    let command = match k {
                        "a" => "selectAll",
                        "x" => "cut",
                        _ if m.shift => "redo",
                        _ => "undo",
                    };
                    let (key, code, vk) = cdp_key(k, None);
                    self.send("Input.dispatchKeyEvent", json!({ "type": "rawKeyDown", "key": key, "code": code, "windowsVirtualKeyCode": vk, "modifiers": cdp_modifiers(&m), "commands": [command] }));
                }
                _ => return,
            }
            cx.stop_propagation();
            return;
        }
        self.key(true, &event.keystroke);
        cx.stop_propagation();
    }

    fn copy_selection(&self, cx: &mut Context<Self>) {
        let Some((workspace, tab)) = self.active().map(|(w, t)| (w.clone(), t.id.clone())) else { return };
        cx.spawn(async move |_, cx| {
            let text = tauri::async_runtime::spawn(async move {
                let app = backend::app();
                browser::browser_selection(app.clone(), app.state::<BrowserManager>(), workspace, tab).await
            })
            .await;
            if let Ok(Ok(text)) = text {
                if !text.is_empty() {
                    cx.update(|cx| cx.write_to_clipboard(ClipboardItem::new_string(text)));
                }
            }
        })
        .detach();
    }

    fn submit_address(&mut self, cx: &mut Context<Self>) {
        let url = normalize_url(self.address.read(cx).value().as_ref());
        self.navigate(url, cx);
    }

    /// Shows `url` in the current tab (opening the panel's browser if needed).
    pub fn navigate(&mut self, url: String, cx: &mut Context<Self>) {
        let Some((workspace, tab)) = self.active().map(|(w, t)| (w.clone(), t.id.clone())) else { return };
        cx.spawn(async move |this, cx| {
            let failed = tauri::async_runtime::spawn(async move {
                let app = backend::app();
                browser::browser_navigate(app.clone(), app.state::<BrowserManager>(), workspace, tab, url).await.map(|_| ()).err().map(|e| e.to_string())
            })
            .await
            .ok()
            .flatten();
            let _ = this.update(cx, |this, cx| {
                this.message = failed.map(Into::into);
                cx.notify();
            });
        })
        .detach();
    }

    /// Opens `url` the way `openUrl` does: the tab already on that site, the blank current tab,
    /// or a new one. Resolves to the (label, url) of a tab it took over, if any.
    pub fn open_url(&mut self, input: String, new_tab: bool, workspace: Option<String>, cx: &mut Context<Self>) -> Task<Result<Option<(String, String)>, String>> {
        let Some(workspace) = workspace.or_else(|| self.workspace.clone()) else { return Task::ready(Err("No workspace is open.".to_string())) };
        let url = normalize_url(&input);
        cx.spawn(async move |this, cx| {
            let needs_open = this.update(cx, |this, _| !this.tabs.contains_key(&workspace)).map_err(|e| e.to_string())?;
            if needs_open {
                let target = workspace.clone();
                let tabs = tauri::async_runtime::spawn(async move {
                    let app = backend::app();
                    browser::browser_open(app.clone(), app.state::<BrowserManager>(), target).await.map(|t| serde_json::to_value(t).unwrap_or(Value::Null)).map_err(|e| e.to_string())
                })
                .await
                .map_err(|e| e.to_string())??;
                let w = workspace.clone();
                this.update(cx, |this, cx| this.set_tabs(w, serde_json::from_value(tabs).unwrap_or_default(), cx)).map_err(|e| e.to_string())?;
            }
            enum Plan {
                Existing(Tab),
                Blank(String),
                New,
            }
            let u = url.clone();
            let w = workspace.clone();
            let plan = this
                .update(cx, |this, _| {
                    let tabs = this.tabs.get(&w).cloned().unwrap_or_default();
                    let current = this.active_tab.get(&w).cloned();
                    if !new_tab {
                        if let Some(tab) = related_tab(&tabs, &u) {
                            return Plan::Existing(tab.clone());
                        }
                        if let Some(tab) = tabs.iter().find(|t| Some(&t.id) == current.as_ref() && (t.url == "about:blank" || t.url.is_empty())) {
                            return Plan::Blank(tab.id.clone());
                        }
                    }
                    Plan::New
                })
                .map_err(|e| e.to_string())?;
            let (target, opened, previous) = match plan {
                Plan::Existing(tab) => (Some(tab.id.clone()), tab.url != url, Some((tab_label(&tab), tab.url.clone()))),
                Plan::Blank(id) => (Some(id), true, None),
                Plan::New => (None, false, None),
            };
            let (w, u) = (workspace.clone(), url.clone());
            let id = tauri::async_runtime::spawn(async move {
                let app = backend::app();
                match target {
                    Some(id) => {
                        if opened {
                            browser::browser_navigate(app.clone(), app.state::<BrowserManager>(), w, id.clone(), u).await.map_err(|e| e.to_string())?;
                        }
                        Ok::<String, String>(id)
                    }
                    None => browser::browser_new_tab(app.clone(), app.state::<BrowserManager>(), w, u).await.map_err(|e| e.to_string()),
                }
            })
            .await
            .map_err(|e| e.to_string())??;
            let w = workspace.clone();
            this.update(cx, |this, cx| {
                this.active_tab.insert(w, id);
                cx.notify();
            })
            .map_err(|e| e.to_string())?;
            Ok(previous)
        })
    }

    /// The tab a plugin fills a secret into: the first whose URL contains `tab`, else the active one.
    pub fn fill_target(&self, workspace: &str, tab: Option<&str>) -> Option<(String, String)> {
        let tabs = self.tabs.get(workspace)?;
        let found = match tab {
            Some(t) => tabs.iter().find(|x| x.url.contains(t)),
            None => self.active_tab.get(workspace).and_then(|id| tabs.iter().find(|x| x.id == *id)).or(tabs.first()),
        };
        found.map(|x| (x.id.clone(), x.url.clone()))
    }

    /// The URL shown in a workspace's active tab.
    pub fn active_url(&self, workspace: &str) -> Option<String> {
        let id = self.active_tab.get(workspace)?;
        Some(self.tabs.get(workspace)?.iter().find(|t| t.id == *id)?.url.clone())
    }

    pub fn close_current(&self) {
        if let Some(id) = self.active().map(|(_, t)| t.id.clone()) {
            self.close_tab(id);
        }
    }

    /// Back, forward, or (0) reload.
    pub fn go(&mut self, delta: i64, cx: &mut Context<Self>) {
        if delta == 0 {
            if let Some(url) = self.active().map(|(_, t)| t.url.clone()) {
                self.navigate(url, cx);
            }
        } else {
            self.history(delta);
        }
    }

    pub fn focus_address(&self, window: &mut Window, cx: &mut App) {
        self.address.read(cx).focus_handle(cx).focus(window, cx);
    }

    fn history(&self, delta: i64) {
        let Some((workspace, tab)) = self.active().map(|(w, t)| (w.clone(), t.id.clone())) else { return };
        tauri::async_runtime::spawn(async move {
            let app = backend::app();
            let _ = browser::browser_history(app.clone(), app.state::<BrowserManager>(), workspace, tab, delta).await;
        });
    }

    pub fn new_tab(&self) {
        let Some(workspace) = self.workspace.clone() else { return };
        tauri::async_runtime::spawn(async move {
            let app = backend::app();
            let _ = browser::browser_new_tab(app.clone(), app.state::<BrowserManager>(), workspace, "about:blank".to_string()).await;
        });
    }

    fn close_tab(&self, tab: String) {
        let Some(workspace) = self.workspace.clone() else { return };
        tauri::async_runtime::spawn(async move {
            let app = backend::app();
            let _ = browser::browser_close_tab(app.clone(), app.state::<BrowserManager>(), workspace, tab).await;
        });
    }

    fn update_address(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let current = self.active().map(|(w, t)| (t.id.clone(), t.url.clone(), w.clone()));
        let Some((id, url, _)) = current else { return };
        let key = (id, url.clone());
        if self.address_for.as_ref() != Some(&key) && !self.address.read(cx).focus_handle(cx).is_focused(window) {
            self.address_for = Some(key);
            self.address.update(cx, |input, cx| input.set_value(if url == "about:blank" { String::new() } else { url }, window, cx));
        }
    }

    /// Browser-style tabs with darker tab surfaces and subtle elevation shadows; tabs an agent is
    /// using carry a small agent badge next to their page icon.
    fn render_tabs(&mut self, c: Colors, dark: bool, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let mut strip = div().flex().items_end().gap_1().px_2().pt_2();
        let workspace = self.workspace.clone().unwrap_or_default();
        let active = self.active_tab.get(&workspace).cloned();
        let now = Instant::now();
        let active_bg = c.raised; // same surface as the toolbar below, so the tab joins it
        let idle_bg = if c.light { theme::blend(c.panel, c.fg, 0.04) } else { theme::blend(c.panel, black(), 0.18) };
        let idle_hover_bg = if c.light { theme::blend(c.panel, c.fg, 0.068) } else { theme::blend(c.panel, black(), 0.26) };
        let idle_shadow = if c.light {
            vec![BoxShadow { color: hsla(0.0, 0.0, 0.0, 0.05), offset: point(px(0.0), px(1.0)), blur_radius: px(2.0), spread_radius: px(0.0), inset: false }]
        } else {
            vec![BoxShadow { color: hsla(0.0, 0.0, 0.0, 0.24), offset: point(px(0.0), px(1.0)), blur_radius: px(3.0), spread_radius: px(0.0), inset: false }]
        };
        for tab in self.tabs.get(&workspace).cloned().unwrap_or_default() {
            let is_active = active.as_ref() == Some(&tab.id);
            let agent = self.agent_tabs.get(&tab.id).is_some_and(|at| now.duration_since(*at) < AGENT_LINGER);
            let icon: AnyElement = if let Some(image) = self.favicon(&tab, dark) {
                img(image).size(px(14.0)).flex_shrink_0().rounded(px(3.0)).into_any_element()
            } else {
                Icon::new(IconName::Globe).size(px(13.0)).text_color(c.subtle).into_any_element()
            };
            let (select, close, ws, middle) = (tab.id.clone(), tab.id.clone(), workspace.clone(), tab.id.clone());
            let group = SharedString::from(format!("tab-group-{}", tab.id));
            strip = strip.child(
                div().id(SharedString::from(format!("tab-{}", tab.id))).group(group.clone()).flex().items_center().gap(px(7.0)).min_w_0().flex_basis(px(160.0)).flex_shrink(1.0)
                    .h(px(30.0)).pl_2p5().pr_1p5().rounded_tl_lg().rounded_tr_lg().rounded_bl(px(0.0)).rounded_br(px(0.0)).text_size(px(12.0)).cursor_pointer()
                    .when(is_active, |d| d.bg(active_bg).font_weight(FontWeight::MEDIUM).text_color(c.fg))
                    .when(!is_active, |d| d.bg(idle_bg).shadow(idle_shadow.clone()).text_color(c.muted).hover(move |d| d.bg(idle_hover_bg).text_color(c.fg)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.active_tab.insert(ws.clone(), select.clone());
                        cx.notify();
                    }))
                    // Middle click closes, like any browser.
                    .on_mouse_up(MouseButton::Middle, cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.close_tab(middle.clone());
                    }))
                    .child(icon)
                    .child(div().flex_1().min_w_0().truncate().child(tab_label(&tab)))
                    .when(agent, |d| d.child(div().size(px(16.0)).flex_shrink_0().flex().items_center().justify_center().rounded_full().bg(Hsla { a: 0.16, ..c.accent })
                        .child(Icon::new(IconName::Bot).size(px(10.0)).text_color(c.accent))))
                    .child(
                        div().id(SharedString::from(format!("tab-close-{}", tab.id))).size(px(16.0)).flex_shrink_0().flex().items_center().justify_center().rounded_sm().cursor_pointer()
                            .when(!is_active, |d| d.invisible().group_hover(group.clone(), |d| d.visible()))
                            .hover(move |d| d.bg(c.active))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.close_tab(close.clone());
                            }))
                            .child(Icon::new(IconName::X).size(px(11.0)).text_color(c.subtle)),
                    ),
            );
        }
        strip.child(
            div().id("new-tab").size(px(28.0)).ml_0p5().mb_0p5().flex_shrink_0().flex().items_center().justify_center().rounded_md().cursor_pointer().text_color(c.subtle)
                .hover(move |d| d.bg(c.hover).text_color(c.fg))
                .tooltip(|window, cx| Tooltip::new("New tab").build(window, cx))
                .on_click(cx.listener(|this, _, _, _| this.new_tab()))
                .child(Icon::new(IconName::Plus).size(px(14.0))),
        )
    }

    /// Back, forward, reload; one rounded address field (connection, URL, certificates); the
    /// picker, the console with its error count, and the device toggle.
    fn render_toolbar(&self, c: Colors, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let button = move |id: &'static str, icon: IconName, on: bool| {
            div().id(id).relative().size(px(28.0)).flex_shrink_0().flex().items_center().justify_center().rounded(px(7.0)).cursor_pointer()
                .text_color(if on { c.accent } else { c.muted }).when(on, |d| d.bg(Hsla { a: 0.14, ..c.accent }))
                .hover(move |d| d.bg(c.hover).text_color(c.fg)).child(Icon::new(icon).size(px(15.0)))
        };
        let tip = |text: &'static str| move |window: &mut Window, cx: &mut App| Tooltip::new(text).build(window, cx);
        let mobile = self.is_mobile();
        let certs_off = self.workspace.as_ref().is_some_and(|w| self.invalid_certs.contains(w));
        let sized = self.active().is_some_and(|(_, t)| self.sizes.contains_key(&t.id));
        let errors = self.active().map_or(0, |(_, t)| self.console.get(&t.id).map_or(0, |l| l.iter().filter(|e| e.level == "error").count()));
        let url = self.active().map(|(_, t)| t.url.clone()).unwrap_or_default();
        let local = url.contains("://localhost") || url.contains("://127.0.0.1") || url.contains("://0.0.0.0");
        let (sec_icon, sec_color, sec_tip) = if url.starts_with("https://") {
            (IconName::Lock, c.success, "Secure connection")
        } else if local {
            (IconName::Monitor, c.subtle, "Local dev server")
        } else if url.starts_with("http://") {
            (IconName::TriangleAlert, c.warning, "Not secure (HTTP)")
        } else {
            (IconName::Search, c.subtle, "Search or enter an address")
        };
        let omni = div().flex_1().min_w_0().h(px(30.0)).flex().items_center().gap_1p5().pl_2().pr_1().rounded_full().bg(c.hover)
            .hover(move |d| d)
            .child(div().id("connection").size(px(20.0)).flex_shrink_0().flex().items_center().justify_center().rounded_full().text_color(sec_color)
                .tooltip(tip(sec_tip)).child(Icon::new(sec_icon).size(px(12.0))))
            .child(div().flex_1().min_w_0().child(Input::new(&self.address).appearance(false).small()))
            .child(
                div().id("tls").flex_shrink_0().rounded_full().px_1p5().py_0p5().text_size(px(10.0)).cursor_pointer()
                    .text_color(if certs_off { c.danger } else { c.subtle }).when(certs_off, |d| d.bg(Hsla { a: 0.14, ..c.danger }))
                    .hover(move |d| d.bg(c.active))
                    .tooltip(tip("Allow invalid HTTPS certificates…"))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.cert_prompt = !this.cert_prompt;
                        cx.notify();
                    }))
                    .child("TLS"),
            );
        div()
            .flex()
            .items_center()
            .gap_1()
            .px_2()
            .py(px(7.0))
            .bg(c.raised)
            .child(button("back", IconName::ArrowLeft, false).tooltip(tip("Back")).on_click(cx.listener(|this, _, _, _| this.history(-1))))
            .child(button("forward", IconName::ArrowRight, false).tooltip(tip("Forward")).on_click(cx.listener(|this, _, _, _| this.history(1))))
            .child(button("reload", IconName::RotateCw, false).tooltip(tip("Reload")).on_click(cx.listener(|this, _, _, cx| {
                if let Some(url) = this.active().map(|(_, t)| t.url.clone()) {
                    this.navigate(url, cx);
                }
            })))
            .child(omni)
            .child(button("pick", IconName::MousePointer2, self.picking).tooltip(tip("Pick an element and send it to the agent")).on_click(cx.listener(|this, _, _, cx| this.toggle_pick(cx))))
            .child(button("console", IconName::Code, self.console_open).tooltip(tip("Console and network errors")).on_click(cx.listener(|this, _, _, cx| {
                this.console_open = !this.console_open;
                cx.notify();
            }))
            .when(errors > 0, |d| d.child(div().absolute().top(px(1.0)).right(px(0.0)).min_w(px(14.0)).h(px(14.0)).px(px(3.0)).flex().items_center().justify_center().rounded_full().bg(c.danger)
                .text_size(px(9.5)).font_weight(FontWeight::SEMIBOLD).text_color(gpui_kit::white()).child(errors.min(99).to_string()))))
            .when(sized || mobile, |d| d.child(button("fit", IconName::Proportions, false).tooltip(tip(if mobile { "Back to the phone size" } else { "Fit the page to the panel" })).on_click(cx.listener(|this, _, _, cx| this.fit_page(cx)))))
            .child(button("device", if mobile { IconName::Smartphone } else { IconName::Monitor }, mobile).tooltip(tip(if mobile { "Desktop view" } else { "Mobile view" })).on_click(cx.listener(move |this, _, _, cx| this.set_device(!mobile, cx))))
    }

    /// "An agent is driving this tab", while one used it in the last few seconds.
    fn render_agent_bar(&self, c: Colors) -> Option<AnyElement> {
        let (_, tab) = self.active()?;
        let at = self.agent_tabs.get(&tab.id)?;
        (Instant::now().duration_since(*at) < AGENT_LINGER).then(|| {
            div().flex().items_center().gap_2().px_3().py_1p5().text_size(px(12.0)).text_color(c.muted)
                .bg(Hsla { a: 0.08, ..c.accent })
                .child(Icon::new(IconName::Bot).size(px(13.0)).text_color(c.accent))
                .child(div().flex_1().truncate().child(div().text_color(c.fg).child("An agent is using this tab")))
                .child(div().text_size(px(11.0)).text_color(c.subtle).child("its cursor shows on the page"))
                .into_any_element()
        })
    }

    fn render_console(&self, c: Colors, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (_, tab) = self.active()?;
        let entries = self.console.get(&tab.id).cloned().unwrap_or_default();
        let mono = crate::fonts::mono(cx);
        let mut list = div().id("console-list").flex_1().min_h_0().overflow_y_scroll().px_2p5().pb_2().font_family(mono).text_size(px(11.0)).flex().flex_col();
        if entries.is_empty() {
            list = list.child(div().text_color(c.subtle).child("Nothing logged yet."));
        }
        for e in &entries {
            let color = match e.level.as_str() { "error" => c.danger, "warning" => c.warning, "info" => c.accent, _ => c.muted };
            list = list.child(div().flex().gap_2().py_0p5()
                .child(div().w(px(48.0)).flex_shrink_0().text_color(color).child(e.level.clone()))
                .child(div().flex_1().min_w_0().whitespace_normal().text_color(c.fg).child(e.text.clone()))
                .when_some(e.source.as_ref().map(|s| s.rsplit('/').next().unwrap_or(s).to_string()), |d, s| d.child(div().max_w(px(120.0)).flex_shrink_0().truncate().text_color(c.subtle).child(s))));
        }
        let small = |id: &'static str, icon: IconName, label: &'static str| div().id(id).flex().items_center().gap_1().rounded_sm().px_1p5().py_0p5().cursor_pointer().hover(move |d| d.bg(c.hover).text_color(c.fg)).child(Icon::new(icon).size(px(11.0))).child(label);
        Some(div().h(px(190.0)).flex_shrink_0().flex().flex_col().bg(c.panel)
            .child(div().flex().items_center().gap_2().px_2p5().py_1p5().text_size(px(11.0)).text_color(c.subtle)
                .child(div().flex_1().child(format!("Console · {} entries", entries.len())))
                .child(small("console-send", IconName::Send, "Send errors to agent").on_click(cx.listener(|this, _, _, cx| this.send_console(cx))))
                .child(small("console-clear", IconName::Trash, "").on_click(cx.listener(|this, _, _, cx| this.clear_console(cx)))))
            .child(list).into_any_element())
    }

    /// The handles on the page's right and bottom edges, to drag it to another size.
    fn render_handles(&self, rect: Bounds<Pixels>, c: Colors, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let Some(own) = self.own_size() else { return Vec::new() };
        let origin = self.panel_bounds.origin;
        let (x, y, w, h) = (rect.origin.x - origin.x, rect.origin.y - origin.y, rect.size.width, rect.size.height);
        let scale = if own.0 > 0 { (w / px(1.0)) / own.0 as f32 } else { 1.0 };
        let handle = |id: &'static str, left: Pixels, top: Pixels, width: Pixels, height: Pixels, cursor: CursorStyle, dx: bool, dy: bool| {
            div().id(id).absolute().left(left).top(top).w(width).h(height).rounded_full().bg(c.border_strong).opacity(0.7).hover(move |d| d.bg(c.accent).opacity(1.0)).cursor(cursor)
                .on_mouse_down(MouseButton::Left, cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    if e.click_count >= 2 {
                        this.fit_page(cx);
                        return;
                    }
                    if let Some(own) = this.own_size() {
                        this.size_drag = Some(SizeDrag { x: dx, y: dy, start: e.position, from: own, scale: scale.max(0.01) });
                    }
                }))
                .into_any_element()
        };
        vec![
            handle("page-handle-x", x + w + px(5.0), y + h / 2.0 - px(20.0), px(5.0), px(40.0), CursorStyle::ResizeLeftRight, true, false),
            handle("page-handle-y", x + w / 2.0 - px(20.0), y + h + px(5.0), px(40.0), px(5.0), CursorStyle::ResizeUpDown, false, true),
            handle("page-handle-xy", x + w + px(5.0), y + h + px(5.0), px(9.0), px(9.0), CursorStyle::ResizeUpLeftDownRight, true, true),
        ]
    }

    /// The agent's cursor and click ripples over the page.
    fn render_cursor(&self, rect: Bounds<Pixels>, c: Colors) -> Option<AnyElement> {
        let (_, tab) = self.active()?;
        let cursor = self.cursors.get(&tab.id).filter(|k| k.at.elapsed() < CURSOR_IDLE)?;
        let a = self.attachment.as_ref()?;
        let origin = self.panel_bounds.origin;
        let at = |x: f32, y: f32| (rect.origin.x - origin.x + rect.size.width * (x / a.width.max(1) as f32), rect.origin.y - origin.y + rect.size.height * (y / a.height.max(1) as f32));
        let (x, y) = at(cursor.x, cursor.y);
        let ripple = cursor.pressed.filter(|p| p.elapsed() < Duration::from_millis(900)).map(|p| {
            div().absolute().left(x - px(14.0)).top(y - px(14.0)).size(px(28.0)).rounded_full().border_2().border_color(c.accent)
                .with_animation(SharedString::from(format!("ripple-{}", p.elapsed().as_nanos() / 1_000_000_000u128)), Animation::new(Duration::from_millis(700)), |d, t| d.opacity(1.0 - t))
        });
        Some(div().absolute().size_full().children(ripple).child(div().absolute().left(x).top(y).child(Icon::new(IconName::MousePointer2).size(px(20.0)).text_color(c.accent))).into_any_element())
    }
}

impl Focusable for BrowserPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for BrowserPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (theme_id, workspace) = {
            let s = self.state.read(cx);
            (s.theme.clone(), s.active().map(|w| w.id.clone()))
        };
        let c = theme::colors(&theme_id);
        if self.workspace != workspace {
            self.workspace = workspace.clone();
        }
        if let Some(id) = workspace.filter(|_| !self.needs_install && !self.installing) {
            self.open_workspace(id, cx);
        }
        self.update_address(window, cx);
        if let Some(next) = self.latest.lock().take() {
            if let Some(old) = self.shown.replace(next) {
                cx.drop_image(old, Some(window));
            }
        }
        let image = self.shown.clone();
        let own = self.own_size();
        let this = cx.entity();
        let has_tab = self.active().is_some();
        let scale = window.scale_factor() as f64;
        let rect = page_rect(self.panel_bounds, own);
        let handles = self.render_handles(rect, c, cx);
        let cursor = self.render_cursor(rect, c);
        let dark = theme::theme_by_id(&theme_id).mode != "light";
        let console = if self.console_open { self.render_console(c, cx) } else { None };
        let notice = self.notice.as_ref().filter(|(_, at)| at.elapsed() < Duration::from_secs(4)).map(|(t, _)| t.clone());
        let picking = self.picking;
        let certs_off = self.workspace.as_ref().is_some_and(|w| self.invalid_certs.contains(w));
        let body = if self.needs_install {
            div().size_full().flex().flex_col().items_center().justify_center().gap_2().text_xs().text_color(c.muted)
                .child("The browser panel needs Chromium.")
                .child(
                    div().id("install-browser").rounded_md().bg(c.accent).text_color(c.on_accent).px_3().py_1().cursor_pointer()
                        .on_click(cx.listener(|this, _, _, cx| this.install(cx)))
                        .child("Download browser"),
                )
                .into_any_element()
        } else {
            div()
                .id("browser-page")
                .size_full()
                .relative()
                .track_focus(&self.focus)
                .key_context("Browser")
                .on_key_down(cx.listener(Self::key_down))
                .on_key_up(cx.listener(|this, e: &KeyUpEvent, _, _| {
                    if !e.keystroke.modifiers.platform {
                        this.key(false, &e.keystroke);
                    }
                }))
                .on_mouse_down(MouseButton::Left, cx.listener(|this, e: &MouseDownEvent, window, cx| {
                    this.focus.focus(window, cx);
                    if !this.bounds.contains(&e.position) {
                        return;
                    }
                    this.buttons |= 1;
                    this.last_click = e.click_count as u32;
                    this.mouse("mousePressed", e.position, "left", e.click_count as u32, &e.modifiers);
                }))
                .on_mouse_up(MouseButton::Left, cx.listener(|this, e: &MouseUpEvent, _, _| {
                    if this.size_drag.take().is_some() {
                        return;
                    }
                    this.buttons &= !1;
                    let clicks = this.last_click;
                    this.mouse("mouseReleased", e.position, "left", clicks, &e.modifiers);
                }))
                .on_mouse_down(MouseButton::Right, cx.listener(|this, e: &MouseDownEvent, _, _| {
                    this.buttons |= 2;
                    this.mouse("mousePressed", e.position, "right", 1, &e.modifiers);
                }))
                .on_mouse_up(MouseButton::Right, cx.listener(|this, e: &MouseUpEvent, _, _| {
                    this.buttons &= !2;
                    this.mouse("mouseReleased", e.position, "right", 1, &e.modifiers);
                }))
                .on_mouse_move(cx.listener(|this, e: &MouseMoveEvent, _, cx| {
                    if let Some(d) = this.size_drag {
                        let delta = e.position - d.start;
                        let (dx, dy) = (delta.x / px(1.0) / d.scale, delta.y / px(1.0) / d.scale);
                        let w = if d.x { (d.from.0 as f32 + dx).round().clamp(MIN_PAGE as f32, MAX_PAGE as f32) as u32 } else { d.from.0 };
                        let h = if d.y { (d.from.1 as f32 + dy).round().clamp(MIN_PAGE as f32, MAX_PAGE as f32) as u32 } else { d.from.1 };
                        if let Some((_, tab)) = this.active() {
                            let id = tab.id.clone();
                            this.sizes.insert(id, (w, h));
                        }
                        cx.notify();
                        return;
                    }
                    let button = if this.buttons & 1 != 0 { "left" } else { "none" };
                    this.mouse("mouseMoved", e.position, button, 0, &e.modifiers);
                }))
                .on_scroll(cx.listener(|this, e: &ScrollEvent, _, _| {
                    let delta = e.delta.pixel_delta(px(16.0));
                    let (x, y) = this.page_point(e.position);
                    // Natural-scroll sign: the page moves with the fingers.
                    this.send("Input.dispatchMouseEvent", json!({ "type": "mouseWheel", "x": x, "y": y, "deltaX": -(delta.x / px(1.0)), "deltaY": -(delta.y / px(1.0)), "modifiers": cdp_modifiers(&e.modifiers) }));
                }))
                .child(
                    canvas(
                        move |bounds, _, cx| {
                            this.update(cx, |this, cx| {
                                this.sync(bounds, scale, cx);
                            });
                        },
                        move |bounds, _, window, _| {
                            if let Some(image) = image {
                                let rect = page_rect(bounds, own);
                                let _ = window.paint_image(rect, rect, Corners::default(), image, 0, false);
                            }
                        },
                    )
                    .size_full(),
                )
                .children(handles)
                .children(cursor)
                .when_some(self.message.clone().filter(|_| !has_tab), |d, text| {
                    d.child(div().absolute().size_full().flex().items_center().justify_center().text_xs().text_color(c.muted).child(text))
                })
                .into_any_element()
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(self.render_tabs(c, dark, cx))
            .child(self.render_toolbar(c, cx))
            .children(self.render_agent_bar(c))
            .when(certs_off || self.cert_prompt, |d| d.child(
                div().flex_shrink_0().px_3().py_2().flex().flex_wrap().items_center().gap_2().text_size(px(11.0)).text_color(c.danger).bg(Hsla { a: 0.1, ..c.danger })
                    .child(if certs_off { "HTTPS certificate checks are off for all tabs in this workspace's browser for this browser session." } else { "Allow untrusted, expired or self-signed HTTPS certificates for all tabs in this workspace's browser? Only use this for sites you trust." })
                    .child(
                        div().id("tls-change").underline().cursor_pointer().when(self.cert_busy, |d| d.opacity(0.5))
                            .on_click(cx.listener(move |this, _, _, cx| if !this.cert_busy { this.set_invalid_certs(!certs_off, cx) }))
                            .child(if certs_off { "Enable certificate checks" } else { "Allow invalid certificates" }),
                    )
                    .when(!certs_off, |d| d.child(div().id("tls-cancel").underline().cursor_pointer().on_click(cx.listener(|this, _, _, cx| {
                        this.cert_prompt = false;
                        cx.notify();
                    })).child("Cancel")))
                    .child("Reload the page after changing this setting."),
            ))
            .when(picking, |d| d.child(div().flex_shrink_0().px_3().py_1().text_size(px(11.0)).text_color(c.accent).bg(Hsla { a: 0.12, ..c.accent }).child("Click an element to send it to the agent · Esc to cancel")))
            .when_some(notice, |d, t| d.child(div().flex_shrink_0().px_3().py_1().text_size(px(11.0)).text_color(c.fg).bg(c.raised).child(t)))
            .children(console)
            .child(div().flex_1().min_h_0().relative().bg(if own.is_some() { c.app } else { hsla(0.0, 0.0, 1.0, 1.0) }).child(body))
    }
}

#[cfg(test)]
mod tests {
    use super::{Tab, cdp_key, favicon_image, normalize_url, page_rect, related_tab};
    use gpui_kit::{Bounds, point, px, size};

    #[test]
    fn related_tabs_match_by_site() {
        let tab = |id: &str, url: &str| Tab { id: id.into(), url: url.into(), title: String::new(), favicon: String::new() };
        let tabs = vec![tab("a", "http://127.0.0.1:5173/x"), tab("b", "https://example.com/docs/a"), tab("c", "https://www.example.com/docs/b/c")];
        assert_eq!(related_tab(&tabs, "http://localhost:5173/y").unwrap().id, "a");
        assert_eq!(related_tab(&tabs, "https://example.com/docs/b/d").unwrap().id, "c");
        assert!(related_tab(&tabs, "https://other.com/").is_none());
    }

    #[test]
    fn urls_are_normalized() {
        assert_eq!(normalize_url("localhost:5173/x"), "http://localhost:5173/x");
        assert_eq!(normalize_url("example.com"), "https://example.com");
        assert_eq!(normalize_url("https://a.b/c"), "https://a.b/c");
        assert_eq!(normalize_url("about:blank"), "about:blank");
        assert_eq!(normalize_url("two words"), "https://www.google.com/search?q=two%20words");
    }

    #[test]
    fn keys_map_to_physical_codes() {
        assert_eq!(cdp_key("left", None), ("ArrowLeft".to_string(), "ArrowLeft".to_string(), 37));
        assert_eq!(cdp_key("a", Some("A")), ("A".to_string(), "KeyA".to_string(), 65));
        assert_eq!(cdp_key(".", Some(".")), (".".to_string(), "Period".to_string(), 190));
    }

    #[test]
    fn a_sized_page_fits_the_panel_without_growing() {
        let panel = Bounds::new(point(px(100.0), px(50.0)), size(px(600.0), px(800.0)));
        assert_eq!(page_rect(panel, None), panel);
        // A phone fits whole: nothing is scaled up, and it's centered.
        let phone = page_rect(panel, Some((390, 700)));
        assert_eq!(phone.size, size(px(390.0), px(700.0)));
        assert_eq!(phone.origin.x, px(100.0 + (600.0 - 390.0) / 2.0));
        // A wide page shrinks to the room it has.
        let wide = page_rect(panel, Some((1200, 800)));
        assert!(wide.size.width < px(600.0) && wide.size.width > px(500.0));
        assert!((wide.size.width / wide.size.height - 1.5).abs() < 0.01);
    }

    #[test]
    fn favicons_decode_from_data_urls() {
        use base64::Engine;
        let mut png = std::io::Cursor::new(Vec::new());
        image::RgbaImage::from_pixel(2, 2, image::Rgba([0, 0, 0, 255])).write_to(&mut png, image::ImageFormat::Png).unwrap();
        let url = format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(png.into_inner()));
        assert!(favicon_image(&url, true).is_some());
        assert!(favicon_image("data:image/png;base64,AAAA", false).is_none());
        assert!(favicon_image("nope", false).is_none());
    }
}
