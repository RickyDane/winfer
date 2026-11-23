//! A proxy in front of the shared browser's DevTools endpoint, one per workspace, handed to that
//! workspace's agents instead of the browser's own port. It keeps them to the workspace's browser
//! context (see [`Scope`]): they see, open and drive only its tabs, and its cookies. It also reads
//! which tab each agent command acts on, so the panel can show the tab an agent is using.
//!
//! Its port stays the same for the app's lifetime, even when the browser restarts, so agents
//! started earlier keep working.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures_util::future::BoxFuture;
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;

/// A single CDP message at or above this size gets logged (with method and tab) before it is
/// forwarded. WebView2 crashed with `partition_alloc` OOM (~1 GiB single allocation) serializing
/// a `Fetch.fulfillRequest` body: the next time an agent pushes something huge through the
/// browser, the diagnostics log should name the method, size and tab first. 64 MiB is far above
/// any normal command (screenshots included) and far below the crash size.
const OVERSIZED_BYTES: usize = 64 * 1024 * 1024;
/// Oversized messages are logged at most once per this window (they often come in bursts).
const OVERSIZED_LOG_GAP: Duration = Duration::from_secs(10);

fn oversized_info(text: &str, limit: usize) -> Option<String> {
    if text.len() < limit {
        return None;
    }
    Some(format!("{} ({} bytes)", method_of(text).unwrap_or_else(|| "response/event".into()), text.len()))
}

/// The `"method":"…"` value, found by scanning instead of parsing: a near-1 GiB body must not
/// be JSON-parsed (that would copy it) just to name it in a log line.
fn method_of(text: &str) -> Option<String> {
    let start = text.find("\"method\"")?;
    let rest = &text[start + 8..];
    let rest = rest.trim_start().strip_prefix(':')?.trim_start().strip_prefix('"')?;
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

/// The `"sessionId":"…"` value, same scanning approach as [`method_of`].
fn session_of(text: &str) -> Option<String> {
    let start = text.find("\"sessionId\"")?;
    let rest = &text[start + 11..];
    let rest = rest.trim_start().strip_prefix(':')?.trim_start().strip_prefix('"')?;
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

fn note_oversized(direction: &str, text: &str, tab: Option<&str>) {
    use std::sync::atomic::{AtomicU64, Ordering};
    static LAST: AtomicU64 = AtomicU64::new(0);
    static SUPPRESSED: AtomicU64 = AtomicU64::new(0);
    let Some(info) = oversized_info(text, OVERSIZED_BYTES) else { return };
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
    let last = LAST.load(Ordering::Relaxed);
    if now.saturating_sub(last) < OVERSIZED_LOG_GAP.as_millis() as u64 {
        SUPPRESSED.fetch_add(1, Ordering::Relaxed);
        return;
    }
    if LAST.compare_exchange(last, now, Ordering::Relaxed, Ordering::Relaxed).is_err() {
        SUPPRESSED.fetch_add(1, Ordering::Relaxed);
        return;
    }
    let suppressed = SUPPRESSED.swap(0, Ordering::Relaxed);
    crate::diagnostics::warn(
        "browser-cdp",
        &format!(
            "oversized CDP message {direction}: {info} tab={}{}",
            tab.unwrap_or("?"),
            if suppressed > 0 { format!(" (+{suppressed} similar suppressed)") } else { String::new() }
        ),
    );
    crate::diagnostics::breadcrumb(format!("oversized CDP {direction}: {info}"));
}

/// What an agent did with a tab (by target id).
#[derive(Debug, PartialEq)]
pub enum Activity<'a> {
    /// It opened the tab itself.
    Opened(&'a str),
    /// It acted on the tab.
    Acted(&'a str),
    /// It navigated the tab to a page (so the panel shows it).
    Navigated(&'a str),
    /// It moved, pressed or released the mouse in the tab (so the panel can draw its cursor).
    Pointer(&'a str, Pointer),
}

/// An agent's mouse event, in page CSS pixels.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Pointer {
    pub x: f64,
    pub y: f64,
    /// CDP's event type: mouseMoved, mousePressed, mouseReleased or mouseWheel.
    pub kind: String,
}

impl Pointer {
    /// The mouse event a command dispatches, if it's one.
    fn of(text: &str) -> Option<Self> {
        if !text.contains("\"Input.dispatchMouseEvent\"") {
            return None;
        }
        let v: Value = serde_json::from_str(text).ok()?;
        let p = &v["params"];
        Some(Self { x: p["x"].as_f64()?, y: p["y"].as_f64()?, kind: p["type"].as_str()?.to_string() })
    }
}

pub type OnActivity = Arc<dyn Fn(Activity) + Send + Sync>;

/// The workspace behind a proxy.
pub trait Upstream: Send + Sync + 'static {
    /// The browser's DevTools port and the workspace's context id, starting either if needed.
    fn prepare(&self) -> BoxFuture<'static, Result<(u16, String), String>>;
    /// Ids of the targets in the workspace's context.
    fn targets(&self) -> BoxFuture<'static, Result<HashSet<String>, String>>;
    /// Opens a tab on `url` in the workspace's context; its target id.
    fn new_tab(&self, url: String) -> BoxFuture<'static, Result<String, String>>;
}

/// After attaching, Playwright sets a tab up (scripts, emulation, evaluations) whether or not it
/// will use it: only input and navigation count as using the tab for this long.
const SETUP: Duration = Duration::from_secs(2);
/// How long a request may wait for the browser's endpoint to answer.
const UPSTREAM_WAIT: Duration = Duration::from_secs(20);

/// A bound, not yet serving, listener: binding is synchronous so `ensure` can hand out its URL.
pub struct Listener {
    inner: std::net::TcpListener,
    pub port: u16,
}

/// The serving proxy; stops accepting when dropped (open connections end with the browser).
pub struct Proxy {
    pub port: u16,
    task: tauri::async_runtime::JoinHandle<()>,
}

impl Drop for Proxy {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Listener {
    pub fn bind() -> std::io::Result<Self> {
        let inner = std::net::TcpListener::bind("127.0.0.1:0")?;
        inner.set_nonblocking(true)?;
        let port = inner.local_addr()?.port();
        Ok(Self { inner, port })
    }

    pub fn serve(self, upstream: Arc<dyn Upstream>, on_activity: OnActivity) -> Proxy {
        let own = self.port;
        let task = tauri::async_runtime::spawn(async move {
            let Ok(listener) = TcpListener::from_std(self.inner) else {
                return;
            };
            while let Ok((stream, _)) = listener.accept().await {
                let (upstream, on_activity) = (upstream.clone(), on_activity.clone());
                tauri::async_runtime::spawn(async move {
                    if let Err(err) = serve(stream, upstream, own, on_activity).await {
                        super::log::warning!("CDP proxy: {err}");
                    }
                });
            }
        });
        Proxy { port: own, task }
    }
}

async fn serve(mut stream: TcpStream, upstream: Arc<dyn Upstream>, own: u16, on_activity: OnActivity) -> Result<(), String> {
    let head = request_head(&stream).await?;
    if head.is_empty() {
        return Ok(());
    }
    let mut parts = head.split_whitespace();
    let (method, path) = (parts.next().unwrap_or("GET").to_string(), parts.next().unwrap_or("/").to_string());
    let prepared = upstream.prepare().await;
    if head.to_ascii_lowercase().contains("upgrade: websocket") {
        let (browser, context) = prepared?;
        return websocket(stream, path, browser, context, upstream, on_activity).await;
    }
    // Plain HTTP (/json/version, /json/list, /json/new…): forward it, keep target lists to the
    // workspace's tabs, and point the URLs in the answer at the proxy, so the agent's WebSocket
    // comes through here too.
    let mut consumed = vec![0; head.len() + 4];
    stream.read_exact(&mut consumed).await.map_err(|e| e.to_string())?;
    let (status, body) = match prepared {
        Ok((browser, _)) => {
            let (status, body) = http(&method, &path, browser, upstream.as_ref()).await?;
            (status, rewrite(&body, browser, own))
        }
        Err(err) => (502, json!({ "error": format!("the browser isn't available: {err}") }).to_string()),
    };
    let reply = format!(
        "HTTP/1.1 {status} {}\r\nContent-Type: application/json; charset=UTF-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        if status < 400 { "OK" } else { "Error" },
        body.len()
    );
    stream.write_all(reply.as_bytes()).await.map_err(|e| e.to_string())?;
    let _ = stream.shutdown().await;
    Ok(())
}

/// The request line and headers, peeked (left in the stream for the WebSocket handshake).
async fn request_head(stream: &TcpStream) -> Result<String, String> {
    let mut buf = vec![0u8; 16 * 1024];
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let n = stream.peek(&mut buf).await.map_err(|e| e.to_string())?;
        if n == 0 {
            return Ok(String::new());
        }
        if let Some(end) = buf[..n].windows(4).position(|w| w == b"\r\n\r\n") {
            return Ok(String::from_utf8_lossy(&buf[..end]).into_owned());
        }
        if n == buf.len() || Instant::now() > deadline {
            return Err("unreadable request".into());
        }
        // `peek` returns at once while the bytes so far are unread: wait for more.
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

/// Answers a plain DevTools HTTP request within the workspace's context.
async fn http(method: &str, path: &str, browser: u16, upstream: &dyn Upstream) -> Result<(u16, String), String> {
    let get = |method: String, path: String| async move {
        tauri::async_runtime::spawn_blocking(move || forward(&method, browser, &path))
            .await
            .map_err(|e| e.to_string())
    };
    let (route, query) = path.split_once('?').unwrap_or((path, ""));
    let route = route.trim_end_matches('/');
    if route == "/json" || route == "/json/list" {
        let (status, body) = get("GET".into(), "/json/list".into()).await?;
        let ours = upstream.targets().await?;
        return Ok((status, list_within(&body, |id| ours.contains(id))));
    }
    if route == "/json/new" {
        let url = if query.is_empty() { "about:blank".to_string() } else { unescape(query) };
        let id = upstream.new_tab(url).await?;
        let (status, body) = get("GET".into(), "/json/list".into()).await?;
        let entry = serde_json::from_str::<Vec<Value>>(&body)
            .ok()
            .and_then(|list| list.into_iter().find(|t| t["id"] == id.as_str()));
        return Ok(match entry {
            Some(entry) => (status, entry.to_string()),
            None => (500, format!("Could not create new page {id}")),
        });
    }
    if let Some(id) = route.strip_prefix("/json/activate/").or_else(|| route.strip_prefix("/json/close/"))
        && !upstream.targets().await?.contains(id)
    {
        return Ok((404, format!("No such target id: {id}")));
    }
    get(method.to_string(), path.to_string()).await
}

/// A `/json/list` answer with only the targets `keep` accepts.
fn list_within(body: &str, keep: impl Fn(&str) -> bool) -> String {
    match serde_json::from_str::<Vec<Value>>(body) {
        Ok(mut list) => {
            list.retain(|t| t["id"].as_str().is_some_and(&keep));
            Value::from(list).to_string()
        }
        Err(_) => body.to_string(),
    }
}

/// `%XX` escapes decoded (the URL in `/json/new?<url>`).
fn unescape(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = bytes.get(i + 1..i + 3).and_then(|h| std::str::from_utf8(h).ok()).and_then(|h| u8::from_str_radix(h, 16).ok());
        match (bytes[i], hex) {
            (b'%', Some(b)) => {
                out.push(b);
                i += 3;
            }
            (b, _) => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn forward(method: &str, browser: u16, path: &str) -> (u16, String) {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(5)))
        .http_status_as_error(false)
        .build()
        .into();
    let url = format!("http://127.0.0.1:{browser}{path}");
    let deadline = Instant::now() + UPSTREAM_WAIT;
    loop {
        let res = if method == "PUT" { agent.put(&url).send_empty() } else { agent.get(&url).call() };
        match res {
            Ok(mut res) => return (res.status().as_u16(), res.body_mut().read_to_string().unwrap_or_default()),
            Err(_) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(200)),
            Err(err) => return (502, format!("{{\"error\":\"the browser isn't reachable: {err}\"}}")),
        }
    }
}

fn rewrite(body: &str, browser: u16, own: u16) -> String {
    body.replace(&format!("127.0.0.1:{browser}"), &format!("127.0.0.1:{own}"))
        .replace(&format!("localhost:{browser}"), &format!("127.0.0.1:{own}"))
}

/// One agent's DevTools connection. Page connections (`/devtools/page/<id>`) pass through for
/// the workspace's own tabs; browser connections are kept to the workspace's context ([`Scope`]).
async fn websocket(
    stream: TcpStream,
    path: String,
    browser: u16,
    context: String,
    upstream: Arc<dyn Upstream>,
    on_activity: OnActivity,
) -> Result<(), String> {
    let (upstream_url, scope) = if let Some(id) = path.strip_prefix("/devtools/page/") {
        let id = id.split(['?', '/']).next().unwrap_or(id).to_string();
        if !upstream.targets().await?.contains(&id) {
            return Err(format!("tab {id} isn't in this workspace"));
        }
        (format!("ws://127.0.0.1:{browser}{path}"), None)
    } else if path.starts_with("/devtools/browser") {
        // The browser's id changes when it restarts: any browser path means the current browser.
        let url = tauri::async_runtime::spawn_blocking(move || super::chrome::ws_url(browser))
            .await
            .map_err(|e| e.to_string())?
            .ok_or("the browser isn't reachable")?;
        (url, Some(Scope::new(context)))
    } else {
        return Err(format!("unknown DevTools path {path}"));
    };
    // Chrome sends big messages in one frame; tungstenite's default limits would end the connection.
    let config = WebSocketConfig::default().max_message_size(None).max_frame_size(None);
    let client = tokio_tungstenite::accept_async_with_config(stream, Some(config))
        .await
        .map_err(|e| format!("handshake: {e}"))?;
    let (browser_ws, _) = tokio_tungstenite::connect_async_with_config(upstream_url, Some(config), false)
        .await
        .map_err(|e| format!("connect to the browser: {e}"))?;

    let conn = Arc::new(Mutex::new(Conn { tracker: Tracker::for_path(&path), scope }));
    let (mut client_tx, mut client_rx) = client.split();
    let (mut browser_tx, mut browser_rx) = browser_ws.split();
    // Both directions can answer or add messages of their own, so each side has one writer.
    let (to_client, mut client_queue) = mpsc::unbounded_channel::<Message>();
    let (to_browser, mut browser_queue) = mpsc::unbounded_channel::<Message>();
    let client_writer = async move {
        while let Some(msg) = client_queue.recv().await {
            if client_tx.send(msg).await.is_err() {
                break;
            }
        }
        let _ = client_tx.close().await;
    };
    let browser_writer = async move {
        while let Some(msg) = browser_queue.recv().await {
            if browser_tx.send(msg).await.is_err() {
                break;
            }
        }
        let _ = browser_tx.close().await;
    };
    let opened_by = on_activity.clone();
    let commands = {
        let (conn, to_client, to_browser) = (conn.clone(), to_client.clone(), to_browser.clone());
        async move {
            while let Some(Ok(msg)) = client_rx.next().await {
                let out = match &msg {
                    Message::Text(text) if text.len() >= OVERSIZED_BYTES => {
                        // Forward untouched, and don't JSON-parse the body here either.
                        let tab = session_of(text).and_then(|s| conn.lock().unwrap().tracker.tab_for_session(&s));
                        note_oversized("agent→browser", text, tab.as_deref());
                        msg
                    }
                    Message::Text(text) => {
                        let mut c = conn.lock().unwrap();
                        let verdict = c.scope.as_mut().map_or(Verdict::Forward(None), |s| s.command(text));
                        let rewritten = match verdict {
                            Verdict::Reply(reply) => {
                                drop(c);
                                let _ = to_client.send(Message::text(reply));
                                continue;
                            }
                            Verdict::Forward(rewritten) => rewritten,
                        };
                        let sent = rewritten.as_deref().unwrap_or(text);
                        let target = c.tracker.command(sent);
                        drop(c);
                        if let Some(target) = target {
                            on_activity(Activity::Acted(&target));
                            if sent.contains("\"Page.navigate\"") {
                                on_activity(Activity::Navigated(&target));
                            }
                            if let Some(pointer) = Pointer::of(sent) {
                                on_activity(Activity::Pointer(&target, pointer));
                            }
                        }
                        rewritten.map_or(msg, Message::text)
                    }
                    _ => msg,
                };
                if to_browser.send(out).is_err() {
                    break;
                }
            }
        }
    };
    let events = async move {
        let mut own = Vec::new();
        while let Some(Ok(msg)) = browser_rx.next().await {
            let out = match &msg {
                Message::Text(text) if text.len() >= OVERSIZED_BYTES => {
                    let tab = session_of(text).and_then(|s| conn.lock().unwrap().tracker.tab_for_session(&s));
                    note_oversized("browser→agent", text, tab.as_deref());
                    Some(msg)
                }
                Message::Text(text) => {
                    let mut c = conn.lock().unwrap();
                    let out = c.scope.as_mut().map_or(Out::Pass, |s| s.event(text, &mut own));
                    let shown = match &out {
                        Out::Pass => Some(text.as_str()),
                        Out::Rewrite(t) => Some(t.as_str()),
                        Out::Drop => None,
                    };
                    let opened = shown.and_then(|t| c.tracker.event(t));
                    drop(c);
                    if let Some(target) = opened {
                        opened_by(Activity::Opened(&target));
                    }
                    match out {
                        Out::Pass => Some(msg),
                        Out::Rewrite(t) => Some(Message::text(t)),
                        Out::Drop => None,
                    }
                }
                _ => Some(msg),
            };
            for command in own.drain(..) {
                let _ = to_browser.send(Message::text(command));
            }
            if let Some(out) = out
                && to_client.send(out).is_err()
            {
                break;
            }
        }
    };
    tokio::select! {
        _ = commands => {}
        _ = events => {}
        _ = client_writer => {}
        _ = browser_writer => {}
    }
    Ok(())
}

/// What one agent connection keeps track of.
struct Conn {
    tracker: Tracker,
    /// None on page connections, which are one tab of the workspace anyway.
    scope: Option<Scope>,
}

/// What to do with an agent's command.
#[derive(Debug, PartialEq)]
pub(crate) enum Verdict {
    /// Send it on (rewritten, if Some).
    Forward(Option<String>),
    /// Don't: answer the agent with this instead.
    Reply(String),
}

/// What to do with a message from the browser.
#[derive(Debug, PartialEq)]
pub(crate) enum Out {
    Pass,
    Rewrite(String),
    Drop,
}

/// Ids of the proxy's own commands, far above any agent's (Chrome takes 32-bit ids only).
const OWN_IDS: u64 = 2_000_000_000;

/// The answers a connection rewrites or learns from.
#[derive(Debug, Clone, Copy)]
enum Ask {
    Targets,
    Contexts,
    NewContext,
    NewTarget,
}

/// Keeps one agent connection to its workspace's browser context in the shared browser: new
/// tabs open in the context (and cookies, downloads and permissions apply to it), other
/// workspaces' targets are left out of what the agent sees, and the browser can't be closed.
///
/// Playwright counts targets of contexts it doesn't know as its default context's, so to an
/// agent the workspace's context is simply the browser's.
pub(crate) struct Scope {
    context: String,
    /// Contexts the agent made itself (its own too).
    created: HashSet<String>,
    /// Target id → whether it's in the workspace, from what the browser reported.
    targets: HashMap<String, bool>,
    /// Sessions the browser auto-attached this connection to on other workspaces' targets, being
    /// detached again.
    hidden: HashSet<String>,
    /// Answers to look at, by command id.
    pending: HashMap<u64, Ask>,
    sent: u64,
}

impl Scope {
    pub(crate) fn new(context: String) -> Self {
        Self {
            context,
            created: HashSet::new(),
            targets: HashMap::new(),
            hidden: HashSet::new(),
            pending: HashMap::new(),
            sent: 0,
        }
    }

    fn owns(&self, context: &str) -> bool {
        context == self.context || self.created.contains(context)
    }

    /// Notes which workspace a target is in; whether it's this one. Targets without a context
    /// (the browser itself) count as everyone's.
    fn learn(&mut self, info: &Value) -> bool {
        let ours = info["browserContextId"].as_str().is_none_or(|c| self.owns(c));
        if let Some(id) = info["targetId"].as_str() {
            self.targets.insert(id.to_string(), ours);
        }
        ours
    }

    fn own_command(&mut self, method: &str, params: Value, session: Option<&str>) -> String {
        self.sent += 1;
        let mut v = json!({ "id": OWN_IDS + self.sent, "method": method, "params": params });
        if let Some(s) = session {
            v["sessionId"] = s.into();
        }
        v.to_string()
    }

    pub(crate) fn command(&mut self, text: &str) -> Verdict {
        let Ok(mut v) = serde_json::from_str::<Value>(text) else {
            return Verdict::Forward(None);
        };
        let (Some(id), Some(method)) = (v["id"].as_u64(), v["method"].as_str().map(String::from)) else {
            return Verdict::Forward(None);
        };
        if let Some(session) = v["sessionId"].as_str() {
            if self.hidden.contains(session) {
                return Verdict::Reply(error(id, Some(session), "No session with given id"));
            }
            return Verdict::Forward(None);
        }
        let in_context = |v: &mut Value, ctx: &str| {
            v["params"]["browserContextId"] = ctx.into();
            Verdict::Forward(Some(v.to_string()))
        };
        match method.as_str() {
            "Target.createTarget" => {
                self.pending.insert(id, Ask::NewTarget);
                let ctx = v["params"]["browserContextId"].as_str();
                if !ctx.is_some_and(|c| self.owns(c)) {
                    return in_context(&mut v, &self.context);
                }
            }
            "Target.getTargets" => {
                self.pending.insert(id, Ask::Targets);
            }
            "Target.getBrowserContexts" => {
                self.pending.insert(id, Ask::Contexts);
            }
            "Target.createBrowserContext" => {
                self.pending.insert(id, Ask::NewContext);
            }
            "Target.disposeBrowserContext" if !v["params"]["browserContextId"].as_str().is_some_and(|c| self.created.contains(c)) => {
                return Verdict::Reply(error(id, None, "Only contexts this connection created can be disposed"));
            }
            "Target.attachToTarget"
            | "Target.activateTarget"
            | "Target.closeTarget"
            | "Target.getTargetInfo"
            | "Target.exposeDevToolsProtocol"
            | "Target.autoAttachRelated"
            | "Browser.getWindowForTarget"
                if v["params"]["targetId"].as_str().is_some_and(|t| self.targets.get(t) == Some(&false)) =>
            {
                return Verdict::Reply(error(id, None, "No target with given id found"));
            }
            "Storage.getCookies"
            | "Storage.setCookies"
            | "Storage.clearCookies"
            | "Browser.setDownloadBehavior"
            | "Browser.cancelDownload"
            | "Browser.grantPermissions"
            | "Browser.resetPermissions"
            | "Browser.setPermission" => match v["params"]["browserContextId"].as_str() {
                None => return in_context(&mut v, &self.context),
                Some(c) if !self.owns(c) => return Verdict::Reply(error(id, None, "Failed to find browser context with id")),
                Some(_) => {}
            },
            "Browser.close" | "Browser.crash" | "Browser.crashGpuProcess" => {
                return Verdict::Reply(error(id, None, "winfer's browser is shared by all workspaces and stays open; close its tabs instead"));
            }
            _ => {}
        }
        Verdict::Forward(None)
    }

    /// A message from the browser. Commands of the proxy's own to send go to `own`.
    pub(crate) fn event(&mut self, text: &str, own: &mut Vec<String>) -> Out {
        // Chrome writes a response's id first and an event's method first.
        if let Some(id) = leading_id(text) {
            if id > OWN_IDS {
                return Out::Drop;
            }
            let Some(ask) = self.pending.remove(&id) else {
                return Out::Pass;
            };
            let Ok(mut v) = serde_json::from_str::<Value>(text) else {
                return Out::Pass;
            };
            let result = &mut v["result"];
            match ask {
                Ask::Targets => {
                    let infos = result["targetInfos"].take();
                    let kept: Vec<Value> = infos.as_array().into_iter().flatten().filter(|i| self.learn(i)).cloned().collect();
                    result["targetInfos"] = kept.into();
                    return Out::Rewrite(v.to_string());
                }
                Ask::Contexts => {
                    let ids = result["browserContextIds"].take();
                    let kept: Vec<Value> = ids.as_array().into_iter().flatten().filter(|c| c.as_str().is_some_and(|c| self.owns(c))).cloned().collect();
                    result["browserContextIds"] = kept.into();
                    return Out::Rewrite(v.to_string());
                }
                Ask::NewContext => {
                    if let Some(c) = result["browserContextId"].as_str() {
                        self.created.insert(c.to_string());
                    }
                }
                Ask::NewTarget => {
                    if let Some(t) = result["targetId"].as_str() {
                        self.targets.insert(t.to_string(), true);
                    }
                }
            }
            return Out::Pass;
        }
        let method = method_of(text);
        let is = |m: &str| method.as_deref() == Some(m);
        let target_event = method.as_deref().is_some_and(|m| m.starts_with("Target."));
        if !target_event && self.hidden.is_empty() {
            return Out::Pass;
        }
        let Ok(v) = serde_json::from_str::<Value>(text) else {
            return Out::Pass;
        };
        let parent = v["sessionId"].as_str();
        if parent.is_some_and(|s| self.hidden.contains(s)) {
            return Out::Drop;
        }
        let params = &v["params"];
        if is("Target.targetCreated") || is("Target.targetInfoChanged") {
            return if self.learn(&params["targetInfo"]) { Out::Pass } else { Out::Drop };
        }
        if is("Target.targetDestroyed") || is("Target.targetCrashed") {
            let id = params["targetId"].as_str().unwrap_or_default();
            let ours = self.targets.get(id) != Some(&false);
            if is("Target.targetDestroyed") {
                self.targets.remove(id);
            }
            return if ours { Out::Pass } else { Out::Drop };
        }
        if is("Target.attachedToTarget") {
            if self.learn(&params["targetInfo"]) {
                return Out::Pass;
            }
            // Auto-attach reached another workspace's target: let it run and let go of it.
            let Some(session) = params["sessionId"].as_str() else {
                return Out::Drop;
            };
            self.hidden.insert(session.to_string());
            if params["waitingForDebugger"] == true {
                own.push(self.own_command("Runtime.runIfWaitingForDebugger", json!({}), Some(session)));
            }
            own.push(self.own_command("Target.detachFromTarget", json!({ "sessionId": session }), parent));
            return Out::Drop;
        }
        if is("Target.detachedFromTarget") {
            let session = params["sessionId"].as_str().unwrap_or_default();
            return if self.hidden.remove(session) { Out::Drop } else { Out::Pass };
        }
        Out::Pass
    }
}

/// The id a response starts with (`{"id":12,…`); None for events.
fn leading_id(text: &str) -> Option<u64> {
    let rest = text.strip_prefix("{\"id\":")?;
    let end = rest.find(|c: char| !c.is_ascii_digit())?;
    rest[..end].parse().ok()
}

fn error(id: u64, session: Option<&str>, message: &str) -> String {
    let mut v = json!({ "id": id, "error": { "code": -32000, "message": message } });
    if let Some(s) = session {
        v["sessionId"] = s.into();
    }
    v.to_string()
}

/// Maps one agent connection's sessions to tabs, and tells which commands act on a tab.
#[derive(Default)]
pub(crate) struct Tracker {
    /// The tab of a page-level connection (/devtools/page/<id>).
    page: Option<String>,
    /// Session id → (page target id, attached at).
    sessions: HashMap<String, (String, Instant)>,
    /// Ids of the agent's `Target.createTarget` calls still waiting for their answer.
    creating: HashSet<u64>,
}

impl Tracker {
    pub(crate) fn for_path(path: &str) -> Self {
        Self {
            page: path.strip_prefix("/devtools/page/").map(|id| id.split(['?', '/']).next().unwrap_or(id).to_string()),
            ..Self::default()
        }
    }

    /// The tab a session belongs to (the page tab itself on page connections).
    pub(crate) fn tab_for_session(&self, session: &str) -> Option<String> {
        self.sessions.get(session).map(|(tab, _)| tab.clone()).or_else(|| self.page.clone())
    }

    /// A message from the browser: learns which session belongs to which tab, and returns the
    /// tab an agent's `Target.createTarget` just opened.
    pub(crate) fn event(&mut self, text: &str) -> Option<String> {
        if !self.creating.is_empty() && text.contains("\"targetId\"") && !text.contains("\"method\"") {
            let v: Value = serde_json::from_str(text).ok()?;
            if self.creating.remove(&v["id"].as_u64()?) {
                return v["result"]["targetId"].as_str().map(String::from);
            }
            return None;
        }
        // Only attach/detach events matter; skip parsing the (often big) rest.
        if text.contains("\"Target.attachedToTarget\"") {
            let Ok(v) = serde_json::from_str::<Value>(text) else { return None };
            let params = &v["params"];
            let (Some(session), Some(target)) = (params["sessionId"].as_str(), params["targetInfo"]["targetId"].as_str()) else {
                return None;
            };
            let page = if params["targetInfo"]["type"] == "page" {
                Some(target.to_string())
            } else {
                // A frame or worker inside a tab (attached through the tab's session) counts as the tab.
                v["sessionId"].as_str().and_then(|parent| self.sessions.get(parent)).map(|(t, _)| t.clone())
            };
            if let Some(page) = page {
                self.sessions.insert(session.to_string(), (page, Instant::now()));
            }
        } else if text.contains("\"Target.detachedFromTarget\"")
            && let Ok(v) = serde_json::from_str::<Value>(text)
            && let Some(session) = v["params"]["sessionId"].as_str()
        {
            self.sessions.remove(session);
        }
        None
    }

    /// A command from the agent: the tab it acts on, if it's an action rather than bookkeeping.
    pub(crate) fn command(&mut self, text: &str) -> Option<String> {
        let v: Value = serde_json::from_str(text).ok()?;
        let method = v["method"].as_str()?;
        if method == "Target.createTarget"
            && let Some(id) = v["id"].as_u64()
        {
            self.creating.insert(id);
        }
        if method == "Target.activateTarget" {
            return v["params"]["targetId"].as_str().map(String::from);
        }
        if !acts(method) || reads_title(&v) {
            return None;
        }
        match v["sessionId"].as_str() {
            Some(session) => {
                let (target, attached) = self.sessions.get(session)?;
                (attached.elapsed() >= SETUP || method.starts_with("Input.") || method.starts_with("Page.navigate")).then(|| target.clone())
            }
            None => self.page.clone(),
        }
    }
}

/// Commands that read or drive the page itself (what an agent does with a tab it's using).
fn acts(method: &str) -> bool {
    method.starts_with("Input.")
        || method.starts_with("Accessibility.")
        || matches!(
            method,
            "Page.navigate"
                | "Page.navigateToHistoryEntry"
                | "Page.reload"
                | "Page.captureScreenshot"
                | "Page.bringToFront"
                | "Page.handleJavaScriptDialog"
                | "Page.printToPDF"
                | "Runtime.evaluate"
                | "Runtime.callFunctionOn"
                | "DOM.setFileInputFiles"
                | "DOM.scrollIntoViewIfNeeded"
                | "DOM.getContentQuads"
                | "DOM.describeNode"
        )
}

/// Playwright's `page.title()`, which Playwright MCP calls on every open tab after each tool call
/// (for the "Open tabs" list in its answer): reading a background tab's title isn't using it.
/// It goes through Playwright's utility script, with the function's source as an argument.
fn reads_title(v: &Value) -> bool {
    let is_title = |s: &str| s.chars().filter(|c| !c.is_whitespace()).eq("()=>document.title".chars());
    let params = &v["params"];
    match v["method"].as_str() {
        Some("Runtime.callFunctionOn") => params["arguments"].as_array().is_some_and(|args| args.iter().any(|a| a["value"].as_str().is_some_and(is_title))),
        Some("Runtime.evaluate") => params["expression"].as_str().is_some_and(|e| e.trim() == "document.title"),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attach(session: &str, target: &str, kind: &str, parent: Option<&str>) -> String {
        let mut v = serde_json::json!({ "method": "Target.attachedToTarget", "params": { "sessionId": session, "targetInfo": { "targetId": target, "type": kind } } });
        if let Some(p) = parent {
            v["sessionId"] = p.into();
        }
        v.to_string()
    }

    fn command(method: &str, session: Option<&str>) -> String {
        let mut v = serde_json::json!({ "id": 1, "method": method, "params": {} });
        if let Some(s) = session {
            v["sessionId"] = s.into();
        }
        v.to_string()
    }

    #[test]
    fn tells_which_tab_a_command_acts_on() {
        let mut t = Tracker::default();
        t.event(&attach("S1", "T1", "page", None));
        t.event(&attach("S2", "F1", "iframe", Some("S1")));
        // Right after attaching only input and navigation count; setup evaluations don't.
        assert_eq!(t.command(&command("Runtime.evaluate", Some("S1"))), None);
        assert_eq!(t.command(&command("Input.dispatchMouseEvent", Some("S1"))).as_deref(), Some("T1"));
        assert_eq!(t.command(&command("Input.insertText", Some("S2"))).as_deref(), Some("T1"));
        // Bookkeeping never counts.
        assert_eq!(t.command(&command("Network.enable", Some("S1"))), None);
        t.sessions.get_mut("S1").unwrap().1 -= SETUP;
        assert_eq!(t.command(&command("Runtime.evaluate", Some("S1"))).as_deref(), Some("T1"));
        // Playwright MCP reads every tab's title after each tool call: that isn't using them.
        let title = serde_json::json!({ "id": 3, "sessionId": "S1", "method": "Runtime.callFunctionOn", "params": {
            "functionDeclaration": "(utilityScript, ...args) => utilityScript.evaluate(...args)",
            "arguments": [{ "objectId": "1" }, { "value": true }, { "value": true }, { "value": false }, { "value": "() => document.title" }, { "value": 0 }],
        } });
        assert_eq!(t.command(&title.to_string()), None);
        // Unknown sessions and detached ones don't.
        t.event(&serde_json::json!({ "method": "Target.detachedFromTarget", "params": { "sessionId": "S1" } }).to_string());
        assert_eq!(t.command(&command("Input.dispatchKeyEvent", Some("S1"))), None);
        // The tab an agent opens itself.
        assert_eq!(t.command(r#"{"id":7,"method":"Target.createTarget","params":{"url":"about:blank"}}"#), None);
        assert_eq!(t.event(r#"{"id":6,"result":{"targetId":"X"}}"#), None);
        assert_eq!(t.event(r#"{"id":7,"result":{"targetId":"T5"}}"#).as_deref(), Some("T5"));
        assert!(t.creating.is_empty());
        let activate = serde_json::json!({ "id": 2, "method": "Target.activateTarget", "params": { "targetId": "T9" } }).to_string();
        assert_eq!(t.command(&activate).as_deref(), Some("T9"));
    }

    #[test]
    fn flags_oversized_cdp_messages_by_method_and_tab() {
        // Under the limit: nothing, however it looks.
        assert_eq!(oversized_info(r#"{"method":"Fetch.fulfillRequest","params":{"body":"AA=="}}"#, 1024), None);
        // Over the limit: method named without parsing the (here padded) body.
        let big = format!(r#"{{"id":1,"sessionId":"S1","method":"Fetch.fulfillRequest","params":{{"body":"{}"}}}}"#, "A".repeat(2048));
        let info = oversized_info(&big, 1024).unwrap();
        assert!(info.starts_with("Fetch.fulfillRequest ("), "{info}");
        assert!(info.contains(&big.len().to_string()), "{info}");
        // Responses have no method; the session still names the tab.
        let response = format!(r#"{{"id":9,"sessionId":"S1","result":{{"body":"{}"}}}}"#, "B".repeat(2048));
        assert!(oversized_info(&response, 1024).unwrap().starts_with("response/event ("));
        assert_eq!(session_of(&response).as_deref(), Some("S1"));
        // Session → tab resolution, including page connections.
        let mut t = Tracker::default();
        t.event(&attach("S1", "T1", "page", None));
        assert_eq!(t.tab_for_session("S1").as_deref(), Some("T1"));
    }

    #[test]
    fn reads_an_agents_mouse_events() {
        let click = r#"{"id":4,"sessionId":"S1","method":"Input.dispatchMouseEvent","params":{"type":"mousePressed","x":120.5,"y":40,"button":"left","clickCount":1}}"#;
        assert_eq!(Pointer::of(click), Some(Pointer { x: 120.5, y: 40.0, kind: "mousePressed".into() }));
        assert_eq!(Pointer::of(&command("Input.dispatchKeyEvent", Some("S1"))), None);
    }

    /// A workspace for real-browser tests: a context in the browser on `port`.
    struct TestSide {
        port: u16,
        context: String,
        cdp: Arc<super::super::cdp::Cdp>,
    }

    impl TestSide {
        async fn new(port: u16) -> Arc<Self> {
            let ws = super::super::chrome::ws_url(port).unwrap();
            let cdp = super::super::cdp::Cdp::connect(&ws, |_| {}).await.unwrap();
            let context = cdp.call("Target.createBrowserContext", json!({}), None).await.unwrap()["browserContextId"]
                .as_str()
                .unwrap()
                .to_string();
            Arc::new(Self { port, context, cdp })
        }
    }

    impl Upstream for TestSide {
        fn prepare(&self) -> BoxFuture<'static, Result<(u16, String), String>> {
            let ready = Ok((self.port, self.context.clone()));
            Box::pin(async move { ready })
        }
        fn targets(&self) -> BoxFuture<'static, Result<HashSet<String>, String>> {
            let (cdp, context) = (self.cdp.clone(), self.context.clone());
            Box::pin(async move {
                let all = cdp.call("Target.getTargets", json!({}), None).await?;
                Ok(all["targetInfos"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter(|t| t["browserContextId"] == context.as_str())
                    .map(|t| t["targetId"].as_str().unwrap().to_string())
                    .collect())
            })
        }
        fn new_tab(&self, url: String) -> BoxFuture<'static, Result<String, String>> {
            let (cdp, context) = (self.cdp.clone(), self.context.clone());
            Box::pin(async move {
                let res = cdp.call("Target.createTarget", json!({ "url": url, "browserContextId": context }), None).await?;
                Ok(res["targetId"].as_str().unwrap().to_string())
            })
        }
    }

    fn info(target: &str, context: &str) -> Value {
        json!({ "targetId": target, "type": "page", "browserContextId": context, "url": "about:blank" })
    }

    #[test]
    fn keeps_an_agent_to_its_context() {
        let mut s = Scope::new("A".into());
        let mut own = Vec::new();
        // New tabs open in the workspace's context, unless the agent made the context itself.
        let Verdict::Forward(Some(cmd)) = s.command(r#"{"id":1,"method":"Target.createTarget","params":{"url":"about:blank"}}"#) else { panic!() };
        assert_eq!(serde_json::from_str::<Value>(&cmd).unwrap()["params"]["browserContextId"], "A");
        assert_eq!(s.event(r#"{"id":2,"result":{"browserContextId":"MINE"}}"#, &mut own), Out::Pass, "not asked");
        assert_eq!(s.command(r#"{"id":3,"method":"Target.createBrowserContext","params":{}}"#), Verdict::Forward(None));
        assert_eq!(s.event(r#"{"id":3,"result":{"browserContextId":"MINE"}}"#, &mut own), Out::Pass);
        assert_eq!(s.command(r#"{"id":4,"method":"Target.createTarget","params":{"url":"x","browserContextId":"MINE"}}"#), Verdict::Forward(None));
        let Verdict::Forward(Some(cmd)) = s.command(r#"{"id":5,"method":"Target.createTarget","params":{"url":"x","browserContextId":"B"}}"#) else { panic!() };
        assert!(cmd.contains(r#""browserContextId":"A""#));

        // Only the workspace's targets are listed and reported.
        assert_eq!(s.command(r#"{"id":6,"method":"Target.getTargets","params":{}}"#), Verdict::Forward(None));
        let list = json!({ "id": 6, "result": { "targetInfos": [info("TA", "A"), info("TB", "B"), info("TM", "MINE")] } }).to_string();
        let Out::Rewrite(list) = s.event(&list, &mut own) else { panic!() };
        let ids: Vec<String> = serde_json::from_str::<Value>(&list).unwrap()["result"]["targetInfos"].as_array().unwrap().iter().map(|t| t["targetId"].as_str().unwrap().into()).collect();
        assert_eq!(ids, ["TA", "TM"]);
        let created = |t: &str, c: &str| json!({ "method": "Target.targetCreated", "params": { "targetInfo": info(t, c) } }).to_string();
        assert_eq!(s.event(&created("TB2", "B"), &mut own), Out::Drop);
        assert_eq!(s.event(&created("TA2", "A"), &mut own), Out::Pass);
        assert_eq!(s.event(r#"{"method":"Target.targetDestroyed","params":{"targetId":"TB2"}}"#, &mut own), Out::Drop);
        assert_eq!(s.event(r#"{"method":"Target.targetDestroyed","params":{"targetId":"TA2"}}"#, &mut own), Out::Pass);
        // Other workspaces' targets can't be driven.
        let Verdict::Reply(err) = s.command(r#"{"id":7,"method":"Target.attachToTarget","params":{"targetId":"TB","flatten":true}}"#) else { panic!() };
        assert!(err.contains(r#""id":7"#) && err.contains("error"));
        assert_eq!(s.command(r#"{"id":8,"method":"Target.attachToTarget","params":{"targetId":"TA","flatten":true}}"#), Verdict::Forward(None));

        // Auto-attach to another workspace's tab: resumed and let go, never shown to the agent.
        let attached = json!({ "method": "Target.attachedToTarget", "params": { "sessionId": "SB", "targetInfo": info("TB", "B"), "waitingForDebugger": true } }).to_string();
        assert_eq!(s.event(&attached, &mut own), Out::Drop);
        let methods: Vec<String> = own.iter().map(|c| serde_json::from_str::<Value>(c).unwrap()["method"].as_str().unwrap().into()).collect();
        assert_eq!(methods, ["Runtime.runIfWaitingForDebugger", "Target.detachFromTarget"]);
        assert!(own[0].contains(r#""sessionId":"SB""#));
        let own_id = serde_json::from_str::<Value>(&own[1]).unwrap()["id"].as_u64().unwrap();
        assert_eq!(s.event(&format!(r#"{{"id":{own_id},"result":{{}}}}"#), &mut own), Out::Drop, "our own answers stay with us");
        assert_eq!(s.event(r#"{"method":"Runtime.consoleAPICalled","params":{},"sessionId":"SB"}"#, &mut own), Out::Drop);
        assert!(matches!(s.command(r#"{"id":9,"method":"Runtime.evaluate","params":{},"sessionId":"SB"}"#), Verdict::Reply(_)));
        assert_eq!(s.event(r#"{"method":"Target.detachedFromTarget","params":{"sessionId":"SB"}}"#, &mut own), Out::Drop);
        assert!(s.hidden.is_empty());
        let mine = json!({ "method": "Target.attachedToTarget", "params": { "sessionId": "SA", "targetInfo": info("TA", "A"), "waitingForDebugger": false } }).to_string();
        assert_eq!(s.event(&mine, &mut own), Out::Pass);

        // Cookies, permissions and downloads are the context's; the shared browser stays open.
        let Verdict::Forward(Some(cmd)) = s.command(r#"{"id":10,"method":"Storage.getCookies","params":{}}"#) else { panic!() };
        assert!(cmd.contains(r#""browserContextId":"A""#));
        assert!(matches!(s.command(r#"{"id":11,"method":"Storage.getCookies","params":{"browserContextId":"B"}}"#), Verdict::Reply(_)));
        assert!(matches!(s.command(r#"{"id":12,"method":"Browser.close","params":{}}"#), Verdict::Reply(_)));
        assert!(matches!(s.command(r#"{"id":13,"method":"Target.disposeBrowserContext","params":{"browserContextId":"A"}}"#), Verdict::Reply(_)));
        assert_eq!(s.command(r#"{"id":14,"method":"Target.disposeBrowserContext","params":{"browserContextId":"MINE"}}"#), Verdict::Forward(None));
        assert_eq!(s.command(r#"{"id":15,"method":"Target.getBrowserContexts"}"#), Verdict::Forward(None));
        let Out::Rewrite(ctxs) = s.event(r#"{"id":15,"result":{"browserContextIds":["A","B","MINE"]}}"#, &mut own) else { panic!() };
        assert!(ctxs.contains(r#"["A","MINE"]"#), "{ctxs}");
    }

    #[test]
    fn lists_only_the_workspaces_targets_over_http() {
        let body = r#"[{"id":"A1","url":"x"},{"id":"B1","url":"y"}]"#;
        assert_eq!(list_within(body, |id| id == "A1"), r#"[{"id":"A1","url":"x"}]"#);
        assert_eq!(unescape("https%3A%2F%2Fexample.com%2F%3Fq%3D1"), "https://example.com/?q=1");
        assert_eq!(unescape("50%"), "50%");
        assert_eq!(leading_id(r#"{"id":42,"result":{}}"#), Some(42));
        assert_eq!(leading_id(r#"{"method":"x","params":{"id":1}}"#), None);
    }

    /// Real Chrome, two workspaces' contexts: an agent through the proxy sees, opens and
    /// auto-attaches to only its own workspace's tabs, and the other workspace's new tabs aren't
    /// held waiting on it.
    #[test]
    fn scopes_an_agent_to_its_workspace_in_a_real_browser() {
        use super::super::{cdp::Cdp, chrome};
        let tmp = std::env::temp_dir().join(format!("winfer-scope-{:x}", rand::random::<u32>()));
        let Some(exe) = chrome::find(&tmp.join("managed")) else {
            eprintln!("no Chrome found; skipping");
            return;
        };
        let launched = chrome::launch(&exe, &tmp.join("profile")).unwrap();
        let _guard = chrome::ChromeGuard(launched.child);
        chrome::wait_ws_url(launched.port, Duration::from_secs(45)).unwrap();

        tauri::async_runtime::block_on(async {
            let a = TestSide::new(launched.port).await;
            let b = TestSide::new(launched.port).await;
            let b_tab = b.new_tab("data:text/html,b".into()).await.unwrap();
            let a_tab = a.new_tab("data:text/html,a".into()).await.unwrap();
            let proxy = Listener::bind().unwrap().serve(a.clone(), Arc::new(|_| {}));

            // Over HTTP, only A's tabs.
            let port = proxy.port;
            let list = tauri::async_runtime::spawn_blocking(move || forward("GET", port, "/json/list").1).await.unwrap();
            assert!(list.contains(&a_tab) && !list.contains(&b_tab), "{list}");

            let ws = tauri::async_runtime::spawn_blocking(move || chrome::ws_url(port)).await.unwrap().unwrap();
            let (tx, rx) = std::sync::mpsc::channel::<super::super::cdp::Event>();
            let tx = Mutex::new(tx);
            let agent = Cdp::connect(&ws, move |e| {
                let _ = tx.lock().unwrap().send(e);
            })
            .await
            .unwrap();
            let targets = agent.call("Target.getTargets", json!({}), None).await.unwrap();
            let ids: Vec<&str> = targets["targetInfos"].as_array().unwrap().iter().filter(|t| t["type"] == "page").map(|t| t["targetId"].as_str().unwrap()).collect();
            assert_eq!(ids, [a_tab.as_str()]);

            // Playwright's way in: auto-attach to everything, holding new targets.
            agent
                .call("Target.setAutoAttach", json!({ "autoAttach": true, "waitForDebuggerOnStart": true, "flatten": true }), None)
                .await
                .unwrap();
            let opened = agent.call("Target.createTarget", json!({ "url": "about:blank" }), None).await.unwrap();
            let opened = opened["targetId"].as_str().unwrap().to_string();
            assert!(a.targets().await.unwrap().contains(&opened), "a new tab opens in the workspace's context");
            // Another workspace's new tab isn't held for the agent: it loads.
            let b_new = b.new_tab("data:text/html,<title>loaded</title>".into()).await.unwrap();
            let b_session = b
                .cdp
                .call("Target.attachToTarget", json!({ "targetId": b_new, "flatten": true }), None)
                .await
                .unwrap()["sessionId"]
                .as_str()
                .unwrap()
                .to_string();
            let mut title = Value::Null;
            for _ in 0..50 {
                title = b.cdp.call("Runtime.evaluate", json!({ "expression": "document.title", "returnByValue": true }), Some(&b_session)).await.unwrap()["result"]["value"].clone();
                if title == "loaded" {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            assert_eq!(title, "loaded");

            tokio::time::sleep(Duration::from_millis(500)).await;
            let attached: Vec<String> = rx
                .try_iter()
                .filter(|e| e.method == "Target.attachedToTarget" && e.params["targetInfo"]["type"] == "page")
                .map(|e| e.params["targetInfo"]["targetId"].as_str().unwrap().to_string())
                .collect();
            assert!(attached.contains(&a_tab) && attached.contains(&opened), "{attached:?}");
            assert!(!attached.contains(&b_tab) && !attached.contains(&b_new), "{attached:?}");
            let err = agent.call("Target.attachToTarget", json!({ "targetId": b_tab, "flatten": true }), None).await;
            assert!(err.is_err(), "{err:?}");
        });
    }

    /// Real Chrome and real Playwright (what Playwright MCP runs on), through the proxy: it works
    /// in the workspace's context and never sees or touches the other one. Opt-in: set
    /// `WINFER_PLAYWRIGHT_CORE` to a `playwright-core` directory (e.g. from the npx cache).
    #[test]
    #[ignore]
    fn playwright_works_in_its_workspace_only() {
        use super::super::chrome;
        let Some(core) = std::env::var_os("WINFER_PLAYWRIGHT_CORE") else {
            eprintln!("WINFER_PLAYWRIGHT_CORE not set; skipping");
            return;
        };
        let tmp = std::env::temp_dir().join(format!("winfer-pw-{:x}", rand::random::<u32>()));
        let exe = chrome::find(&tmp.join("managed")).expect("a Chrome (WINFER_CHROME)");
        let launched = chrome::launch(&exe, &tmp.join("profile")).unwrap();
        let _guard = chrome::ChromeGuard(launched.child);
        chrome::wait_ws_url(launched.port, Duration::from_secs(45)).unwrap();
        let script = tmp.join("agent.js");
        std::fs::write(
            &script,
            r#"
const { chromium } = require(process.argv[3]);
(async () => {
  const browser = await chromium.connectOverCDP(process.argv[2], { noDefaults: true });
  const context = browser.contexts()[0];
  const before = context.pages().map(p => p.url());
  const page = await context.newPage();
  await page.setContent('<button onclick="document.title=\'clicked\'">go</button>');
  await page.click('button');
  await context.addCookies([{ name: 'agent', value: '1', domain: 'example.com', path: '/' }]);
  const cookies = (await context.cookies()).map(c => c.name);
  console.log(JSON.stringify({ before, title: await page.title(), cookies, pages: context.pages().length }));
  await browser.close();
})().catch(e => { console.error(e); process.exit(1); });
"#,
        )
        .unwrap();

        tauri::async_runtime::block_on(async {
            let a = TestSide::new(launched.port).await;
            let b = TestSide::new(launched.port).await;
            let a_tab = a.new_tab("data:text/html,a".into()).await.unwrap();
            b.new_tab("data:text/html,b".into()).await.unwrap();
            b.cdp
                .call("Storage.setCookies", json!({ "browserContextId": b.context, "cookies": [{ "name": "other", "value": "1", "domain": "example.com", "path": "/" }] }), None)
                .await
                .unwrap();
            let proxy = Listener::bind().unwrap().serve(a.clone(), Arc::new(|_| {}));
            let url = format!("http://127.0.0.1:{}", proxy.port);
            let out = tauri::async_runtime::spawn_blocking(move || std::process::Command::new("node").arg(&script).arg(url).arg(core).output().unwrap())
                .await
                .unwrap();
            let stdout = String::from_utf8_lossy(&out.stdout);
            assert!(out.status.success(), "{stdout}\n{}", String::from_utf8_lossy(&out.stderr));
            let report: Value = serde_json::from_str(stdout.trim()).unwrap();
            assert_eq!(report["before"], json!(["data:text/html,a"]), "only the workspace's tab: {report}");
            assert_eq!(report["title"], "clicked");
            assert_eq!(report["cookies"], json!(["agent"]), "only the workspace's cookies: {report}");
            assert_eq!(report["pages"], 2);

            let a_targets = a.targets().await.unwrap();
            assert!(a_targets.contains(&a_tab) && a_targets.len() >= 2, "the agent's page is in A: {a_targets:?}");
            assert_eq!(b.targets().await.unwrap().len(), 1, "nothing opened in B");
            let b_cookies = b.cdp.call("Storage.getCookies", json!({ "browserContextId": b.context }), None).await.unwrap();
            let names: Vec<&str> = b_cookies["cookies"].as_array().unwrap().iter().map(|c| c["name"].as_str().unwrap()).collect();
            assert_eq!(names, ["other"]);
            assert!(chrome::ws_url(launched.port).is_some(), "closing Playwright left the shared browser running");
        });
    }

    /// Through a real browser: the agent's URLs point at the proxy, and its input in a tab is reported.
    #[test]
    fn reports_an_agents_tab_through_a_real_browser() {
        use super::super::{cdp::Cdp, chrome};
        let tmp = std::env::temp_dir().join(format!("winfer-proxy-{:x}", rand::random::<u32>()));
        let Some(exe) = chrome::find(&tmp.join("managed")) else {
            eprintln!("no Chrome found; skipping");
            return;
        };
        let launched = chrome::launch(&exe, &tmp.join("profile")).unwrap();
        let _guard = chrome::ChromeGuard(launched.child);
        chrome::wait_ws_url(launched.port, Duration::from_secs(45)).unwrap();

        tauri::async_runtime::block_on(async {
            let (tx, rx) = std::sync::mpsc::channel::<String>();
            let tx = Mutex::new(tx);
            let proxy = Listener::bind().unwrap().serve(
                TestSide::new(launched.port).await,
                Arc::new(move |a: Activity| {
                    let _ = tx.lock().unwrap().send(format!("{a:?}"));
                }),
            );
            let ws = tauri::async_runtime::spawn_blocking(move || chrome::ws_url(proxy.port).map(|u| (u, proxy))).await.unwrap();
            let (ws, _proxy) = ws.expect("the proxy answers /json/version");
            assert!(ws.contains(&format!("127.0.0.1:{}", _proxy.port)), "{ws}");

            let agent = Cdp::connect(&ws, |_| {}).await.unwrap();
            let target = agent.call("Target.createTarget", serde_json::json!({ "url": "about:blank" }), None).await.unwrap();
            let target = target["targetId"].as_str().unwrap().to_string();
            let session = agent.call("Target.attachToTarget", serde_json::json!({ "targetId": target, "flatten": true }), None).await.unwrap();
            let session = session["sessionId"].as_str().unwrap().to_string();
            agent
                .call("Input.dispatchMouseEvent", serde_json::json!({ "type": "mouseMoved", "x": 5, "y": 5 }), Some(&session))
                .await
                .unwrap();
            assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), format!("Opened({target:?})"));
            assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), format!("Acted({target:?})"));
            let pointer = Pointer { x: 5.0, y: 5.0, kind: "mouseMoved".into() };
            assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), format!("{:?}", Activity::Pointer(&target, pointer)));
        });
    }

    #[test]
    fn page_connections_act_on_their_tab() {
        let mut t = Tracker::for_path("/devtools/page/ABC");
        assert_eq!(t.command(&command("Page.navigate", None)).as_deref(), Some("ABC"));
        assert_eq!(rewrite(r#"{"webSocketDebuggerUrl":"ws://127.0.0.1:9222/devtools/browser/x"}"#, 9222, 5000), r#"{"webSocketDebuggerUrl":"ws://127.0.0.1:5000/devtools/browser/x"}"#);
    }
}
