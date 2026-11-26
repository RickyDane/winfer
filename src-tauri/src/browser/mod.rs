//! Embedded browser: one headless Chromium for the whole app, in which each workspace has its own
//! browser context (its own tabs and cookies). Shown in the panel via CDP screencast and shared
//! with the workspace's agents through a proxy that keeps them to its context (`proxy`).
//!
//! Contexts live in memory only, so each workspace's cookies are saved and put back (`cookies`).
//!
//! WKWebView has no CDP, which browser-use / Playwright need, hence a real Chromium.

mod cdp;
pub mod chrome;
mod cookies;
pub mod inspect;
pub mod log;
mod proxy;

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::process::Child;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};

use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::{AppHandle, Manager, State};
use tokio::sync::OnceCell;

use cdp::{Cdp, Event};
use chrome::Executable;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Tab {
    id: String,
    url: String,
    title: String,
    /// The page's icon URL, read from the page when it loads ("" until then, or if it has none).
    favicon: String,
}

/// Icons fetched so far, by URL: their data URL, or None if there was none to get.
static FAVICONS: std::sync::LazyLock<Mutex<HashMap<String, Option<String>>>> = std::sync::LazyLock::new(Default::default);

/// Fetches an icon (once per URL) as a data URL; None if it's missing, too big or not an image.
fn favicon_data_url(url: &str) -> Option<String> {
    if let Some(known) = FAVICONS.lock().unwrap().get(url) {
        return known.clone();
    }
    let fetched = fetch_favicon(url);
    if fetched.is_none() {
        log::info!("no icon at {url}");
    }
    FAVICONS.lock().unwrap().insert(url.to_string(), fetched.clone());
    fetched
}

fn fetch_favicon(url: &str) -> Option<String> {
    const MAX_BYTES: u64 = 512 * 1024;
    if let Some(inline) = url.strip_prefix("data:") {
        return inline.starts_with("image/").then(|| url.to_string());
    }
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return None;
    }
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(10)))
        .build()
        .into();
    let mut res = agent.get(url).header("User-Agent", "winfer").call().ok()?;
    let mime = res
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(';').next())
        .map(|v| v.trim().to_ascii_lowercase())
        .filter(|v| v.starts_with("image/"))
        .or_else(|| url.split('?').next()?.ends_with(".ico").then(|| "image/x-icon".to_string()))?;
    let bytes = res.body_mut().with_config().limit(MAX_BYTES).read_to_vec().ok()?;
    if bytes.is_empty() {
        return None;
    }
    Some(format!("data:{mime};base64,{}", base64::engine::general_purpose::STANDARD.encode(bytes)))
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct TabsEvent {
    workspace_id: String,
    tabs: Vec<Tab>,
}

struct Screencast {
    /// Token from the view that started it, so a stale detach can't stop a newer screencast.
    attach_id: String,
    target_id: String,
    session_id: String,
    channel: Channel,
    sizing: Sizing,
    /// A frame arrived; until then the panel shows nothing (frames only come when the page paints).
    got_frame: bool,
    /// Frames sent to the view, and how many of them Chrome got an ack for. Chrome stops
    /// capturing while a few frames are unacked, so acking once the view drew (or skipped) a frame
    /// paces the screencast to what the view can show instead of the page's paint rate.
    frames: u64,
    acked: u64,
    /// The ack's `sessionId` (the screencast's, the same for all its frames).
    ack_id: Value,
}

/// A frame the view never reports done (it went away mid-frame) is acked after this, so the
/// screencast can't stall.
const FRAME_ACK_FALLBACK: std::time::Duration = std::time::Duration::from_secs(1);

/// An agent acted on a tab (see `proxy`).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct AgentEvent {
    workspace_id: String,
    target_id: String,
    /// The tab wasn't opened by an agent: it took over one that was already open.
    adopted: bool,
}

/// An agent's mouse event in a tab (see `proxy::Pointer`).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct PointerEvent {
    workspace_id: String,
    target_id: String,
    #[serde(flatten)]
    pointer: proxy::Pointer,
}

/// Agent activity is reported at most this often per tab.
const AGENT_EVERY: std::time::Duration = std::time::Duration::from_millis(500);

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ViewportEvent {
    workspace_id: String,
    target_id: String,
    width: u32,
    height: u32,
}

/// How long frames may still show a size we replaced (frames lag behind a resize).
const STALE_SIZES: std::time::Duration = std::time::Duration::from_millis(1500);
/// How often to check that emulation is still on (Playwright's disconnect clears it).
const CHECK_EVERY: std::time::Duration = std::time::Duration::from_secs(1);

/// The shown tab's page size. Agents (Playwright) resize the page with their own emulation:
/// the panel adopts their size rather than fighting it, and keeps its scale and mobile flags.
struct Sizing {
    viewport: Viewport,
    /// Sizes recently replaced, whose in-flight frames mustn't count as a resize.
    replaced: Vec<(u32, u32, std::time::Instant)>,
    last_check: Option<std::time::Instant>,
}

#[derive(Debug, PartialEq)]
enum FrameAction {
    None,
    /// Something else resized the page: show it at this size.
    Adopt(u32, u32),
    /// Time to check our emulation is still applied.
    Check,
}

impl Sizing {
    fn new(viewport: Viewport) -> Self {
        Self {
            viewport,
            replaced: Vec::new(),
            last_check: None,
        }
    }

    fn set(&mut self, viewport: Viewport) {
        let now = std::time::Instant::now();
        self.replaced.retain(|(_, _, at)| at.elapsed() < STALE_SIZES);
        self.replaced
            .push((self.viewport.width, self.viewport.height, now));
        self.viewport = viewport;
    }

    /// What to do about a screencast frame with this `metadata`.
    fn on_frame(&mut self, metadata: &Value) -> FrameAction {
        let (Some(w), Some(h)) = (
            metadata["deviceWidth"].as_f64(),
            metadata["deviceHeight"].as_f64(),
        ) else {
            return FrameAction::None;
        };
        let (w, h) = (w.round() as u32, h.round() as u32);
        let near = |a: u32, b: u32| a.abs_diff(b) <= 1;
        if near(w, self.viewport.width) && near(h, self.viewport.height) {
            if self.last_check.is_some_and(|t| t.elapsed() < CHECK_EVERY) {
                return FrameAction::None;
            }
            self.last_check = Some(std::time::Instant::now());
            return FrameAction::Check;
        }
        // A screenshot (the panel's first picture of a tab, an agent's, the element picker's)
        // makes Chrome report a frame or two at the page's size in device pixels. Taken for a
        // resize, it doubled the page on a 2x display each time a tab was shown.
        let scale = self.viewport.device_scale_factor;
        let in_device_pixels = |rw: u32, rh: u32| {
            scale > 1.0 && near(w, (rw as f64 * scale).round() as u32) && near(h, (rh as f64 * scale).round() as u32)
        };
        let stale = self.replaced.iter().any(|&(rw, rh, at)| {
            at.elapsed() < STALE_SIZES && ((near(w, rw) && near(h, rh)) || in_device_pixels(rw, rh))
        });
        if stale || in_device_pixels(self.viewport.width, self.viewport.height) || w == 0 || h == 0 {
            return FrameAction::None;
        }
        self.set(Viewport {
            width: w,
            height: h,
            ..self.viewport.clone()
        });
        FrameAction::Adopt(w, h)
    }
}

/// A workspace's part of the shared browser. Lives as long as the app, so the proxy URL its
/// agents got stays valid when the browser restarts.
struct Instance {
    workspace_id: String,
    app: AppHandle,
    /// The workspace's browser context; made again when the browser restarted.
    context: Mutex<Option<String>>,
    /// The cookies saved last, to skip unchanged saves.
    saved_cookies: Mutex<Option<Vec<Value>>>,
    /// The DevTools connection; replaced when it drops or the browser restarted.
    cdp: Mutex<Option<Arc<Cdp>>>,
    /// One (re)connect at a time.
    connecting: tokio::sync::Mutex<()>,
    /// Page targets in creation order.
    tabs: Mutex<Vec<Tab>>,
    sessions: Mutex<HashMap<String, String>>,
    /// Opt-in for this app run only, never for the app's own network requests. Applied to each
    /// of the workspace's tabs (the browser-wide switch would cover other workspaces too).
    ignore_certificate_errors: AtomicBool,
    /// One attach per target even when the UI and target discovery race for it.
    attaching: Mutex<HashMap<String, Arc<OnceCell<String>>>>,
    screencast: Mutex<Option<Screencast>>,
    /// Serializes attach/detach so a slow attach can't replace the screencast a newer one started.
    attach_lock: tokio::sync::Mutex<()>,
    /// The attach the panel wants now; older attaches still waiting on the lock give up.
    wanted_attach: Mutex<Option<String>>,
    console: Mutex<HashMap<String, VecDeque<inspect::ConsoleEntry>>>,
    /// Agents' DevTools endpoint: a proxy that keeps them to the context and sees which tab
    /// they act on.
    proxy: proxy::Proxy,
    /// When an agent last acted on each tab, to throttle `browser://agent`.
    agent_seen: Mutex<HashMap<String, std::time::Instant>>,
    /// Tabs agents opened themselves; acting on any other tab takes it over.
    agent_opened: Mutex<HashSet<String>>,
}

impl Instance {
    /// Whether a target of `info` is in this workspace (pages of other workspaces and of review
    /// screenshots' throwaway contexts aren't).
    fn owns(&self, info: &Value) -> bool {
        let context = self.context.lock().unwrap();
        context.is_some() && info["browserContextId"].as_str() == context.as_deref()
    }

    fn has_tab(&self, id: &str) -> bool {
        self.tabs.lock().unwrap().iter().any(|t| t.id == id)
    }

    fn emit_tabs(&self) {
        let tabs = self.tabs.lock().unwrap().clone();
        let _ = crate::diagnostics::emit_logged(
            &self.app,
            "browser://tabs",
            TabsEvent {
                workspace_id: self.workspace_id.clone(),
                tabs,
            },
        );
    }

    fn on_event(self: &Arc<Self>, event: Event) {
        match event.method.as_str() {
            "Target.targetCreated" | "Target.targetInfoChanged" => {
                let info = &event.params["targetInfo"];
                if info["type"] != "page" || !self.owns(info) {
                    return;
                }
                let id = info["targetId"].as_str().unwrap_or_default().to_string();
                let mut tab = Tab {
                    url: info["url"].as_str().unwrap_or_default().into(),
                    title: info["title"].as_str().unwrap_or_default().into(),
                    favicon: String::new(),
                    id: id.clone(),
                };
                // Attach to every page right away so its console errors are captured
                // even before anyone looks at it (agents open tabs in the background).
                if event.method == "Target.targetCreated" {
                    log::info!("[{}] tab {} opened ({})", self.workspace_id, short(&id), tab.url);
                    let instance = self.clone();
                    let target = id.clone();
                    tauri::async_runtime::spawn(async move {
                        let _ = log::check(&format!("[{}] attach to new tab {}", instance.workspace_id, short(&target)), instance.session(&target).await);
                        // The new tab took the front, which hides the one on screen: it'd stop
                        // painting and taking input. Showing the new tab brings it back in front.
                        instance.front_shown_tab().await;
                    });
                }
                {
                    let mut tabs = self.tabs.lock().unwrap();
                    match tabs.iter_mut().find(|t| t.id == id) {
                        Some(existing) => {
                            // Keep the icon until the next page load reads it again.
                            tab.favicon = std::mem::take(&mut existing.favicon);
                            *existing = tab;
                        }
                        None => tabs.push(tab),
                    }
                }
                self.emit_tabs();
            }
            "Target.targetDestroyed" => {
                let id = event.params["targetId"].as_str().unwrap_or_default();
                if !self.has_tab(id) && !self.sessions.lock().unwrap().contains_key(id) {
                    return;
                }
                log::info!("[{}] tab {} closed", self.workspace_id, short(id));
                self.tabs.lock().unwrap().retain(|t| t.id != id);
                self.sessions.lock().unwrap().remove(id);
                self.attaching.lock().unwrap().remove(id);
                self.console.lock().unwrap().remove(id);
                self.agent_seen.lock().unwrap().remove(id);
                self.agent_opened.lock().unwrap().remove(id);
                self.emit_tabs();
            }
            // Our session ended without the tab closing (the page's process swapped or died):
            // forget it so the next call attaches again instead of failing on a dead session.
            "Target.detachedFromTarget" => {
                let session = event.params["sessionId"].as_str().unwrap_or_default();
                let target = {
                    let mut sessions = self.sessions.lock().unwrap();
                    let target = sessions.iter().find(|(_, s)| s.as_str() == session).map(|(t, _)| t.clone());
                    if let Some(t) = &target {
                        sessions.remove(t);
                    }
                    target
                };
                if let Some(target) = target {
                    log::warning!("[{}] session to tab {} detached", self.workspace_id, short(&target));
                    self.attaching.lock().unwrap().remove(&target);
                }
                self.screencast_lost(session);
            }
            "Target.targetCrashed" if self.has_tab(event.params["targetId"].as_str().unwrap_or_default()) => {
                log::warning!(
                    "[{}] tab {} crashed (status {}, code {})",
                    self.workspace_id,
                    short(event.params["targetId"].as_str().unwrap_or_default()),
                    event.params["status"],
                    event.params["errorCode"]
                );
            }
            "Overlay.inspectNodeRequested" => {
                let (Some(session), Some(node)) = (
                    event.session_id.clone(),
                    event.params["backendNodeId"].as_i64(),
                ) else {
                    return;
                };
                let instance = self.clone();
                tauri::async_runtime::spawn(async move {
                    if let Err(err) = instance.clone().finish_pick(session, node).await {
                        let _ = crate::diagnostics::emit_logged(&instance.app, "browser://pick-error", err);
                    }
                });
            }
            _ if event.method.starts_with("Runtime.")
                || event.method.starts_with("Log.")
                || event.method.starts_with("Network.") =>
            {
                let Some(entry) = inspect::console_entry(&event) else {
                    return;
                };
                let target = event.session_id.as_deref().and_then(|session| {
                    self.sessions
                        .lock()
                        .unwrap()
                        .iter()
                        .find(|(_, s)| s.as_str() == session)
                        .map(|(t, _)| t.clone())
                });
                if let Some(target) = target {
                    self.record_console(&target, entry);
                }
            }
            // A page (re)loaded: read its icon, early at DOMContentLoaded and again once scripts ran.
            "Page.domContentEventFired" | "Page.loadEventFired" => {
                let Some(session) = event.session_id.clone() else {
                    return;
                };
                let instance = self.clone();
                tauri::async_runtime::spawn(async move { instance.read_favicon(&session).await });
            }
            "Page.screencastFrame" => {
                let Some(session) = event.session_id.as_deref() else {
                    return;
                };
                let mut screencast = self.screencast.lock().unwrap();
                let Some(sc) = screencast.as_mut().filter(|sc| sc.session_id == session) else {
                    // Not the screencast on show (it was just replaced).
                    drop(screencast);
                    self.send_frame_acks(session, &event.params["sessionId"], 1);
                    return;
                };
                sc.got_frame = true;
                sc.ack_id = event.params["sessionId"].clone();
                sc.frames += 1;
                let sent = base64::engine::general_purpose::STANDARD
                    .decode(event.params["data"].as_str().unwrap_or_default())
                    .is_ok_and(|bytes| sc.channel.send(InvokeResponseBody::Raw(bytes)).is_ok());
                let (attach_id, frame) = (sc.attach_id.clone(), sc.frames);
                let action = sc.sizing.on_frame(&event.params["metadata"]);
                if action != FrameAction::None {
                    let (instance, target, session, viewport) = (
                        self.clone(),
                        sc.target_id.clone(),
                        session.to_string(),
                        sc.sizing.viewport.clone(),
                    );
                    tauri::async_runtime::spawn(async move {
                        instance.on_resized(action, target, session, viewport).await;
                    });
                }
                drop(screencast);
                if sent {
                    // Acked once the view drew it (`browser_frame_done`), or after a while.
                    let instance = self.clone();
                    tauri::async_runtime::spawn(async move {
                        tokio::time::sleep(FRAME_ACK_FALLBACK).await;
                        instance.ack_frames(&attach_id, Some(frame));
                    });
                } else {
                    // The view won't report a frame it never got.
                    self.ack_frames(&attach_id, Some(frame));
                }
            }
            _ => {}
        }
    }

    /// Acks the screencast's frames up to number `upto` (None: one more), never more than were
    /// sent. Only for the screencast `attach_id` started; a stale one's acks are dropped.
    fn ack_frames(&self, attach_id: &str, upto: Option<u64>) {
        let (session, ack_id, n) = {
            let mut screencast = self.screencast.lock().unwrap();
            let Some(sc) = screencast.as_mut().filter(|sc| sc.attach_id == attach_id) else {
                return;
            };
            let to = upto.unwrap_or(sc.acked + 1).min(sc.frames);
            if to <= sc.acked {
                return;
            }
            let n = to - sc.acked;
            sc.acked = to;
            (sc.session_id.clone(), sc.ack_id.clone(), n)
        };
        self.send_frame_acks(&session, &ack_id, n);
    }

    fn send_frame_acks(&self, session: &str, ack_id: &Value, n: u64) {
        if let Some(cdp) = self.live_cdp() {
            for _ in 0..n {
                cdp.send("Page.screencastFrameAck", json!({ "sessionId": ack_id }), Some(session));
            }
        }
    }

    /// An agent acted on a tab: tell the panel (throttled), which highlights it.
    fn on_agent_activity(&self, activity: proxy::Activity) {
        let target = match activity {
            proxy::Activity::Opened(target) => {
                self.agent_opened.lock().unwrap().insert(target.to_string());
                return;
            }
            proxy::Activity::Acted(target) => target,
            // Not throttled: each navigation brings its tab on screen.
            proxy::Activity::Navigated(target) => {
                let _ = crate::diagnostics::emit_logged(
                    &self.app,
                    "browser://agent-navigate",
                    AgentEvent {
                        workspace_id: self.workspace_id.clone(),
                        target_id: target.to_string(),
                        adopted: !self.agent_opened.lock().unwrap().contains(target),
                    },
                );
                return;
            }
            // Not throttled: the cursor follows every move and click.
            proxy::Activity::Pointer(target, pointer) => {
                if self.tabs.lock().unwrap().iter().any(|t| t.id == target) {
                    let _ = crate::diagnostics::emit_logged(
                        &self.app,
                        "browser://agent-pointer",
                        PointerEvent {
                            workspace_id: self.workspace_id.clone(),
                            target_id: target.to_string(),
                            pointer,
                        },
                    );
                }
                return;
            }
        };
        // Review screenshots' hidden pages and non-page targets aren't tabs.
        if !self.tabs.lock().unwrap().iter().any(|t| t.id == target) {
            return;
        }
        let now = std::time::Instant::now();
        {
            let mut seen = self.agent_seen.lock().unwrap();
            if seen.get(target).is_some_and(|at| now - *at < AGENT_EVERY) {
                return;
            }
            seen.insert(target.to_string(), now);
        }
        let _ = crate::diagnostics::emit_logged(
            &self.app,
            "browser://agent",
            AgentEvent {
                workspace_id: self.workspace_id.clone(),
                target_id: target.to_string(),
                adopted: !self.agent_opened.lock().unwrap().contains(target),
            },
        );
    }

    /// Follows up on a frame: re-applies our scale and mobile flags at an agent's new size and
    /// tells the panel, or restores emulation that a disconnecting agent cleared.
    async fn on_resized(&self, action: FrameAction, target: String, session: String, v: Viewport) {
        let Some(cdp) = self.live_cdp() else {
            return;
        };
        match action {
            FrameAction::None => {}
            FrameAction::Adopt(width, height) => {
                let _ = apply_viewport(&cdp, &target, &session, &v).await;
                let _ = crate::diagnostics::emit_logged(
                    &self.app,
                    "browser://viewport",
                    ViewportEvent {
                        workspace_id: self.workspace_id.clone(),
                        target_id: target,
                        width,
                        height,
                    },
                );
            }
            FrameAction::Check => {
                let ratio = cdp
                    .call(
                        "Runtime.evaluate",
                        json!({ "expression": "devicePixelRatio", "returnByValue": true }),
                        Some(&session),
                    )
                    .await
                    .ok()
                    .and_then(|r| r["result"]["value"].as_f64());
                if ratio.is_some_and(|r| (r - v.device_scale_factor).abs() > 0.01) {
                    let _ = apply_viewport(&cdp, &target, &session, &v).await;
                }
            }
        }
    }

    /// The live connection, if there is one (no connecting).
    fn live_cdp(&self) -> Option<Arc<Cdp>> {
        self.cdp.lock().unwrap().clone().filter(|cdp| !cdp.is_closed())
    }

    /// The connection, made (again) if there's none or it dropped, e.g. after the computer slept.
    async fn cdp(self: &Arc<Self>) -> Result<Arc<Cdp>, String> {
        if let Some(cdp) = self.live_cdp() {
            return Ok(cdp);
        }
        let _guard = self.connecting.lock().await;
        if let Some(cdp) = self.live_cdp() {
            return Ok(cdp);
        }
        let reconnect = self.cdp.lock().unwrap().take().is_some();
        if reconnect {
            log::warning!("[{}] browser connection dropped, reconnecting", self.workspace_id);
            // Sessions and the screencast belonged to the old connection.
            self.sessions.lock().unwrap().clear();
            self.attaching.lock().unwrap().clear();
            *self.screencast.lock().unwrap() = None;
        }
        let manager = self.app.state::<BrowserManager>();
        let ws_url = log::check(&format!("[{}] browser start", self.workspace_id), manager.ws_url(&self.app).await)?;
        log::info!("[{}] connecting to {ws_url}", self.workspace_id);
        let weak: Weak<Self> = Arc::downgrade(self);
        let cdp = Cdp::connect(&ws_url, move |event| {
            if let Some(instance) = weak.upgrade() {
                instance.on_event(event);
            }
        })
        .await?;
        // Before discovery, so the context's tabs are told apart from the first event on.
        log::check(&format!("[{}] browser context", self.workspace_id), self.ensure_context(&cdp).await)?;
        cdp.call("Target.setDiscoverTargets", json!({ "discover": true }), None)
            .await?;
        *self.cdp.lock().unwrap() = Some(cdp.clone());
        // When it drops (the computer slept, the socket failed), tell the panel: showing its tab
        // again connects anew, instead of the view sitting frozen until something fails.
        let (weak, watched) = (Arc::downgrade(self), cdp.clone());
        tauri::async_runtime::spawn(async move {
            watched.closed().await;
            if let Some(instance) = weak.upgrade() {
                log::warning!("[{}] browser connection lost", instance.workspace_id);
                let _ = crate::diagnostics::emit_logged(&instance.app, "browser://disconnected", instance.workspace_id.clone());
            }
        });
        Ok(cdp)
    }

    /// The workspace's context in the browser `cdp` talks to: the one made earlier if the
    /// browser still has it, else a new one with the saved cookies put back.
    async fn ensure_context(&self, cdp: &Cdp) -> Res<String> {
        let known = self.context.lock().unwrap().clone();
        if let Some(id) = &known {
            let contexts = cdp.call("Target.getBrowserContexts", json!({}), None).await?;
            if contexts["browserContextIds"].as_array().is_some_and(|ids| ids.iter().any(|c| c == id.as_str())) {
                return Ok(id.clone());
            }
            // The browser restarted: the context and its tabs went with it.
            log::warning!("[{}] browser restarted, making the workspace's context again", self.workspace_id);
            *self.context.lock().unwrap() = None;
            self.tabs.lock().unwrap().clear();
            self.console.lock().unwrap().clear();
            self.agent_seen.lock().unwrap().clear();
            self.agent_opened.lock().unwrap().clear();
            self.emit_tabs();
        }
        let id = cdp
            .call("Target.createBrowserContext", json!({ "disposeOnDetach": false }), None)
            .await?["browserContextId"]
            .as_str()
            .ok_or("createBrowserContext returned no id")?
            .to_string();
        self.restore_cookies(cdp, &id).await;
        *self.context.lock().unwrap() = Some(id.clone());
        Ok(id)
    }

    fn cookie_file(&self) -> Option<PathBuf> {
        data_dir(&self.app).ok().map(|dir| cookies::path(&dir.join("browser-cookies"), &self.workspace_id))
    }

    /// Puts the workspace's saved cookies into its new context.
    async fn restore_cookies(&self, cdp: &Cdp, context: &str) {
        let Some(file) = self.cookie_file().filter(|f| f.exists()) else { return };
        let loaded = tauri::async_runtime::spawn_blocking(move || cookies::key().map(|key| cookies::load(&file, &key)))
            .await
            .ok()
            .flatten()
            .unwrap_or_default();
        if loaded.is_empty() {
            return;
        }
        let set = |cookies: Vec<Value>| cdp.call("Storage.setCookies", json!({ "cookies": cookies, "browserContextId": context }), None);
        // One cookie Chrome turns down fails them all: then set them one by one.
        let mut restored = loaded.len();
        if set(loaded.clone()).await.is_err() {
            for cookie in loaded {
                if set(vec![cookie]).await.is_err() {
                    restored -= 1;
                }
            }
        }
        log::info!("[{}] restored {restored} cookies", self.workspace_id);
        *self.saved_cookies.lock().unwrap() = None;
    }

    /// Saves the workspace's cookies if they changed. Only on a live connection: never starts
    /// the browser for it.
    async fn save_cookies(&self) -> Res<()> {
        let (Some(cdp), Some(context)) = (self.live_cdp(), self.context.lock().unwrap().clone()) else {
            return Ok(());
        };
        let res = cdp.call("Storage.getCookies", json!({ "browserContextId": context }), None).await?;
        let current: Vec<Value> = res["cookies"].as_array().into_iter().flatten().filter_map(cookies::param).collect();
        if self.saved_cookies.lock().unwrap().as_ref() == Some(&current) {
            return Ok(());
        }
        let Some(file) = self.cookie_file() else { return Ok(()) };
        if current.is_empty() && !file.exists() {
            // Nothing to keep: don't make a key (a keychain prompt) or a file for it.
            *self.saved_cookies.lock().unwrap() = Some(current);
            return Ok(());
        }
        let all = res["cookies"].as_array().cloned().unwrap_or_default();
        let written = tauri::async_runtime::spawn_blocking(move || {
            let key = cookies::key().ok_or("no keychain")?;
            cookies::store(&file, &key, &all)
        })
        .await
        .map_err(|e| e.to_string())??;
        *self.saved_cookies.lock().unwrap() = Some(written);
        Ok(())
    }

    /// Ids of the targets in the workspace's context.
    async fn context_targets(self: &Arc<Self>) -> Res<HashSet<String>> {
        let cdp = self.cdp().await?;
        let targets = cdp.call("Target.getTargets", json!({}), None).await?;
        Ok(targets["targetInfos"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|t| self.owns(t))
            .filter_map(|t| t["targetId"].as_str().map(String::from))
            .collect())
    }

    /// Opens a tab in the workspace's context. With certificate errors ignored the page loads
    /// only once the tab has the setting (it's per tab), so it opens blank first.
    async fn open_tab(self: &Arc<Self>, url: &str) -> Res<String> {
        let cdp = self.cdp().await?;
        let context = self.context.lock().unwrap().clone().ok_or("no browser context")?;
        let deferred = self.ignore_certificate_errors.load(Ordering::SeqCst) && url != "about:blank";
        let first = if deferred { "about:blank" } else { url };
        let res = cdp.call("Target.createTarget", json!({ "url": first, "browserContextId": context }), None).await?;
        let id = res["targetId"].as_str().ok_or("createTarget returned no id")?.to_string();
        if deferred {
            self.call_target(&id, "Page.navigate", json!({ "url": url })).await?;
        }
        Ok(id)
    }

    async fn session(self: &Arc<Self>, target_id: &str) -> Result<String, String> {
        if let Some(session) = self.sessions.lock().unwrap().get(target_id) {
            return Ok(session.clone());
        }
        let cell = self
            .attaching
            .lock()
            .unwrap()
            .entry(target_id.to_string())
            .or_default()
            .clone();
        let session = cell
            .get_or_try_init(|| async {
                let cdp = self.cdp().await?;
                let res = cdp
                    .call(
                        "Target.attachToTarget",
                        json!({ "targetId": target_id, "flatten": true }),
                        None,
                    )
                    .await?;
                let session = res["sessionId"]
                    .as_str()
                    .ok_or("attach returned no session")?
                    .to_string();
                // Register before enabling domains so their first events map to this target.
                self.sessions
                    .lock()
                    .unwrap()
                    .insert(target_id.to_string(), session.clone());
                // Agents (Playwright) auto-attach with waitForDebuggerOnStart, so a tab can be held
                // until they resume it; one that went away never would. Harmless otherwise.
                let _ = cdp.call("Runtime.runIfWaitingForDebugger", json!({}), Some(&session)).await;
                if self.ignore_certificate_errors.load(Ordering::SeqCst) {
                    set_certificate_policy(&cdp, &session, true).await?;
                }
                cdp.call("Page.enable", json!({}), Some(&session)).await?;
                for domain in ["Runtime.enable", "Log.enable", "Network.enable"] {
                    if let Err(err) = cdp.call(domain, json!({}), Some(&session)).await {
                        log::warning!("[{}] {domain} on tab {}: {err}", self.workspace_id, short(target_id));
                    }
                }
                Ok::<_, String>(session)
            })
            .await?;
        Ok(session.clone())
    }

    /// Reads the icon of the page behind `session` and, if it changed, updates its tab.
    async fn read_favicon(self: &Arc<Self>, session: &str) {
        const SCRIPT: &str = r#"(() => {
            const pick = document.querySelector('link[rel~="icon" i][href]') ?? document.querySelector('link[rel="apple-touch-icon" i][href]');
            if (pick) return pick.href;
            return /^https?:$/.test(location.protocol) ? location.origin + "/favicon.ico" : "";
        })()"#;
        let Some(cdp) = self.live_cdp() else {
            return;
        };
        let Some(target) = self
            .sessions
            .lock()
            .unwrap()
            .iter()
            .find(|(_, s)| s.as_str() == session)
            .map(|(t, _)| t.clone())
        else {
            return;
        };
        let Ok(res) = cdp
            .call("Runtime.evaluate", json!({ "expression": SCRIPT, "returnByValue": true }), Some(session))
            .await
        else {
            return;
        };
        let href = res["result"]["value"].as_str().unwrap_or_default().to_string();
        // Hand the panel the icon's bytes, so it can check them against its background
        // (most sites don't allow reading a cross-origin icon's pixels).
        let favicon = if href.is_empty() {
            href
        } else {
            tauri::async_runtime::spawn_blocking(move || favicon_data_url(&href))
                .await
                .ok()
                .flatten()
                .unwrap_or_default()
        };
        let changed = {
            let mut tabs = self.tabs.lock().unwrap();
            match tabs.iter_mut().find(|t| t.id == target) {
                Some(tab) if tab.favicon != favicon => {
                    tab.favicon = favicon;
                    true
                }
                _ => false,
            }
        };
        if changed {
            self.emit_tabs();
        }
    }

    /// Sends `method` to a tab, attaching once more if the session turned out to be gone.
    async fn call_target(self: &Arc<Self>, target_id: &str, method: &str, params: Value) -> Res<Value> {
        let session = self.session(target_id).await?;
        let cdp = self.cdp().await?;
        match cdp.call(method, params.clone(), Some(&session)).await {
            Err(err) if err.contains("ession") => {
                log::warning!("[{}] {method} on tab {}: {err}; attaching again", self.workspace_id, short(target_id));
                self.forget_session(target_id, &session);
                let session = self.session(target_id).await?;
                cdp.call(method, params, Some(&session)).await
            }
            result => result,
        }
    }

    /// Puts the tab the panel shows in front again, focused, so it paints and takes typing.
    async fn front_shown_tab(&self) {
        // Serialized with attaches, so this can't put back a tab the panel just moved away from.
        let _guard = self.attach_lock.lock().await;
        let shown = self.screencast.lock().unwrap().as_ref().map(|sc| (sc.target_id.clone(), sc.session_id.clone()));
        let (Some((target, session)), Some(cdp)) = (shown, self.live_cdp()) else {
            return;
        };
        front(&cdp, &target, &session, &self.workspace_id).await;
    }

    fn forget_session(self: &Arc<Self>, target_id: &str, session: &str) {
        let mut sessions = self.sessions.lock().unwrap();
        if sessions.get(target_id).is_some_and(|s| s == session) {
            sessions.remove(target_id);
            self.attaching.lock().unwrap().remove(target_id);
        }
        drop(sessions);
        self.screencast_lost(session);
    }

    /// The panel's screencast ran on `session`, which is gone: its frames will never come (and a
    /// new session's are filtered out), so have the panel show the tab again.
    fn screencast_lost(self: &Arc<Self>, session: &str) {
        let lost = {
            let mut screencast = self.screencast.lock().unwrap();
            match screencast.as_ref() {
                Some(sc) if sc.session_id == session => screencast.take(),
                _ => None,
            }
        };
        let Some(sc) = lost else { return };
        // A closing tab detaches too; only show it again if it's still open.
        let instance = self.clone();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
            if !instance.tabs.lock().unwrap().iter().any(|t| t.id == sc.target_id) {
                return;
            }
            log::warning!("[{}] screencast of tab {} lost its session, showing it again", instance.workspace_id, short(&sc.target_id));
            let _ = crate::diagnostics::emit_logged(&instance.app, "browser://disconnected", instance.workspace_id.clone());
        });
    }
}

/// Makes a tab the front, focused one. Only the front tab paints (screencast frames) and has
/// focus (where typing goes), and agents keep opening tabs in front of the one on screen.
async fn front(cdp: &Cdp, target_id: &str, session: &str, workspace_id: &str) {
    for (method, params, session) in [
        ("Target.activateTarget", json!({ "targetId": target_id }), None),
        ("Page.bringToFront", json!({}), Some(session)),
        ("Emulation.setFocusEmulationEnabled", json!({ "enabled": true }), Some(session)),
    ] {
        if let Err(err) = cdp.call(method, params, session).await {
            log::warning!("[{workspace_id}] {method} on tab {}: {err}", short(target_id));
        }
    }
}

/// A target id's first characters, enough to tell tabs apart in the log.
fn short(id: &str) -> &str {
    &id[..id.len().min(8)]
}

#[derive(Default)]
pub struct BrowserManager {
    instances: Mutex<HashMap<String, Arc<Instance>>>,
    /// The browser all workspaces share, started on first use and again if it quits.
    browser: Mutex<Option<Browser>>,
}

struct Browser {
    child: Child,
    port: u16,
    exe: Executable,
    profile: PathBuf,
}

/// How often each workspace's cookies are saved (also when the app quits).
const SAVE_COOKIES_EVERY: std::time::Duration = std::time::Duration::from_secs(60);

fn data_dir(app: &AppHandle) -> Result<PathBuf, String> {
    app.path().app_data_dir().map_err(|e| e.to_string())
}

fn managed_dir(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(data_dir(app)?.join("browser"))
}

impl BrowserManager {
    /// The shared browser's DevTools port, launching it if it isn't running (without waiting
    /// for it to come up). Synchronous so PTY spawns can get a CDP URL.
    fn running(&self, app: &AppHandle) -> Res<u16> {
        let mut browser = self.browser.lock().unwrap();
        if let Some(b) = browser.as_mut() {
            match b.child.try_wait() {
                Ok(None) => return Ok(b.port),
                status => log::warning!("browser quit ({status:?}), starting it again"),
            }
        }
        let exe = chrome::find(&managed_dir(app)?).ok_or("no browser installed")?;
        let profile = data_dir(app)?.join("browser-profiles").join("shared");
        let launched = log::check(&format!("launch {}", exe.path.display()), chrome::launch(&exe, &profile))?;
        log::info!("browser launched on port {}", launched.port);
        let port = launched.port;
        *browser = Some(Browser { child: launched.child, port, exe, profile });
        Ok(port)
    }

    /// The running browser's port, if one runs.
    fn port(&self) -> Option<u16> {
        self.browser.lock().unwrap().as_ref().map(|b| b.port)
    }

    /// The browser's WebSocket URL, starting it if needed and polling until it's up. A browser
    /// that quit because another one held its profile is restarted after that one is ended
    /// (`spawn` reclaims the profile).
    async fn ws_url(&self, app: &AppHandle) -> Res<String> {
        let mut port = self.running(app)?;
        let mut deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        let mut retries = 2;
        loop {
            let url = tauri::async_runtime::spawn_blocking(move || chrome::ws_url(port))
                .await
                .map_err(|e| e.to_string())?;
            if let Some(url) = url {
                return Ok(url);
            }
            let (current, exited) = {
                let mut browser = self.browser.lock().unwrap();
                let b = browser.as_mut().ok_or("the browser was shut down")?;
                (b.port, b.child.try_wait().ok().flatten())
            };
            if current != port {
                // Someone else started it again meanwhile.
                port = current;
                continue;
            }
            if let Some(status) = exited {
                if status.code() == Some(chrome::PROFILE_IN_USE) && retries > 0 {
                    retries -= 1;
                    // Holds the lock while restarting so `running` doesn't start a second
                    // browser on the profile meanwhile.
                    tokio::task::block_in_place(|| {
                        let mut browser = self.browser.lock().unwrap();
                        let b = browser.as_mut().ok_or("the browser was shut down")?;
                        chrome::spawn(&b.exe, &b.profile, b.port).map(|c| b.child = c)
                    })?;
                    deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
                    continue;
                }
                let profile = self.browser.lock().unwrap().as_ref().map(|b| b.profile.display().to_string()).unwrap_or_default();
                return Err(if status.code() == Some(chrome::PROFILE_IN_USE) {
                    format!("another browser keeps the profile {profile} locked. End the Chrome processes using it, then retry.")
                } else {
                    format!("the browser quit while starting ({status})")
                });
            }
            if std::time::Instant::now() > deadline {
                return Err("the browser didn't start in time".into());
            }
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }
    }

    /// The workspace's part of the browser, starting the browser. Synchronous so PTY spawns can
    /// get the CDP URL. The instance (and its proxy port) is made once and kept, also across
    /// browser restarts.
    fn ensure(&self, app: &AppHandle, workspace_id: &str) -> Res<Arc<Instance>> {
        self.running(app)?;
        let mut instances = self.instances.lock().unwrap();
        if let Some(instance) = instances.get(workspace_id) {
            return Ok(instance.clone());
        }
        let listener = proxy::Listener::bind().map_err(|e| format!("CDP proxy: {e}"))?;
        let instance = Arc::new_cyclic(|weak: &Weak<Instance>| Instance {
            workspace_id: workspace_id.to_string(),
            app: app.clone(),
            context: Mutex::default(),
            saved_cookies: Mutex::default(),
            cdp: Mutex::default(),
            connecting: tokio::sync::Mutex::new(()),
            tabs: Mutex::default(),
            sessions: Mutex::default(),
            ignore_certificate_errors: AtomicBool::new(false),
            attaching: Mutex::default(),
            screencast: Mutex::default(),
            attach_lock: tokio::sync::Mutex::new(()),
            wanted_attach: Mutex::default(),
            console: Mutex::default(),
            proxy: {
                let weak = weak.clone();
                listener.serve(
                    Arc::new(AgentSide(weak.clone())),
                    Arc::new(move |activity: proxy::Activity| {
                        if let Some(instance) = weak.upgrade() {
                            instance.on_agent_activity(activity);
                        }
                    }),
                )
            },
            agent_seen: Mutex::default(),
            agent_opened: Mutex::default(),
        });
        instances.insert(workspace_id.to_string(), instance.clone());
        let weak = Arc::downgrade(&instance);
        tauri::async_runtime::spawn(async move {
            loop {
                tokio::time::sleep(SAVE_COOKIES_EVERY).await;
                let Some(instance) = weak.upgrade() else { return };
                if let Err(err) = instance.save_cookies().await {
                    log::warning!("[{}] saving cookies: {err}", instance.workspace_id);
                }
            }
        });
        let _ = crate::diagnostics::emit_logged(app, "browser://certificate-policy", json!({ "workspaceId": workspace_id, "allow": false }));
        Ok(instance)
    }

    /// CDP endpoint for agents in this workspace, starting the browser if one is installed.
    pub fn cdp_url(&self, app: &AppHandle, workspace_id: &str) -> Option<String> {
        self.ensure(app, workspace_id)
            .ok()
            .map(|i| format!("http://127.0.0.1:{}", i.proxy.port))
    }

    /// The workspaces whose browser is open, with their tabs (starts nothing).
    #[allow(dead_code)] // used by winferd, not the Tauri app
    pub fn open_tabs(&self) -> Vec<(String, Value)> {
        let instances: Vec<Arc<Instance>> = self.instances.lock().unwrap().values().cloned().collect();
        instances.iter().map(|i| (i.workspace_id.clone(), serde_json::to_value(&*i.tabs.lock().unwrap()).unwrap_or_default())).collect()
    }

    /// Saves every workspace's cookies, then ends the browser. Not left to `Drop`: tasks may
    /// still hold it when the app exits, and the browser would outlive winfer.
    pub fn shutdown(&self) {
        let instances: Vec<_> = self.instances.lock().unwrap().drain().map(|(_, i)| i).collect();
        tauri::async_runtime::block_on(async {
            let saves = instances.iter().map(|i| async move {
                if let Err(err) = i.save_cookies().await {
                    log::warning!("[{}] saving cookies: {err}", i.workspace_id);
                }
            });
            let _ = tokio::time::timeout(std::time::Duration::from_secs(3), futures_util::future::join_all(saves)).await;
        });
        if let Some(mut browser) = self.browser.lock().unwrap().take() {
            chrome::terminate(&mut browser.child, browser.port);
        }
    }
}

/// What a workspace's proxy needs from it.
struct AgentSide(Weak<Instance>);

impl AgentSide {
    fn instance(&self) -> Res<Arc<Instance>> {
        self.0.upgrade().ok_or_else(|| "the workspace's browser is gone".to_string())
    }
}

impl proxy::Upstream for AgentSide {
    fn prepare(&self) -> futures_util::future::BoxFuture<'static, Res<(u16, String)>> {
        let instance = self.instance();
        Box::pin(async move {
            let instance = instance?;
            instance.cdp().await?;
            let port = instance.app.state::<BrowserManager>().port().ok_or("the browser isn't running")?;
            let context = instance.context.lock().unwrap().clone().ok_or("no browser context")?;
            Ok((port, context))
        })
    }

    fn targets(&self) -> futures_util::future::BoxFuture<'static, Res<HashSet<String>>> {
        let instance = self.instance();
        Box::pin(async move { instance?.context_targets().await })
    }

    fn new_tab(&self, url: String) -> futures_util::future::BoxFuture<'static, Res<String>> {
        let instance = self.instance();
        Box::pin(async move { instance?.open_tab(&url).await })
    }
}

/// Kills browsers left behind by a previous run that didn't exit cleanly (crash, force quit).
/// They'd otherwise keep running and hold the profile lock, so the next launch would fail.
/// Runs in the background: startup doesn't wait for it, browser launches do.
pub fn kill_orphans(app: &AppHandle) {
    if let Ok(dir) = data_dir(app) {
        chrome::reclaim_all_in_background(dir.join("browser-profiles"));
    }
}

/// Playwright MCP server attached to the workspace browser.
pub fn playwright_server(cdp_url: &str) -> crate::mcp::Server {
    let npx_args = ["-y", "@playwright/mcp@latest", "--cdp-endpoint", cdp_url].map(String::from);
    // npx is a .cmd shim on Windows, which agents can only start through cmd.
    let (command, args) = if cfg!(windows) {
        ("cmd", [vec!["/c".into(), "npx".into()], npx_args.to_vec()].concat())
    } else {
        ("npx", npx_args.to_vec())
    };
    crate::mcp::Server {
        name: "winfer-browser",
        command: command.into(),
        args,
        env: Vec::new(),
    }
}

fn sanitize(id: &str) -> String {
    id.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

type Res<T> = Result<T, String>;

async fn instance(
    app: &AppHandle,
    manager: &BrowserManager,
    workspace_id: &str,
) -> Res<Arc<Instance>> {
    let instance = log::check(&format!("[{workspace_id}] start"), manager.ensure(app, workspace_id))?;
    log::check(&format!("[{workspace_id}] connect"), instance.cdp().await.map(drop))?;
    Ok(instance)
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserStatus {
    executable: Option<Executable>,
}

#[tauri::command]
pub fn browser_status(app: AppHandle) -> Res<BrowserStatus> {
    Ok(BrowserStatus {
        executable: chrome::find(&managed_dir(&app)?),
    })
}

#[tauri::command]
pub async fn browser_install(app: AppHandle) -> Res<Executable> {
    let dir = managed_dir(&app)?;
    let result = tauri::async_runtime::spawn_blocking(move || chrome::install(&dir))
        .await
        .map_err(|e| e.to_string())
        .and_then(|r| r);
    log::check("browser install", result)
}

/// Starts the workspace browser and returns its tabs (creating a first tab if there are none).
#[tauri::command]
pub async fn browser_open(
    app: AppHandle,
    manager: State<'_, BrowserManager>,
    workspace_id: String,
) -> Res<Vec<Tab>> {
    let instance = instance(&app, &manager, &workspace_id).await?;
    let cdp = instance.cdp().await?;
    // Discovery events populate tabs asynchronously; fetch the list directly too.
    let targets = cdp.call("Target.getTargets", json!({}), None).await?;
    let pages: Vec<&Value> = targets["targetInfos"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|t| t["type"] == "page" && instance.owns(t))
        .collect();
    if pages.is_empty() && instance.tabs.lock().unwrap().is_empty() {
        // A new context has no tab yet (a browser of its own would have opened one).
        let id = log::check(&format!("[{workspace_id}] first tab"), instance.open_tab("about:blank").await)?;
        if !instance.has_tab(&id) {
            instance.tabs.lock().unwrap().push(Tab { id, url: "about:blank".into(), title: String::new(), favicon: String::new() });
        }
    }
    {
        let mut tabs = instance.tabs.lock().unwrap();
        for info in pages {
            let id = info["targetId"].as_str().unwrap_or_default();
            if !tabs.iter().any(|t| t.id == id) {
                tabs.push(Tab {
                    id: id.into(),
                    url: info["url"].as_str().unwrap_or_default().into(),
                    title: info["title"].as_str().unwrap_or_default().into(),
                    favicon: String::new(),
                });
            }
        }
    }
    Ok(instance.tabs.lock().unwrap().clone())
}

#[tauri::command]
pub async fn browser_allow_invalid_certificates(
    app: AppHandle,
    manager: State<'_, BrowserManager>,
    workspace_id: String,
    allow: bool,
) -> Res<()> {
    let instance = instance(&app, &manager, &workspace_id).await?;
    let cdp = instance.cdp().await?;
    // Set first, so tabs attaching meanwhile get it too; then applied to every tab of the
    // workspace (each is attached as it opens).
    instance.ignore_certificate_errors.store(allow, Ordering::SeqCst);
    let tabs: Vec<String> = instance.tabs.lock().unwrap().iter().map(|t| t.id.clone()).collect();
    let mut failed = None;
    for tab in tabs {
        let applied = match instance.session(&tab).await {
            Ok(session) => set_certificate_policy(&cdp, &session, allow).await,
            Err(err) => Err(err),
        };
        if let Err(err) = applied {
            log::warning!("[{workspace_id}] certificate policy on tab {}: {err}", short(&tab));
            failed.get_or_insert(err);
        }
    }
    failed.map_or(Ok(()), Err)
}

/// Whether the tab behind `session` ignores certificate errors. Per tab, so it doesn't reach
/// other workspaces' tabs in the shared browser.
async fn set_certificate_policy(cdp: &Cdp, session: &str, allow: bool) -> Res<()> {
    cdp.call("Security.setIgnoreCertificateErrors", json!({ "ignore": allow }), Some(session)).await?;
    Ok(())
}

#[tauri::command]
pub async fn browser_new_tab(
    app: AppHandle,
    manager: State<'_, BrowserManager>,
    workspace_id: String,
    url: String,
) -> Res<String> {
    let instance = instance(&app, &manager, &workspace_id).await?;
    let id = log::check(&format!("[{workspace_id}] new tab for {url}"), instance.open_tab(&url).await)?;
    log::info!("[{workspace_id}] new tab {} for {url}", short(&id));
    Ok(id)
}

/// Loads `url` in a hidden, throwaway context of the browser and returns a JPEG screenshot as a
/// data URL. Used by the review inbox to show what an agent's app looks like. Its pages aren't in
/// the workspace's context, so they never show up as tabs.
#[tauri::command]
pub async fn browser_screenshot(
    app: AppHandle,
    manager: State<'_, BrowserManager>,
    workspace_id: String,
    url: String,
) -> Res<String> {
    let instance = instance(&app, &manager, &workspace_id).await?;
    let cdp = instance.cdp().await?;
    let context = cdp
        .call("Target.createBrowserContext", json!({}), None)
        .await?["browserContextId"]
        .as_str()
        .ok_or("createBrowserContext returned no id")?
        .to_string();
    let ignore_certificate_errors = instance.ignore_certificate_errors.load(Ordering::SeqCst);
    let shot = capture(&cdp, &context, &url, ignore_certificate_errors).await;
    // Disposing the context closes its page too.
    let _ = cdp
        .call(
            "Target.disposeBrowserContext",
            json!({ "browserContextId": context }),
            None,
        )
        .await;
    shot
}

async fn capture(cdp: &Cdp, context: &str, url: &str, ignore_certificate_errors: bool) -> Res<String> {
    let target = cdp
        .call(
            "Target.createTarget",
            json!({ "url": "about:blank", "browserContextId": context }),
            None,
        )
        .await?["targetId"]
        .as_str()
        .ok_or("createTarget returned no id")?
        .to_string();
    let session = cdp
        .call(
            "Target.attachToTarget",
            json!({ "targetId": target, "flatten": true }),
            None,
        )
        .await?["sessionId"]
        .as_str()
        .ok_or("attach returned no session")?
        .to_string();
    if ignore_certificate_errors {
        set_certificate_policy(cdp, &session, true).await?;
    }
    let session = Some(session.as_str());
    cdp.call(
        "Emulation.setDeviceMetricsOverride",
        json!({ "width": 1280, "height": 800, "deviceScaleFactor": 1, "mobile": false }),
        session,
    )
    .await?;
    let nav = cdp
        .call("Page.navigate", json!({ "url": url }), session)
        .await?;
    if let Some(error) = nav["errorText"].as_str() {
        return Err(format!("{url} didn't load ({error})"));
    }
    // Wait for the load, then a moment for client-side rendering.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    loop {
        let state = cdp
            .call(
                "Runtime.evaluate",
                json!({ "expression": "document.readyState", "returnByValue": true }),
                session,
            )
            .await?;
        if state["result"]["value"] == "complete" || std::time::Instant::now() > deadline {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
    tokio::time::sleep(std::time::Duration::from_millis(1200)).await;
    let shot = cdp
        .call(
            "Page.captureScreenshot",
            json!({ "format": "jpeg", "quality": 70 }),
            session,
        )
        .await?;
    let data = shot["data"].as_str().ok_or("no screenshot data")?;
    Ok(format!("data:image/jpeg;base64,{data}"))
}

#[tauri::command]
pub async fn browser_close_tab(
    app: AppHandle,
    manager: State<'_, BrowserManager>,
    workspace_id: String,
    target_id: String,
) -> Res<()> {
    let instance = instance(&app, &manager, &workspace_id).await?;
    let result = instance.cdp().await?.call("Target.closeTarget", json!({ "targetId": target_id }), None).await;
    log::check(&format!("[{workspace_id}] close tab {}", short(&target_id)), result).map(drop)
}

#[tauri::command]
pub async fn browser_navigate(
    app: AppHandle,
    manager: State<'_, BrowserManager>,
    workspace_id: String,
    target_id: String,
    url: String,
) -> Res<()> {
    let instance = instance(&app, &manager, &workspace_id).await?;
    let res = instance.call_target(&target_id, "Page.navigate", json!({ "url": &url })).await;
    let res = log::check(&format!("[{workspace_id}] navigate tab {} to {url}", short(&target_id)), res)?;
    if let Some(error) = res["errorText"].as_str() {
        log::warning!("[{workspace_id}] tab {} didn't load {url}: {error}", short(&target_id));
    }
    Ok(())
}

/// Back (-1), forward (+1) or reload (0).
#[tauri::command]
pub async fn browser_history(
    app: AppHandle,
    manager: State<'_, BrowserManager>,
    workspace_id: String,
    target_id: String,
    delta: i64,
) -> Res<()> {
    let instance = instance(&app, &manager, &workspace_id).await?;
    let what = format!("[{workspace_id}] history {delta} in tab {}", short(&target_id));
    if delta == 0 {
        // A tab behind another (an agent's) doesn't paint, so its reload would stay blank.
        let shown = instance.screencast.lock().unwrap().as_ref().is_some_and(|sc| sc.target_id == target_id);
        if shown {
            instance.front_shown_tab().await;
        }
        return log::check(&what, instance.call_target(&target_id, "Page.reload", json!({})).await).map(drop);
    }
    let history = log::check(&what, instance.call_target(&target_id, "Page.getNavigationHistory", json!({})).await)?;
    let session = instance.session(&target_id).await?;
    let cdp = instance.cdp().await?;
    let index = history["currentIndex"].as_i64().unwrap_or(0) + delta;
    if let Some(entry) = history["entries"]
        .as_array()
        .and_then(|e| e.get(index as usize))
        .filter(|_| index >= 0)
    {
        cdp.call(
            "Page.navigateToHistoryEntry",
            json!({ "entryId": entry["id"] }),
            Some(&session),
        )
        .await?;
    }
    Ok(())
}

/// JPEG quality for a screencast about to attach (by attach id), when it isn't the default 80:
/// a server streaming to a desktop over the relay sends lighter frames.
static SCREENCAST_QUALITY: std::sync::LazyLock<Mutex<HashMap<String, u8>>> = std::sync::LazyLock::new(Default::default);

#[allow(dead_code)] // used by winferd, not the Tauri app
pub fn set_screencast_quality(attach_id: &str, quality: u8) {
    SCREENCAST_QUALITY.lock().unwrap().insert(attach_id.to_string(), quality.clamp(20, 95));
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Viewport {
    width: u32,
    height: u32,
    device_scale_factor: f64,
    mobile: bool,
}

/// Emulates `v` in the tab and sizes its window to match, so the page keeps its size when an
/// agent's disconnect clears all emulation.
async fn apply_viewport(cdp: &Cdp, target_id: &str, session: &str, v: &Viewport) -> Res<()> {
    cdp.call(
        "Emulation.setDeviceMetricsOverride",
        json!({ "width": v.width, "height": v.height, "deviceScaleFactor": v.device_scale_factor, "mobile": v.mobile }),
        Some(session),
    )
    .await?;
    cdp.call(
        "Emulation.setTouchEmulationEnabled",
        json!({ "enabled": v.mobile }),
        Some(session),
    )
    .await?;
    // Best effort: older browsers lack setContentsSize.
    if let Ok(window) = cdp
        .call("Browser.getWindowForTarget", json!({ "targetId": target_id }), None)
        .await
    {
        let _ = cdp
            .call(
                "Browser.setContentsSize",
                json!({ "windowId": window["windowId"], "width": v.width, "height": v.height }),
                None,
            )
            .await;
    }
    Ok(())
}

/// Streams the tab's frames (JPEG bytes) to `on_frame`, replacing any previous screencast.
#[tauri::command]
pub async fn browser_attach(
    app: AppHandle,
    manager: State<'_, BrowserManager>,
    workspace_id: String,
    target_id: String,
    viewport: Viewport,
    attach_id: String,
    on_frame: Channel,
) -> Res<()> {
    let instance = instance(&app, &manager, &workspace_id).await?;
    *instance.wanted_attach.lock().unwrap() = Some(attach_id.clone());
    let cdp = instance.cdp().await?;
    let _guard = instance.attach_lock.lock().await;
    // The panel moved on (another tab or device) while this waited.
    if instance.wanted_attach.lock().unwrap().as_deref() != Some(attach_id.as_str()) {
        return Ok(());
    }
    let previous = instance.screencast.lock().unwrap().take();
    if let Some(prev) = previous {
        let _ = cdp
            .call("Page.stopScreencast", json!({}), Some(&prev.session_id))
            .await;
    }
    let what = format!("[{workspace_id}] show tab {}", short(&target_id));
    log::info!("{what}");
    let session = log::check(&what, instance.session(&target_id).await)?;
    // Whichever tab an agent last drove may be in front; a tab behind it shows nothing and
    // ignores typing.
    front(&cdp, &target_id, &session, &workspace_id).await;
    log::check(&what, apply_viewport(&cdp, &target_id, &session, &viewport).await)?;
    *instance.screencast.lock().unwrap() = Some(Screencast {
        attach_id: attach_id.clone(),
        target_id,
        session_id: session.clone(),
        channel: on_frame,
        sizing: Sizing::new(viewport),
        got_frame: false,
        frames: 0,
        acked: 0,
        ack_id: Value::Null,
    });
    let result = cdp
        .call(
            "Page.startScreencast",
            json!({ "format": "jpeg", "quality": SCREENCAST_QUALITY.lock().unwrap().remove(&attach_id).unwrap_or(80), "everyNthFrame": 1 }),
            Some(&session),
        )
        .await;
    log::check(&what, result)?;
    // Frames only come when the page paints; a static page that has painted already sends none
    // and the panel would stay blank. Show a screenshot until the first frame.
    let instance = instance.clone();
    tauri::async_runtime::spawn(async move {
        let shot = cdp
            .call("Page.captureScreenshot", json!({ "format": "jpeg", "quality": 80 }), Some(&session))
            .await;
        let Some(bytes) = shot
            .ok()
            .and_then(|r| r["data"].as_str().and_then(|d| base64::engine::general_purpose::STANDARD.decode(d).ok()))
        else {
            return;
        };
        let screencast = instance.screencast.lock().unwrap();
        if let Some(sc) = screencast.as_ref().filter(|sc| sc.attach_id == attach_id && !sc.got_frame) {
            let _ = sc.channel.send(InvokeResponseBody::Raw(bytes));
        }
    });
    Ok(())
}

/// The view drew (or skipped for a newer one) a frame of the screencast `attach_id` started.
#[tauri::command]
pub async fn browser_frame_done(
    manager: State<'_, BrowserManager>,
    workspace_id: String,
    attach_id: String,
) -> Res<()> {
    let instance = manager.instances.lock().unwrap().get(&workspace_id).cloned();
    if let Some(instance) = instance {
        instance.ack_frames(&attach_id, None);
    }
    Ok(())
}

#[tauri::command]
pub async fn browser_detach(
    manager: State<'_, BrowserManager>,
    workspace_id: String,
    attach_id: String,
) -> Res<()> {
    let Some(instance) = manager
        .instances
        .lock()
        .unwrap()
        .get(&workspace_id)
        .cloned()
    else {
        return Ok(());
    };
    {
        let mut wanted = instance.wanted_attach.lock().unwrap();
        if wanted.as_deref() == Some(attach_id.as_str()) {
            *wanted = None;
        }
    }
    let _guard = instance.attach_lock.lock().await;
    let previous = {
        let mut current = instance.screencast.lock().unwrap();
        if current.as_ref().is_some_and(|sc| sc.attach_id == attach_id) {
            current.take()
        } else {
            None
        }
    };
    if let (Some(prev), Ok(cdp)) = (previous, instance.cdp().await) {
        let _ = cdp
            .call("Page.stopScreencast", json!({}), Some(&prev.session_id))
            .await;
    }
    Ok(())
}

#[tauri::command]
pub async fn browser_resize(
    app: AppHandle,
    manager: State<'_, BrowserManager>,
    workspace_id: String,
    target_id: String,
    viewport: Viewport,
) -> Res<()> {
    let instance = instance(&app, &manager, &workspace_id).await?;
    let session = instance.session(&target_id).await?;
    let cdp = instance.cdp().await?;
    if let Some(sc) = instance
        .screencast
        .lock()
        .unwrap()
        .as_mut()
        .filter(|sc| sc.session_id == session)
    {
        sc.sizing.set(viewport.clone());
    }
    log::check(
        &format!("[{workspace_id}] resize tab {} to {}x{}", short(&target_id), viewport.width, viewport.height),
        apply_viewport(&cdp, &target_id, &session, &viewport).await,
    )
}

/// Forwards one input event; `event` is a CDP Input.* method name plus its params.
#[tauri::command]
pub async fn browser_input(
    app: AppHandle,
    manager: State<'_, BrowserManager>,
    workspace_id: String,
    target_id: String,
    method: String,
    params: Value,
) -> Res<()> {
    if !matches!(
        method.as_str(),
        "Input.dispatchMouseEvent" | "Input.dispatchKeyEvent" | "Input.insertText"
    ) {
        return Err(format!("unsupported input method {method}"));
    }
    let instance = instance(&app, &manager, &workspace_id).await?;
    // Only the event type: key events would put what the user types in the log.
    let what = format!("[{workspace_id}] {method} {} in tab {}", params["type"].as_str().unwrap_or_default(), short(&target_id));
    log::check(&what, instance.call_target(&target_id, &method, params).await).map(drop)
}

/// Types a plugin's secret into the field matching `selector` (replacing its contents) without
/// the value passing through the webview or the agent that asked for it.
#[tauri::command]
pub async fn browser_fill_secret(
    app: AppHandle,
    manager: State<'_, BrowserManager>,
    workspace_id: String,
    target_id: String,
    selector: String,
    plugin: String,
    key: String,
) -> Res<()> {
    let value = crate::vault::value(&plugin, &key)?;
    let instance = instance(&app, &manager, &workspace_id).await?;
    let session = instance.session(&target_id).await?;
    let cdp = instance.cdp().await?;
    let selector = serde_json::to_string(&selector).map_err(|e| e.to_string())?;
    let focus = format!(
        "(() => {{ const el = document.querySelector({selector}); if (!el) return 'missing'; \
         const editable = el.isContentEditable || el.tagName === 'TEXTAREA' || (el.tagName === 'INPUT' && !['button','submit','checkbox','radio','file','hidden'].includes(el.type)); \
         if (!editable) return 'not-editable'; el.scrollIntoView({{ block: 'center' }}); el.focus(); \
         if (el.select) el.select(); else document.execCommand('selectAll'); return 'ok'; }})()"
    );
    let res = cdp
        .call("Runtime.evaluate", json!({ "expression": focus, "returnByValue": true }), Some(&session))
        .await?;
    match res["result"]["value"].as_str() {
        Some("ok") => {}
        Some("missing") => return Err(format!("no element matches {selector} on this page")),
        Some("not-editable") => return Err(format!("{selector} isn't a text field")),
        _ => return Err("couldn't reach the page".into()),
    }
    cdp.call("Input.insertText", json!({ "text": value }), Some(&session)).await?;
    Ok(())
}

/// Script for `browser_select_probe`: describes the `<select>` at the point and remembers it.
fn select_probe_js(x: f64, y: f64) -> String {
    format!(
        "(() => {{ const el = document.elementFromPoint({x}, {y}); const s = el && el.closest && el.closest('select'); \
         if (!s || s.disabled || s.multiple || s.size > 1) return null; window.__winferSelect = s; \
         const r = s.getBoundingClientRect(); const options = []; \
         const add = (o, group) => {{ if (!o.hidden) options.push({{ index: o.index, label: o.label || o.text, disabled: o.disabled, group }}); }}; \
         for (const c of s.children) {{ \
           if (c.tagName === 'OPTGROUP') {{ options.push({{ index: -1, label: c.label, disabled: true, group: true }}); for (const o of c.children) if (o.tagName === 'OPTION') add(o, false); }} \
           else if (c.tagName === 'OPTION') add(c, false); }} \
         return {{ rect: {{ x: r.x, y: r.y, width: r.width, height: r.height }}, options, selected: s.selectedIndex }}; }})()"
    )
}

/// Script for `browser_select_pick`: applies `index` (`None`: just forget the remembered select).
fn select_pick_js(index: Option<i64>) -> String {
    let index = index.map_or("null".to_string(), |i| i.to_string());
    format!(
        "(() => {{ const s = window.__winferSelect; delete window.__winferSelect; if (!s) return; const i = {index}; s.focus(); \
         if (i !== null && s.selectedIndex !== i) {{ s.selectedIndex = i; \
           s.dispatchEvent(new Event('input', {{ bubbles: true }})); s.dispatchEvent(new Event('change', {{ bubbles: true }})); }} }})()"
    )
}

/// Finds the dropdown `<select>` under a page point (CSS pixels) for the panel's own option list:
/// headless Chrome opens a native popup that the screencast never paints. Remembers the element for
/// `browser_select_pick`. `None` for anything else (including multi-selects and listboxes, which
/// the page draws itself).
#[tauri::command]
pub async fn browser_select_probe(
    app: AppHandle,
    manager: State<'_, BrowserManager>,
    workspace_id: String,
    target_id: String,
    x: f64,
    y: f64,
) -> Res<Option<Value>> {
    let instance = instance(&app, &manager, &workspace_id).await?;
    let expression = select_probe_js(x, y);
    let res = instance
        .call_target(&target_id, "Runtime.evaluate", json!({ "expression": expression, "returnByValue": true }))
        .await;
    let res = log::check(&format!("[{workspace_id}] select probe in tab {}", short(&target_id)), res)?;
    Ok(Some(res["result"]["value"].clone()).filter(|v| v.is_object()))
}

/// Applies the panel's option list choice to the `<select>` `browser_select_probe` found (`None`:
/// dismissed), firing the `input` and `change` events a real pick does.
#[tauri::command]
pub async fn browser_select_pick(
    app: AppHandle,
    manager: State<'_, BrowserManager>,
    workspace_id: String,
    target_id: String,
    index: Option<i64>,
) -> Res<()> {
    let instance = instance(&app, &manager, &workspace_id).await?;
    let expression = select_pick_js(index);
    let res = instance
        .call_target(&target_id, "Runtime.evaluate", json!({ "expression": expression }))
        .await;
    log::check(&format!("[{workspace_id}] select pick in tab {}", short(&target_id)), res).map(drop)
}

/// Selected text in the page (for ⌘C).
#[tauri::command]
pub async fn browser_selection(
    app: AppHandle,
    manager: State<'_, BrowserManager>,
    workspace_id: String,
    target_id: String,
) -> Res<String> {
    let instance = instance(&app, &manager, &workspace_id).await?;
    let res = instance
        .call_target(
            &target_id,
            "Runtime.evaluate",
            json!({ "expression": "window.getSelection().toString()", "returnByValue": true }),
        )
        .await;
    let res = log::check(&format!("[{workspace_id}] selection in tab {}", short(&target_id)), res)?;
    Ok(res["result"]["value"]
        .as_str()
        .unwrap_or_default()
        .to_string())
}

/// Script for `browser_image_at`. `__X__`, `__Y__` and `__DATA__` are filled in: with data, it
/// reads the image's pixels as a PNG data URL (which cross-origin or SVG images refuse; the caller
/// then screenshots `rect` instead).
const IMAGE_AT_JS: &str = "(async () => { \
  const el = document.elementFromPoint(__X__, __Y__); \
  const pic = el && el.closest && el.closest('picture'); \
  const img = el && el.closest ? (el.closest('img, canvas') || (pic && pic.querySelector('img'))) : null; \
  if (!img) return null; \
  const r = img.getBoundingClientRect(); \
  const x = Math.max(0, r.x), y = Math.max(0, r.y); \
  const rect = { x: x + scrollX, y: y + scrollY, width: Math.min(r.x + r.width, innerWidth) - x, height: Math.min(r.y + r.height, innerHeight) - y }; \
  const src = img.tagName === 'IMG' ? (img.currentSrc || img.src) : ''; \
  if (!__DATA__) return { src, rect }; \
  try { \
    let source = img; \
    if (img.tagName === 'IMG') { \
      const bitmap = await createImageBitmap(await (await fetch(src)).blob()); \
      source = document.createElement('canvas'); source.width = bitmap.width; source.height = bitmap.height; \
      source.getContext('2d').drawImage(bitmap, 0, 0); \
    } \
    return { src, rect, data: source.toDataURL('image/png') }; \
  } catch (e) { return { src, rect }; } \
})()";

/// The image (`<img>` or `<canvas>`) under a page point (CSS pixels), for the panel's image menu.
/// `None` when there is none. Without `with_data` only the address is returned; with it the pixels
/// too, as base64 PNG (`data`).
#[tauri::command]
pub async fn browser_image_at(
    app: AppHandle,
    manager: State<'_, BrowserManager>,
    workspace_id: String,
    target_id: String,
    x: f64,
    y: f64,
    with_data: bool,
) -> Res<Option<Value>> {
    let instance = instance(&app, &manager, &workspace_id).await?;
    let expression = IMAGE_AT_JS
        .replace("__X__", &x.to_string())
        .replace("__Y__", &y.to_string())
        .replace("__DATA__", if with_data { "true" } else { "false" });
    let what = format!("[{workspace_id}] image at point in tab {}", short(&target_id));
    let res = instance
        .call_target(&target_id, "Runtime.evaluate", json!({ "expression": expression, "returnByValue": true, "awaitPromise": true }))
        .await;
    let res = log::check(&what, res)?;
    let found = &res["result"]["value"];
    if !found.is_object() {
        return Ok(None);
    }
    let src = found["src"].as_str().unwrap_or_default();
    if !with_data {
        return Ok(Some(json!({ "src": src })));
    }
    let data = match found["data"].as_str() {
        Some(url) => url.strip_prefix("data:image/png;base64,").unwrap_or(url).to_string(),
        None => {
            // The page wouldn't hand over the pixels: take what's painted there instead.
            let rect = &found["rect"];
            let (width, height) = (rect["width"].as_f64().unwrap_or(0.0), rect["height"].as_f64().unwrap_or(0.0));
            if width < 1.0 || height < 1.0 {
                return Err("the image is off screen".into());
            }
            let shot = instance
                .call_target(
                    &target_id,
                    "Page.captureScreenshot",
                    json!({
                        "format": "png",
                        "clip": { "x": rect["x"], "y": rect["y"], "width": width, "height": height, "scale": 1 },
                        "captureBeyondViewport": false
                    }),
                )
                .await;
            let shot = log::check(&format!("[{workspace_id}] image screenshot in tab {}", short(&target_id)), shot)?;
            shot["data"].as_str().ok_or("screenshot returned no data")?.to_string()
        }
    };
    Ok(Some(json!({ "src": src, "data": data })))
}

/// Script for `browser_paste_image`: fires a `paste` event carrying the image on the focused
/// element, as a real paste would. A file input with nothing handling the event gets the file.
const PASTE_IMAGE_JS: &str = "(() => { \
  const bin = atob(__DATA__); const bytes = new Uint8Array(bin.length); \
  for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i); \
  const file = new File([bytes], __NAME__, { type: __MIME__ }); \
  const transfer = new DataTransfer(); transfer.items.add(file); \
  let target = document.activeElement; \
  while (target && target.shadowRoot && target.shadowRoot.activeElement) target = target.shadowRoot.activeElement; \
  if (!target) target = document.body; \
  const event = new ClipboardEvent('paste', { clipboardData: transfer, bubbles: true, cancelable: true, composed: true }); \
  target.dispatchEvent(event); \
  if (!event.defaultPrevented && target.tagName === 'INPUT' && target.type === 'file') { \
    target.files = transfer.files; \
    target.dispatchEvent(new Event('input', { bubbles: true })); target.dispatchEvent(new Event('change', { bubbles: true })); \
  } \
})()";

/// Pastes an image (base64 `data` of type `mime`) into the tab: headless Chrome has no system
/// clipboard to paste from, so the page gets the `paste` event with the image attached.
#[tauri::command]
pub async fn browser_paste_image(
    app: AppHandle,
    manager: State<'_, BrowserManager>,
    workspace_id: String,
    target_id: String,
    data: String,
    mime: String,
) -> Res<()> {
    if !mime.starts_with("image/") || !data.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/' | b'=')) {
        return Err("not an image".into());
    }
    let ext = mime.trim_start_matches("image/").split(['+', ';']).next().unwrap_or("png");
    let quote = |s: &str| serde_json::to_string(s).unwrap_or_default();
    let expression = PASTE_IMAGE_JS
        .replace("__DATA__", &quote(&data))
        .replace("__NAME__", &quote(&format!("image.{ext}")))
        .replace("__MIME__", &quote(&mime));
    let instance = instance(&app, &manager, &workspace_id).await?;
    let res = instance
        .call_target(&target_id, "Runtime.evaluate", json!({ "expression": expression }))
        .await;
    log::check(&format!("[{workspace_id}] paste image in tab {}", short(&target_id)), res).map(drop)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    /// The policy is per tab (a page session, so other workspaces' tabs keep it off),
    /// reversible, and supported by real Chrome.
    #[test]
    fn certificate_policy_can_be_enabled_and_revoked() {
        let tmp = std::env::temp_dir().join(format!("winfer-tls-{:x}", rand::random::<u32>()));
        let Some(exe) = chrome::find(&tmp.join("managed")) else {
            eprintln!("no Chrome found; skipping");
            return;
        };
        let launched = chrome::launch(&exe, &tmp.join("profile")).unwrap();
        let _guard = chrome::ChromeGuard(launched.child);
        tauri::async_runtime::block_on(async {
            let cdp = Cdp::connect(&chrome::wait_ws_url(launched.port, Duration::from_secs(45)).unwrap(), |_| {}).await.unwrap();
            let target = cdp.call("Target.createTarget", json!({ "url": "about:blank" }), None).await.unwrap();
            let session = cdp
                .call("Target.attachToTarget", json!({ "targetId": target["targetId"], "flatten": true }), None)
                .await
                .unwrap()["sessionId"]
                .as_str()
                .unwrap()
                .to_string();
            for allow in [false, true, false] {
                set_certificate_policy(&cdp, &session, allow).await.unwrap();
            }
        });
    }

    #[test]
    fn adopts_an_agents_size_but_not_stale_frames() {
        let v = Viewport { width: 1000, height: 700, device_scale_factor: 2.0, mobile: false };
        let frame = |w: u32, h: u32| json!({ "deviceWidth": w, "deviceHeight": h });
        let mut sizing = Sizing::new(v.clone());
        assert_eq!(sizing.on_frame(&frame(1000, 700)), FrameAction::Check);
        assert_eq!(sizing.on_frame(&frame(1000, 700)), FrameAction::None, "checks are throttled");
        assert_eq!(sizing.on_frame(&json!({})), FrameAction::None);

        // The panel resized: frames at the old size are still in flight.
        sizing.set(Viewport { width: 800, ..v.clone() });
        assert_eq!(sizing.on_frame(&frame(1000, 700)), FrameAction::None);

        // An agent resized: adopt it, keeping our scale, and ignore the panel's old size after.
        assert_eq!(sizing.on_frame(&frame(500, 400)), FrameAction::Adopt(500, 400));
        assert_eq!((sizing.viewport.width, sizing.viewport.device_scale_factor), (500, 2.0));
        assert_eq!(sizing.on_frame(&frame(800, 700)), FrameAction::None);

        // A screenshot's frames in device pixels (now, or of the size just replaced) aren't resizes.
        assert_eq!(sizing.on_frame(&frame(1000, 800)), FrameAction::None);
        assert_eq!(sizing.on_frame(&frame(1600, 1400)), FrameAction::None);
        let mut one_x = Sizing::new(Viewport { device_scale_factor: 1.0, ..v.clone() });
        assert_eq!(one_x.on_frame(&frame(2000, 1400)), FrameAction::Adopt(2000, 1400), "at 1x there's no such mix-up");
    }

    /// Real Chrome: a review screenshot loads the page in a throwaway context and returns a JPEG.
    #[test]
    fn screenshots_a_page_in_a_hidden_context() {
        let tmp = std::env::temp_dir().join(format!("winfer-shot-{:x}", rand::random::<u32>()));
        let Some(exe) = chrome::find(&tmp.join("managed")) else {
            eprintln!("no Chrome found; skipping");
            return;
        };
        let launched = chrome::launch(&exe, &tmp.join("profile")).unwrap();
        let _guard = chrome::ChromeGuard(launched.child);
        tauri::async_runtime::block_on(async {
            let cdp = Cdp::connect(&chrome::wait_ws_url(launched.port, Duration::from_secs(45)).unwrap(), |_| {}).await.unwrap();
            let context = cdp
                .call("Target.createBrowserContext", json!({}), None)
                .await
                .unwrap()["browserContextId"]
                .as_str()
                .unwrap()
                .to_string();
            let shot = capture(&cdp, &context, "data:text/html,<h1 style='color:red'>winfer</h1>", false)
                .await
                .unwrap();
            let jpeg = base64::engine::general_purpose::STANDARD
                .decode(shot.strip_prefix("data:image/jpeg;base64,").unwrap())
                .unwrap();
            assert_eq!(&jpeg[..2], &[0xFF, 0xD8], "JPEG magic");
            assert!(capture(&cdp, &context, "http://127.0.0.1:9/", false).await.is_err(), "refused URL errors");
        });
    }

    /// Real Chrome: a connection whose browser goes away reports itself closed, so it's replaced.
    #[test]
    fn notices_a_lost_connection() {
        let tmp = std::env::temp_dir().join(format!("winfer-lost-{:x}", rand::random::<u32>()));
        let Some(exe) = chrome::find(&tmp.join("managed")) else {
            eprintln!("no Chrome found; skipping");
            return;
        };
        let launched = chrome::launch(&exe, &tmp.join("profile")).unwrap();
        let mut guard = chrome::ChromeGuard(launched.child);
        tauri::async_runtime::block_on(async {
            let cdp = Cdp::connect(&chrome::wait_ws_url(launched.port, Duration::from_secs(45)).unwrap(), |_| {}).await.unwrap();
            cdp.call("Browser.getVersion", json!({}), None).await.unwrap();
            assert!(!cdp.is_closed());
            guard.0.kill().unwrap();
            tokio::time::timeout(Duration::from_secs(10), cdp.closed()).await.expect("close noticed");
            assert!(cdp.is_closed());
            assert!(cdp.call("Browser.getVersion", json!({}), None).await.is_err());
        });
    }

    /// Real Chrome: an animating page's screencast stops after a few unacked frames, and each
    /// ack (with any frame's `sessionId`) lets it go on: what `ack_frames`' pacing relies on.
    #[test]
    fn screencast_waits_for_acks() {
        let tmp = std::env::temp_dir().join(format!("winfer-ack-{:x}", rand::random::<u32>()));
        let Some(exe) = chrome::find(&tmp.join("managed")) else {
            eprintln!("no Chrome found; skipping");
            return;
        };
        let launched = chrome::launch(&exe, &tmp.join("profile")).unwrap();
        let _guard = chrome::ChromeGuard(launched.child);
        tauri::async_runtime::block_on(async {
            let (tx, rx) = mpsc::channel::<Event>();
            let tx = Mutex::new(tx);
            let cdp = Cdp::connect(&chrome::wait_ws_url(launched.port, Duration::from_secs(45)).unwrap(), move |e| {
                let _ = tx.lock().unwrap().send(e);
            })
            .await
            .unwrap();
            let page = "data:text/html,<style>@keyframes s{to{transform:rotate(360deg)}}div{width:200px;height:200px;background:red;animation:s .5s linear infinite}</style><div></div>";
            let target = cdp.call("Target.createTarget", json!({ "url": page }), None).await.unwrap();
            let session = cdp
                .call("Target.attachToTarget", json!({ "targetId": target["targetId"], "flatten": true }), None)
                .await
                .unwrap()["sessionId"]
                .as_str()
                .unwrap()
                .to_string();
            cdp.call("Page.enable", json!({}), Some(&session)).await.unwrap();
            cdp.call("Page.startScreencast", json!({ "format": "jpeg", "quality": 60, "everyNthFrame": 1 }), Some(&session))
                .await
                .unwrap();
            // Frames over `secs`, acking each one (or none), and the last one's ack id.
            let count = |ack: bool, secs: u64| {
                let deadline = std::time::Instant::now() + Duration::from_secs(secs);
                let (mut frames, mut last) = (0, Value::Null);
                while let Some(left) = deadline.checked_duration_since(std::time::Instant::now()) {
                    let Ok(e) = rx.recv_timeout(left) else { break };
                    if e.method != "Page.screencastFrame" {
                        continue;
                    }
                    frames += 1;
                    last = e.params["sessionId"].clone();
                    if ack {
                        cdp.send("Page.screencastFrameAck", json!({ "sessionId": last }), Some(&session));
                    }
                }
                (frames, last)
            };
            assert!(count(true, 2).0 > 6, "an acked screencast of an animation streams");
            let (unacked, last) = count(false, 2);
            assert!((1..=4).contains(&unacked), "{unacked} frames came without acks");
            assert_eq!(count(false, 1).0, 0, "it waits for acks");
            // Late acks, all with the newest frame's id, release it.
            for _ in 0..unacked {
                cdp.send("Page.screencastFrameAck", json!({ "sessionId": last }), Some(&session));
            }
            assert!(count(true, 2).0 > 6, "acks let it go on");
        });
    }

    /// Real Chrome: discover tabs, screencast a frame, type into a page. Skips without Chrome.
    #[test]
    fn screencast_and_input_round_trip() {
        let tmp = std::env::temp_dir().join(format!("winfer-cdp-{:x}", rand::random::<u32>()));
        let Some(exe) = chrome::find(&tmp.join("managed")) else {
            eprintln!("no Chrome found; skipping");
            return;
        };
        let launched = chrome::launch(&exe, &tmp.join("profile")).unwrap();
        let _guard = chrome::ChromeGuard(launched.child);

        tauri::async_runtime::block_on(async {
            let (tx, rx) = mpsc::channel::<Event>();
            let tx = Mutex::new(tx);
            let cdp = Cdp::connect(&chrome::wait_ws_url(launched.port, Duration::from_secs(45)).unwrap(), move |e| {
                let _ = tx.lock().unwrap().send(e);
            })
            .await
            .unwrap();
            cdp.call(
                "Target.setDiscoverTargets",
                json!({ "discover": true }),
                None,
            )
            .await
            .unwrap();

            let page = "data:text/html,<input id=i autofocus><h1>winfer</h1>";
            let target = cdp
                .call("Target.createTarget", json!({ "url": page }), None)
                .await
                .unwrap();
            let target_id = target["targetId"].as_str().unwrap().to_string();
            let session = cdp
                .call(
                    "Target.attachToTarget",
                    json!({ "targetId": target_id, "flatten": true }),
                    None,
                )
                .await
                .unwrap()["sessionId"]
                .as_str()
                .unwrap()
                .to_string();
            cdp.call("Page.enable", json!({}), Some(&session))
                .await
                .unwrap();
            cdp.call(
                "Emulation.setDeviceMetricsOverride",
                json!({ "width": 400, "height": 300, "deviceScaleFactor": 1, "mobile": false }),
                Some(&session),
            )
            .await
            .unwrap();
            cdp.call(
                "Page.startScreencast",
                json!({ "format": "jpeg", "quality": 80 }),
                Some(&session),
            )
            .await
            .unwrap();

            // Discovery reported the page, and a JPEG frame arrives. Frames only come when the
            // page paints, which a background tab on a slow CI machine may not do by itself:
            // repaint now and then until one shows up.
            let deadline = std::time::Instant::now() + Duration::from_secs(30);
            let (mut saw_target, mut frame) = (false, None);
            let mut repaints = 0u32;
            while std::time::Instant::now() < deadline && (!saw_target || frame.is_none()) {
                let Ok(e) = rx.recv_timeout(Duration::from_millis(200)) else {
                    if frame.is_none() {
                        repaints += 1;
                        cdp.send(
                            "Runtime.evaluate",
                            json!({ "expression": format!("document.documentElement.style.background = 'hsl({}, 50%, 50%)'", repaints * 37 % 360) }),
                            Some(&session),
                        );
                    }
                    continue;
                };
                if e.method == "Target.targetCreated"
                    && e.params["targetInfo"]["targetId"] == target_id.as_str()
                {
                    saw_target = true;
                }
                if e.method == "Page.screencastFrame" && frame.is_none() {
                    let bytes = base64::engine::general_purpose::STANDARD
                        .decode(e.params["data"].as_str().unwrap())
                        .unwrap();
                    cdp.send(
                        "Page.screencastFrameAck",
                        json!({ "sessionId": e.params["sessionId"] }),
                        Some(&session),
                    );
                    frame = Some(bytes);
                }
            }
            assert!(saw_target, "target discovery event missing");
            let frame = frame.expect("no screencast frame");
            assert_eq!(&frame[..2], &[0xFF, 0xD8], "not a JPEG");

            // Typing reaches the focused input.
            cdp.call(
                "Input.insertText",
                json!({ "text": "hello" }),
                Some(&session),
            )
            .await
            .unwrap();
            let value = cdp
                .call("Runtime.evaluate", json!({ "expression": "document.getElementById('i').value", "returnByValue": true }), Some(&session))
                .await
                .unwrap();
            assert_eq!(value["result"]["value"], "hello");
        });
    }

    /// Real Chrome: showing tabs the way `browser_attach` does (viewport, screencast, a first
    /// screenshot) never reads as an agent resizing the page, though the screenshot makes Chrome
    /// report frames in device pixels.
    #[test]
    fn showing_a_tab_again_keeps_its_size() {
        let tmp = std::env::temp_dir().join(format!("winfer-switch-{:x}", rand::random::<u32>()));
        let Some(exe) = chrome::find(&tmp.join("managed")) else {
            eprintln!("no Chrome found; skipping");
            return;
        };
        let launched = chrome::launch(&exe, &tmp.join("profile")).unwrap();
        let _guard = chrome::ChromeGuard(launched.child);
        tauri::async_runtime::block_on(async {
            let (tx, rx) = mpsc::channel::<Event>();
            let tx = Mutex::new(tx);
            let cdp = Cdp::connect(&chrome::wait_ws_url(launched.port, Duration::from_secs(45)).unwrap(), move |e| {
                let _ = tx.lock().unwrap().send(e);
            })
            .await
            .unwrap();
            let context = cdp.call("Target.createBrowserContext", json!({}), None).await.unwrap()["browserContextId"].as_str().unwrap().to_string();
            let page = "data:text/html,<style>@keyframes s{to{transform:rotate(360deg)}}div{width:50px;height:50px;background:red;animation:s 1s linear infinite}</style><div></div>";
            let mut tabs = Vec::new();
            for _ in 0..2 {
                let id = cdp.call("Target.createTarget", json!({ "url": page, "browserContextId": context }), None).await.unwrap()["targetId"].as_str().unwrap().to_string();
                let session = cdp.call("Target.attachToTarget", json!({ "targetId": id, "flatten": true }), None).await.unwrap()["sessionId"].as_str().unwrap().to_string();
                cdp.call("Page.enable", json!({}), Some(&session)).await.unwrap();
                tabs.push((id, session));
            }
            let v = Viewport { width: 900, height: 600, device_scale_factor: 2.0, mobile: false };
            let mut shown: Option<String> = None;
            let mut device_pixel_frames = 0;
            for round in 0..4 {
                let (id, session) = &tabs[round % 2];
                if let Some(previous) = &shown {
                    let _ = cdp.call("Page.stopScreencast", json!({}), Some(previous)).await;
                }
                front(&cdp, id, session, "test").await;
                apply_viewport(&cdp, id, session, &v).await.unwrap();
                cdp.call("Page.startScreencast", json!({ "format": "jpeg", "quality": 50, "everyNthFrame": 1 }), Some(session)).await.unwrap();
                cdp.call("Page.captureScreenshot", json!({ "format": "jpeg", "quality": 80 }), Some(session)).await.unwrap();
                let mut sizing = Sizing::new(v.clone());
                let deadline = std::time::Instant::now() + Duration::from_millis(1200);
                while let Some(left) = deadline.checked_duration_since(std::time::Instant::now()) {
                    let Ok(e) = rx.recv_timeout(left) else { break };
                    if e.method != "Page.screencastFrame" || e.session_id.as_deref() != Some(session.as_str()) {
                        continue;
                    }
                    cdp.send("Page.screencastFrameAck", json!({ "sessionId": e.params["sessionId"] }), Some(session));
                    let m = &e.params["metadata"];
                    if m["deviceWidth"] == 1800 {
                        device_pixel_frames += 1;
                    }
                    assert!(!matches!(sizing.on_frame(m), FrameAction::Adopt(..)), "round {round}: frame {m} taken for a resize");
                }
                let size = cdp.call("Runtime.evaluate", json!({ "expression": "[innerWidth, innerHeight]", "returnByValue": true }), Some(session)).await.unwrap();
                assert_eq!(size["result"]["value"], json!([900, 600]), "round {round}");
                shown = Some(session.clone());
            }
            eprintln!("{device_pixel_frames} frames came in device pixels");
        });
    }

    /// The panel's tab after an agent opened another one in front: it must paint and take typing.
    #[test]
    fn shown_tab_gets_focus_back_from_an_agents_tab() {
        let tmp = std::env::temp_dir().join(format!("winfer-cdp-{:x}", rand::random::<u32>()));
        let Some(exe) = chrome::find(&tmp.join("managed")) else {
            eprintln!("no Chrome found; skipping");
            return;
        };
        let launched = chrome::launch(&exe, &tmp.join("profile")).unwrap();
        let _guard = chrome::ChromeGuard(launched.child);

        tauri::async_runtime::block_on(async {
            let ws = chrome::wait_ws_url(launched.port, Duration::from_secs(45)).unwrap();
            let cdp = Cdp::connect(&ws, |_| {}).await.unwrap();
            let open = |url: &'static str| {
                let cdp = cdp.clone();
                async move {
                    let id = cdp.call("Target.createTarget", json!({ "url": url }), None).await.unwrap()["targetId"].as_str().unwrap().to_string();
                    let session = cdp.call("Target.attachToTarget", json!({ "targetId": id, "flatten": true }), None).await.unwrap()["sessionId"]
                        .as_str()
                        .unwrap()
                        .to_string();
                    (id, session)
                }
            };
            let eval = |session: String, expr: &'static str| {
                let cdp = cdp.clone();
                async move {
                    cdp.call("Runtime.evaluate", json!({ "expression": expr, "returnByValue": true }), Some(&session)).await.unwrap()["result"]["value"].clone()
                }
            };
            let (shown, session) = open("data:text/html,<input id=i>").await;
            let _agent_tab = open("data:text/html,agent").await;
            tokio::time::sleep(Duration::from_millis(500)).await;
            assert_eq!(eval(session.clone(), "document.visibilityState").await, "hidden");
            assert_eq!(eval(session.clone(), "document.hasFocus()").await, false);

            front(&cdp, &shown, &session, "test").await;
            tokio::time::sleep(Duration::from_millis(300)).await;
            assert_eq!(eval(session.clone(), "document.visibilityState").await, "visible");
            assert_eq!(eval(session.clone(), "document.hasFocus()").await, true);

            // Click-to-focus then type, the way the panel forwards input.
            eval(session.clone(), "document.getElementById('i').focus()").await;
            for (kind, text) in [("keyDown", Some("h")), ("keyUp", None), ("keyDown", Some("i")), ("keyUp", None)] {
                let mut params = json!({ "type": kind, "key": text.unwrap_or("i") });
                if let Some(t) = text {
                    params["text"] = json!(t);
                }
                cdp.call("Input.dispatchKeyEvent", params, Some(&session)).await.unwrap();
            }
            assert_eq!(eval(session.clone(), "document.getElementById('i').value").await, "hi");
        });
    }

    /// Real Chrome: the scripts find a dropdown under a point, list its options (groups and
    /// disabled ones included), skip other elements, and a pick fires `input` then `change`.
    #[test]
    fn select_probe_and_pick() {
        let tmp = std::env::temp_dir().join(format!("winfer-select-{:x}", rand::random::<u32>()));
        let Some(exe) = chrome::find(&tmp.join("managed")) else {
            eprintln!("no Chrome found; skipping");
            return;
        };
        let launched = chrome::launch(&exe, &tmp.join("profile")).unwrap();
        let _guard = chrome::ChromeGuard(launched.child);
        tauri::async_runtime::block_on(async {
            let cdp = cdp::Cdp::connect(&chrome::wait_ws_url(launched.port, Duration::from_secs(45)).unwrap(), |_| {})
                .await
                .unwrap();
            let page = "data:text/html,<body style=margin:0>\
                <select id=a style='position:absolute;left:10px;top:20px;width:100px;height:30px'>\
                <option>One<option disabled>Two<optgroup label=G><option>Three</option></optgroup></select>\
                <select id=m multiple style='position:absolute;left:10px;top:100px'><option>x<option>y</select>\
                <button style='position:absolute;left:10px;top:200px'>b</button>\
                <script>window.log=[];a.oninput=()=>log.push('input');a.onchange=()=>log.push('change')</script>";
            let target = cdp.call("Target.createTarget", json!({ "url": page }), None).await.unwrap();
            let session = cdp
                .call("Target.attachToTarget", json!({ "targetId": target["targetId"], "flatten": true }), None)
                .await
                .unwrap()["sessionId"]
                .as_str()
                .unwrap()
                .to_string();
            let eval = |expression: String| {
                let cdp = &cdp;
                let session = session.clone();
                async move {
                    cdp.call("Runtime.evaluate", json!({ "expression": expression, "returnByValue": true }), Some(&session))
                        .await
                        .unwrap()["result"]["value"]
                        .clone()
                }
            };
            // The page may still be loading.
            for _ in 0..50 {
                if eval("document.readyState".into()).await == "complete" {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }

            // Layout may lag the load event, especially on slow runners.
            let mut probe = eval(select_probe_js(50.0, 30.0)).await;
            for _ in 0..50 {
                if !probe.is_null() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
                probe = eval(select_probe_js(50.0, 30.0)).await;
            }
            assert_eq!(probe["rect"]["x"], 10);
            assert_eq!(probe["rect"]["width"], 100);
            assert_eq!(probe["selected"], 0);
            let options = probe["options"].as_array().unwrap();
            let labels: Vec<_> = options.iter().map(|o| o["label"].as_str().unwrap()).collect();
            assert_eq!(labels, ["One", "Two", "G", "Three"]);
            assert_eq!(options[1]["disabled"], true);
            assert_eq!(options[2]["group"], true);
            assert_eq!(options[3]["index"], 2);

            assert!(eval(select_probe_js(20.0, 110.0)).await.is_null(), "multi-select is drawn by the page");
            assert!(eval(select_probe_js(15.0, 205.0)).await.is_null(), "a button isn't a dropdown");
            assert!(eval(select_probe_js(300.0, 300.0)).await.is_null(), "nothing there");

            // The probe for the dropdown again (the misses cleared nothing), then pick "Three".
            eval(select_probe_js(50.0, 30.0)).await;
            eval(select_pick_js(Some(2))).await;
            assert_eq!(eval("a.value".into()).await, "Three");
            assert_eq!(eval("log.join()".into()).await, "input,change");
            // Dismissing changes nothing and forgets the select.
            eval(select_probe_js(50.0, 30.0)).await;
            eval(select_pick_js(None)).await;
            assert_eq!(eval("a.value".into()).await, "Three");
            assert_eq!(eval("String(window.__winferSelect)".into()).await, "undefined");
        });
    }
}
