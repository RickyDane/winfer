//! Rendered HTML files and videos: a wry webview placed over the viewer's body. The page and the
//! files next to it (styles, scripts, images, media) come from disk through the `winferfile`
//! protocol, limited to the page's own folder; web links open in the system browser.

use super::{Content, ViewerPanel, expand, extension, video};
use crate::theme::Colors;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use parking_lot::Mutex;
use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use wry::{NewWindowResponse, Rect, WebView, WebViewBuilder, dpi};

const SCHEME: &str = "winferfile";

/// The main window's raw handle, so the webview can be built while nothing borrows the window.
struct ParentWindow(wry::raw_window_handle::RawWindowHandle);

impl wry::raw_window_handle::HasWindowHandle for ParentWindow {
    fn window_handle(&self) -> Result<wry::raw_window_handle::WindowHandle<'_>, wry::raw_window_handle::HandleError> {
        // SAFETY: taken from the live window right before the synchronous build (see the plugin host).
        Ok(unsafe { wry::raw_window_handle::WindowHandle::borrow_raw(self.0) })
    }
}

/// Sends clicks on links to other sites to the host instead of navigating the viewer away.
const LINKS_JS: &str = r#"document.addEventListener('click', function (e) {
  if (e.defaultPrevented || e.button !== 0 || e.metaKey || e.ctrlKey || e.shiftKey || e.altKey) return;
  var a = e.target && e.target.closest ? e.target.closest('a[href]') : null;
  if (!a) return;
  var u;
  try { u = new URL(a.href, location.href); } catch (_) { return; }
  if (u.protocol === 'mailto:' || ((u.protocol === 'http:' || u.protocol === 'https:') && u.origin !== location.origin)) {
    e.preventDefault();
    window.ipc.postMessage(u.href);
  }
});"#;

#[derive(Default)]
pub(super) struct HtmlView {
    webview: Option<WebView>,
    starting: bool,
    failed: Option<String>,
    /// The folder the protocol serves from: the shown page's.
    root: Arc<Mutex<PathBuf>>,
    /// The page and modification time the webview shows.
    pub(super) loaded: Option<(String, u64)>,
    bounds: Bounds<Pixels>,
    /// The bounds the webview was last given.
    placed: Option<Bounds<Pixels>>,
    /// Whether the viewer is on screen with nothing drawn over it (set by the root view).
    allowed: bool,
    visible: bool,
}

pub(super) fn is_html(path: &str) -> bool {
    matches!(extension(path).as_str(), "html" | "htm" | "xhtml")
}

/// The protocol's origin (wry maps custom protocols to `http://<name>.localhost` on Windows).
fn origin() -> String {
    if cfg!(any(target_os = "windows", target_os = "android")) { format!("http://{SCHEME}.localhost") } else { format!("{SCHEME}://localhost") }
}

/// A page's URL; a video's is that of its player page.
fn page_url(path: &str) -> Option<String> {
    let query = if video::is_video(path) { format!("?{}", video::PLAYER) } else { String::new() };
    url::Url::from_file_path(expand(path)).ok().map(|u| format!("{}{}{query}", origin(), u.path()))
}

/// The file a protocol URL path names (percent-decoded, drive letters kept on Windows).
fn file_for(uri_path: &str) -> Option<PathBuf> {
    url::Url::parse(&format!("file://{uri_path}")).ok()?.to_file_path().ok()
}

fn mime_for(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref() {
        Some("html" | "htm") => "text/html; charset=utf-8",
        Some("xhtml") => "application/xhtml+xml",
        Some("css") => "text/css; charset=utf-8",
        Some("js" | "mjs" | "cjs") => "text/javascript; charset=utf-8",
        Some("json" | "map") => "application/json",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("avif") => "image/avif",
        Some("ico") => "image/x-icon",
        Some("bmp") => "image/bmp",
        Some("woff") => "font/woff",
        Some("woff2") => "font/woff2",
        Some("ttf") => "font/ttf",
        Some("otf") => "font/otf",
        Some("mp4" | "m4v") => "video/mp4",
        Some("mov") => "video/quicktime",
        Some("webm") => "video/webm",
        Some("mp3") => "audio/mpeg",
        Some("wav") => "audio/wav",
        Some("ogg") => "audio/ogg",
        Some("pdf") => "application/pdf",
        Some("wasm") => "application/wasm",
        Some("txt" | "md" | "csv") => "text/plain; charset=utf-8",
        Some("xml") => "application/xml",
        _ => "application/octet-stream",
    }
}

fn respond(mime: &str, status: u16, body: Vec<u8>) -> wry::http::Response<Cow<'static, [u8]>> {
    wry::http::Response::builder().status(status).header("Content-Type", mime).header("Cache-Control", "no-store").body(Cow::Owned(body)).unwrap()
}

/// Serves a file under `root`, or the part of it a `Range` header asks for; anything outside it
/// (`..`, symlinks out) is refused.
fn serve(root: &Path, uri_path: &str, query: Option<&str>, range: Option<&str>) -> wry::http::Response<Cow<'static, [u8]>> {
    let Some(file) = file_for(uri_path).and_then(|f| f.canonicalize().ok()) else {
        return respond("text/plain", 404, b"Not found".to_vec());
    };
    if !root.canonicalize().is_ok_and(|r| file.starts_with(r)) {
        return respond("text/plain", 403, b"Outside the page's folder".to_vec());
    }
    if query == Some(video::PLAYER) && video::is_video(&file.to_string_lossy()) {
        return respond("text/html; charset=utf-8", 200, video::player_page(uri_path).into_bytes());
    }
    let served = match range {
        Some(range) => video::serve_range(&file, mime_for(&file), range),
        None => std::fs::read(&file).map(|bytes| respond(mime_for(&file), 200, bytes)),
    };
    served.unwrap_or_else(|e| respond("text/plain", 404, e.to_string().into_bytes()))
}

impl ViewerPanel {
    /// The page the webview should show: a video, or an HTML file rendered (not its source or
    /// editor).
    fn wanted_html(&self) -> Option<(String, u64)> {
        let path = self.shown.as_ref()?;
        match &self.content {
            Content::Video { modified } if video::is_video(path) => Some((path.clone(), *modified)),
            Content::Text { modified, .. } if is_html(path) && !self.editing && !self.source && !self.diff.on => Some((path.clone(), *modified)),
            _ => None,
        }
    }

    /// Called by the root view every frame: whether the viewer is on screen and uncovered.
    pub fn set_html_allowed(&mut self, allowed: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.html.allowed = allowed;
        self.sync_html(window, cx);
    }

    /// Gives keyboard focus back to GPUI after a click in the page (see `Root::reclaim_keyboard`).
    pub fn reclaim_keyboard(&self) {
        if let Some(webview) = &self.html.webview {
            let _ = webview.focus_parent();
        }
    }

    pub(super) fn sync_html(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let want = self.wanted_html();
        if want.is_some() && self.html.webview.is_none() && !self.html.starting && self.html.failed.is_none() {
            self.start_html(window, cx);
        }
        if let (Some(webview), Some((path, modified))) = (&self.html.webview, &want) {
            if self.html.loaded.as_ref() != Some(&(path.clone(), *modified)) {
                let same_page = self.html.loaded.as_ref().is_some_and(|(p, _)| p == path);
                *self.html.root.lock() = PathBuf::from(expand(path)).parent().map(Path::to_path_buf).unwrap_or_default();
                // Reloading the same page keeps its scroll position.
                let _ = if same_page { webview.reload() } else { page_url(path).map_or(Ok(()), |url| webview.load_url(&url)) };
                self.html.loaded = Some((path.clone(), *modified));
            }
        }
        self.apply_html();
    }

    /// Places the webview over the viewer's body, or hides it.
    fn apply_html(&mut self) {
        let Some(webview) = &self.html.webview else { return };
        let b = self.html.bounds;
        let show = self.html.allowed && self.wanted_html().is_some() && b.size.width > px(1.0) && b.size.height > px(1.0);
        // Only when they change: this runs every frame, and moving a video that plays full screen
        // would pull it back to the panel's size.
        if show && self.html.placed != Some(b) {
            self.html.placed = Some(b);
            let _ = webview.set_bounds(Rect {
                position: dpi::LogicalPosition::new(f64::from(b.origin.x / px(1.0)), f64::from(b.origin.y / px(1.0))).into(),
                size: dpi::LogicalSize::new(f64::from(b.size.width / px(1.0)), f64::from(b.size.height / px(1.0))).into(),
            });
        }
        if self.html.visible != show {
            if !show {
                let _ = webview.evaluate_script(video::PAUSE_JS);
                // Hiding the webview while it's first responder would leave keys going nowhere.
                let _ = webview.focus_parent();
            }
            let _ = webview.set_visible(show);
            self.html.visible = show;
        }
    }

    /// Builds the webview from a task, as the plugin host does (WebView2 pumps messages while building).
    fn start_html(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.html.starting = true;
        let root = self.html.root.clone();
        cx.spawn_in(window, async move |this, cx| {
            let Ok(parent) = cx.update(|window, _| wry::raw_window_handle::HasWindowHandle::window_handle(window).map(|h| ParentWindow(h.as_raw()))) else {
                return;
            };
            let (tx, rx) = async_channel::unbounded::<String>();
            let popup = tx.clone();
            let built = parent.map_err(|err| err.to_string()).and_then(|parent| {
                WebViewBuilder::new()
                    .with_custom_protocol(SCHEME.into(), move |_, request| {
                        let root = root.lock().clone();
                        let range = request.headers().get("range").and_then(|v| v.to_str().ok());
                        serve(&root, request.uri().path(), request.uri().query(), range)
                    })
                    .with_ipc_handler(move |request| {
                        let _ = tx.try_send(request.into_body());
                    })
                    .with_new_window_req_handler(move |url, _| {
                        let _ = popup.try_send(url);
                        NewWindowResponse::Deny
                    })
                    .with_initialization_script(LINKS_JS)
                    .with_visible(false)
                    .with_focused(false)
                    .with_devtools(cfg!(debug_assertions))
                    .with_bounds(Rect { position: dpi::LogicalPosition::new(0.0, 0.0).into(), size: dpi::LogicalSize::new(1.0, 1.0).into() })
                    .build_as_child(&parent)
                    .map_err(|err| err.to_string())
            });
            let ok = this.update(cx, |this, cx| {
                this.html.starting = false;
                match built {
                    Ok(webview) => this.html.webview = Some(webview),
                    Err(err) => this.html.failed = Some(format!("Can't show HTML pages or videos: {err}")),
                }
                this.html.visible = false;
                this.html.placed = None;
                this.html.loaded = None;
                cx.notify();
                this.html.failed.is_none()
            });
            if !matches!(ok, Ok(true)) {
                return;
            }
            // Links to other sites (and popups) go to the system browser; local pages open as tabs.
            while let Ok(url) = rx.recv().await {
                let local = url.strip_prefix(&origin()).and_then(|p| file_for(p.split(['?', '#']).next().unwrap_or(p)));
                let done = this.update(cx, |this, cx| match local {
                    Some(file) => this.open(file.display().to_string(), None, false, cx),
                    None if url.starts_with("http://") || url.starts_with("https://") || url.starts_with("mailto:") => cx.open_url(&url),
                    None => {}
                });
                if done.is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    /// The viewer's body while a page shows: a placeholder the webview is laid over.
    pub(super) fn render_html(&mut self, c: Colors, cx: &mut Context<Self>) -> AnyElement {
        let measure = cx.entity();
        div().relative().size_full().bg(c.panel)
            .child(
                canvas(
                    move |bounds, _, cx| {
                        measure.update(cx, |this, _| {
                            if this.html.bounds != bounds {
                                this.html.bounds = bounds;
                                this.apply_html();
                            }
                        })
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            )
            .when_some(self.html.failed.clone(), |d, err| d.p_4().text_xs().text_color(c.danger).child(err))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{file_for, is_html, page_url, serve};

    #[test]
    fn recognizes_html_files() {
        assert!(is_html("/a/index.html"));
        assert!(is_html("/a/PAGE.HTM"));
        assert!(!is_html("/a/notes.md"));
    }

    #[cfg(unix)]
    #[test]
    fn page_urls_round_trip() {
        let url = page_url("/tmp/my site/index.html").unwrap();
        assert_eq!(url, "winferfile://localhost/tmp/my%20site/index.html");
        let path = url.strip_prefix("winferfile://localhost").unwrap();
        assert_eq!(file_for(path).unwrap().display().to_string(), "/tmp/my site/index.html");
    }

    #[test]
    fn serves_only_the_pages_folder() {
        let dir = std::env::temp_dir().join(format!("winfer-html-{}", std::process::id()));
        let site = dir.join("site");
        std::fs::create_dir_all(&site).unwrap();
        std::fs::write(site.join("style.css"), "body{}").unwrap();
        std::fs::write(dir.join("secret.txt"), "no").unwrap();
        let path_of = |p: &std::path::Path| url::Url::from_file_path(p).unwrap().path().to_string();
        let get = |uri_path: &str| serve(&site, uri_path, None, None);
        let ok = get(&path_of(&site.join("style.css")));
        assert_eq!(ok.status(), 200);
        assert_eq!(ok.headers()["Content-Type"], "text/css; charset=utf-8");
        assert_eq!(get(&path_of(&dir.join("secret.txt"))).status(), 403);
        assert_eq!(get(&format!("{}/../secret.txt", path_of(&site))).status(), 403);
        assert_eq!(serve(&site, &path_of(&dir.join("secret.txt")), None, Some("bytes=0-1")).status(), 403);
        std::fs::write(site.join("demo.mp4"), "video").unwrap();
        let video = path_of(&site.join("demo.mp4"));
        assert_eq!(serve(&site, &video, Some("player"), None).headers()["Content-Type"], "text/html; charset=utf-8");
        assert_eq!(get(&video).headers()["Content-Type"], "video/mp4");
        assert_eq!(serve(&site, &video, None, Some("bytes=0-1")).status(), 206);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
