//! `winfer code --serve`: the session without the terminal UI, for winfer's web UI pane.
//! Every backend event goes out on stdout as one JSON line (the shape of `--print --json`, see
//! [`super::event_json`]), and commands come in on stdin as JSON lines (`{"type":"send","text":…}`,
//! see `src/features/code/protocol.ts`). The web UI uses pipes to preserve JSON bytes; when
//! invoked in a terminal, stdin is put in raw mode to avoid echo and line-length limits.
//!
//! Like the TUI it saves the session (so `--continue`/`--resume` and the app's session tracking
//! work) and reports turns, approvals and the session id through winfer's hooks.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::mpsc::{channel, Sender};

use serde_json::{json, Value};

use super::backend::{self, Kind, Opts};
use super::config::{self, Config};
use super::event::{Cmd, Decision, Ev, Mode, Todo, UserMsg};
use super::handoff::{self, HandoffContext};
use super::session::{self, Entry, PendingHandoff, Saved};
use super::tui::app::{hook, mentions, project_files, Launch};

enum In {
    Ev(u64, Ev),
    Line(String),
    Out(Value),
    Eof,
}

struct Serve {
    tx: Sender<In>,
    cwd: PathBuf,
    kind: Kind,
    model: String,
    actual_model: String,
    mode: Mode,
    /// Ultracode for this session only: off on launch, new and resume, never persisted to the config.
    ultracode: bool,
    system: Option<String>,
    handle: Option<backend::Handle>,
    generation: u64,
    saved: Saved,
    /// Assistant messages by id: their entry in `saved.transcript`.
    assistant: HashMap<String, usize>,
    /// Tool calls by id: their entry in `saved.transcript` (upserted as streaming titles arrive).
    tools: HashMap<String, usize>,
    busy: bool,
    /// A compaction is running: its turn's end mustn't start another one.
    compacting: bool,
    /// Approvals waiting for an answer, oldest first.
    pending: Vec<String>,
    /// Background tasks' output files, for `task_log`.
    task_files: HashMap<String, String>,
    usage_sent: Option<std::time::Instant>,
    /// Latest plan/todos from the backend, included in cross-backend context handoffs.
    todos: Vec<Todo>,
    /// Short failure/outcome notes for tool entries (keyed by `saved.transcript` index), kept in
    /// memory so saved session files stay small while handoffs retain error details.
    tool_notes: HashMap<usize, String>,
    /// Previous backend's `(kind, session_id)` while a cross-backend handoff is still uncommitted,
    /// so switching back before sending a message resumes the original session seamlessly.
    origin_session: Option<(Kind, String)>,
}

fn out(v: &Value) {
    let mut o = std::io::stdout().lock();
    let _ = writeln!(o, "{v}");
    let _ = o.flush();
}

impl Serve {
    fn spec(&self) -> String {
        if self.model.is_empty() { self.kind.id().to_string() } else { format!("{}/{}", self.kind.id(), self.model) }
    }

    fn start_backend(&mut self, resume: Option<String>) {
        self.actual_model.clear();
        if let Some(h) = self.handle.take() {
            h.send(Cmd::Shutdown);
        }
        self.generation += 1;
        let generation = self.generation;
        let (etx, erx) = channel::<Ev>();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            for ev in erx {
                if tx.send(In::Ev(generation, ev)).is_err() {
                    break;
                }
            }
        });
        let cfg = Config::load();
        let opts = Opts {
            cwd: self.cwd.clone(),
            model: self.model.clone(),
            effort: cfg.effort.clone(),
            ultracode: self.ultracode,
            mode: self.mode,
            resume,
            system: self.system.clone(),
            mcp: config::mcp_servers(&cfg),
            allowed_commands: cfg.allowed_commands.clone(),
            trim_at: cfg.trim_at,
            compact_at: cfg.compact_at,
            lean: cfg.lean_bridges && !super::LEAN_OFF.load(std::sync::atomic::Ordering::Relaxed),
            skills: cfg.lean_skills(),
            bg_owner: String::new(),
            sub_model: cfg.subagent_for(self.kind.id()).0,
            sub_effort: cfg.subagent_for(self.kind.id()).1,
            account: backend::account_for(self.kind, self.saved.account.as_deref()),
        };
        self.saved.account = opts.account.clone();
        self.handle = Some(backend::start(self.kind, opts, etx));
        self.busy = false;
    }

    fn send(&self, cmd: Cmd) {
        if let Some(h) = &self.handle {
            h.send(cmd);
        }
    }

    fn compact(&mut self) {
        self.compacting = true;
        self.busy = true;
        self.send(Cmd::Compact);
    }

    fn save(&mut self) {
        if self.saved.transcript.iter().all(|e| e.role != "user") {
            return;
        }
        self.saved.updated = session::now();
        self.saved.model = self.spec();
        session::save(&self.saved);
    }

    fn report_usage(&mut self) {
        if let Some(u) = &self.saved.usage {
            let source = self.saved.backend_session.as_deref()
                .and_then(|s| super::tui::app::session_log(self.kind, s, &self.saved.cwd));
            let model = if self.actual_model.is_empty() { &self.model } else { &self.actual_model };
            super::tui::app::report_usage(self.kind, model, source, u);
            self.usage_sent = Some(std::time::Instant::now());
        }
    }

    fn ready(&self) {
        let cfg = Config::load();
        out(&json!({
            "type": "ready",
            "session": self.saved.id,
            "backend_session": self.saved.backend_session,
            "title": self.saved.title,
            "model": self.spec(),
            "mode": self.mode,
            "effort": cfg.effort,
            "ultracode": self.ultracode,
            "subagent_model": cfg.subagent_for(self.kind.id()).0,
            "subagent_effort": cfg.subagent_for(self.kind.id()).1,
            "cwd": self.cwd.to_string_lossy(),
            "branch": super::tui::app::git_branch(&self.cwd),
            "trim_at": cfg.trim_at,
            "compact_at": cfg.compact_at,
            "mcp": config::mcp_servers(&cfg).keys().cloned().collect::<Vec<_>>(),
        }));
    }

    /// Picks up a saved session: the web UI gets its transcript, the backend its conversation.
    fn restore(&mut self, saved: Saved) {
        let (kind, model) = backend::parse_spec(&saved.model);
        self.kind = kind;
        self.model = model;
        self.actual_model.clear();
        self.assistant.clear();
        self.tools.clear();
        self.pending.clear();
        self.todos.clear();
        self.tool_notes.clear();
        self.origin_session = None;
        out(&json!({
            "type": "restore",
            "transcript": saved.transcript.iter().map(|e| json!({
                "role": e.role,
                "text": e.text,
                "state": e.state,
                "from_model": e.from_model,
                "from_actual": e.from_actual,
                "to_model": e.to_model,
            })).collect::<Vec<_>>()
        }));
        let resume = saved.backend_session.clone();
        self.ultracode = false;
        self.saved = saved;
        self.usage_sent = None;
        self.on_event(Ev::Usage(self.saved.restored_usage()));
        self.start_backend(resume);
    }

    /// `/btw`: a side question answered from the transcript by a one-off call, off the turn.
    fn btw(&self, id: String, question: String) {
        let mut context = String::new();
        for e in self.saved.transcript.iter().rev() {
            let line = format!("[{}] {}\n", e.role, e.text);
            if context.len() + line.len() > super::tui::app::BTW_CONTEXT_BYTES {
                break;
            }
            context.insert_str(0, &line);
        }
        let user = format!(
            "<session cwd=\"{}\" status=\"{}\">\n{context}\n</session>\n\nQuestion: {question}",
            self.cwd.to_string_lossy(),
            if self.busy { "the agent is still working on the last message" } else { "idle" },
        );
        let (provider, model, tx) = (self.kind.id(), self.model.clone(), self.tx.clone());
        std::thread::spawn(move || {
            let answer = crate::llm::respond(provider, &model, super::tui::app::BTW_SYSTEM, &user);
            let v = match answer {
                Ok(a) => json!({ "type": "btw", "id": id, "answer": a }),
                Err(e) => json!({ "type": "btw", "id": id, "error": e }),
            };
            let _ = tx.send(In::Out(v));
        });
    }

    fn diff(&self) {
        let o = crate::env::no_window(std::process::Command::new("git").args(["diff", "HEAD", "--no-color"]).current_dir(&self.cwd)).output();
        match o {
            Ok(o) if o.status.success() => {
                let d = String::from_utf8_lossy(&o.stdout).into_owned();
                if d.trim().is_empty() {
                    out(&json!({ "type": "notice", "text": "No uncommitted changes." }));
                    return;
                }
                let files = d.matches("\ndiff --git").count() + usize::from(d.starts_with("diff --git"));
                let id = format!("diff-{}", session::now());
                let title = format!("{files} file{}", if files == 1 { "" } else { "s" });
                out(&json!({ "type": "tool_start", "id": id, "name": "Diff", "title": title, "input": {} }));
                out(&json!({ "type": "tool_end", "id": id, "ok": true, "output": "", "diff": d }));
            }
            _ => out(&json!({ "type": "error", "text": "Not a git repository." })),
        }
    }

    fn fresh_session(&mut self) {
        self.saved = Saved { id: session::new_id(), cwd: self.cwd.to_string_lossy().into_owned(), model: self.spec(), ..Default::default() };
        self.assistant.clear();
        self.tools.clear();
        self.todos.clear();
        self.tool_notes.clear();
        self.origin_session = None;
    }

    fn user_message(&mut self, text: &str) {
        if self.saved.title.is_empty() {
            self.saved.title = text.lines().next().unwrap_or("").chars().take(80).collect();
        }
        self.saved.transcript.push(Entry { role: "user".into(), text: text.into(), ..Default::default() });
    }

    fn on_event(&mut self, ev: Ev) {
        out(&super::event_json(&ev));
        match ev {
            Ev::Session(id) => {
                self.saved.backend_session = Some(id);
                self.save();
            }
            Ev::Mode(mode) => self.mode = mode,
            Ev::Model(model) => self.actual_model = model,
            Ev::Account(id) => {
                self.saved.account = Some(id);
                self.save();
            }
            Ev::Usage(u) => {
                self.saved.usage = Some(u);
                if self.usage_sent.is_none_or(|t| t.elapsed() >= std::time::Duration::from_millis(500)) {
                    self.report_usage();
                }
            }
            Ev::Task(t) => {
                if let Some(f) = t.output_file {
                    self.task_files.insert(t.id, f);
                }
            }
            Ev::TextDelta { id, text } => match self.assistant.get(&id) {
                Some(&i) => self.saved.transcript[i].text.push_str(&text),
                None => {
                    self.assistant.insert(id, self.saved.transcript.len());
                    self.saved.transcript.push(Entry { role: "assistant".into(), text, ..Default::default() });
                }
            },
            Ev::Text { id, text } => match self.assistant.get(&id) {
                Some(&i) => self.saved.transcript[i].text = text,
                None => {
                    self.assistant.insert(id, self.saved.transcript.len());
                    self.saved.transcript.push(Entry { role: "assistant".into(), text, ..Default::default() });
                }
            },
            Ev::ToolStart { id, name, title, .. } => {
                let text = format!("{name}\t{title}");
                match self.tools.get(&id).copied() {
                    Some(i) if i < self.saved.transcript.len() => self.saved.transcript[i].text = text,
                    _ => {
                        self.tools.insert(id, self.saved.transcript.len());
                        self.saved.transcript.push(Entry { role: "tool".into(), text, state: Some("done".into()), ..Default::default() });
                    }
                }
            }
            Ev::ToolEnd { id, ok, output, .. } => {
                if let Some(&i) = self.tools.get(&id) {
                    if let Some(entry) = self.saved.transcript.get_mut(i) {
                        entry.state = Some(if ok { "done" } else { "failed" }.into());
                    }
                    if !ok {
                        let snippet = output.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim();
                        if !snippet.is_empty() {
                            self.tool_notes.insert(i, snippet.chars().take(200).collect());
                        }
                    }
                }
            }
            Ev::Todos(todos) => self.todos = todos,
            Ev::Approval(a) => {
                self.pending.push(a.id.clone());
                hook("needs_input", json!({ "session_id": self.saved.id, "message": a.title }));
            }
            Ev::TurnStart => {
                self.busy = true;
                hook("turn_start", json!({ "session_id": self.saved.id }));
            }
            Ev::TurnDone => {
                self.busy = false;
                self.pending.clear();
                self.report_usage();
                self.save();
                let reply = self.saved.transcript.iter().rev().find(|e| e.role == "assistant").map(|e| e.text.clone());
                hook("turn_done", json!({ "session_id": self.saved.id, "last_assistant_message": reply.unwrap_or_default() }));
                // The ChatGPT backend compacts itself; the bridges are told to at the threshold.
                if !std::mem::take(&mut self.compacting) && !self.kind.native() {
                    let at = Config::load().compact_at as u64;
                    if let Some(u) = self.saved.usage.as_ref().filter(|u| at > 0 && u.window > 0 && u.context * 100 > u.window * at) {
                        out(&json!({ "type": "notice", "text": format!("Context at {}%; compacting…", u.context * 100 / u.window) }));
                        self.compact();
                    }
                }
            }
            Ev::Undone(Ok(_)) => {
                if let Some(i) = self.saved.transcript.iter().rposition(|e| e.role == "user") {
                    self.saved.transcript.truncate(i);
                    self.assistant.retain(|_, v| *v < i);
                    self.tools.retain(|_, v| *v < i);
                    self.tool_notes.retain(|k, _| *k < i);
                    if self.saved.pending_handoff.as_ref().and_then(|p| p.marker_index).is_some_and(|m| m >= i) {
                        self.saved.pending_handoff = None;
                        self.origin_session = None;
                    }
                }
                self.save();
            }
            // The TUI sends these as the next message; so does the web UI's session.
            Ev::BgNote(note) => {
                let mut cmd = UserMsg { text: format!("[winfer] {note}"), images: Vec::new(), skill: None };
                if self.apply_pending_handoff(&mut cmd) {
                    self.save();
                }
                self.send(if self.busy { Cmd::Steer(cmd) } else { Cmd::Send(cmd) });
            }
            Ev::Exited(_) => {
                self.busy = false;
                self.compacting = false;
                self.pending.clear();
            }
            _ => {}
        }
    }

    /// If a cross-backend handoff is pending, builds the context digest from the transcript so far
    /// and attaches it to `msg`. Must be called before pushing the new user turn to `transcript`.
    fn apply_pending_handoff(&mut self, msg: &mut UserMsg) -> bool {
        let Some(pending) = self.saved.pending_handoff.take() else {
            return false;
        };
        self.origin_session = None;
        let branch = super::tui::app::git_branch(&self.cwd);
        let git_status = handoff::git_status_short(&self.cwd);
        let digest = handoff::build_digest(&HandoffContext {
            from_spec: &pending.from_spec,
            from_actual: pending.from_actual.as_deref(),
            to_spec: &pending.to_spec,
            cwd: &self.cwd,
            branch: branch.as_deref(),
            git_status: git_status.as_deref(),
            todos: &self.todos,
            transcript: &self.saved.transcript,
            tool_notes: &self.tool_notes,
        });
        if let Some(digest) = digest {
            handoff::wrap_user_message(msg, &digest);
        }
        true
    }

    fn switch_model(&mut self, spec: &str) {
        let (kind, model) = backend::parse_spec(spec);
        let spec = format!("{}/{}", kind.id(), model);
        Config::update(|c| c.model = spec.clone());
        if kind == self.kind {
            self.model = model.clone();
            self.send(Cmd::SetModel(model));
            // If an uncommitted handoff is still pending, keep its target model up to date.
            if let Some(pending) = self.saved.pending_handoff.as_mut() {
                pending.to_spec = spec.clone();
                if let Some(idx) = pending.marker_index {
                    if let Some(e) = self.saved.transcript.get_mut(idx).filter(|e| e.role == "handoff") {
                        e.to_model = Some(spec.clone());
                        e.text = handoff::fallback_text(&pending.from_spec, &spec);
                    }
                }
                let (from_model, from_actual, to_model) = (pending.from_spec.clone(), pending.from_actual.clone(), pending.to_spec.clone());
                self.save();
                out(&json!({
                    "type": "handoff",
                    "from_model": from_model,
                    "from_actual": from_actual,
                    "to_model": to_model,
                    "replace": true,
                }));
            }
        } else {
            let returning_to_origin = self
                .saved
                .pending_handoff
                .as_ref()
                .is_some_and(|p| backend::parse_spec(&p.from_spec).0 == kind);

            if returning_to_origin {
                if let Some(pending) = self.saved.pending_handoff.take() {
                    if let Some(idx) = pending.marker_index {
                        if idx + 1 == self.saved.transcript.len() && self.saved.transcript[idx].role == "handoff" {
                            self.saved.transcript.pop();
                        }
                    }
                }
                let resume = self.origin_session.take().filter(|(k, _)| *k == kind).map(|(_, s)| s);
                self.kind = kind;
                self.model = model;
                self.saved.backend_session = resume.clone();
                self.pending.clear();
                self.start_backend(resume);
                self.save();
                out(&json!({ "type": "handoff_cancel" }));
            } else if handoff::has_context(&self.saved.transcript) {
                let (from_spec, from_actual, replace, marker_index) = match self.saved.pending_handoff.take() {
                    Some(prev) => {
                        let idx = prev.marker_index.filter(|&i| i < self.saved.transcript.len() && self.saved.transcript[i].role == "handoff");
                        if let Some(i) = idx {
                            self.saved.transcript[i].to_model = Some(spec.clone());
                            self.saved.transcript[i].text = handoff::fallback_text(&prev.from_spec, &spec);
                        }
                        (prev.from_spec, prev.from_actual, idx.is_some(), idx)
                    }
                    None => {
                        let from_spec = self.spec();
                        let from_actual = (!self.actual_model.is_empty()).then(|| self.actual_model.clone());
                        if let Some(sess) = self.saved.backend_session.take() {
                            self.origin_session = Some((self.kind, sess));
                        }
                        let idx = self.saved.transcript.len();
                        self.saved.transcript.push(Entry {
                            role: "handoff".into(),
                            text: handoff::fallback_text(&from_spec, &spec),
                            from_model: Some(from_spec.clone()),
                            from_actual: from_actual.clone(),
                            to_model: Some(spec.clone()),
                            ..Default::default()
                        });
                        (from_spec, from_actual, false, Some(idx))
                    }
                };
                self.kind = kind;
                self.model = model;
                self.saved.backend_session = None;
                self.saved.usage = None;
                self.pending.clear();
                self.saved.pending_handoff = Some(PendingHandoff {
                    from_spec: from_spec.clone(),
                    from_actual: from_actual.clone(),
                    to_spec: spec.clone(),
                    marker_index,
                });
                self.start_backend(None);
                self.save();
                out(&json!({
                    "type": "handoff",
                    "from_model": from_spec,
                    "from_actual": from_actual,
                    "to_model": spec,
                    "replace": replace,
                }));
            } else {
                // Empty session: switch cleanly without cluttering the welcome screen.
                self.kind = kind;
                self.model = model;
                self.saved.backend_session = None;
                self.saved.pending_handoff = None;
                self.origin_session = None;
                self.pending.clear();
                self.start_backend(None);
            }
        }
        out(&json!({ "type": "model", "model": self.spec() }));
        // The subagent model is per backend.
        self.ready();
    }

    /// Text typed into the PTY by the app itself rather than the web UI (its broadcast bar, its
    /// "approve" button): `y`/`n` answer the oldest approval, anything else is a message.
    fn on_text(&mut self, text: &str) {
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        if let (Some(id), true) = (self.pending.first().cloned(), matches!(text, "y" | "n" | "1" | "3")) {
            let decision = if matches!(text, "y" | "1") { "allow" } else { "deny" };
            out(&json!({ "type": "answered", "id": id }));
            self.on_line(&json!({ "type": "approve", "id": id, "decision": decision }).to_string());
            return;
        }
        out(&json!({ "type": "user", "text": text }));
        self.on_line(&json!({ "type": if self.busy { "steer" } else { "send" }, "text": text }).to_string());
    }

    fn on_line(&mut self, line: &str) {
        // A JSON command can follow stray keystrokes on the same line (a bare "y" with no Enter).
        let (stray, json_part) = match line.find('{') {
            Some(i) => (&line[..i], &line[i..]),
            None => (line, ""),
        };
        if !stray.trim().is_empty() {
            self.on_text(stray);
        }
        let Ok(v) = serde_json::from_str::<Value>(json_part) else { return };
        let s = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or_default().to_string();
        let mut images = save_images(&v);
        match s("type").as_str() {
            "send" | "steer" => {
                let text = match save_files(&v) {
                    Ok(paths) => s("text") + &paths.iter().map(|p| format!("\n@\"{}\"", p.to_string_lossy().replace('"', "\\\""))).collect::<String>(),
                    Err(e) => { out(&json!({ "type": "error", "text": format!("Couldn't attach files: {e}") })); return; }
                };
                let n = images.len();
                // `@path` mentions, as in the TUI: images attach, text files inline for ChatGPT.
                let full = text.clone() + &mentions(&self.cwd, self.kind, &text, &mut images);
                let mut msg = UserMsg { text: full, images, skill: v.get("skill").and_then(Value::as_str).map(String::from) };
                // Build any pending handoff digest BEFORE appending this turn to the transcript.
                let had_handoff = self.apply_pending_handoff(&mut msg);
                self.user_message(&if n == 0 { text.clone() } else { format!("{text}{}[{n} image{}]", if text.is_empty() { "" } else { " " }, if n == 1 { "" } else { "s" }) });
                if had_handoff {
                    self.save();
                }
                let steer = s("type") == "steer" && self.busy;
                self.send(if steer { Cmd::Steer(msg) } else { Cmd::Send(msg) });
            }
            // The `@` completion list for the composer, walked off the main loop.
            "files" => {
                let cwd = self.cwd.clone();
                std::thread::spawn(move || out(&json!({ "type": "files", "files": project_files(&cwd) })));
            }
            "interrupt" => {
                self.send(Cmd::Interrupt);
                hook("interrupted", json!({ "session_id": self.saved.id }));
            }
            "approve" => {
                self.pending.retain(|p| *p != s("id"));
                let decision = match s("decision").as_str() {
                    "allow" => Decision::Allow,
                    "always" => Decision::Always,
                    _ => Decision::Deny,
                };
                if decision != Decision::Deny {
                    hook("approved", json!({ "session_id": self.saved.id }));
                }
                self.send(Cmd::Approve(s("id"), decision));
            }
            "answer" => {
                self.pending.retain(|p| *p != s("id"));
                let answers = v.get("answers").and_then(Value::as_array).map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default();
                hook("approved", json!({ "session_id": self.saved.id }));
                self.send(Cmd::Answer(s("id"), answers));
            }
            "set_model" => self.switch_model(&s("model")),
            "set_effort" => {
                let effort = v.get("effort").and_then(Value::as_str).filter(|e| *e != "default").map(String::from);
                Config::update(|c| c.effort = effort.clone());
                self.send(Cmd::SetEffort(effort));
            }
            "set_ultracode" => {
                let on = v.get("ultracode").and_then(Value::as_bool).unwrap_or(false);
                self.ultracode = on;
                self.send(Cmd::SetUltracode(on));
            }
            "set_subagent" => {
                let get = |k: &str| v.get(k).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty() && *s != "default").map(String::from);
                let model = get("model").map(|m| m.strip_prefix(&format!("{}/", self.kind.id())).map(String::from).unwrap_or(m));
                let effort = model.as_ref().and(get("effort"));
                let kind = self.kind.id();
                let (m, e) = (model.clone(), effort.clone());
                Config::update(|c| c.set_subagent(kind, m, e));
                self.send(Cmd::SetSubagent { model, effort });
                self.ready();
            }
            "set_mode" => {
                if let Ok(mode) = serde_json::from_value::<Mode>(v["mode"].clone()) {
                    self.mode = mode;
                    Config::update(|c| c.mode = mode);
                    self.send(Cmd::SetMode(mode));
                }
            }
            "compact" => self.compact(),
            "set_compaction" => {
                let pct = |k: &str| v.get(k).and_then(Value::as_u64).map(|p| p.min(99) as u8);
                let cfg = Config::update(|c| {
                    if let Some(p) = pct("trim") {
                        c.trim_at = p;
                    }
                    if let Some(p) = pct("compact") {
                        c.compact_at = p;
                    }
                });
                self.send(Cmd::SetCompaction { trim: cfg.trim_at, compact: cfg.compact_at });
                self.ready();
            }
            "diff" => self.diff(),
            // The tail of a background task's output, like the TUI's task view (the web UI asks
            // a few times a second while it's open and the task runs).
            "task_log" => {
                let id = s("id");
                let text = self
                    .task_files
                    .get(&id)
                    .and_then(|f| read_task_log(std::path::Path::new(f)));
                out(&json!({ "type": "task_log", "id": id, "text": text, "file": self.task_files.get(&id) }));
            }
            "btw" => self.btw(s("id"), s("question")),
            "export" => {
                let path = self.cwd.join(format!("winfer-session-{}.md", self.saved.id));
                let text = match std::fs::write(&path, s("markdown")) {
                    Ok(_) => format!("Saved {}", path.display()),
                    Err(e) => format!("Couldn't save: {e}"),
                };
                out(&json!({ "type": "notice", "text": text }));
            }
            // The `/cost` sheet: what the context's `tokens` are made of, estimated from the
            // backend's session log off the main loop (logs can be large).
            "context_breakdown" => {
                let tokens = v["tokens"].as_u64().unwrap_or(0);
                let (kind, cwd) = (self.kind, self.saved.cwd.clone());
                let session = self.saved.backend_session.clone();
                std::thread::spawn(move || {
                    let parts = session
                        .and_then(|s| super::tui::app::session_log(kind, &s, &cwd))
                        .and_then(|src| crate::context_breakdown::breakdown(kind.id(), &src, tokens));
                    out(&json!({ "type": "context_breakdown", "tokens": tokens, "parts": parts }));
                });
            }
            "sessions" => {
                let cwd = self.cwd.to_string_lossy().into_owned();
                let list: Vec<Value> = session::list()
                    .into_iter()
                    .filter(|x| x.cwd == cwd)
                    .take(50)
                    .map(|x| json!({ "id": x.id, "title": x.title, "model": x.model, "updated": x.updated, "messages": x.transcript.iter().filter(|e| e.role == "user").count() }))
                    .collect();
                out(&json!({ "type": "sessions", "sessions": list }));
            }
            "resume" => {
                if let Some(saved) = session::load(&s("id")) {
                    if self.busy {
                        self.send(Cmd::Interrupt);
                    }
                    self.save();
                    out(&json!({ "type": "cleared" }));
                    self.restore(saved);
                    self.ready();
                    hook("session_start", json!({ "session_id": self.saved.id }));
                }
            }
            "undo" => self.send(Cmd::Undo { files: v.get("files").and_then(Value::as_bool).unwrap_or(true) }),
            "stop_task" => self.send(Cmd::StopTask(s("id"))),
            "new" => {
                self.pending.clear();
                self.fresh_session();
                self.ultracode = false;
                self.start_backend(None);
                out(&json!({ "type": "cleared" }));
                self.ready();
                hook("session_start", json!({ "session_id": self.saved.id }));
            }
            _ => {}
        }
    }
}

/// Browser uploads get their own directory; never trust a supplied filename as a path.
fn save_files(v: &Value) -> Result<Vec<PathBuf>, String> {
    use base64::Engine;
    let Some(files) = v.get("files").and_then(Value::as_array) else { return Ok(Vec::new()) };
    files.iter().map(|file| {
        if let Some(path) = file.get("path").and_then(Value::as_str) {
            let path = PathBuf::from(path);
            return if path.is_file() { Ok(path) } else { Err(format!("Not a file: {}", path.display())) };
        }
        let data = file.get("data").and_then(Value::as_str).ok_or("Missing file data")?;
        let bytes = base64::engine::general_purpose::STANDARD.decode(data).map_err(|e| e.to_string())?;
        let name = file.get("name").and_then(Value::as_str).unwrap_or("attachment");
        let name: String = name.chars().map(|c| if c.is_alphanumeric() || ".-_".contains(c) { c } else { '_' }).collect();
        let name = if name.is_empty() || name == "." || name == ".." { "attachment" } else { &name };
        let dir = std::env::temp_dir().join("winfer-code").join(format!("drop-{:016x}", rand::random::<u64>()));
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let path = dir.join(name);
        std::fs::write(&path, bytes).map_err(|e| e.to_string())?;
        Ok(path)
    }).collect()
}

/// Images the web UI pasted (base64 in the command) as files, where the TUI keeps its pasted
/// ones: `<temp>/winfer-code/paste-<millis>-<n>.<ext>`. Backends take image paths.
fn save_images(v: &Value) -> Vec<PathBuf> {
    use base64::Engine;
    let Some(list) = v.get("images").and_then(Value::as_array) else { return Vec::new() };
    let dir = std::env::temp_dir().join("winfer-code");
    let _ = std::fs::create_dir_all(&dir);
    let ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
    list.iter()
        .enumerate()
        .filter_map(|(i, img)| {
            let data = base64::engine::general_purpose::STANDARD.decode(img.get("data")?.as_str()?).ok()?;
            let ext = match img.get("mime").and_then(Value::as_str).unwrap_or("image/png") {
                "image/jpeg" | "image/jpg" => "jpg",
                "image/gif" => "gif",
                "image/webp" => "webp",
                _ => "png",
            };
            let path = dir.join(format!("paste-{ms}-{i}.{ext}"));
            std::fs::write(&path, data).ok()?;
            Some(path)
        })
        .collect()
}

/// Reads a background task's output tail: Antigravity's `board.jsonl` and worker
/// `transcript.jsonl` files are formatted into readable logs; shell logs are cleaned of ANSI.
fn read_task_log(path: &std::path::Path) -> Option<String> {
    let raw = super::tui::app::read_tail(path, super::tui::app::TASK_LOG_MAX).ok()?;
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    let formatted = match name {
        "board.jsonl" => backend::agy::format_board_log(&raw),
        "transcript.jsonl" => backend::agy::format_transcript_log(&raw),
        _ => super::tui::app::clean_log(&raw),
    };
    Some(formatted)
}

/// Raw mode on the PTY: no echo of our own input, and no canonical-mode line limit.
fn raw_stdin() {
    use std::io::IsTerminal;
    if std::io::stdin().is_terminal() {
        let _ = ratatui::crossterm::terminal::enable_raw_mode();
    }
}

pub fn run(launch: Launch) -> i32 {
    raw_stdin();
    let cfg = Config::load();
    let spec = if launch.model.is_empty() { cfg.model.clone() } else { launch.model.clone() };
    let (kind, model) = backend::parse_spec(&spec);
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let (tx, rx) = channel::<In>();
    let mut app = Serve {
        tx: tx.clone(),
        cwd: cwd.clone(),
        kind,
        model,
        actual_model: String::new(),
        mode: launch.mode.unwrap_or(cfg.mode),
        ultracode: false,
        system: launch.system.clone(),
        handle: None,
        generation: 0,
        saved: Saved::default(),
        assistant: HashMap::new(),
        tools: HashMap::new(),
        busy: false,
        pending: Vec::new(),
        task_files: HashMap::new(),
        usage_sent: None,
        compacting: false,
        todos: Vec::new(),
        tool_notes: HashMap::new(),
        origin_session: None,
    };
    app.fresh_session();

    let cwd_str = cwd.to_string_lossy().into_owned();
    let resume = launch.resume.clone().or_else(|| launch.cont.then(|| session::list().into_iter().find(|s| s.cwd == cwd_str).map(|s| s.id)).flatten());
    match resume.and_then(|id| session::load(&id)) {
        Some(saved) => app.restore(saved),
        None => app.start_backend(None),
    }
    app.ready();
    hook("session_start", json!({ "session_id": app.saved.id }));

    // Models load slowly (each backend asks its service): they follow when ready.
    let models = tx.clone();
    std::thread::spawn(move || {
        let list: Vec<Value> = backend::all_models().into_iter().map(|m| json!({ "spec": m.spec, "label": m.label, "detail": m.detail, "efforts": m.efforts })).collect();
        let _ = models.send(In::Out(json!({ "type": "models", "models": list })));
    });

    let input = tx.clone();
    std::thread::spawn(move || {
        let mut stdin = std::io::stdin().lock();
        let mut buf = Vec::new();
        let mut chunk = [0u8; 8192];
        loop {
            match stdin.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    for &b in &chunk[..n] {
                        if b == b'\n' || b == b'\r' {
                            if !buf.is_empty() {
                                let line = String::from_utf8_lossy(&buf).into_owned();
                                buf.clear();
                                if input.send(In::Line(line)).is_err() {
                                    return;
                                }
                            }
                        } else if b == 3 || b == 4 {
                            // ctrl+c / ctrl+d typed into the raw PTY: quit like the TUI would.
                            let _ = input.send(In::Eof);
                            return;
                        } else {
                            buf.push(b);
                        }
                    }
                }
            }
        }
        let _ = input.send(In::Eof);
    });

    if let Some(p) = launch.prompt.filter(|p| !p.trim().is_empty()) {
        let mut msg = UserMsg { text: p.clone(), images: Vec::new(), skill: None };
        let had_handoff = app.apply_pending_handoff(&mut msg);
        app.user_message(&p);
        if had_handoff {
            app.save();
        }
        out(&json!({ "type": "user", "text": p }));
        app.send(Cmd::Send(msg));
    }

    for msg in rx {
        match msg {
            In::Ev(generation, ev) if generation == app.generation => app.on_event(ev),
            In::Ev(..) => {}
            In::Line(line) => app.on_line(&line),
            In::Out(v) => out(&v),
            In::Eof => break,
        }
    }
    app.save();
    app.send(Cmd::Shutdown);
    use std::io::IsTerminal;
    if std::io::stdin().is_terminal() {
        let _ = ratatui::crossterm::terminal::disable_raw_mode();
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_serve() -> Serve {
        let (tx, _rx) = channel();
        Serve {
            tx,
            cwd: std::env::temp_dir(),
            kind: Kind::Claude,
            model: "fable".into(),
            actual_model: "claude-fable-5-2".into(),
            mode: Mode::Ask,
            ultracode: false,
            system: None,
            handle: None,
            generation: 0,
            saved: Saved { id: "test-handoff".into(), cwd: std::env::temp_dir().to_string_lossy().into_owned(), model: "claude/fable".into(), ..Default::default() },
            assistant: HashMap::new(),
            tools: HashMap::new(),
            busy: false,
            pending: Vec::new(),
            task_files: HashMap::new(),
            usage_sent: None,
            compacting: false,
            todos: Vec::new(),
            tool_notes: HashMap::new(),
            origin_session: None,
        }
    }

    #[test]
    fn pending_handoff_injects_digest_once_without_polluting_transcript() {
        let mut s = test_serve();
        s.user_message("Add context handoff");
        s.saved.transcript.push(Entry { role: "assistant".into(), text: "Planned the digest builder.".into(), ..Default::default() });
        s.saved.pending_handoff = Some(PendingHandoff {
            from_spec: "claude/fable".into(),
            from_actual: Some("claude-fable-5-2".into()),
            to_spec: "agy/gemini-4-neon".into(),
            marker_index: None,
        });

        let mut msg = UserMsg { text: "Implement it now".into(), images: Vec::new(), skill: None };
        assert!(s.apply_pending_handoff(&mut msg));
        assert!(msg.text.contains("<context_handoff from=\"claude/fable (claude-fable-5-2)\" to=\"agy/gemini-4-neon\""));
        assert!(msg.text.contains("Add context handoff"));
        assert!(msg.text.contains("Planned the digest builder."));
        assert!(msg.text.ends_with("\n\nImplement it now"));
        assert!(s.saved.pending_handoff.is_none());

        // Subsequent message must not re-inject the handoff block.
        let mut next = UserMsg { text: "Looks good".into(), images: Vec::new(), skill: None };
        assert!(!s.apply_pending_handoff(&mut next));
        assert_eq!(next.text, "Looks good");
    }

    #[test]
    fn tool_end_records_failure_note_for_handoff() {
        let mut s = test_serve();
        s.user_message("Run tests");
        s.on_event(Ev::ToolStart { id: "t1".into(), name: "Bash".into(), title: "cargo test".into(), input: json!({}) });
        s.on_event(Ev::ToolEnd { id: "t1".into(), ok: false, output: "error[E0308]: mismatched types\nmore details".into(), diff: None });
        s.saved.pending_handoff = Some(PendingHandoff {
            from_spec: "claude/fable".into(),
            from_actual: None,
            to_spec: "codex/gpt-6.1-sol".into(),
            marker_index: None,
        });
        let mut msg = UserMsg { text: "Fix the type error".into(), images: Vec::new(), skill: None };
        assert!(s.apply_pending_handoff(&mut msg));
        assert!(msg.text.contains("Bash cargo test (failed) — error[E0308]: mismatched types"));
    }
}
