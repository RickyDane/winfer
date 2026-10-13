//! Embedded browser: one headless Chromium per workspace (own profile), shown in the panel via
//! CDP screencast and shared with agents through `WINFER_CDP_URL`.
//!
//! WKWebView has no CDP, which browser-use / Playwright need, hence a real Chromium.

mod cdp;
pub mod chrome;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::process::Child;
use std::sync::{Arc, Mutex, Weak};

use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::OnceCell;

use cdp::{Cdp, Event};
use chrome::Executable;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Tab {
    id: String,
    url: String,
    title: String,
    /// Opened by an agent (not from the winfer UI).
    by_agent: bool,
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
    session_id: String,
    channel: Channel,
}

struct Instance {
    workspace_id: String,
    app: AppHandle,
    child: Mutex<Child>,
    port: u16,
    ws_url: String,
    cdp: OnceCell<Arc<Cdp>>,
    /// Page targets in creation order.
    tabs: Mutex<Vec<Tab>>,
    created_by_ui: Mutex<HashSet<String>>,
    sessions: Mutex<HashMap<String, String>>,
    screencast: Mutex<Option<Screencast>>,
}

impl Drop for Instance {
    /// Asks Chrome to quit first so it flushes cookies and the profile, then force-kills.
    fn drop(&mut self) {
        let mut child = self.child.lock().unwrap();
        #[cfg(unix)]
        unsafe {
            libc::kill(child.id() as i32, libc::SIGTERM);
        }
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while std::time::Instant::now() < deadline {
            if child.try_wait().ok().flatten().is_some() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        let _ = child.kill();
        let _ = child.wait();
    }
}

impl Instance {
    fn emit_tabs(&self) {
        let tabs = self.tabs.lock().unwrap().clone();
        let _ = self.app.emit(
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
                if info["type"] != "page" {
                    return;
                }
                let id = info["targetId"].as_str().unwrap_or_default().to_string();
                let by_agent = !self.created_by_ui.lock().unwrap().contains(&id);
                let tab = Tab {
                    url: info["url"].as_str().unwrap_or_default().into(),
                    title: info["title"].as_str().unwrap_or_default().into(),
                    by_agent,
                    id: id.clone(),
                };
                {
                    let mut tabs = self.tabs.lock().unwrap();
                    match tabs.iter_mut().find(|t| t.id == id) {
                        Some(existing) => {
                            *existing = Tab {
                                by_agent: existing.by_agent,
                                ..tab
                            }
                        }
                        None => tabs.push(tab),
                    }
                }
                self.emit_tabs();
            }
            "Target.targetDestroyed" => {
                let id = event.params["targetId"].as_str().unwrap_or_default();
                self.tabs.lock().unwrap().retain(|t| t.id != id);
                self.sessions.lock().unwrap().remove(id);
                self.emit_tabs();
            }
            "Page.screencastFrame" => {
                let Some(session) = event.session_id.as_deref() else {
                    return;
                };
                let screencast = self.screencast.lock().unwrap();
                if let Some(sc) = screencast.as_ref().filter(|sc| sc.session_id == session) {
                    if let Ok(bytes) = base64::engine::general_purpose::STANDARD
                        .decode(event.params["data"].as_str().unwrap_or_default())
                    {
                        let _ = sc.channel.send(InvokeResponseBody::Raw(bytes));
                    }
                }
                drop(screencast);
                if let Some(cdp) = self.cdp.get() {
                    cdp.send(
                        "Page.screencastFrameAck",
                        json!({ "sessionId": event.params["sessionId"] }),
                        Some(session),
                    );
                }
            }
            _ => {}
        }
    }

    async fn cdp(self: &Arc<Self>) -> Result<Arc<Cdp>, String> {
        let weak: Weak<Self> = Arc::downgrade(self);
        let ws_url = self.ws_url.clone();
        let cdp = self
            .cdp
            .get_or_try_init(|| async move {
                let cdp = Cdp::connect(&ws_url, move |event| {
                    if let Some(instance) = weak.upgrade() {
                        instance.on_event(event);
                    }
                })
                .await?;
                cdp.call(
                    "Target.setDiscoverTargets",
                    json!({ "discover": true }),
                    None,
                )
                .await?;
                Ok::<_, String>(cdp)
            })
            .await?;
        Ok(cdp.clone())
    }

    async fn session(self: &Arc<Self>, target_id: &str) -> Result<String, String> {
        if let Some(session) = self.sessions.lock().unwrap().get(target_id) {
            return Ok(session.clone());
        }
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
        cdp.call("Page.enable", json!({}), Some(&session)).await?;
        self.sessions
            .lock()
            .unwrap()
            .insert(target_id.to_string(), session.clone());
        Ok(session)
    }
}

#[derive(Default)]
pub struct BrowserManager {
    instances: Mutex<HashMap<String, Arc<Instance>>>,
}

fn data_dir(app: &AppHandle) -> Result<PathBuf, String> {
    app.path().app_data_dir().map_err(|e| e.to_string())
}

fn managed_dir(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(data_dir(app)?.join("browser"))
}

impl BrowserManager {
    /// Starts (or returns) the workspace's browser. Synchronous so PTY spawns can get the CDP URL.
    fn ensure(&self, app: &AppHandle, workspace_id: &str) -> Result<Arc<Instance>, String> {
        let mut instances = self.instances.lock().unwrap();
        if let Some(instance) = instances.get(workspace_id) {
            let exited = instance
                .child
                .lock()
                .unwrap()
                .try_wait()
                .ok()
                .flatten()
                .is_some();
            if !exited {
                return Ok(instance.clone());
            }
            instances.remove(workspace_id);
        }
        let exe = chrome::find(&managed_dir(app)?).ok_or("no browser installed")?;
        let profile = data_dir(app)?
            .join("browser-profiles")
            .join(sanitize(workspace_id));
        let launched = chrome::launch(&exe, &profile)?;
        let instance = Arc::new(Instance {
            workspace_id: workspace_id.to_string(),
            app: app.clone(),
            child: Mutex::new(launched.child),
            port: launched.port,
            ws_url: launched.ws_url,
            cdp: OnceCell::new(),
            tabs: Mutex::default(),
            created_by_ui: Mutex::default(),
            sessions: Mutex::default(),
            screencast: Mutex::default(),
        });
        instances.insert(workspace_id.to_string(), instance.clone());
        Ok(instance)
    }

    /// CDP endpoint for agents in this workspace, starting the browser if one is installed.
    pub fn cdp_url(&self, app: &AppHandle, workspace_id: &str) -> Option<String> {
        self.ensure(app, workspace_id)
            .ok()
            .map(|i| format!("http://127.0.0.1:{}", i.port))
    }

    pub fn shutdown(&self) {
        self.instances.lock().unwrap().clear();
    }
}

/// Kills browsers left behind by a previous run that didn't exit cleanly (crash, force quit).
/// They'd otherwise keep running and hold the profile lock, so the next launch would fail.
pub fn kill_orphans(app: &AppHandle) {
    let Ok(profiles) = data_dir(app).map(|d| d.join("browser-profiles")) else {
        return;
    };
    #[cfg(unix)]
    {
        let _ = std::process::Command::new("pkill")
            .arg("-f")
            .arg(format!("user-data-dir={}", profiles.display()))
            .status();
    }
    #[cfg(windows)]
    {
        let filter = format!(
            "CommandLine like '%--user-data-dir={}%'",
            profiles.display().to_string().replace('\\', "\\\\")
        );
        let _ = std::process::Command::new("wmic")
            .args(["process", "where", &filter, "call", "terminate"])
            .status();
    }
}

/// CLI args that give an agent the Playwright MCP server, attached to the workspace browser.
pub fn agent_mcp_args(provider_id: &str, cdp_url: &str) -> Vec<String> {
    let args = json!(["-y", "@playwright/mcp@latest", "--cdp-endpoint", cdp_url]);
    match provider_id {
        "claude" => {
            let config =
                json!({ "mcpServers": { "winfer-browser": { "command": "npx", "args": args } } });
            vec!["--mcp-config".into(), config.to_string()]
        }
        "codex" => vec![
            "-c".into(),
            "mcp_servers.winfer_browser.command=\"npx\"".into(),
            "-c".into(),
            format!("mcp_servers.winfer_browser.args={args}"),
        ],
        _ => Vec::new(),
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
    let instance = manager.ensure(app, workspace_id)?;
    instance.cdp().await?;
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
    tauri::async_runtime::spawn_blocking(move || chrome::install(&dir))
        .await
        .map_err(|e| e.to_string())?
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
    {
        let ui = instance.created_by_ui.lock().unwrap();
        let mut tabs = instance.tabs.lock().unwrap();
        for info in targets["targetInfos"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|t| t["type"] == "page")
        {
            let id = info["targetId"].as_str().unwrap_or_default();
            if !tabs.iter().any(|t| t.id == id) {
                tabs.push(Tab {
                    id: id.into(),
                    url: info["url"].as_str().unwrap_or_default().into(),
                    title: info["title"].as_str().unwrap_or_default().into(),
                    by_agent: !ui.contains(id) && info["url"] != "about:blank",
                });
            }
        }
    }
    Ok(instance.tabs.lock().unwrap().clone())
}

#[tauri::command]
pub async fn browser_new_tab(
    app: AppHandle,
    manager: State<'_, BrowserManager>,
    workspace_id: String,
    url: String,
) -> Res<String> {
    let instance = instance(&app, &manager, &workspace_id).await?;
    let res = instance
        .cdp()
        .await?
        .call("Target.createTarget", json!({ "url": url }), None)
        .await?;
    let id = res["targetId"]
        .as_str()
        .ok_or("createTarget returned no id")?
        .to_string();
    instance.created_by_ui.lock().unwrap().insert(id.clone());
    if let Some(tab) = instance
        .tabs
        .lock()
        .unwrap()
        .iter_mut()
        .find(|t| t.id == id)
    {
        tab.by_agent = false;
    }
    instance.emit_tabs();
    Ok(id)
}

#[tauri::command]
pub async fn browser_close_tab(
    app: AppHandle,
    manager: State<'_, BrowserManager>,
    workspace_id: String,
    target_id: String,
) -> Res<()> {
    let instance = instance(&app, &manager, &workspace_id).await?;
    instance
        .cdp()
        .await?
        .call("Target.closeTarget", json!({ "targetId": target_id }), None)
        .await?;
    Ok(())
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
    let session = instance.session(&target_id).await?;
    instance
        .cdp()
        .await?
        .call("Page.navigate", json!({ "url": url }), Some(&session))
        .await?;
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
    let session = instance.session(&target_id).await?;
    let cdp = instance.cdp().await?;
    if delta == 0 {
        cdp.call("Page.reload", json!({}), Some(&session)).await?;
        return Ok(());
    }
    let history = cdp
        .call("Page.getNavigationHistory", json!({}), Some(&session))
        .await?;
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

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Viewport {
    width: u32,
    height: u32,
    device_scale_factor: f64,
    mobile: bool,
}

async fn apply_viewport(cdp: &Cdp, session: &str, v: &Viewport) -> Res<()> {
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
    let cdp = instance.cdp().await?;
    let previous = instance.screencast.lock().unwrap().take();
    if let Some(prev) = previous {
        let _ = cdp
            .call("Page.stopScreencast", json!({}), Some(&prev.session_id))
            .await;
    }
    let session = instance.session(&target_id).await?;
    apply_viewport(&cdp, &session, &viewport).await?;
    *instance.screencast.lock().unwrap() = Some(Screencast {
        attach_id,
        session_id: session.clone(),
        channel: on_frame,
    });
    cdp.call(
        "Page.startScreencast",
        json!({ "format": "jpeg", "quality": 80, "everyNthFrame": 1 }),
        Some(&session),
    )
    .await?;
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
    apply_viewport(&cdp, &session, &viewport).await
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
    let session = instance.session(&target_id).await?;
    instance
        .cdp()
        .await?
        .call(&method, params, Some(&session))
        .await?;
    Ok(())
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
    let session = instance.session(&target_id).await?;
    let res = instance
        .cdp()
        .await?
        .call(
            "Runtime.evaluate",
            json!({ "expression": "window.getSelection().toString()", "returnByValue": true }),
            Some(&session),
        )
        .await?;
    Ok(res["result"]["value"]
        .as_str()
        .unwrap_or_default()
        .to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    /// Real Chrome: discover tabs, screencast a frame, type into a page. Skips without Chrome.
    #[test]
    fn screencast_and_input_round_trip() {
        let tmp = std::env::temp_dir().join(format!("winfer-cdp-{:x}", rand::random::<u32>()));
        let Some(exe) = chrome::find(&tmp.join("managed")) else {
            eprintln!("no Chrome found; skipping");
            return;
        };
        let mut launched = chrome::launch(&exe, &tmp.join("profile")).unwrap();

        tauri::async_runtime::block_on(async {
            let (tx, rx) = mpsc::channel::<Event>();
            let tx = Mutex::new(tx);
            let cdp = Cdp::connect(&launched.ws_url, move |e| {
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

            // Discovery reported the page, and a JPEG frame arrives.
            let deadline = std::time::Instant::now() + Duration::from_secs(15);
            let (mut saw_target, mut frame) = (false, None);
            while std::time::Instant::now() < deadline && (!saw_target || frame.is_none()) {
                let Ok(e) = rx.recv_timeout(Duration::from_millis(200)) else {
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

        let _ = launched.child.kill();
        let _ = launched.child.wait();
    }

    #[test]
    fn mcp_args_per_provider() {
        let claude = agent_mcp_args("claude", "http://127.0.0.1:9");
        assert_eq!(claude[0], "--mcp-config");
        assert!(claude[1].contains("--cdp-endpoint") && claude[1].contains("http://127.0.0.1:9"));
        let codex = agent_mcp_args("codex", "http://127.0.0.1:9");
        assert_eq!(codex.len(), 4);
        assert!(agent_mcp_args("agy", "x").is_empty());
    }
}
