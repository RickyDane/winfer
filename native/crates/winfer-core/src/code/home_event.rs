//! Home's app events ("[winfer] Task 7 · api/2 (Claude Code) finished.") as typed values.
//!
//! Home builds every event it sends an orchestrator with these constructors, so the text the
//! orchestrator reads (and the transcript keeps) is produced in one place and `parse` is its exact
//! inverse: a restored transcript, or one from before events were typed, shows the same rows. The
//! orchestrator always gets the full raw text; only the chat's rendering differs.

use super::bg_note::{self, BgNote};

pub const PREFIX: &str = "[winfer] ";
/// What an orchestrator answers when an event needs nothing from the user.
pub const SILENT: &str = "[silent]";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EventKind {
    Finished,
    NeedsInput,
    Error,
    /// A task orchestrator's report (or a remote task's orchestrator message).
    Report,
    /// One of the orchestrator's background tasks exited (or was stopped).
    BgExit,
    /// "The user opened Home …": a prompt from the app, shown as a divider.
    Opened,
    /// "The user mentioned these workspaces …": shown as a divider.
    Mention,
    /// Any other app note.
    Info,
}

#[derive(Clone, Debug, PartialEq)]
pub struct HomeEvent {
    pub kind: EventKind,
    /// "Task 7".
    pub task: Option<String>,
    /// Where the agent runs: "api/2", "Pane 2", "dane-server:1", "its orchestrator on mac".
    pub place: Option<String>,
    pub agent: Option<String>,
    pub branch: Option<String>,
    /// A report's workspace.
    pub workspace: Option<String>,
    /// The question, error, finish message or report body.
    pub detail: String,
    pub bg: Option<BgNote>,
    /// The full text the orchestrator gets.
    pub raw: String,
}

impl HomeEvent {
    fn new(kind: EventKind, raw: String) -> Self {
        HomeEvent { kind, task: None, place: None, agent: None, branch: None, workspace: None, detail: String::new(), bg: None, raw }
    }

    /// An agent an orchestrator follows changed state. `place` is "api/2" (or "pane 2" in a
    /// fork's own workspace), `task` the task's title ("Task 3") when the conductor gets it, `what`
    /// "finished" / "needs input" / "hit an error".
    pub fn pane(task: Option<&str>, place: &str, agent: &str, branch: Option<&str>, what: &str, message: Option<&str>) -> Self {
        let b = branch.map(|b| format!(", branch {b}")).unwrap_or_default();
        let message = message.map(str::trim).filter(|m| !m.is_empty()).map(|m| m.chars().take(200).collect::<String>());
        let m = message.as_ref().map(|m| format!(": {m}")).unwrap_or_default();
        let t = task.map(|t| format!("{t} · ")).unwrap_or_default();
        let place = match place.strip_prefix("pane ") {
            Some(n) => format!("Pane {n}"),
            None => place.to_string(),
        };
        let text = format!("{PREFIX}{t}{place} ({agent}{b}) {what}{m}");
        let raw = if text.ends_with(['.', '?', '!']) { text } else { format!("{text}.") };
        HomeEvent {
            kind: kind_of(what),
            task: task.map(str::to_string),
            place: Some(place),
            agent: Some(agent.to_string()),
            branch: branch.map(str::to_string),
            detail: message.unwrap_or_default(),
            ..HomeEvent::new(EventKind::Info, raw)
        }
    }

    /// A task orchestrator's report to the conductor.
    pub fn report(task: &str, workspace: &str, text: &str) -> Self {
        let raw = format!("{PREFIX}{task} ({workspace}): {text}");
        HomeEvent { task: Some(task.into()), workspace: Some(workspace.into()), detail: text.into(), ..HomeEvent::new(EventKind::Report, raw) }
    }

    /// A remote task's orchestrator said something.
    pub fn remote_said(task: &str, text: &str) -> Self {
        let raw = format!("{PREFIX}{task}: the other machine said: {text}");
        HomeEvent { task: Some(task.into()), detail: text.into(), ..HomeEvent::new(EventKind::Report, raw) }
    }

    /// The app prompting the orchestrator ("The user opened Home. …").
    pub fn prompt(text: &str) -> Self {
        parse(&format!("{PREFIX}{text}")).unwrap_or_else(|| HomeEvent::new(EventKind::Info, format!("{PREFIX}{text}")))
    }

    /// "dane-server" when the place is a pane on a paired machine ("dane-server:1").
    pub fn machine(&self) -> Option<&str> {
        let p = self.place.as_deref()?;
        if let Some(m) = p.strip_prefix("its orchestrator on ") {
            return Some(m);
        }
        let (m, n) = p.rsplit_once(':')?;
        (!m.is_empty() && !m.contains(['/', ' ']) && n.chars().all(|c| c.is_ascii_digit())).then_some(m)
    }

    /// A short line for places with no event row (the phone, tooltips).
    pub fn summary(&self) -> String {
        let who = [self.task.as_deref(), self.place.as_deref()].into_iter().flatten().collect::<Vec<_>>().join(" · ");
        match self.kind {
            EventKind::Finished => format!("{who} finished"),
            EventKind::NeedsInput => format!("{who} needs input{}", colon(&self.detail)),
            EventKind::Error => format!("{who} hit an error{}", colon(&self.detail)),
            EventKind::Report => format!("{} · report", self.task.as_deref().unwrap_or("Task")),
            EventKind::BgExit => self.bg.as_ref().map_or_else(|| "Background task".to_string(), |b| format!("Background task {}", b.title())),
            EventKind::Opened => "Home opened · brief requested".into(),
            EventKind::Mention => "Mentioned workspaces".into(),
            EventKind::Info => first_line(self.raw.strip_prefix(PREFIX).unwrap_or(&self.raw)),
        }
    }

    /// Prompts from the app, not things that happened: drawn as a muted divider.
    pub fn is_divider(&self) -> bool {
        matches!(self.kind, EventKind::Opened | EventKind::Mention)
    }
}

fn colon(s: &str) -> String {
    if s.is_empty() { String::new() } else { format!(": {}", first_line(s)) }
}

fn first_line(s: &str) -> String {
    s.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim().chars().take(160).collect()
}

fn kind_of(what: &str) -> EventKind {
    match what {
        "finished" => EventKind::Finished,
        "needs input" => EventKind::NeedsInput,
        "hit an error" => EventKind::Error,
        _ => EventKind::Info,
    }
}

/// "Task 12" at the start of `s`, and the rest.
fn task_prefix(s: &str) -> Option<(&str, &str)> {
    let digits = s.strip_prefix("Task ")?;
    let n = digits.len() - digits.trim_start_matches(|c: char| c.is_ascii_digit()).len();
    (n > 0).then(|| s.split_at(5 + n))
}

/// One event (`text` starts with "[winfer] "); `None` for anything else.
pub fn parse(text: &str) -> Option<HomeEvent> {
    let body = text.strip_prefix(PREFIX)?;
    let raw = text.to_string();
    if body.starts_with("The user opened Home") {
        return Some(HomeEvent::new(EventKind::Opened, raw));
    }
    if body.starts_with("The user mentioned these workspaces") {
        return Some(HomeEvent::new(EventKind::Mention, raw));
    }
    if let Some(note) = bg_note::parse(body) {
        let mut e = HomeEvent::new(EventKind::BgExit, raw);
        e.detail = note.output.clone();
        e.bg = Some(note);
        return Some(e);
    }
    let (task, rest) = match task_prefix(body) {
        Some((t, rest)) => (Some(t.to_string()), rest),
        None => (None, body),
    };
    if let Some(task) = &task {
        if let Some(said) = rest.strip_prefix(": the other machine said: ") {
            return Some(HomeEvent { task: Some(task.clone()), detail: said.into(), ..HomeEvent::new(EventKind::Report, raw) });
        }
        if let Some(r) = rest.strip_prefix(" (")
            && let Some((ws, report)) = r.split_once("): ")
            && !ws.contains(['(', ')'])
        {
            return Some(HomeEvent { task: Some(task.clone()), workspace: Some(ws.into()), detail: report.into(), ..HomeEvent::new(EventKind::Report, raw) });
        }
    }
    let rest = match &task {
        Some(_) => rest.strip_prefix(" · ")?,
        None => rest,
    };
    if let Some((place, r)) = rest.split_once(" (")
        && let Some((who, after)) = r.split_once(") ")
    {
        for what in ["finished", "needs input", "hit an error"] {
            let Some(tail) = after.strip_prefix(what) else { continue };
            let detail = match tail.strip_prefix(": ") {
                Some(m) => m.trim().to_string(),
                None if tail == "." || tail.is_empty() => String::new(),
                None => continue,
            };
            let (agent, branch) = match who.split_once(", branch ") {
                Some((a, b)) => (a.to_string(), Some(b.to_string())),
                None => (who.to_string(), None),
            };
            return Some(HomeEvent { kind: kind_of(what), task, place: Some(place.into()), agent: Some(agent), branch, detail, ..HomeEvent::new(EventKind::Info, raw) });
        }
    }
    Some(HomeEvent { task, ..HomeEvent::new(EventKind::Info, raw) })
}

/// The events in a message Home sent (it joins several queued ones with newlines). `None` when
/// the message isn't one: the user's own messages never start with "[winfer] ".
pub fn parse_message(text: &str) -> Option<Vec<HomeEvent>> {
    if !text.starts_with(PREFIX) {
        return None;
    }
    let mut chunks: Vec<String> = Vec::new();
    for line in text.split('\n') {
        match chunks.last_mut() {
            Some(c) if !line.starts_with(PREFIX) => {
                c.push('\n');
                c.push_str(line);
            }
            _ => chunks.push(line.to_string()),
        }
    }
    Some(chunks.iter().filter_map(|c| parse(c.trim_end())).collect())
}

/// An orchestrator's answer that says the event needed nothing from the user.
pub fn is_silent(text: &str) -> bool {
    text.trim().trim_end_matches('.').trim().eq_ignore_ascii_case(SILENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pane_events_round_trip() {
        let e = HomeEvent::pane(Some("Task 7"), "winfer/5", "WinferCode", Some("winfer/winfer-4"), "finished", None);
        assert_eq!(e.raw, "[winfer] Task 7 · winfer/5 (WinferCode, branch winfer/winfer-4) finished.");
        assert_eq!(parse(&e.raw).unwrap(), HomeEvent { detail: String::new(), ..e.clone() });
        let e = HomeEvent::pane(Some("Task 2"), "winfer/2", "Claude Code", None, "needs input", Some(" Keep the migration? "));
        assert_eq!(e.raw, "[winfer] Task 2 · winfer/2 (Claude Code) needs input: Keep the migration?");
        assert_eq!(parse(&e.raw).unwrap(), e);
        let e = HomeEvent::pane(None, "pane 2", "Codex", None, "hit an error", None);
        assert_eq!(e.raw, "[winfer] Pane 2 (Codex) hit an error.");
        assert_eq!(parse(&e.raw).unwrap(), e);
    }

    #[test]
    fn remote_panes_get_a_machine() {
        let e = parse("[winfer] dane-server:1 (WinferCode) finished: Not logged in · Please run /login.").unwrap();
        assert_eq!((e.kind, e.machine(), e.detail.as_str()), (EventKind::Finished, Some("dane-server"), "Not logged in · Please run /login."));
        assert_eq!(parse("[winfer] Task 3 · its orchestrator on mac (winfer) needs input: Go?").unwrap().machine(), Some("mac"));
        assert_eq!(parse("[winfer] Task 3 · api/2 (claude) finished.").unwrap().machine(), None);
    }

    #[test]
    fn reports_bg_exits_and_prompts() {
        let r = HomeEvent::report("Task 7", "winfer", "## Summary\nDone.\n\n- a");
        let p = parse(&r.raw).unwrap();
        assert_eq!((p.kind, p.task.as_deref(), p.workspace.as_deref(), p.detail.as_str()), (EventKind::Report, Some("Task 7"), Some("winfer"), "## Summary\nDone.\n\n- a"));
        assert_eq!(parse(&HomeEvent::remote_said("Task 4", "hi").raw).unwrap().kind, EventKind::Report);
        let b = parse("[winfer] Background task bg1 \"nebula release watch\" (`watch.sh`) exited with code 0 after 1495s. Its output since your last read: ok").unwrap();
        assert_eq!((b.kind, b.bg.as_ref().unwrap().code, b.detail.as_str()), (EventKind::BgExit, Some(0), "ok"));
        assert_eq!(HomeEvent::prompt("The user opened Home. Give a short brief.").kind, EventKind::Opened);
        assert!(parse("[winfer] The user mentioned these workspaces (…):\n- @api = x").unwrap().is_divider());
        assert_eq!(parse("[winfer] Home switched your agents (a) to b: r").unwrap().kind, EventKind::Info);
        assert!(parse("hello").is_none());
    }

    #[test]
    fn messages_split_into_events() {
        let text = "[winfer] Task 1 · a/1 (c) finished.\n[winfer] Task 2 (ws): line one\nline two\n[winfer] x:1 (c) hit an error.";
        let evs = parse_message(text).unwrap();
        assert_eq!(evs.iter().map(|e| e.kind).collect::<Vec<_>>(), [EventKind::Finished, EventKind::Report, EventKind::Error]);
        assert_eq!(evs[1].detail, "line one\nline two");
        assert!(parse_message("fix [winfer] stuff").is_none());
    }

    #[test]
    fn silent_answers() {
        assert!(is_silent("[silent]") && is_silent(" [SILENT]. ") && !is_silent("[silent] but also"));
    }
}
