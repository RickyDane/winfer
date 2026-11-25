//! What the core tells the app it runs in (telemetry and the like). The desktop app installs its
//! hooks at startup; without them (tests, `winferd`) nothing happens.

use std::sync::OnceLock;

pub trait Hooks: Send + Sync {
    /// A chat sent a new turn (not a steer).
    fn turn_sent(&self, _model: &str, _effort: &str, _ultracode: bool) {}
    /// A pane was added.
    fn pane_opened(&self, _provider: &str, _effort: Option<&str>) {}
    /// A pane got its own worktree.
    fn worktree_created(&self, _provider: &str) {}
    /// The backend set up its data dir (`backend::start`).
    fn started(&self, _data_dir: &std::path::Path) {}
    /// The app is quitting (`backend::shutdown`).
    fn stopping(&self) {}
}

static HOOKS: OnceLock<Box<dyn Hooks>> = OnceLock::new();

/// Installs the app's hooks (once).
pub fn install(hooks: Box<dyn Hooks>) {
    let _ = HOOKS.set(hooks);
}

pub fn get() -> Option<&'static dyn Hooks> {
    HOOKS.get().map(|h| h.as_ref())
}
