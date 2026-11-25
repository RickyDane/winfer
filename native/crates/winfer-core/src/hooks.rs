//! What the core tells the app it runs in (telemetry and the like). The desktop app installs its
//! hooks at startup; without them (tests, `winferd`) nothing happens.

use std::sync::OnceLock;

pub trait Hooks: Send + Sync {
    /// A chat sent a new turn (not a steer).
    fn turn_sent(&self, _model: &str, _effort: &str, _ultracode: bool) {}
    /// A pane was added.
    fn pane_opened(&self, _provider: &str, _effort: Option<&str>) {}
}

static HOOKS: OnceLock<Box<dyn Hooks>> = OnceLock::new();

/// Installs the app's hooks (once).
pub fn install(hooks: Box<dyn Hooks>) {
    let _ = HOOKS.set(hooks);
}

pub fn get() -> Option<&'static dyn Hooks> {
    HOOKS.get().map(|h| h.as_ref())
}
