//! Agent status (port of `src/features/status/agentStatus.ts`): hook events, terminal
//! notifications and, for agents without hooks, output activity.

//!
//! The state lives where the caller keeps it (a GPUI entity in the app, a mutex in `winferd`):
//! [`Cell`] reads and changes it.

use crate::backend;
use crate::model::{AgentStatus, AppState};
use crate::terminal::SESSIONS;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};
use winfer_backend::notify;

const IDLE_AFTER: Duration = Duration::from_millis(4000);

/// Hook agents marked "working" only because the user pressed Enter, and when.
static GUESSED: LazyLock<Mutex<HashMap<String, Instant>>> = LazyLock::new(Default::default);

/// Access to the app state: read it, or change it (and repaint what shows it).
pub trait Cell {
    fn get(&self) -> &AppState;
    fn set(&mut self, f: &mut dyn FnMut(&mut AppState));
}

/// The state held in a plain mutex guard (`winferd`).
impl Cell for AppState {
    fn get(&self) -> &AppState {
        self
    }
    fn set(&mut self, f: &mut dyn FnMut(&mut AppState)) {
        f(self)
    }
}

/// Whether the user looks at this machine's window (never on a server).
pub fn window_focused() -> bool {
    crate::hooks::get().is_some_and(|h| h.window_focused())
}

/// `true` if this report finished the agent's work (for the review inbox).
pub fn report(state: &mut impl Cell, id: &str, status: AgentStatus, message: Option<String>) -> bool {
    let current = state.get().pane(id).cloned();
    let Some(pane) = current.filter(|p| p.status != status) else { return false };
    if status != AgentStatus::Working {
        GUESSED.lock().unwrap().remove(id);
    }
    let focused = window_focused();
    state.set(&mut |s| {
        match &message {
            Some(m) => s.status_messages.insert(id.to_string(), m.clone()),
            None => s.status_messages.remove(id),
        };
        s.set_pane_status(id, status, focused);
        if status == AgentStatus::Done {
            s.mark_finished(id);
        }
    });
    if matches!(status, AgentStatus::NeedsInput | AgentStatus::Done | AgentStatus::Error) {
        notify_user(state, id, status, message);
    }
    let hooks = state.get().has_hooks(&pane);
    status == AgentStatus::Done || (status == AgentStatus::Idle && pane.status == AgentStatus::Working && !hooks)
}

fn notify_user(state: &mut impl Cell, id: &str, status: AgentStatus, message: Option<String>) {
    // A server has no screen: the machines and phones it's paired with notify from its status pushes.
    if backend::profile().server {
        return;
    }
    let s = state.get();
    let Some(pane) = s.pane(id) else { return };
    if window_focused() && s.focused_pane_id.as_deref() == Some(id) {
        return;
    }
    if status == AgentStatus::NeedsInput && s.auto_approve.contains(&pane.provider) {
        return;
    }
    let workspace = s.workspaces.iter().find(|w| w.id == pane.workspace_id).map_or(String::new(), |w| w.name.clone());
    let name = s.provider_name(&pane.provider);
    let id = id.to_string();
    let request = status == AgentStatus::NeedsInput;
    backend::background(move || {
        // A notification with the agent's actual question and buttons to answer it. The hook
        // fires as the prompt appears; the terminal gets a moment to draw it.
        let (title, body, kind) = if request {
            std::thread::sleep(Duration::from_millis(400));
            let screen = SESSIONS.get(&id).map(|s| s.read_text(40)).unwrap_or_default();
            let r = crate::request::describe_request(&screen, message.as_deref());
            let asks = if r.kind == "approval" { "asks for permission" } else { "is waiting for you" };
            let place = if workspace.is_empty() { String::new() } else { format!(" · {workspace}") };
            (format!("{name} {asks}{place}"), r.text, r.kind)
        } else {
            let title = if status == AgentStatus::Error { format!("{name} exited with an error") } else { format!("{name} finished") };
            (title, message.map_or(workspace.clone(), |m| format!("{workspace}: {m}")), "info")
        };
        let notification = json!({ "paneId": id, "title": title, "body": body, "kind": kind });
        if let Ok(n) = serde_json::from_value(notification) {
            let _ = notify::notify_action(backend::app().clone(), n);
        }
    });
}

/// A terminal notification (OSC 9/777, bell): only new questions count for hook agents.
pub fn notice(state: &mut impl Cell, id: &str, message: Option<String>) {
    let s = state.get();
    let Some(pane) = s.pane(id) else { return };
    if pane.provider == "shell" || (pane.status == AgentStatus::Done && s.has_hooks(pane)) {
        return;
    }
    report(state, id, AgentStatus::NeedsInput, message);
}

/// The user submitted a line to an agent: it's working now, and that line is its task.
pub fn submitted(state: &mut impl Cell, id: &str, line: &str) {
    let s = state.get();
    let Some(pane) = s.pane(id) else { return };
    if pane.provider == "shell" {
        return;
    }
    if s.has_hooks(pane) && pane.status != AgentStatus::Working {
        GUESSED.lock().unwrap().insert(id.to_string(), Instant::now());
    }
    set_task(state, id, line);
    report(state, id, AgentStatus::Working, None);
}

pub fn set_task(state: &mut impl Cell, id: &str, line: &str) {
    let line = line.trim();
    if line.is_empty() || line.starts_with('/') || crate::hooks::get().is_some_and(|h| h.orchestrated(id)) {
        return;
    }
    state.set(&mut |s| {
        s.set_task(id, line);
    });
}

/// Output arrived from an agent without hooks.
pub fn output(state: &mut impl Cell, id: &str) {
    let s = state.get();
    if s.pane(id).is_some_and(|p| p.provider != "shell" && !s.has_hooks(p) && p.status != AgentStatus::Working) {
        let focused = window_focused();
        let id = id.to_string();
        state.set(&mut |s| {
            s.set_pane_status(&id, AgentStatus::Working, focused);
        });
    }
}

/// An `agent://status` event from an agent's hooks.
pub fn hook(state: &mut impl Cell, payload: &Value) -> bool {
    let Some(id) = payload["paneId"].as_str() else { return false };
    GUESSED.lock().unwrap().remove(id);
    if let Some(session) = payload["session"].as_str() {
        let (id, session) = (id.to_string(), session.to_string());
        state.set(&mut |s| {
            s.set_pane_session(&id, &session);
        });
    }
    let Some(status) = payload["status"].as_str().and_then(AgentStatus::parse) else { return false };
    if status == AgentStatus::Done {
        let reply = payload["reply"].as_str().filter(|r| !r.is_empty()).map(str::to_string);
        state.set(&mut |s| {
            match reply.clone() {
                Some(r) => s.replies.insert(id.to_string(), r),
                None => s.replies.remove(id),
            };
        });
    }
    report(state, id, status, payload["message"].as_str().map(str::to_string))
}

/// Runs each second: agents without hooks that went quiet are idle, and an input-guessed
/// "working" no hook confirmed is taken back.
pub fn tick(state: &mut impl Cell) -> Vec<String> {
    let quiet = |id: &str| SESSIONS.get(id).is_none_or(|s| s.output_age_ms() > IDLE_AFTER.as_millis() as u64);
    let guessed: Vec<(String, Instant)> = GUESSED.lock().unwrap().iter().map(|(k, v)| (k.clone(), *v)).collect();
    for (id, at) in guessed {
        let working = state.get().pane(&id).is_some_and(|p| p.status == AgentStatus::Working);
        if !working {
            GUESSED.lock().unwrap().remove(&id);
        } else if at.elapsed() > IDLE_AFTER && quiet(&id) {
            GUESSED.lock().unwrap().remove(&id);
            let focused = window_focused();
            state.set(&mut |s| {
                s.set_pane_status(&id, AgentStatus::Idle, focused);
            });
        }
    }
    let s = state.get();
    let idle: Vec<String> = s.all_panes().filter(|p| p.status == AgentStatus::Working && p.provider != "shell" && !s.has_hooks(p) && quiet(&p.id)).map(|p| p.id.clone()).collect();
    let mut finished = Vec::new();
    for id in idle {
        if report(state, &id, AgentStatus::Idle, None) {
            finished.push(id);
        }
    }
    finished
}
