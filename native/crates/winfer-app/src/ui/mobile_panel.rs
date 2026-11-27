//! The side panel on the phone (docs/mobile/protocol.md, "Side panel"): a workspace's browser,
//! the Android mirror, terminals and files, each usable from the phone.
//!
//! Terminals are Winfer Connect's `term.*` stream with the phone as peer `phone:<device>`. The
//! browser and the Android mirror stream JPEG frames (`panel.frame`), each acknowledged by the
//! phone (`panel.frameDone`) before the next goes out, so the rate follows what its relay carries.
//! Browser frames are screenshots of the tab's viewport (the panel's own screencast stays as it is);
//! Android frames are the mirror's pictures, which runs while a phone watches it.
//! A phone that stops asking (it asks at least every 20 s while open) loses its streams.

use super::*;
use crate::ui::browser::{normalize_url, ops};
use crate::ui::capture::{self, Source};
use winfer_core::scrcpy::Touch;
use crate::ui::viewer::{expand, insert_comment, viewer_projects};
use parking_lot::Mutex;
use serde_json::json;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;
use winfer_backend::browser::{self as browser_backend, BrowserManager};
use winfer_backend::files;

/// A phone silent for this long is gone: its streams stop.
const LEASE: Duration = Duration::from_secs(45);
/// A frame not acknowledged by then counts as lost; the next one goes anyway.
const FRAME_WAIT: Duration = Duration::from_secs(4);
/// Largest file the phone opens: a relay frame is at most 1 MiB, base64'd twice.
const MAX_TEXT: u64 = 400 << 10;
const MAX_IMAGE: u64 = 480 << 10;
const IMAGES: [&str; 8] = ["png", "jpg", "jpeg", "gif", "webp", "bmp", "svg", "ico"];

#[derive(Clone, Debug, PartialEq)]
enum Feed {
    Browser { workspace: String, tab: String },
    Android,
}

struct Watch {
    feed: Feed,
    serial: u64,
    max_width: u32,
    quality: u8,
    sent: u64,
    acked: u64,
    sent_at: Instant,
}

#[derive(Default)]
struct Phone {
    seen: Option<Instant>,
    watch: Option<Watch>,
}

static PHONES: LazyLock<Mutex<HashMap<String, Phone>>> = LazyLock::new(Default::default);
static SERIAL: AtomicU64 = AtomicU64::new(1);

/// Any request from the phone renews its lease.
pub(super) fn seen(device: &str) {
    PHONES.lock().entry(device.to_string()).or_default().seen = Some(Instant::now());
}

fn alive(p: &Phone) -> bool {
    p.seen.is_some_and(|t| t.elapsed() < LEASE)
}

fn hash(bytes: &[u8]) -> u64 {
    let mut h = DefaultHasher::new();
    bytes.hash(&mut h);
    h.finish()
}

/// The phone drew frame `seq`: the picture gets lighter when frames take long, heavier when quick.
fn frame_done(device: &str, seq: u64) {
    let mut phones = PHONES.lock();
    let Some(w) = phones.get_mut(device).and_then(|p| p.watch.as_mut()) else { return };
    if seq < w.acked || seq > w.sent {
        return;
    }
    w.acked = seq;
    let ms = w.sent_at.elapsed().as_millis();
    w.quality = next_quality(w.quality, ms);
}

fn next_quality(quality: u8, ms: u128) -> u8 {
    if ms > 700 {
        quality.saturating_sub(8).max(30)
    } else if ms < 180 {
        (quality + 3).min(70)
    } else {
        quality
    }
}

/// Starts streaming `feed` to the phone, replacing what it watched.
fn watch(device: &str, feed: Feed, max_width: u32) {
    let serial = SERIAL.fetch_add(1, Ordering::Relaxed);
    {
        let mut phones = PHONES.lock();
        let p = phones.entry(device.to_string()).or_default();
        p.seen = Some(Instant::now());
        p.watch = Some(Watch { feed, serial, max_width: max_width.clamp(240, 1600), quality: 55, sent: 0, acked: 0, sent_at: Instant::now() });
    }
    let device = device.to_string();
    tauri::async_runtime::spawn(async move { pump(device, serial).await });
}

/// Stops the phone's stream, if it still shows what `left` says (a screen it already left could
/// otherwise stop the next screen's stream).
fn unwatch(device: &str, left: impl Fn(&Feed) -> bool) {
    if let Some(p) = PHONES.lock().get_mut(device)
        && p.watch.as_ref().is_some_and(|w| left(&w.feed))
    {
        p.watch = None;
    }
}

async fn sleep(ms: u64) {
    tokio::time::sleep(Duration::from_millis(ms)).await;
}

/// Sends frames of the phone's watch `serial` until it watches something else or goes away.
async fn pump(device: String, serial: u64) {
    let peer = crate::connect::phone_peer(&device);
    let (mut last_hash, mut last_picture, mut last_error) = (0u64, 0u64, None::<String>);
    let mut last_sent = Instant::now() - Duration::from_secs(60);
    loop {
        let (feed, max_width, quality, waiting) = {
            let phones = PHONES.lock();
            let Some(w) = phones.get(&device).filter(|p| alive(p)).and_then(|p| p.watch.as_ref()).filter(|w| w.serial == serial) else { return };
            (w.feed.clone(), w.max_width, w.quality, w.sent > w.acked && w.sent_at.elapsed() < FRAME_WAIT)
        };
        if waiting {
            sleep(20).await;
            continue;
        }
        let frame = match &feed {
            Feed::Browser { workspace, tab } => {
                let app = backend::app();
                let shot = browser_backend::browser_capture(app, &app.state::<BrowserManager>(), workspace, tab, max_width, quality).await;
                match shot {
                    // A still page: the same picture again only now and then.
                    Ok((bytes, ..)) if hash(&bytes) == last_hash && last_sent.elapsed() < Duration::from_secs(5) => None,
                    Ok((bytes, w, h)) => {
                        last_hash = hash(&bytes);
                        Some((bytes, w, h))
                    }
                    Err(e) => {
                        if last_error.as_ref() != Some(&e) {
                            crate::connect::send(&peer, json!({ "t": "panel.error", "feed": "browser", "workspaceId": workspace, "tab": tab, "error": e }));
                            last_error = Some(e);
                        }
                        sleep(1000).await;
                        continue;
                    }
                }
            }
            Feed::Android => {
                let after = last_picture;
                let next = tauri::async_runtime::spawn_blocking(move || capture::newest_jpeg(Source::Android, after, max_width, quality)).await.ok().flatten();
                next.map(|(picture, bytes, w, h)| {
                    last_picture = picture;
                    (bytes, w as f64, h as f64)
                })
            }
        };
        let Some((bytes, width, height)) = frame else {
            sleep(if matches!(feed, Feed::Android) { 30 } else { 250 }).await;
            continue;
        };
        last_error = None;
        let seq = {
            let mut phones = PHONES.lock();
            let Some(w) = phones.get_mut(&device).and_then(|p| p.watch.as_mut()).filter(|w| w.serial == serial) else { return };
            w.sent += 1;
            w.sent_at = Instant::now();
            w.sent
        };
        last_sent = Instant::now();
        let mut msg = json!({
            "t": "panel.frame", "feed": if matches!(feed, Feed::Android) { "android" } else { "browser" },
            "seq": seq, "data": crate::connect_term::b64(&bytes), "width": width, "height": height,
        });
        if let Feed::Browser { workspace, tab } = &feed {
            msg["workspaceId"] = json!(workspace);
            msg["tab"] = json!(tab);
        }
        crate::connect::send(&peer, msg);
        // Room for the phone's input between frames.
        sleep(if matches!(feed, Feed::Android) { 15 } else { 60 }).await;
    }
}

/// Whether the phone may read `path`: it's in a workspace (its folder or an agent's worktree) or
/// open in the viewer.
fn allowed(path: &Path, roots: &[PathBuf], docs: &[String]) -> bool {
    let Ok(real) = path.canonicalize() else { return false };
    roots.iter().filter_map(|r| r.canonicalize().ok()).any(|r| real.starts_with(r)) || docs.iter().any(|d| Path::new(&expand(d)).canonicalize().ok().as_ref() == Some(&real))
}

fn extension(path: &str) -> String {
    Path::new(path).extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default()
}

fn mime(ext: &str) -> &'static str {
    match ext {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "svg" => "image/svg+xml",
        "ico" => "image/x-icon",
        _ => "application/octet-stream",
    }
}

/// A folder's entries for the phone: folders first, then files, by name (no `.git`).
fn list_dir(dir: &Path) -> Result<Vec<Value>, String> {
    let mut entries: Vec<(bool, String, u64)> = std::fs::read_dir(dir)
        .map_err(|e| format!("can't open {}: {e}", dir.display()))?
        .filter_map(Result::ok)
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            if name == ".git" || name == ".DS_Store" {
                return None;
            }
            let meta = std::fs::metadata(e.path()).ok()?;
            Some((meta.is_dir(), name, meta.len()))
        })
        .collect();
    entries.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.to_lowercase().cmp(&b.1.to_lowercase())));
    Ok(entries.into_iter().take(2000).map(|(dir, name, size)| json!({ "name": name, "dir": dir, "size": size })).collect())
}

/// A file as the phone shows it: text (with when it changed), an image, or neither.
fn read_file(path: &str) -> Result<Value, String> {
    let p = Path::new(path);
    let meta = std::fs::metadata(p).map_err(|e| format!("can't open {path}: {e}"))?;
    if !meta.is_file() {
        return Err(format!("{path} isn't a file"));
    }
    let modified = files::files_modified(path.to_string()).unwrap_or(0);
    let ext = extension(path);
    if IMAGES.contains(&ext.as_str()) {
        if meta.len() > MAX_IMAGE {
            return Ok(json!({ "path": path, "kind": "large", "size": meta.len(), "modified": modified }));
        }
        let bytes = std::fs::read(p).map_err(|e| format!("can't read {path}: {e}"))?;
        return Ok(json!({ "path": path, "kind": "image", "mime": mime(&ext), "data": crate::connect_term::b64(&bytes), "size": meta.len(), "modified": modified }));
    }
    if meta.len() > MAX_TEXT {
        return Ok(json!({ "path": path, "kind": "large", "size": meta.len(), "modified": modified }));
    }
    let bytes = std::fs::read(p).map_err(|e| format!("can't read {path}: {e}"))?;
    if bytes.iter().take(8000).any(|b| *b == 0) {
        return Ok(json!({ "path": path, "kind": "binary", "size": meta.len(), "modified": modified }));
    }
    let kind = if matches!(ext.as_str(), "md" | "markdown" | "mdx") { "markdown" } else { "text" };
    Ok(json!({ "path": path, "kind": kind, "text": String::from_utf8_lossy(&bytes), "size": meta.len(), "modified": modified }))
}

/// Answers once `job` is done.
fn later(job: tauri::async_runtime::JoinHandle<Result<Value, String>>, reply: impl Fn(Value) + 'static, cx: &mut Context<Root>) {
    cx.spawn(async move |_, _| {
        reply(match job.await.map_err(|e| e.to_string()).and_then(|r| r) {
            Ok(v) => ok(v),
            Err(e) => no(e),
        })
    })
    .detach();
}

fn str_of(req: &Value, key: &str) -> String {
    req[key].as_str().unwrap_or_default().to_string()
}

fn ok(value: Value) -> Value {
    json!({ "t": "ack", "ok": true, "value": value })
}

fn no(error: impl Into<String>) -> Value {
    json!({ "t": "ack", "ok": false, "error": error.into() })
}

impl Root {
    /// Once a second: phones that went quiet lose their streams; the Android mirror runs while one watches it.
    pub(super) fn tick_phone_panel(&mut self, cx: &mut Context<Self>) {
        let (gone, android) = {
            let mut phones = PHONES.lock();
            let gone: Vec<String> = phones.iter().filter(|(_, p)| !alive(p)).map(|(d, _)| d.clone()).collect();
            for d in &gone {
                phones.remove(d);
            }
            (gone, phones.values().any(|p| p.watch.as_ref().is_some_and(|w| w.feed == Feed::Android)))
        };
        for device in gone {
            crate::connect_term::drop_peer(&crate::connect::phone_peer(&device));
        }
        if self.android.read(cx).phone_watching() != android {
            self.android.update(cx, |a, cx| a.set_phone(android, cx));
        }
    }

    /// `term.*` from a phone: a pane's or a workspace side terminal's stream.
    pub(super) fn on_phone_term(&mut self, device: &str, req: &Value, cx: &mut Context<Self>) {
        let pane = str_of(req, "paneId");
        // Chats speak JSON lines; the phone has their thread instead.
        if self.code_models.contains_key(&pane) {
            crate::connect::send(&crate::connect::phone_peer(device), json!({ "t": "term.exit", "paneId": pane, "code": 0, "gone": true }));
            return;
        }
        self.on_term_request(&crate::connect::phone_peer(device), req, cx);
    }

    /// `panel.get`, `browser.call`, `android.call`, `files.call` and `panel.frameDone` from a phone.
    pub(super) fn on_phone_panel(&mut self, device: &str, kind: &str, req: &Value, reply: impl Fn(Value) + 'static, cx: &mut Context<Self>) {
        match kind {
            "panel.frameDone" => frame_done(device, req["seq"].as_u64().unwrap_or(0)),
            "panel.get" => reply(self.phone_panel_get(req, cx)),
            "browser.call" => self.phone_browser(device, req, reply, cx),
            "android.call" => self.phone_android(device, req, reply, cx),
            "files.call" => self.phone_files(req, reply, cx),
            _ => reply(no(format!("unknown request {kind}"))),
        }
    }

    /// The workspace's terminals: its side terminal, then its panes that are terminals (not chats).
    fn phone_panel_get(&self, req: &Value, cx: &App) -> Value {
        let s = self.state.read(cx);
        let Some(ws) = s.workspaces.iter().find(|w| w.id == str_of(req, "workspaceId") && w.remote.is_none()) else { return no("That workspace is gone.") };
        let running = |id: &str| SESSIONS.get(id).is_some_and(|x| x.spawned.load(Ordering::Relaxed) && !x.exited.load(Ordering::Relaxed));
        let side = format!("side-terminal-{}", ws.id);
        let mut terminals = vec![json!({ "id": side, "title": "Terminal", "provider": "shell", "side": true, "running": running(&side) })];
        for p in super::mobile::ordered(ws).into_iter().filter(|p| p.provider != "winfer" && !self.code_models.contains_key(&p.id)) {
            terminals.push(json!({ "id": p.id, "title": s.pane_label(p), "provider": p.provider, "side": false, "running": running(&p.id) }));
        }
        ok(json!({ "workspaceId": ws.id, "name": ws.name, "root": ws.root_dir, "terminals": terminals }))
    }

    fn phone_browser(&mut self, device: &str, req: &Value, reply: impl Fn(Value) + 'static, cx: &mut Context<Self>) {
        let op = str_of(req, "op");
        let ws = str_of(req, "workspaceId");
        if !self.state.read(cx).workspaces.iter().any(|w| w.id == ws && w.remote.is_none()) {
            return reply(no("That workspace is gone."));
        }
        let tab = str_of(req, "tab");
        match op.as_str() {
            "state" => {
                let retry = req["retry"] == true;
                let value = self.browser.update(cx, |b, cx| b.phone_state(&ws, retry, cx));
                reply(ok(value))
            }
            "install" => {
                self.browser.update(cx, |b, cx| b.phone_install(cx));
                reply(ok(Value::Null))
            }
            "select" | "watch" => {
                self.browser.update(cx, |b, cx| b.phone_select(&ws, &tab, cx));
                if op == "watch" {
                    watch(device, Feed::Browser { workspace: ws.clone(), tab: tab.clone() }, req["maxWidth"].as_u64().unwrap_or(800) as u32);
                }
                // Only the tab in front paints.
                tauri::async_runtime::spawn(async move {
                    let app = backend::app();
                    let _ = browser_backend::browser_front(app, &app.state::<BrowserManager>(), &ws, &tab).await;
                });
                reply(ok(Value::Null))
            }
            "unwatch" => {
                unwatch(device, |f| matches!(f, Feed::Browser { workspace, .. } if *workspace == ws));
                reply(ok(Value::Null))
            }
            "input" => {
                let (method, params) = (str_of(req, "method"), req["params"].clone());
                tauri::async_runtime::spawn(async move {
                    let _ = ops::input(ws, tab, method, params).await;
                });
                if !req["id"].is_null() {
                    reply(ok(Value::Null));
                }
            }
            "newTab" | "closeTab" | "navigate" | "history" => {
                let url = req["url"].as_str().map(str::trim).filter(|u| !u.is_empty()).map_or_else(|| "about:blank".to_string(), normalize_url);
                let delta = req["delta"].as_i64().unwrap_or(0);
                cx.spawn(async move |this, cx| {
                    let target = ws.clone();
                    let done: Result<Value, String> = tauri::async_runtime::spawn(async move {
                        match op.as_str() {
                            "newTab" => ops::new_tab(target, url).await.map(Value::String),
                            "closeTab" => ops::close_tab(target, tab).await.map(|_| Value::Null),
                            "navigate" => ops::navigate(target, tab, url).await.map(|_| Value::Null),
                            _ => ops::history(target, tab, delta).await.map(|_| Value::Null),
                        }
                    })
                    .await
                    .map_err(|e| e.to_string())
                    .and_then(|r| r);
                    // A new tab shows here too.
                    if let Ok(Value::String(id)) = &done {
                        let id = id.clone();
                        let _ = this.update(cx, |this, cx| this.browser.update(cx, |b, cx| b.phone_select(&ws, &id, cx)));
                    }
                    reply(match done {
                        Ok(v) => ok(v),
                        Err(e) => no(e),
                    });
                })
                .detach();
            }
            other => reply(no(format!("The browser can't {other} from the phone."))),
        }
    }

    fn phone_android(&mut self, device: &str, req: &Value, reply: impl Fn(Value) + 'static, cx: &mut Context<Self>) {
        let op = str_of(req, "op");
        let at = (req["x"].as_f64().unwrap_or(0.0) as f32, req["y"].as_f64().unwrap_or(0.0) as f32);
        let quiet = req["id"].is_null();
        let answer = |v: Value| {
            if !quiet {
                reply(v)
            }
        };
        match op.as_str() {
            "state" => answer(ok(self.android.read(cx).phone_state(cx))),
            "watch" => {
                watch(device, Feed::Android, req["maxWidth"].as_u64().unwrap_or(720) as u32);
                self.android.update(cx, |a, cx| a.set_phone(true, cx));
                answer(ok(self.android.read(cx).phone_state(cx)))
            }
            "unwatch" => {
                unwatch(device, |f| *f == Feed::Android);
                answer(ok(Value::Null))
            }
            "select" => {
                self.android.update(cx, |a, cx| a.phone_select(str_of(req, "serial"), cx));
                answer(ok(Value::Null))
            }
            "startAvd" => {
                self.android.update(cx, |a, cx| a.phone_start_avd(str_of(req, "name"), cx));
                answer(ok(Value::Null))
            }
            "touch" => {
                let touch = match req["kind"].as_str() {
                    Some("down") => Touch::Down,
                    Some("move") => Touch::Move,
                    _ => Touch::Up,
                };
                self.android.update(cx, |a, _| a.phone_touch(touch, at));
                answer(ok(Value::Null))
            }
            "scroll" => {
                self.android.read(cx).phone_scroll(at, req["v"].as_f64().unwrap_or(0.0) as f32);
                answer(ok(Value::Null))
            }
            "key" => {
                self.android.read(cx).phone_key(req["code"].as_u64().unwrap_or(0) as u32);
                answer(ok(Value::Null))
            }
            "text" => {
                self.android.read(cx).phone_text(req["text"].as_str().unwrap_or_default());
                answer(ok(Value::Null))
            }
            other => answer(no(format!("The Android mirror can't {other} from the phone."))),
        }
    }

    /// Folders the phone may read in: workspaces' and their agents' worktrees.
    fn phone_roots(&self, cx: &App) -> Vec<PathBuf> {
        let s = self.state.read(cx);
        s.workspaces
            .iter()
            .filter(|w| w.remote.is_none())
            .flat_map(|w| std::iter::once(w.root_dir.clone()).chain(w.panes.iter().filter_map(|p| p.worktree.as_ref().map(|t| t.path.clone()))))
            .filter(|r| !r.is_empty())
            .map(|r| PathBuf::from(expand(&r)))
            .collect()
    }

    fn phone_files(&mut self, req: &Value, reply: impl Fn(Value) + 'static, cx: &mut Context<Self>) {
        let op = str_of(req, "op");
        let (roots, docs) = (self.phone_roots(cx), self.state.read(cx).viewer.docs.clone());
        let path = expand(&str_of(req, "path"));
        let may = |p: &str| allowed(Path::new(p), &roots, &docs);
        match op.as_str() {
            "docs" => {
                let s = self.state.read(cx);
                let Some(ws) = s.workspaces.iter().find(|w| w.id == str_of(req, "workspaceId")) else { return reply(no("That workspace is gone.")) };
                let group = viewer_projects(&s.viewer.docs, &s.workspaces).into_iter().find(|g| g.id == ws.id);
                let docs = group.map(|g| g.docs).unwrap_or_default();
                let active = s.viewer.active.clone().filter(|a| docs.contains(a));
                reply(ok(json!({ "root": expand(&ws.root_dir), "docs": docs, "active": active })))
            }
            "list" => {
                let s = self.state.read(cx);
                let Some(ws) = s.workspaces.iter().find(|w| w.id == str_of(req, "workspaceId") && w.remote.is_none()) else { return reply(no("That workspace is gone.")) };
                let root = PathBuf::from(expand(&ws.root_dir));
                let dir = req["dir"].as_str().filter(|d| !d.is_empty()).map_or_else(|| root.clone(), |d| PathBuf::from(expand(d)));
                if !allowed(&dir, &roots, &[]) {
                    return reply(no("That folder isn't in a workspace."));
                }
                later(backend::background(move || list_dir(&dir).map(|entries| json!({ "root": root, "dir": dir, "entries": entries }))), reply, cx);
            }
            "read" => {
                if !may(&path) {
                    return reply(no("That file isn't in a workspace."));
                }
                later(backend::background(move || read_file(&path)), reply, cx);
            }
            "write" | "comment" => {
                if !may(&path) {
                    return reply(no("That file isn't in a workspace."));
                }
                let expected = req["expected"].as_u64();
                let text = if op == "write" {
                    Some(str_of(req, "text"))
                } else {
                    None
                };
                let (after, body) = (req["afterLine"].as_u64().unwrap_or(0) as usize, str_of(req, "body"));
                cx.spawn(async move |_, _| {
                    let target = path.clone();
                    let saved = tauri::async_runtime::spawn(async move {
                        let text = match text {
                            Some(t) => t,
                            None => {
                                if body.trim().is_empty() {
                                    return Err("Nothing to comment.".to_string());
                                }
                                let current = files::files_read_text(target.clone()).await?;
                                let current = serde_json::to_value(current).map_err(|e| e.to_string())?;
                                insert_comment(current["text"].as_str().unwrap_or_default(), after, &body, "user")
                            }
                        };
                        files::files_write_text(target, text, expected).await
                    })
                    .await
                    .map_err(|e| e.to_string())
                    .and_then(|r| r);
                    reply(match saved {
                        Ok(modified) => ok(json!({ "modified": modified })),
                        Err(e) if e == "changed" => no("The file changed on the desktop. Reload it first."),
                        Err(e) => no(e),
                    });
                })
                .detach();
            }
            "open" => {
                if !may(&path) {
                    return reply(no("That file isn't in a workspace."));
                }
                self.viewer.update(cx, |v, cx| v.open(path, None, true, cx));
                reply(ok(Value::Null))
            }
            "close" => {
                self.viewer.update(cx, |v, cx| v.close(&path, cx));
                reply(ok(Value::Null))
            }
            other => reply(no(format!("Files can't {other} from the phone."))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Feed, PHONES, Watch, allowed, frame_done, list_dir, next_quality, read_file};
    use std::path::Path;
    use std::time::Instant;

    #[test]
    fn quality_follows_frame_times() {
        assert_eq!(next_quality(55, 900), 47);
        assert_eq!(next_quality(32, 900), 30);
        assert_eq!(next_quality(55, 100), 58);
        assert_eq!(next_quality(69, 100), 70);
        assert_eq!(next_quality(55, 400), 55);
    }

    #[test]
    fn acks_only_count_for_sent_frames() {
        let device = "ack-test";
        watch_for_test(device);
        frame_done(device, 3);
        assert_eq!(PHONES.lock()[device].watch.as_ref().unwrap().acked, 0, "never sent");
        PHONES.lock().get_mut(device).unwrap().watch.as_mut().unwrap().sent = 2;
        frame_done(device, 2);
        assert_eq!(PHONES.lock()[device].watch.as_ref().unwrap().acked, 2);
        PHONES.lock().remove(device);
    }

    fn watch_for_test(device: &str) {
        let mut phones = PHONES.lock();
        let p = phones.entry(device.to_string()).or_default();
        p.seen = Some(Instant::now());
        p.watch = Some(Watch { feed: Feed::Android, serial: 0, max_width: 720, quality: 55, sent: 0, acked: 0, sent_at: Instant::now() });
    }

    #[test]
    fn files_stay_in_workspaces() {
        let root = std::env::temp_dir().join(format!("winfer-phone-files-{}", std::process::id()));
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::fs::write(root.join("sub/a.md"), "# hi\n").unwrap();
        std::fs::write(root.join("b.png"), [0u8; 8]).unwrap();
        let roots = vec![root.clone()];
        assert!(allowed(&root.join("sub/a.md"), &roots, &[]));
        assert!(allowed(&root.join("sub/../b.png"), &roots, &[]));
        assert!(!allowed(&root.join(".."), &roots, &[]));
        assert!(!allowed(Path::new("/etc/hosts"), &roots, &[]));
        assert!(allowed(Path::new("/etc/hosts"), &roots, &["/etc/hosts".into()]));
        let listed = list_dir(&root).unwrap();
        assert_eq!(listed[0]["name"], "sub");
        assert_eq!(listed[0]["dir"], true);
        let md = read_file(root.join("sub/a.md").to_str().unwrap()).unwrap();
        assert_eq!(md["kind"], "markdown");
        assert_eq!(md["text"], "# hi\n");
        let png = read_file(root.join("b.png").to_str().unwrap()).unwrap();
        assert_eq!(png["kind"], "image");
        assert_eq!(png["mime"], "image/png");
        std::fs::remove_dir_all(&root).unwrap();
    }
}
