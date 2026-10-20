//! A pass-through proxy in front of the browser's DevTools endpoint, handed to agents instead of
//! the browser's own port. It reads which tab each agent command acts on, so the panel can show
//! the tab an agent is using.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;

/// What an agent did with a tab (by target id).
#[derive(Debug, PartialEq)]
pub enum Activity<'a> {
    /// It opened the tab itself.
    Opened(&'a str),
    /// It acted on the tab.
    Acted(&'a str),
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

/// After attaching, Playwright sets a tab up (scripts, emulation, evaluations) whether or not it
/// will use it: only input and navigation count as using the tab for this long.
const SETUP: Duration = Duration::from_secs(2);
/// How long a request may wait for the browser to come up (it starts without waiting).
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

    pub fn serve(self, browser_port: u16, on_activity: OnActivity) -> Proxy {
        let own = self.port;
        let task = tauri::async_runtime::spawn(async move {
            let Ok(listener) = TcpListener::from_std(self.inner) else {
                return;
            };
            while let Ok((stream, _)) = listener.accept().await {
                let on_activity = on_activity.clone();
                tauri::async_runtime::spawn(async move {
                    if let Err(err) = serve(stream, browser_port, own, on_activity).await {
                        super::log::warning!("CDP proxy: {err}");
                    }
                });
            }
        });
        Proxy { port: own, task }
    }
}

async fn serve(mut stream: TcpStream, browser: u16, own: u16, on_activity: OnActivity) -> Result<(), String> {
    let head = request_head(&stream).await?;
    if head.is_empty() {
        return Ok(());
    }
    if head.to_ascii_lowercase().contains("upgrade: websocket") {
        return websocket(stream, browser, on_activity).await;
    }
    // Plain HTTP (/json/version, /json/list, /json/new…): forward it and point the URLs in the
    // answer at the proxy, so the agent's WebSocket comes through here too.
    let mut consumed = vec![0; head.len() + 4];
    stream.read_exact(&mut consumed).await.map_err(|e| e.to_string())?;
    let mut parts = head.split_whitespace();
    let (method, path) = (parts.next().unwrap_or("GET").to_string(), parts.next().unwrap_or("/").to_string());
    let (status, body) = tauri::async_runtime::spawn_blocking(move || forward(&method, browser, &path))
        .await
        .map_err(|e| e.to_string())?;
    let body = rewrite(&body, browser, own);
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

async fn websocket(stream: TcpStream, browser: u16, on_activity: OnActivity) -> Result<(), String> {
    // Chrome sends big messages in one frame; tungstenite's default limits would end the connection.
    let config = WebSocketConfig::default().max_message_size(None).max_frame_size(None);
    let path = Arc::new(Mutex::new(String::new()));
    let seen = path.clone();
    #[allow(clippy::result_large_err)] // tungstenite's callback signature
    let client = tokio_tungstenite::accept_hdr_async_with_config(
        stream,
        move |req: &Request, res: Response| {
            *seen.lock().unwrap() = req.uri().to_string();
            Ok(res)
        },
        Some(config),
    )
    .await
    .map_err(|e| format!("handshake: {e}"))?;
    let path = path.lock().unwrap().clone();
    let (upstream, _) = tokio_tungstenite::connect_async_with_config(format!("ws://127.0.0.1:{browser}{path}"), Some(config), false)
        .await
        .map_err(|e| format!("connect to the browser: {e}"))?;

    let tracker = Arc::new(Mutex::new(Tracker::for_path(&path)));
    let (mut client_tx, mut client_rx) = client.split();
    let (mut upstream_tx, mut upstream_rx) = upstream.split();
    let opened_by = on_activity.clone();
    let commands = {
        let tracker = tracker.clone();
        async move {
            while let Some(Ok(msg)) = client_rx.next().await {
                if let Message::Text(text) = &msg {
                    let target = tracker.lock().unwrap().command(text);
                    if let Some(target) = target {
                        on_activity(Activity::Acted(&target));
                        if let Some(pointer) = Pointer::of(text) {
                            on_activity(Activity::Pointer(&target, pointer));
                        }
                    }
                }
                if upstream_tx.send(msg).await.is_err() {
                    break;
                }
            }
            let _ = upstream_tx.close().await;
        }
    };
    let events = async move {
        while let Some(Ok(msg)) = upstream_rx.next().await {
            if let Message::Text(text) = &msg {
                let opened = tracker.lock().unwrap().event(text);
                if let Some(target) = opened {
                    opened_by(Activity::Opened(&target));
                }
            }
            if client_tx.send(msg).await.is_err() {
                break;
            }
        }
        let _ = client_tx.close().await;
    };
    tokio::select! {
        _ = commands => {}
        _ = events => {}
    }
    Ok(())
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
    fn reads_an_agents_mouse_events() {
        let click = r#"{"id":4,"sessionId":"S1","method":"Input.dispatchMouseEvent","params":{"type":"mousePressed","x":120.5,"y":40,"button":"left","clickCount":1}}"#;
        assert_eq!(Pointer::of(click), Some(Pointer { x: 120.5, y: 40.0, kind: "mousePressed".into() }));
        assert_eq!(Pointer::of(&command("Input.dispatchKeyEvent", Some("S1"))), None);
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
                launched.port,
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
