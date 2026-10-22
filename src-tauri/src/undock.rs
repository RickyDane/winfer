//! The main window, built here instead of from tauri.conf.json so it can open the undocked
//! browser: the frontend calls `window.open(UNDOCK_URL)` and renders the browser panel into the
//! returned window with a React portal. The new window shares the main webview's configuration
//! (same web process, same origin), so it is one app with two windows: no state to sync.
//!
//! Sharing the configuration means wry's `ipc` message handler is already registered on it;
//! wry catches the Objective-C exception that re-adding it throws, which needs the release
//! profile to unwind (Cargo.toml) — with `panic = "abort"` that exception kills the app.
//!
//! WebKit's `window.close()` doesn't close a native window, so the frontend closes and focuses
//! it through the commands here. Its place and size are kept here too (WebKit's `screenX` and
//! `window.open` coordinates aren't reliable), so it reopens where it was, say on another screen.

use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager, WebviewWindow, WebviewWindowBuilder};

/// Undocked browser windows are labelled `browser-<n>`: a fresh label each time, so an old one
/// still closing (or left over from a reload of the main window) never blocks a new one.
const PREFIX: &str = "browser-";
/// The URL the frontend opens for it; any other `window.open` is refused, as before.
#[cfg(desktop)]
const UNDOCK_URL: &str = "about:blank#winfer-browser";
/// The label of the window the frontend is using right now.
static CURRENT: Mutex<Option<String>> = Mutex::new(None);

fn undocked(app: &AppHandle) -> Vec<WebviewWindow> {
    app.webview_windows().into_iter().filter(|(label, _)| label.starts_with(PREFIX)).map(|(_, w)| w).collect()
}

/// Builds the main window from the first window in tauri.conf.json (which has `create: false`).
pub fn create_main_window(app: &AppHandle) -> tauri::Result<WebviewWindow> {
    let config = app.config().app.windows.first().cloned().expect("tauri.conf.json defines the main window");
    #[allow(unused_mut)]
    let mut builder = WebviewWindowBuilder::from_config(app, &config)?;
    #[cfg(desktop)]
    {
        let handle = app.clone();
        builder = builder.on_new_window(move |url, features| {
            use tauri::webview::NewWindowResponse;
            if url.as_str() != UNDOCK_URL {
                return NewWindowResponse::Deny;
            }
            // The frontend only asks when it has no window: any still open is stale, and its
            // closing mustn't read as the new one closing.
            *CURRENT.lock().unwrap() = None;
            for old in undocked(&handle) {
                let _ = old.destroy();
            }
            match open_browser_window(&handle, url, features) {
                Ok(window) => NewWindowResponse::Create { window },
                Err(err) => {
                    eprintln!("undocked browser window failed: {err}");
                    NewWindowResponse::Deny
                }
            }
        });
    }
    let main = builder.build()?;
    // The undocked browser is part of the main window: it goes when the main window goes.
    let handle = app.clone();
    main.on_window_event(move |event| {
        if let tauri::WindowEvent::Destroyed = event {
            for window in undocked(&handle) {
                let _ = window.destroy();
            }
        }
    });
    Ok(main)
}

#[cfg(desktop)]
fn open_browser_window(app: &AppHandle, url: tauri::Url, features: tauri::webview::NewWindowFeatures) -> tauri::Result<WebviewWindow> {
    use std::sync::atomic::{AtomicU32, Ordering};
    static NEXT: AtomicU32 = AtomicU32::new(1);
    let label = format!("{PREFIX}{}", NEXT.fetch_add(1, Ordering::Relaxed));
    let saved = bounds::load(app);
    let mut builder = WebviewWindowBuilder::new(app, &label, tauri::WebviewUrl::External(url))
        // Shares the opener's webview configuration, which makes the window scriptable from it.
        .window_features(features)
        .title("winfer browser")
        .min_inner_size(360.0, 300.0)
        .background_color(tauri::window::Color(0x0b, 0x0b, 0x0d, 0xff));
    builder = match saved {
        Some(b) => builder.position(b.x, b.y).inner_size(b.width, b.height),
        None => builder.inner_size(1100.0, 800.0).center(),
    };
    let window = builder.build()?;
    // Saved on a screen that's gone (monitor unplugged): bring it back into view.
    if matches!(window.current_monitor(), Ok(None)) {
        let _ = window.center();
    }
    *CURRENT.lock().unwrap() = Some(label.clone());
    let tracked = window.clone();
    let handle = app.clone();
    window.on_window_event(move |event| match event {
        tauri::WindowEvent::Moved(_) | tauri::WindowEvent::Resized(_) => bounds::remember(&tracked),
        tauri::WindowEvent::Destroyed => {
            bounds::save(&handle);
            let mut current = CURRENT.lock().unwrap();
            if current.as_deref() == Some(label.as_str()) {
                *current = None;
                // Closed by its own close button (or docked): the panel goes back to the side.
                let _ = handle.emit("browser://undocked-closed", ());
            }
        }
        _ => {}
    });
    Ok(window)
}

/// Closes the undocked browser window(s): docking, or leftovers after the main window reloaded.
#[tauri::command]
pub fn browser_window_close(app: AppHandle) {
    *CURRENT.lock().unwrap() = None;
    for window in undocked(&app) {
        let _ = window.destroy();
    }
}

/// Brings the undocked browser window to the front.
#[tauri::command]
pub fn browser_window_focus(app: AppHandle) {
    for window in undocked(&app) {
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

/// Matches the window's title bar to the app theme ("light" or "dark").
#[tauri::command]
pub fn browser_window_theme(app: AppHandle, mode: String) {
    let theme = match mode.as_str() {
        "light" => tauri::Theme::Light,
        _ => tauri::Theme::Dark,
    };
    for window in undocked(&app) {
        let _ = window.set_theme(Some(theme));
    }
}

#[cfg(desktop)]
mod bounds {
    use serde::{Deserialize, Serialize};
    use std::sync::Mutex;
    use tauri::{AppHandle, Manager, WebviewWindow};

    /// Logical (point) coordinates: one space across screens with different scales on macOS.
    #[derive(Clone, Copy, Serialize, Deserialize)]
    pub struct Bounds {
        pub x: f64,
        pub y: f64,
        pub width: f64,
        pub height: f64,
    }

    static LATEST: Mutex<Option<Bounds>> = Mutex::new(None);

    fn path(app: &AppHandle) -> Option<std::path::PathBuf> {
        app.path().app_data_dir().ok().map(|d| d.join("browser-window.json"))
    }

    pub fn load(app: &AppHandle) -> Option<Bounds> {
        let b: Bounds = serde_json::from_slice(&std::fs::read(path(app)?).ok()?).ok()?;
        (b.width >= 100.0 && b.height >= 100.0 && b.x.is_finite() && b.y.is_finite()).then_some(b)
    }

    pub fn remember(window: &WebviewWindow) {
        let (Ok(pos), Ok(size), Ok(scale)) = (window.outer_position(), window.inner_size(), window.scale_factor()) else {
            return;
        };
        // Minimized windows report nonsense; keep the last real place.
        if window.is_minimized().unwrap_or(false) || size.width == 0 {
            return;
        }
        let (pos, size) = (pos.to_logical::<f64>(scale), size.to_logical::<f64>(scale));
        *LATEST.lock().unwrap() = Some(Bounds { x: pos.x, y: pos.y, width: size.width, height: size.height });
    }

    pub fn save(app: &AppHandle) {
        let Some(b) = LATEST.lock().unwrap().take() else { return };
        if let (Some(path), Ok(json)) = (path(app), serde_json::to_vec(&b)) {
            let _ = std::fs::write(path, json);
        }
    }
}
