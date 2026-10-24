//! `winfer code --serve`: the session without the terminal UI, for winfer's web UI pane.
//! Every backend event goes out on stdout as one JSON line (the shape of `--print --json`, see
//! [`super::event_json`]), and commands come in on stdin as JSON lines (`{"type":"send","text":…}`,
//! see `src/features/code/protocol.ts`). It runs in the pane's PTY like the TUI would, so stdin is
//! put in raw mode: no echo, and no line-length limit on long messages.
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
use super::event::{Cmd, Decision, Ev, Mode, UserMsg};
use super::session::{self, Entry, Saved};
use super::tui::app::{hook, Launch};

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
    mode: Mode,
    system: Option<String>,
    handle: Option<backend::Handle>,
    generation: u64,
    saved: Saved,
    /// Assistant messages by id: their entry in `saved.transcript`.
    assistant: HashMap<String, usize>,
    busy: bool,
    /// Approvals waiting for an answer, oldest first.
    pending: Vec<String>,
    /// Background tasks' output files, for `task_log`.
    task_files: HashMap<String, String>,
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
        };
        self.handle = Some(backend::start(self.kind, opts, etx));
        self.busy = false;
    }

    fn send(&self, cmd: Cmd) {
        if let Some(h) = &self.handle {
            h.send(cmd);
        }
    }

    fn save(&mut self) {
        if self.saved.transcript.iter().all(|e| e.role != "user") {
            return;
        }
        self.saved.updated = session::now();
        self.saved.model = self.spec();
        session::save(&self.saved);
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
        self.assistant.clear();
        self.pending.clear();
        out(&json!({ "type": "restore", "transcript": saved.transcript.iter().map(|e| json!({ "role": e.role, "text": e.text, "state": e.state })).collect::<Vec<_>>() }));
        let resume = saved.backend_session.clone();
        self.saved = saved;
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
        let o = std::process::Command::new("git").args(["diff", "HEAD", "--no-color"]).current_dir(&self.cwd).output();
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
            Ev::ToolStart { name, title, .. } => {
                self.saved.transcript.push(Entry { role: "tool".into(), text: format!("{name}\t{title}"), state: Some("done".into()), ..Default::default() });
            }
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
                self.save();
                let reply = self.saved.transcript.iter().rev().find(|e| e.role == "assistant").map(|e| e.text.clone());
                hook("turn_done", json!({ "session_id": self.saved.id, "last_assistant_message": reply.unwrap_or_default() }));
            }
            Ev::Undone(Ok(_)) => {
                if let Some(i) = self.saved.transcript.iter().rposition(|e| e.role == "user") {
                    self.saved.transcript.truncate(i);
                    self.assistant.retain(|_, v| *v < i);
                }
                self.save();
            }
            // The TUI sends these as the next message; so does the web UI's session.
            Ev::BgNote(note) => {
                let cmd = UserMsg { text: format!("[winfer] {note}"), images: Vec::new(), skill: None };
                self.send(if self.busy { Cmd::Steer(cmd) } else { Cmd::Send(cmd) });
            }
            Ev::Exited(_) => {
                self.busy = false;
                self.pending.clear();
            }
            _ => {}
        }
    }

    fn switch_model(&mut self, spec: &str) {
        let (kind, model) = backend::parse_spec(spec);
        let spec = format!("{}/{}", kind.id(), model);
        Config::update(|c| c.model = spec.clone());
        if kind == self.kind {
            self.model = model.clone();
            self.send(Cmd::SetModel(model));
        } else {
            // Another backend can't continue this one's conversation: it starts fresh.
            self.kind = kind;
            self.model = model;
            self.saved.backend_session = None;
            self.start_backend(None);
            out(&json!({ "type": "notice", "text": format!("Switched to {} · new conversation for this backend", kind.label()) }));
        }
        out(&json!({ "type": "model", "model": self.spec() }));
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
        let images = save_images(&v);
        let msg = |text: String| UserMsg { text, images: images.clone(), skill: v.get("skill").and_then(Value::as_str).map(String::from) };
        match s("type").as_str() {
            "send" | "steer" => {
                let text = s("text");
                let n = images.len();
                self.user_message(&if n == 0 { text.clone() } else { format!("{text}{}[{n} image{}]", if text.is_empty() { "" } else { " " }, if n == 1 { "" } else { "s" }) });
                let steer = s("type") == "steer" && self.busy;
                self.send(if steer { Cmd::Steer(msg(text)) } else { Cmd::Send(msg(text)) });
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
            "set_mode" => {
                if let Ok(mode) = serde_json::from_value::<Mode>(v["mode"].clone()) {
                    self.mode = mode;
                    Config::update(|c| c.mode = mode);
                    self.send(Cmd::SetMode(mode));
                }
            }
            "compact" => self.send(Cmd::Compact),
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
                    .and_then(|f| super::tui::app::read_tail(std::path::Path::new(f), super::tui::app::TASK_LOG_MAX).ok())
                    .map(|raw| super::tui::app::clean_log(&raw));
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
                self.start_backend(None);
                out(&json!({ "type": "cleared" }));
                self.ready();
                hook("session_start", json!({ "session_id": self.saved.id }));
            }
            _ => {}
        }
    }
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

/// Raw mode on the PTY: no echo of our own input, and no canonical-mode line limit.
fn raw_stdin() {
    let _ = ratatui::crossterm::terminal::enable_raw_mode();
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
        mode: launch.mode.unwrap_or(cfg.mode),
        system: launch.system.clone(),
        handle: None,
        generation: 0,
        saved: Saved::default(),
        assistant: HashMap::new(),
        busy: false,
        pending: Vec::new(),
        task_files: HashMap::new(),
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
        app.user_message(&p);
        out(&json!({ "type": "user", "text": p }));
        app.send(Cmd::Send(UserMsg { text: p, images: Vec::new(), skill: None }));
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
    let _ = ratatui::crossterm::terminal::disable_raw_mode();
    0
}
