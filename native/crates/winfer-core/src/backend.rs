//! Starts the backend the way the Tauri app's `run()` does, and carries its events to the UI.

use std::sync::OnceLock;
use tauri::{AppHandle, Manager};
use winfer_backend::{android, browser, chatgpt_voice, diagnostics, env, harness, hooks, mobile, plugins, pty, realtime, store, transfer, usage_history};

/// What runs the backend: the desktop app or the headless `winferd`.
#[derive(Clone, Copy)]
pub struct Profile {
    /// Own folder and identifier: the native app runs beside the Tauri app without sharing its
    /// database, hooks or browser profiles; `winferd` beside both.
    pub identifier: &'static str,
    pub server: bool,
}

pub const DESKTOP: Profile = Profile { identifier: "dev.winfer.native", server: false };
pub const SERVER: Profile = Profile { identifier: "dev.winfer.server", server: true };

static PROFILE: OnceLock<Profile> = OnceLock::new();

/// Sets the profile, before anything else uses the backend (default: the desktop).
pub fn configure(profile: Profile) {
    let _ = PROFILE.set(profile);
    if profile.server {
        env::skip_interactive_rc();
    }
}

pub fn profile() -> Profile {
    *PROFILE.get_or_init(|| DESKTOP)
}

/// `WINFER_NATIVE_IDENTIFIER` overrides it, to run a second copy with its own data and lock.
pub fn identifier() -> String {
    std::env::var("WINFER_NATIVE_IDENTIFIER").ok().filter(|v| !v.is_empty()).unwrap_or_else(|| profile().identifier.into())
}

/// Where this profile keeps its data (the same folder `start` uses).
pub fn data_dir() -> std::path::PathBuf {
    dirs::data_dir().unwrap_or_else(std::env::temp_dir).join(identifier())
}

static APP: OnceLock<AppHandle> = OnceLock::new();

pub fn app() -> &'static AppHandle {
    APP.get().expect("backend started")
}

/// Runs a blocking backend call off the UI thread.
pub fn background<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> tauri::async_runtime::JoinHandle<T> {
    tauri::async_runtime::spawn_blocking(f)
}

/// Held for as long as this process runs; `None` if another winfer holds it.
pub fn acquire_instance_lock() -> Option<std::fs::File> {
    let dir = dirs::data_dir()?.join(identifier());
    std::fs::create_dir_all(&dir).ok()?;
    let file = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(dir.join("instance.lock")).ok()?;
    file.try_lock().ok()?;
    Some(file)
}

pub fn start() -> Result<(), String> {
    let app = AppHandle::new(&identifier());
    let data_dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    // A server has no keychain: its secrets (phones' pairing keys, the push key) go to a private
    // file in its data folder. Set before any thread starts.
    if profile().server && std::env::var_os("WINFER_VAULT_FILE").is_none() {
        // SAFETY: nothing else runs yet.
        unsafe { std::env::set_var("WINFER_VAULT_FILE", data_dir.join("vault.json")) };
    }
    diagnostics::install_panic_hook(data_dir.join("logs"));
    std::thread::spawn(|| {
        env::login_env();
    });
    app.manage(pty::PtyManager::default());
    app.manage(browser::BrowserManager::default());
    app.manage(android::AndroidManager::default());
    app.manage(realtime::LiveBrowser::default());
    app.manage(chatgpt_voice::ChatgptVoice::default());
    std::fs::create_dir_all(&data_dir).map_err(|e| e.to_string())?;
    diagnostics::init(&data_dir);
    if let Some(h) = crate::hooks::get() {
        h.started(&data_dir);
    }
    diagnostics::breadcrumb("app setup".to_string());
    if let Err(e) = transfer::apply_pending(&data_dir) {
        diagnostics::error("import", &format!("import failed: {e}"));
    }
    plugins::init(&data_dir);
    app.manage(store::Store::open(&data_dir.join("winfer.db")).map_err(|e| e.to_string())?);
    app.manage(usage_history::UsageHistory::open(&data_dir.join("usage.db")).map_err(|e| e.to_string())?);
    usage_history::start(&app);
    hooks::start(app.clone(), &data_dir).map_err(|e| e.to_string())?;
    mobile::start(app.clone(), &data_dir);
    crate::connect::start(app.clone(), &data_dir);
    browser::kill_orphans(&app);
    // Development aid: where a test client can reach the hook server.
    #[cfg(debug_assertions)]
    if let (Ok(path), Some(server)) = (std::env::var("WINFER_NATIVE_HOOK_FILE"), hooks::server()) {
        let _ = std::fs::write(path, format!("{} {}", server.addr, server.token));
    }
    APP.set(app).map_err(|_| "backend already started".to_string())
}

pub fn shutdown() {
    let app = app();
    diagnostics::mark_clean_exit();
    app.state::<realtime::LiveBrowser>().shutdown();
    app.state::<browser::BrowserManager>().shutdown();
    app.state::<android::AndroidManager>().shutdown();
    harness::bg::stop_all();
    if let Some(h) = crate::hooks::get() {
        h.stopping();
    }
}
