//! The main window, built here instead of from tauri.conf.json so it can open the undocked
//! browser: the frontend calls `window.open(UNDOCK_URL)` and renders the browser panel into the
//! returned window with a React portal. The new window shares the main webview's configuration
//! (same web process, same origin), so it is one app with two windows: no state to sync.
//!
//! The undocked window's place and size are kept here (WebKit's `screenX` and `window.open`
//! coordinates aren't reliable), so it reopens where it was, say on the second screen.

use tauri::{AppHandle, Manager, WebviewWindow, WebviewWindowBuilder};

/// The undocked browser window's label.
pub const LABEL: &str = "browser";
/// The URL the frontend opens for it; any other `window.open` is refused, as before.
#[cfg(desktop)]
const UNDOCK_URL: &str = "about:blank#winfer-browser";

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
            if let Some(existing) = handle.get_webview_window(LABEL) {
                let _ = existing.set_focus();
                return NewWindowResponse::Deny;
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
            if let Some(browser) = handle.get_webview_window(LABEL) {
                let _ = browser.close();
            }
        }
    });
    Ok(main)
}

#[cfg(desktop)]
fn open_browser_window(app: &AppHandle, url: tauri::Url, features: tauri::webview::NewWindowFeatures) -> tauri::Result<WebviewWindow> {
    let saved = bounds::load(app);
    let mut builder = WebviewWindowBuilder::new(app, LABEL, tauri::WebviewUrl::External(url))
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
    let tracked = window.clone();
    let handle = app.clone();
    window.on_window_event(move |event| match event {
        tauri::WindowEvent::Moved(_) | tauri::WindowEvent::Resized(_) => bounds::remember(&tracked),
        tauri::WindowEvent::Destroyed => bounds::save(&handle),
        _ => {}
    });
    Ok(window)
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
