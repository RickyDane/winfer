//! Agent status on the app's GPUI state: the logic is `winfer_core::status`.

use crate::model::{AgentStatus, AppState};
use gpui_kit::{App, Entity};
use serde_json::Value;
use winfer_core::status as core;

pub use core::window_focused;

struct Gpui<'a> {
    state: &'a Entity<AppState>,
    cx: &'a mut App,
}

impl core::Cell for Gpui<'_> {
    fn get(&self) -> &AppState {
        self.state.read(self.cx)
    }
    fn set(&mut self, f: &mut dyn FnMut(&mut AppState)) {
        self.state.update(self.cx, |s, cx| {
            f(s);
            cx.notify();
        });
    }
}

/// `true` if this report finished the agent's work (for the review inbox).
pub fn report(state: &Entity<AppState>, id: &str, status: AgentStatus, message: Option<String>, cx: &mut App) -> bool {
    core::report(&mut Gpui { state, cx }, id, status, message)
}

pub fn notice(state: &Entity<AppState>, id: &str, message: Option<String>, cx: &mut App) {
    core::notice(&mut Gpui { state, cx }, id, message)
}

pub fn submitted(state: &Entity<AppState>, id: &str, line: &str, cx: &mut App) {
    core::submitted(&mut Gpui { state, cx }, id, line)
}

pub fn set_task(state: &Entity<AppState>, id: &str, line: &str, cx: &mut App) {
    core::set_task(&mut Gpui { state, cx }, id, line)
}

pub fn output(state: &Entity<AppState>, id: &str, cx: &mut App) {
    core::output(&mut Gpui { state, cx }, id)
}

pub fn hook(state: &Entity<AppState>, payload: &Value, cx: &mut App) -> bool {
    core::hook(&mut Gpui { state, cx }, payload)
}

pub fn tick(state: &Entity<AppState>, cx: &mut App) -> Vec<String> {
    core::tick(&mut Gpui { state, cx })
}
