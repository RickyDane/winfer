//! Page context for agents: console/exception/network errors per tab, and an element picker
//! (Chrome's own inspect mode, so the highlight shows in the screencast).

use std::collections::VecDeque;
use std::sync::Arc;

use base64::Engine;
use serde::Serialize;
use serde_json::{Value, json};
use tauri::{AppHandle, Emitter, Manager, State};

use super::cdp::Event;
use super::{BrowserManager, Instance, Res, instance};

const MAX_ENTRIES: usize = 200;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConsoleEntry {
    /// "error", "warning", "info" or "log".
    level: String,
    text: String,
    source: Option<String>,
    timestamp: f64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ConsoleEvent {
    workspace_id: String,
    target_id: String,
    entry: ConsoleEntry,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Picked {
    workspace_id: String,
    url: String,
    selector: String,
    html: String,
    text: String,
    /// PNG of the element, for agents that read images from disk.
    screenshot: Option<String>,
}

fn now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64() * 1000.0)
}

/// Turns a CDP event into a console entry, if it is one we keep.
pub fn console_entry(event: &Event) -> Option<ConsoleEntry> {
    let p = &event.params;
    let entry = |level: &str, text: String, source: Option<String>| ConsoleEntry {
        level: level.into(),
        text,
        source,
        timestamp: now(),
    };
    match event.method.as_str() {
        "Runtime.consoleAPICalled" => {
            let level = match p["type"].as_str()? {
                "error" | "assert" => "error",
                "warning" => "warning",
                "info" => "info",
                _ => "log",
            };
            let text = p["args"]
                .as_array()?
                .iter()
                .map(|a| {
                    a["value"]
                        .as_str()
                        .map(String::from)
                        .or_else(|| a["description"].as_str().map(String::from))
                        .unwrap_or_else(|| a["value"].to_string())
                })
                .collect::<Vec<_>>()
                .join(" ");
            let frame = &p["stackTrace"]["callFrames"][0];
            let source = frame["url"]
                .as_str()
                .filter(|u| !u.is_empty())
                .map(|u| format!("{u}:{}", frame["lineNumber"].as_i64().unwrap_or(0) + 1));
            Some(entry(level, text, source))
        }
        "Runtime.exceptionThrown" => {
            let d = &p["exceptionDetails"];
            let text = d["exception"]["description"]
                .as_str()
                .or(d["text"].as_str())
                .unwrap_or("Uncaught exception")
                .to_string();
            let source = d["url"]
                .as_str()
                .map(|u| format!("{u}:{}", d["lineNumber"].as_i64().unwrap_or(0) + 1));
            Some(entry("error", text, source))
        }
        "Log.entryAdded" => {
            let e = &p["entry"];
            let level = match e["level"].as_str()? {
                "error" => "error",
                "warning" => "warning",
                _ => return None, // browser-internal verbose/info noise
            };
            Some(entry(
                level,
                e["text"].as_str()?.to_string(),
                e["url"].as_str().map(String::from),
            ))
        }
        "Network.responseReceived" => {
            let r = &p["response"];
            let status = r["status"].as_i64()?;
            let kind = p["type"].as_str().unwrap_or("");
            (status >= 400 && matches!(kind, "Document" | "XHR" | "Fetch" | "Script")).then(|| {
                entry(
                    "error",
                    format!("HTTP {status} {} ({kind})", r["url"].as_str().unwrap_or("")),
                    None,
                )
            })
        }
        "Network.loadingFailed" if p["canceled"] != true => Some(entry(
            "error",
            format!(
                "Network error: {} ({})",
                p["errorText"].as_str().unwrap_or("failed"),
                p["type"].as_str().unwrap_or("")
            ),
            None,
        )),
        _ => None,
    }
}

impl Instance {
    pub(super) fn record_console(&self, target_id: &str, entry: ConsoleEntry) {
        {
            let mut console = self.console.lock().unwrap();
            let entries = console.entry(target_id.to_string()).or_default();
            entries.push_back(entry.clone());
            while entries.len() > MAX_ENTRIES {
                entries.pop_front();
            }
        }
        let _ = self.app.emit(
            "browser://console",
            ConsoleEvent {
                workspace_id: self.workspace_id.clone(),
                target_id: target_id.to_string(),
                entry,
            },
        );
    }

    /// Handles `Overlay.inspectNodeRequested`: describe the element, screenshot it, emit it.
    pub(super) async fn finish_pick(
        self: Arc<Self>,
        session: String,
        backend_node_id: i64,
    ) -> Res<()> {
        let cdp = self.cdp().await?;
        let _ = cdp
            .call(
                "Overlay.setInspectMode",
                json!({ "mode": "none", "highlightConfig": {} }),
                Some(&session),
            )
            .await;

        let object = cdp
            .call(
                "DOM.resolveNode",
                json!({ "backendNodeId": backend_node_id }),
                Some(&session),
            )
            .await?;
        let object_id = object["object"]["objectId"]
            .as_str()
            .ok_or("couldn't resolve element")?;
        let info = cdp
            .call(
                "Runtime.callFunctionOn",
                json!({ "objectId": object_id, "functionDeclaration": DESCRIBE_ELEMENT, "returnByValue": true }),
                Some(&session),
            )
            .await?;
        let info = &info["result"]["value"];

        let screenshot = self
            .screenshot_element(&session, backend_node_id)
            .await
            .ok();
        let url = cdp
            .call(
                "Runtime.evaluate",
                json!({ "expression": "location.href", "returnByValue": true }),
                Some(&session),
            )
            .await?;

        let picked = Picked {
            workspace_id: self.workspace_id.clone(),
            url: url["result"]["value"].as_str().unwrap_or_default().into(),
            selector: info["selector"].as_str().unwrap_or_default().into(),
            html: info["html"].as_str().unwrap_or_default().into(),
            text: info["text"].as_str().unwrap_or_default().into(),
            screenshot,
        };
        let _ = self.app.emit("browser://picked", picked);
        Ok(())
    }

    async fn screenshot_element(&self, session: &str, backend_node_id: i64) -> Res<String> {
        let cdp = self.cdp.get().ok_or("not connected")?;
        let model = cdp
            .call(
                "DOM.getBoxModel",
                json!({ "backendNodeId": backend_node_id }),
                Some(session),
            )
            .await?;
        let quad: Vec<f64> = model["model"]["border"]
            .as_array()
            .ok_or("no box")?
            .iter()
            .filter_map(Value::as_f64)
            .collect();
        let (xs, ys): (Vec<f64>, Vec<f64>) = quad.chunks(2).map(|p| (p[0], p[1])).unzip();
        let (x0, x1) = (
            xs.iter().cloned().fold(f64::MAX, f64::min),
            xs.iter().cloned().fold(f64::MIN, f64::max),
        );
        let (y0, y1) = (
            ys.iter().cloned().fold(f64::MAX, f64::min),
            ys.iter().cloned().fold(f64::MIN, f64::max),
        );
        // Box model is in viewport coordinates; screenshot clips are in page coordinates.
        let metrics = cdp
            .call("Page.getLayoutMetrics", json!({}), Some(session))
            .await?;
        let (sx, sy) = (
            metrics["cssVisualViewport"]["pageX"]
                .as_f64()
                .unwrap_or(0.0),
            metrics["cssVisualViewport"]["pageY"]
                .as_f64()
                .unwrap_or(0.0),
        );
        let pad = 8.0;
        let shot = cdp
            .call(
                "Page.captureScreenshot",
                json!({
                    "format": "png",
                    "clip": { "x": (x0 + sx - pad).max(0.0), "y": (y0 + sy - pad).max(0.0), "width": (x1 - x0 + 2.0 * pad).max(1.0), "height": (y1 - y0 + 2.0 * pad).max(1.0), "scale": 1 },
                    "captureBeyondViewport": true
                }),
                Some(session),
            )
            .await?;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(shot["data"].as_str().unwrap_or_default())
            .map_err(|e| e.to_string())?;
        let dir = self
            .app
            .path()
            .app_cache_dir()
            .map_err(|e| e.to_string())?
            .join("picks");
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let path = dir.join(format!("element-{}.png", now() as u64));
        std::fs::write(&path, bytes).map_err(|e| e.to_string())?;
        Ok(path.to_string_lossy().into_owned())
    }
}

/// Runs in the page with `this` = the picked element.
const DESCRIBE_ELEMENT: &str = r##"function () {
  const el = this;
  const esc = (s) => (window.CSS && CSS.escape ? CSS.escape(s) : s);
  const part = (n) => {
    if (n.id) return "#" + esc(n.id);
    let s = n.tagName.toLowerCase();
    const testId = n.getAttribute("data-testid");
    if (testId) return s + '[data-testid="' + testId + '"]';
    const cls = [...n.classList].filter((c) => !/^(css|sc|jsx)-|\d{3,}/.test(c)).slice(0, 2);
    if (cls.length) s += "." + cls.map(esc).join(".");
    const parent = n.parentElement;
    if (parent && [...parent.children].filter((c) => c.matches(s)).length > 1) {
      const same = [...parent.children].filter((c) => c.tagName === n.tagName);
      s += ":nth-of-type(" + (same.indexOf(n) + 1) + ")";
    }
    return s;
  };
  const parts = [];
  for (let n = el; n && n.nodeType === 1 && parts.length < 5; n = n.parentElement) {
    parts.unshift(part(n));
    if (n.id || document.querySelectorAll(parts.join(" > ")).length === 1) break;
  }
  const html = el.outerHTML.length > 1500 ? el.outerHTML.slice(0, 1500) + "…" : el.outerHTML;
  const text = (el.innerText || "").trim().slice(0, 300);
  return { selector: parts.join(" > "), html, text };
}"##;

#[tauri::command]
pub async fn browser_console(
    app: AppHandle,
    manager: State<'_, BrowserManager>,
    workspace_id: String,
    target_id: String,
) -> Res<Vec<ConsoleEntry>> {
    let instance = instance(&app, &manager, &workspace_id).await?;
    let console = instance.console.lock().unwrap();
    Ok(console
        .get(&target_id)
        .map(|e| e.iter().cloned().collect())
        .unwrap_or_default())
}

#[tauri::command]
pub async fn browser_console_clear(
    app: AppHandle,
    manager: State<'_, BrowserManager>,
    workspace_id: String,
    target_id: String,
) -> Res<()> {
    let instance = instance(&app, &manager, &workspace_id).await?;
    instance
        .console
        .lock()
        .unwrap()
        .insert(target_id, VecDeque::new());
    Ok(())
}

/// Starts Chrome's inspect mode: hovering highlights elements, clicking emits `browser://picked`.
#[tauri::command]
pub async fn browser_pick_start(
    app: AppHandle,
    manager: State<'_, BrowserManager>,
    workspace_id: String,
    target_id: String,
) -> Res<()> {
    let instance = instance(&app, &manager, &workspace_id).await?;
    let session = instance.session(&target_id).await?;
    let cdp = instance.cdp().await?;
    cdp.call("DOM.enable", json!({}), Some(&session)).await?;
    cdp.call("Overlay.enable", json!({}), Some(&session))
        .await?;
    cdp.call(
        "Overlay.setInspectMode",
        json!({
            "mode": "searchForNode",
            "highlightConfig": {
                "showInfo": true,
                "contentColor": { "r": 109, "g": 124, "b": 255, "a": 0.25 },
                "borderColor": { "r": 109, "g": 124, "b": 255, "a": 0.9 },
                "marginColor": { "r": 245, "g": 181, "b": 68, "a": 0.2 }
            }
        }),
        Some(&session),
    )
    .await?;
    Ok(())
}

#[tauri::command]
pub async fn browser_pick_cancel(
    app: AppHandle,
    manager: State<'_, BrowserManager>,
    workspace_id: String,
    target_id: String,
) -> Res<()> {
    let instance = instance(&app, &manager, &workspace_id).await?;
    let session = instance.session(&target_id).await?;
    instance
        .cdp()
        .await?
        .call(
            "Overlay.setInspectMode",
            json!({ "mode": "none", "highlightConfig": {} }),
            Some(&session),
        )
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(method: &str, params: Value) -> Event {
        Event {
            method: method.into(),
            params,
            session_id: None,
        }
    }

    #[test]
    fn keeps_errors_and_drops_noise() {
        let e = console_entry(&ev(
            "Runtime.consoleAPICalled",
            json!({ "type": "error", "args": [{ "type": "string", "value": "Failed to load" }, { "type": "number", "value": 42 }],
                    "stackTrace": { "callFrames": [{ "url": "http://localhost:3000/app.js", "lineNumber": 9 }] } }),
        ))
        .unwrap();
        assert_eq!(
            (e.level.as_str(), e.text.as_str()),
            ("error", "Failed to load 42")
        );
        assert_eq!(e.source.as_deref(), Some("http://localhost:3000/app.js:10"));

        let e = console_entry(&ev("Network.responseReceived", json!({ "type": "Fetch", "response": { "status": 500, "url": "http://localhost:3000/api" } }))).unwrap();
        assert_eq!(e.text, "HTTP 500 http://localhost:3000/api (Fetch)");

        assert!(
            console_entry(&ev(
                "Network.responseReceived",
                json!({ "type": "Image", "response": { "status": 404, "url": "x" } })
            ))
            .is_none()
        );
        assert!(
            console_entry(&ev(
                "Log.entryAdded",
                json!({ "entry": { "level": "verbose", "text": "x" } })
            ))
            .is_none()
        );
        assert!(
            console_entry(&ev(
                "Network.loadingFailed",
                json!({ "canceled": true, "errorText": "x" })
            ))
            .is_none()
        );
    }
}

#[cfg(test)]
mod chrome_tests {
    use super::super::{cdp::Cdp, chrome};
    use super::*;
    use std::sync::{Mutex, mpsc};
    use std::time::Duration;

    /// Real Chrome: console errors parse, and the element describer yields a unique selector.
    #[test]
    fn console_and_element_description() {
        let tmp = std::env::temp_dir().join(format!("winfer-inspect-{:x}", rand::random::<u32>()));
        let Some(exe) = chrome::find(&tmp.join("managed")) else {
            eprintln!("no Chrome found; skipping");
            return;
        };
        let launched = chrome::launch(&exe, &tmp.join("profile")).unwrap();
        let _guard = chrome::ChromeGuard(launched.child);
        tauri::async_runtime::block_on(async {
            let (tx, rx) = mpsc::channel::<Event>();
            let tx = Mutex::new(tx);
            let cdp = Cdp::connect(&chrome::wait_ws_url(launched.port, std::time::Duration::from_secs(20)).unwrap(), move |e| {
                let _ = tx.lock().unwrap().send(e);
            })
            .await
            .unwrap();
            let page = "data:text/html,<ul><li>a</li><li class=item>b</li></ul><button data-testid=save>Save</button>";
            let target = cdp
                .call("Target.createTarget", json!({ "url": page }), None)
                .await
                .unwrap();
            let session = cdp
                .call(
                    "Target.attachToTarget",
                    json!({ "targetId": target["targetId"], "flatten": true }),
                    None,
                )
                .await
                .unwrap()["sessionId"]
                .as_str()
                .unwrap()
                .to_string();
            cdp.call("Runtime.enable", json!({}), Some(&session))
                .await
                .unwrap();
            cdp.call(
                "Runtime.evaluate",
                json!({ "expression": "console.error('boom', 42)" }),
                Some(&session),
            )
            .await
            .unwrap();

            let deadline = std::time::Instant::now() + Duration::from_secs(10);
            let mut found = None;
            while found.is_none() && std::time::Instant::now() < deadline {
                if let Ok(e) = rx.recv_timeout(Duration::from_millis(200)) {
                    found = console_entry(&e).filter(|c| c.level == "error");
                }
            }
            assert_eq!(found.expect("no console error captured").text, "boom 42");

            for (query, expected) in [
                ("li.item", "li.item"),
                ("button", r#"button[data-testid="save"]"#),
            ] {
                let obj = cdp
                    .call(
                        "Runtime.evaluate",
                        json!({ "expression": format!("document.querySelector('{query}')") }),
                        Some(&session),
                    )
                    .await
                    .unwrap();
                let info = cdp
                    .call(
                        "Runtime.callFunctionOn",
                        json!({ "objectId": obj["result"]["objectId"], "functionDeclaration": DESCRIBE_ELEMENT, "returnByValue": true }),
                        Some(&session),
                    )
                    .await
                    .unwrap();
                let selector = info["result"]["value"]["selector"]
                    .as_str()
                    .unwrap()
                    .to_string();
                assert!(selector.ends_with(expected), "{query}: {selector}");
                let count = cdp
                    .call("Runtime.evaluate", json!({ "expression": format!("document.querySelectorAll({selector:?}).length"), "returnByValue": true }), Some(&session))
                    .await
                    .unwrap();
                assert_eq!(count["result"]["value"], 1, "{selector} isn't unique");
            }

            // Inspect mode reacts to the synthetic clicks the panel sends.
            cdp.call("DOM.enable", json!({}), Some(&session))
                .await
                .unwrap();
            cdp.call("Overlay.enable", json!({}), Some(&session))
                .await
                .unwrap();
            cdp.call(
                "Overlay.setInspectMode",
                json!({ "mode": "searchForNode", "highlightConfig": {} }),
                Some(&session),
            )
            .await
            .unwrap();
            let rect = cdp
                .call(
                    "Runtime.evaluate",
                    json!({ "expression": "JSON.stringify(document.querySelector('button').getBoundingClientRect())", "returnByValue": true }),
                    Some(&session),
                )
                .await
                .unwrap();
            let rect: Value =
                serde_json::from_str(rect["result"]["value"].as_str().unwrap()).unwrap();
            let (x, y) = (
                rect["x"].as_f64().unwrap() + 5.0,
                rect["y"].as_f64().unwrap() + 5.0,
            );
            for kind in ["mouseMoved", "mousePressed", "mouseReleased"] {
                cdp.call(
                    "Input.dispatchMouseEvent",
                    json!({ "type": kind, "x": x, "y": y, "button": "left", "clickCount": 1 }),
                    Some(&session),
                )
                .await
                .unwrap();
            }
            let deadline = std::time::Instant::now() + Duration::from_secs(10);
            let mut picked = None;
            while picked.is_none() && std::time::Instant::now() < deadline {
                if let Ok(e) = rx.recv_timeout(Duration::from_millis(200)) {
                    if e.method == "Overlay.inspectNodeRequested" {
                        picked = e.params["backendNodeId"].as_i64();
                    }
                }
            }
            let node = picked.expect("inspect mode didn't report the clicked node");
            let described = cdp
                .call(
                    "DOM.describeNode",
                    json!({ "backendNodeId": node }),
                    Some(&session),
                )
                .await
                .unwrap();
            assert_eq!(described["node"]["nodeName"], "BUTTON");
        });
    }
}
