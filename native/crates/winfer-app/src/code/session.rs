//! The running side of a winfer code chat pane: the `winfer code --serve` process behind a terminal
//! `Session`, its JSON lines turned into [`CodeState`] updates, and commands written back
//! (port of `features/code/transport.ts`, `typedInput.ts` and the pane's store lifecycle).

use super::protocol::*;
use super::state::{Attachment, CodeState, Item, now_ms};
use crate::model::{AppState, Pane};
use crate::status;
use crate::terminal::{self, CodeHooks, Events, Session, SpawnSettings};
use gpui_kit::{Context, Entity, EventEmitter};
use std::sync::Arc;
use std::time::Duration;

/// What the pump receives: bytes from the process, or text typed into the pane from outside.
enum Msg {
    Bytes(Vec<u8>),
    Input(String),
}

pub struct CodeModel {
    pub pane_id: String,
    pub state: CodeState,
    session: Arc<Session>,
    app: Entity<AppState>,
    lines: LineBuffer,
    /// Commands written before the process was up.
    pending: Vec<String>,
    flushing: bool,
    /// The composer's text. The view mirrors it into its input whenever `draft_rev` moves.
    pub draft: String,
    pub draft_rev: u64,
    pub attachments: Vec<Attachment>,
    pub plan_sidebar: bool,
    screen_at: u64,
}

/// Text typed into a winfer code pane from outside it (the agent bar's broadcast, send_to_pane,
/// the phone, Approve and Interrupt), read the way its terminal UI reads keys: text goes into the
/// composer, Enter sends what's there (or accepts a pending approval), Esc interrupts, and "y"
/// accepts a pending approval.
pub fn type_into(state: &mut CodeState, draft: &mut String, data: &str) {
    if data == "\x1b" {
        state.send(Cmd::Interrupt);
        return;
    }
    let approval = state.approvals.first().cloned();
    if let Some(a) = &approval {
        if a.questions.is_empty() && draft.is_empty() && (data == "y" || data == "\r") {
            state.send(Cmd::Approve { id: a.id.clone(), decision: Decision::Allow });
            return;
        }
    }
    // A bracketed paste is text, not keys: its line breaks stay in the composer instead of sending.
    if let Some(pasted) = data.strip_prefix("\x1b[200~") {
        let pasted = pasted.strip_suffix("\x1b[201~").unwrap_or(pasted);
        draft.push_str(&pasted.replace("\r\n", "\n").replace('\r', "\n"));
        return;
    }
    let clean = data.replace("\x1b[200~", "").replace("\x1b[201~", "");
    let parts: Vec<&str> = clean.split('\r').collect();
    for (i, part) in parts.iter().enumerate() {
        draft.push_str(part);
        if i == parts.len() - 1 {
            break;
        }
        if draft.trim().is_empty() {
            draft.clear();
            continue;
        }
        let text = std::mem::take(draft);
        match super::commands::run_command(&text, state) {
            Some(_) => {}
            None => state.submit(&text, &[], None),
        }
    }
}

impl CodeModel {
    #[cfg(test)]
    pub fn for_test(app: Entity<AppState>) -> Self {
        let (tx, _rx) = async_channel::unbounded();
        let session = Session::new("test-pane", tx);
        Self {
            pane_id: "test-pane".to_string(),
            state: CodeState::default(),
            session,
            app,
            lines: LineBuffer::default(),
            pending: Vec::new(),
            flushing: false,
            draft: String::new(),
            draft_rev: 0,
            attachments: Vec::new(),
            plan_sidebar: false,
            screen_at: 0,
        }
    }

    pub fn new(pane: Pane, settings: SpawnSettings, session: Arc<Session>, events: Events, app: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let (tx, rx) = async_channel::unbounded::<Msg>();
        let (tap, input) = (tx.clone(), tx.clone());
        session.attach_code(CodeHooks {
            tap: Box::new(move |bytes| {
                let _ = tap.try_send(Msg::Bytes(bytes.to_vec()));
            }),
            input: Box::new(move |data| {
                let _ = input.try_send(Msg::Input(data.to_string()));
            }),
        });
        let pane_id = pane.id.clone();
        let plan_sidebar = false;
        let model = Self { pane_id: pane_id.clone(), state: CodeState::default(), session: session.clone(), app, lines: LineBuffer::default(), pending: Vec::new(), flushing: false, draft: String::new(), draft_rev: 0, attachments: Vec::new(), plan_sidebar, screen_at: 0 };
        cx.spawn(async move |this, cx| {
            while let Ok(first) = rx.recv().await {
                let mut batch = vec![first];
                while let Ok(next) = rx.try_recv() {
                    batch.push(next);
                }
                if this.update(cx, |m, cx| m.on_messages(batch, cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
        #[cfg(debug_assertions)]
        if std::env::var_os("WINFER_NATIVE_CODE_DEMO").is_some() {
            let tx = tx.clone();
            cx.spawn(async move |this, cx| {
                for (delay, step) in super::demo::script(&std::env::var("WINFER_NATIVE_CODE_DEMO").unwrap_or_default()) {
                    cx.background_executor().timer(Duration::from_millis(delay)).await;
                    if let super::demo::Step::Gate(file) = &step {
                        while std::fs::remove_file(file).is_err() {
                            cx.background_executor().timer(Duration::from_millis(200)).await;
                        }
                        continue;
                    }
                    if let super::demo::Step::OpenTaskLog(id) = &step {
                        let id = (*id).to_string();
                        let _ = this.update(cx, |m, cx| {
                            m.state.open_task_log(&id);
                            cx.notify();
                        });
                        continue;
                    }
                    let _ = match step {
                        super::demo::Step::Type(text) => tx.try_send(Msg::Input(text.to_string())),
                        super::demo::Step::Gate(_) | super::demo::Step::OpenTaskLog(_) => Ok(()),
                        super::demo::Step::Ev(v) => tx.try_send(Msg::Bytes(format!("{v}\n").into_bytes())),
                    };
                }
            })
            .detach();
            return model;
        }
        let first = (!pane.attachments.is_empty()).then(|| (pane.prompt.clone().unwrap_or_default(), pane.attachments.clone()));
        terminal::spawn(session, pane, settings, events);
        let mut model = model;
        if let Some((text, attachments)) = first {
            // `flush` holds this until the process is up.
            model.attachments = attachments;
            model.submit(&text, None, cx);
        }
        model
    }

    /// Forgets the transcript, before the history is replayed from the start (a remote chat
    /// pane re-attaching to its machine).
    pub fn reset(&mut self, cx: &mut Context<Self>) {
        self.state = CodeState::default();
        self.lines = LineBuffer::default();
        cx.notify();
    }

    /// The process exited (the app got `pty://exit` for this pane).
    pub fn on_exit(&mut self, code: u32, cx: &mut Context<Self>) {
        self.state.apply(Ev::Exited { text: format!("WinferCode exited (code {code})") });
        self.flush(cx);
        cx.notify();
    }

    fn on_messages(&mut self, batch: Vec<Msg>, cx: &mut Context<Self>) {
        for msg in batch {
            match msg {
                Msg::Bytes(bytes) => {
                    for line in self.lines.push(&bytes) {
                        if let Some(ev) = parse_line(&line) {
                            self.on_event(ev, cx);
                        }
                    }
                }
                Msg::Input(data) => {
                    let mut draft = std::mem::take(&mut self.draft);
                    let before = draft.clone();
                    let items = self.state.items.len();
                    type_into(&mut self.state, &mut draft, &data);
                    self.note_new_user_messages(items, cx);
                    if draft != before {
                        self.draft_rev += 1;
                    }
                    self.draft = draft;
                }
            }
        }
        self.flush(cx);
        self.sync_screen();
        cx.notify();
    }

    fn on_event(&mut self, ev: Ev, cx: &mut Context<Self>) {
        let user = matches!(ev, Ev::User { .. });
        let items = self.state.items.len();
        let ended = matches!(ev, Ev::TurnDone);
        self.state.apply(ev);
        if user {
            self.note_new_user_messages(items, cx);
        }
        if ended {
            cx.emit(CodeEvent::TurnDone);
        }
    }

    /// Each message the user sends (typed, the start prompt, or sent from elsewhere) becomes the
    /// pane's task.
    fn note_new_user_messages(&mut self, from: usize, cx: &mut Context<Self>) {
        let texts: Vec<String> = self.state.items.iter().skip(from).filter_map(|i| match i {
            Item::User { text, at, .. } if *at > 0 => Some(text.clone()),
            _ => None,
        }).collect();
        if let Some(text) = texts.last() {
            let id = self.pane_id.clone();
            status::set_task(&self.app, &id, text, cx);
        }
    }

    /// Writes the commands the state queued, holding them until the process is up.
    pub fn flush(&mut self, cx: &mut Context<Self>) {
        self.drain(cx);
        if !self.pending.is_empty() && !self.flushing {
            self.flushing = true;
            // One retry loop at a time; it ends once everything is written (or the process is gone).
            cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor().timer(Duration::from_millis(100)).await;
                    let more = this.update(cx, |m, cx| {
                        m.drain(cx);
                        let more = !m.pending.is_empty();
                        m.flushing = more;
                        more
                    });
                    if !more.unwrap_or(false) {
                        break;
                    }
                }
            })
            .detach();
        }
    }

    fn drain(&mut self, cx: &mut Context<Self>) {
        for cmd in std::mem::take(&mut self.state.outbox) {
            if let Cmd::Export(md) = &cmd {
                self.export(md.clone(), cx);
                continue;
            }
            self.pending.push(cmd.line());
        }
        while let Some(line) = self.pending.first() {
            if self.session.exited.load(std::sync::atomic::Ordering::Relaxed) {
                self.pending.clear();
                break;
            }
            if !self.session.write_process(line) {
                break;
            }
            self.pending.remove(0);
        }
    }

    pub fn send(&mut self, cmd: Cmd, cx: &mut Context<Self>) {
        self.state.send(cmd);
        self.flush(cx);
        cx.notify();
    }

    pub fn submit(&mut self, text: &str, skill: Option<&str>, cx: &mut Context<Self>) {
        let items = self.state.items.len();
        let attachments = std::mem::take(&mut self.attachments);
        self.state.submit(text, &attachments, skill);
        self.note_new_user_messages(items, cx);
        self.flush(cx);
        cx.notify();
    }

    pub fn set_draft(&mut self, text: String) {
        self.draft = text;
    }

    /// Runs the composer's text: a slash command, or a message (a known skill's `/name` is sent
    /// as that skill).
    pub fn run(&mut self, text: &str, cx: &mut Context<Self>) -> Option<super::commands::Effect> {
        let skill = super::commands::skill_call(text, &self.state.skills);
        if let Some((name, args)) = skill {
            self.submit(&args, Some(&name), cx);
            return None;
        }
        match super::commands::run_command(text, &mut self.state) {
            Some(effect) => {
                self.flush(cx);
                cx.notify();
                Some(effect)
            }
            None => {
                self.submit(text, None, cx);
                None
            }
        }
    }

    fn export(&mut self, markdown: String, cx: &mut Context<Self>) {
        let dir = self.state.cwd.clone().unwrap_or_default();
        let name = format!("winfer-session-{}.md", chrono_stamp());
        let path = std::path::Path::new(&dir).join(&name);
        match std::fs::write(&path, markdown) {
            Ok(()) => self.state.notice(format!("Saved {}", path.display())),
            Err(e) => self.state.notice(format!("Couldn't save the export: {e}")),
        }
        cx.notify();
    }

    /// The transcript as text on the terminal screen, so read_pane, summaries and the phone's
    /// mirror see the chat (at most a few times a second).
    fn sync_screen(&mut self) {
        let now = now_ms();
        if now.saturating_sub(self.screen_at) < 250 {
            return;
        }
        self.screen_at = now;
        self.session.set_screen_text(&screen_text(&self.state));
    }
}

/// The last part of the transcript as plain text.
pub fn screen_text(state: &CodeState) -> String {
    let mut out = String::new();
    for item in state.items.iter().rev().take(40).collect::<Vec<_>>().into_iter().rev() {
        match item {
            Item::User { text, .. } => out.push_str(&format!("> {text}\n")),
            Item::Text { text, .. } => out.push_str(&format!("{text}\n")),
            Item::Reasoning { .. } => {}
            Item::Tool { name, title, ok, .. } => out.push_str(&format!("{} {name} {title}\n", match ok { Some(true) => "✓", Some(false) => "✗", None => "…" })),
            Item::Notice { text, .. } => out.push_str(&format!("· {text}\n")),
            Item::Handoff { from_model, from_actual, to_model, .. } => {
                let from = super::names::model_name(from_model, from_actual.as_deref());
                let to = super::names::model_name(to_model, None);
                out.push_str(&format!("⇄ Context handoff: {from} → {to}\n"));
            }
            Item::Worked { .. } => {}
        }
    }
    for a in &state.approvals {
        out.push_str(&format!("Approve {}? {}\n", a.tool, a.title));
    }
    if state.busy {
        out.push_str("(working…)\n");
    }
    out
}

fn chrono_stamp() -> String {
    let secs = now_ms() / 1000;
    format!("{secs}")
}

pub enum CodeEvent {
    TurnDone,
}

impl EventEmitter<CodeEvent> for CodeModel {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_text_lands_in_the_composer_and_enter_sends_it() {
        let mut s = CodeState::default();
        let mut draft = String::new();
        type_into(&mut s, &mut draft, "fix the bug");
        assert_eq!(draft, "fix the bug");
        assert!(s.outbox.is_empty());
        type_into(&mut s, &mut draft, "\r");
        assert!(draft.is_empty());
        assert_eq!(s.outbox[0].json()["text"], "fix the bug");
    }

    #[test]
    fn a_pasted_block_goes_into_the_composer_whole() {
        let mut s = CodeState::default();
        let mut draft = String::new();
        type_into(&mut s, &mut draft, "\x1b[200~one\rtwo\r\x1b[201~");
        assert!(s.outbox.is_empty());
        assert_eq!(draft, "one\ntwo\n");
        type_into(&mut s, &mut draft, "\r");
        assert_eq!(s.outbox.len(), 1);
        assert_eq!(s.outbox[0].json()["text"], "one\ntwo");
    }

    #[test]
    fn escape_interrupts_and_y_or_enter_approve() {
        let mut s = CodeState::default();
        let mut draft = String::new();
        type_into(&mut s, &mut draft, "\x1b");
        assert_eq!(s.outbox[0].json()["type"], "interrupt");
        s.apply(Ev::Approval(Approval { id: "a1".into(), tool: "Bash".into(), ..Default::default() }));
        type_into(&mut s, &mut draft, "y");
        assert_eq!(s.outbox.last().unwrap().json()["decision"], "allow");
        assert!(s.approvals.is_empty());
        s.apply(Ev::Approval(Approval { id: "a2".into(), ..Default::default() }));
        type_into(&mut s, &mut draft, "\r");
        assert_eq!(s.outbox.last().unwrap().json()["id"], "a2");
    }

    #[test]
    fn typed_slash_commands_run() {
        let mut s = CodeState::default();
        let mut draft = String::new();
        type_into(&mut s, &mut draft, "/mode yolo\r");
        assert_eq!(s.mode, Mode::Yolo);
    }

    #[test]
    fn the_screen_shows_the_chat() {
        let mut s = CodeState::default();
        s.submit("hi", &[], None);
        s.apply(Ev::Text { id: "a".into(), text: "hello".into() });
        s.apply(Ev::Handoff { from_model: "claude/fable".into(), from_actual: Some("claude-fable-5-2".into()), to_model: "agy/gemini-4-neon".into(), replace: false });
        let t = screen_text(&s);
        assert!(t.contains("> hi") && t.contains("hello") && t.contains("Context handoff: Fable 5.2 → Gemini 4 Neon"));
    }
}
