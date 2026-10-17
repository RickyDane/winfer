//! App state and input handling for the `winfer code` TUI.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use serde_json::json;

use super::anim::Tween;
use super::composer::Composer;
use super::picker::{PickItem, PickKind, Picker};
use super::selection::{self, Clicks, Selection};
use super::transcript::{Block, Item, NoticeKind, Status};
use crate::harness::backend::{self, Handle, Kind, Opts};
use crate::harness::config::{self, Config};
use crate::harness::event::{Approval, Cmd, Decision, Ev, Mode, ModelInfo, Todo, Usage, UserMsg};
use crate::harness::session::{self, Entry, Saved};
use crate::harness::tools;

pub enum Msg {
    Term(Event),
    Agent(u64, Ev),
    Models(Vec<ModelInfo>),
    Files(Vec<String>),
    Shell { id: String, command: String, output: String, ok: bool },
}

pub struct Launch {
    pub model: String,
    pub resume: Option<String>,
    pub cont: bool,
    pub prompt: Option<String>,
    pub mode: Option<Mode>,
    pub system: Option<String>,
}

pub const COMMANDS: [(&str, &str); 20] = [
    ("/model", "Switch model (ChatGPT, Claude, Codex)"),
    ("/effort", "Set reasoning effort"),
    ("/mode", "Permission mode: ask, auto-edit, plan, yolo"),
    ("/new", "Start a new conversation"),
    ("/resume", "Resume a past session"),
    ("/compact", "Summarize the conversation to free context"),
    ("/compaction", "When to trim and auto-compact (% of context)"),
    ("/undo", "Revert the last turn's file changes (ChatGPT, Claude Code)"),
    ("/init", "Create an AGENTS.md for this project"),
    ("/diff", "Show uncommitted git changes"),
    ("/cost", "Token usage and cost"),
    ("/copy", "Copy the last reply"),
    ("/export", "Save the conversation as Markdown"),
    ("/mcp", "MCP servers in use"),
    ("/login", "How to sign in to each backend"),
    ("/help", "Keys and commands"),
    ("/clear", "Clear the screen and start over"),
    ("/review", "Review uncommitted changes"),
    ("/status", "Session, model and directory"),
    ("/quit", "Exit"),
];

const VERBS: [&str; 10] = ["Thinking", "Working", "Pondering", "Crafting", "Tinkering", "Brewing", "Wrangling", "Cooking", "Noodling", "Forging"];

pub struct Completion {
    pub items: Vec<PickItem>,
    pub selected: usize,
    pub start: usize,
    pub slash: bool,
}

pub struct App {
    pub tx: Sender<Msg>,
    pub epoch: Instant,
    pub cwd: PathBuf,
    pub branch: Option<String>,
    pub config: Config,
    pub items: Vec<Item>,
    pub composer: Composer,
    pub kind: Kind,
    pub model: String,
    pub actual_model: String,
    pub handle: Option<Handle>,
    generation: u64,
    pub alive: bool,
    pub busy: bool,
    pub turn_started: Instant,
    pub verb: &'static str,
    pub approvals: VecDeque<Approval>,
    pub approval_choice: usize,
    pub picker: Option<Picker>,
    pub overlay: Tween,
    pub completion: Option<Completion>,
    pub todos: Vec<Todo>,
    pub usage: Usage,
    pub ctx_shown: Tween,
    pub queue: Vec<UserMsg>,
    pub toast: Option<(String, Instant, NoticeKind)>,
    pub models: Vec<ModelInfo>,
    pub files: Vec<String>,
    pub scroll: usize,
    /// Text marked with the mouse in the transcript, and where the transcript was last drawn.
    pub selection: Option<Selection>,
    pub view: selection::View,
    clicks: Clicks,
    pub expanded: bool,
    pub quit_armed: Option<Instant>,
    pub quit: bool,
    pub saved: Saved,
    system: Option<String>,
    shell_context: Vec<String>,
    shell_seq: u64,
    hooked: bool,
}

impl App {
    pub fn new(tx: Sender<Msg>, launch: Launch) -> App {
        let mut app = App::blank(tx, &launch);
        let resume = launch.resume.clone().or_else(|| {
            launch.cont.then(|| session::list().into_iter().find(|s| s.cwd == app.saved.cwd).map(|s| s.id)).flatten()
        });
        match resume.and_then(|id| session::load(&id)) {
            Some(saved) => app.restore(saved),
            None => app.start_backend(None),
        }
        app.load_async();
        if let Some(p) = launch.prompt.filter(|p| !p.trim().is_empty()) {
            app.composer.set(&p);
            app.submit();
        }
        app
    }

    /// The app without a backend running yet.
    pub fn blank(tx: Sender<Msg>, launch: &Launch) -> App {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let mut config = Config::load();
        if let Some(m) = launch.mode {
            config.mode = m;
        }
        let spec = if !launch.model.is_empty() { launch.model.clone() } else { config.model.clone() };
        let (kind, model) = backend::parse_spec(&spec);
        let branch = git_branch(&cwd);
        let mut app = App {
            tx,
            epoch: Instant::now(),
            cwd: cwd.clone(),
            branch,
            config,
            items: Vec::new(),
            composer: Composer::default(),
            kind,
            model,
            actual_model: String::new(),
            handle: None,
            generation: 0,
            alive: false,
            busy: false,
            turn_started: Instant::now(),
            verb: VERBS[0],
            approvals: VecDeque::new(),
            approval_choice: 0,
            picker: None,
            overlay: Tween::new(0.0),
            completion: None,
            todos: Vec::new(),
            usage: Usage::default(),
            ctx_shown: Tween::new(0.0),
            queue: Vec::new(),
            toast: None,
            models: Vec::new(),
            files: Vec::new(),
            scroll: 0,
            selection: None,
            view: selection::View::default(),
            clicks: Clicks::default(),
            expanded: false,
            quit_armed: None,
            quit: false,
            saved: Saved { id: session::new_id(), cwd: cwd.to_string_lossy().into_owned(), ..Default::default() },
            system: launch.system.clone(),
            shell_context: Vec::new(),
            shell_seq: 0,
            hooked: false,
        };
        app.saved.model = app.spec();
        app
    }

    pub fn spec(&self) -> String {
        if self.model.is_empty() { self.kind.id().to_string() } else { format!("{}/{}", self.kind.id(), self.model) }
    }

    fn load_async(&self) {
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(Msg::Models(backend::all_models()));
        });
        let tx = self.tx.clone();
        let cwd = self.cwd.clone();
        std::thread::spawn(move || {
            let mut files: Vec<String> = tools::walk(&cwd, 50_000).into_iter().map(|(p, _)| p.to_string_lossy().into_owned()).collect();
            files.sort();
            let _ = tx.send(Msg::Files(files));
        });
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
                if tx.send(Msg::Agent(generation, ev)).is_err() {
                    break;
                }
            }
        });
        let opts = Opts {
            cwd: self.cwd.clone(),
            model: self.model.clone(),
            effort: self.config.effort.clone(),
            mode: self.config.mode,
            resume,
            system: self.system.clone(),
            mcp: config::mcp_servers(&self.config),
            allowed_commands: self.config.allowed_commands.clone(),
            trim_at: self.config.trim_at,
            compact_at: self.config.compact_at,
            lean: self.config.lean_bridges && !crate::harness::LEAN_OFF.load(std::sync::atomic::Ordering::Relaxed),
        };
        self.handle = Some(backend::start(self.kind, opts, etx));
        self.alive = true;
        self.busy = false;
        self.approvals.clear();
    }

    fn send(&self, cmd: Cmd) {
        if let Some(h) = &self.handle {
            h.send(cmd);
        }
    }

    pub fn notice(&mut self, text: impl Into<String>, kind: NoticeKind) {
        self.items.push(Item::new(Block::Notice { text: text.into(), kind }));
        self.scroll = 0;
    }

    pub fn toast(&mut self, text: impl Into<String>, kind: NoticeKind) {
        self.toast = Some((text.into(), Instant::now(), kind));
    }

    /// Whether anything on screen is moving (the loop ticks fast only then).
    pub fn animating(&self) -> bool {
        self.busy
            || self.items.is_empty()
            || self.overlay.moving()
            || self.ctx_shown.moving()
            || !self.approvals.is_empty()
            || self.toast.as_ref().is_some_and(|t| t.1.elapsed() < Duration::from_millis(4000))
            || self.items.iter().rev().take(8).any(Item::animated)
    }

    // ---- agent events ----

    fn find(&mut self, pred: impl Fn(&Block) -> bool) -> Option<&mut Item> {
        self.items.iter_mut().rev().take(400).find(|i| pred(&i.block))
    }

    fn end_reasoning(&mut self) {
        for item in self.items.iter_mut().rev().take(20) {
            if let Block::Reasoning { ended, .. } = &mut item.block {
                if ended.is_none() {
                    *ended = Some(Instant::now());
                    item.touch();
                }
            }
        }
    }

    pub fn on_agent(&mut self, generation: u64, ev: Ev) {
        if generation != self.generation {
            return;
        }
        match ev {
            Ev::Session(s) => {
                self.saved.backend_session = Some(s);
                if !self.hooked {
                    self.hooked = true;
                    hook("session_start", json!({ "session_id": self.saved.id }));
                }
            }
            Ev::Model(m) => self.actual_model = m,
            Ev::TextDelta { id, text } => {
                self.end_reasoning();
                match self.find(|b| matches!(b, Block::Assistant { id: i, .. } if *i == id)) {
                    Some(item) => {
                        if let Block::Assistant { text: t, .. } = &mut item.block {
                            t.push_str(&text);
                        }
                        item.touch();
                    }
                    None => self.items.push(Item::new(Block::Assistant { id, text })),
                }
            }
            Ev::Text { id, text } => {
                self.end_reasoning();
                match self.find(|b| matches!(b, Block::Assistant { id: i, .. } if *i == id)) {
                    Some(item) => {
                        if let Block::Assistant { text: t, .. } = &mut item.block {
                            *t = text;
                        }
                        item.touch();
                    }
                    None if !text.trim().is_empty() => self.items.push(Item::new(Block::Assistant { id, text })),
                    None => {}
                }
            }
            Ev::ReasoningDelta { id, text } => match self.find(|b| matches!(b, Block::Reasoning { id: i, .. } if *i == id)) {
                Some(item) => {
                    if let Block::Reasoning { text: t, ended, .. } = &mut item.block {
                        t.push_str(&text);
                        *ended = None;
                    }
                    item.touch();
                }
                None => self.items.push(Item::new(Block::Reasoning { id, text, started: Instant::now(), ended: None })),
            },
            Ev::ToolStart { id, name, title, .. } => {
                self.end_reasoning();
                if name == "Todos" || name == "TodoWrite" {
                    return;
                }
                match self.find(|b| matches!(b, Block::Tool { id: i, .. } if *i == id)) {
                    Some(item) => {
                        if let Block::Tool { title: t, .. } = &mut item.block {
                            *t = title;
                        }
                        item.touch();
                    }
                    None => self.items.push(Item::new(Block::Tool {
                        id,
                        name,
                        title,
                        output: String::new(),
                        diff: None,
                        status: Status::Running,
                        started: Instant::now(),
                        ended: None,
                    })),
                }
            }
            Ev::ToolOutput { id, chunk } => {
                if let Some(item) = self.find(|b| matches!(b, Block::Tool { id: i, .. } if *i == id)) {
                    if let Block::Tool { output, .. } = &mut item.block {
                        if output.len() < 400_000 {
                            output.push_str(&chunk);
                        }
                    }
                    item.touch();
                }
            }
            Ev::ToolEnd { id, ok, output, diff } => {
                if let Some(item) = self.find(|b| matches!(b, Block::Tool { id: i, .. } if *i == id)) {
                    if let Block::Tool { output: o, diff: d, status, ended, .. } = &mut item.block {
                        if !output.is_empty() || o.is_empty() {
                            *o = output;
                        }
                        *d = diff;
                        *status = if ok { Status::Ok } else { Status::Failed };
                        *ended = Some(Instant::now());
                    }
                    item.touch();
                }
            }
            Ev::Approval(a) => {
                if self.approvals.is_empty() {
                    self.approval_choice = 0;
                    hook("needs_input", json!({ "session_id": self.saved.id, "message": format!("{}: {}", a.tool, a.title) }));
                    bell();
                }
                self.approvals.push_back(a);
                self.scroll = 0;
            }
            Ev::Todos(t) => self.todos = t,
            Ev::Usage(u) => {
                let pct = if u.window > 0 { u.context as f32 / u.window as f32 } else { 0.0 };
                self.ctx_shown.set(pct, 600);
                report_usage(&u);
                self.usage = u;
            }
            Ev::Notice(n) => self.notice(n, NoticeKind::Info),
            Ev::Error(e) => self.notice(e, NoticeKind::Error),
            Ev::TurnStart => {
                if !self.busy {
                    self.busy = true;
                    self.turn_started = Instant::now();
                    self.verb = VERBS[rand::random::<u32>() as usize % VERBS.len()];
                    hook("turn_start", json!({ "session_id": self.saved.id }));
                }
            }
            Ev::TurnDone => self.turn_done(),
            Ev::Exited(reason) => {
                self.alive = false;
                self.busy = false;
                self.approvals.clear();
                self.finish_running();
                self.notice(format!("{reason}\nUse /model to pick another backend, or /new to retry."), NoticeKind::Error);
            }
        }
    }

    fn finish_running(&mut self) {
        self.end_reasoning();
        for item in self.items.iter_mut().rev().take(50) {
            if let Block::Tool { status, ended, .. } = &mut item.block {
                if *status == Status::Running {
                    *status = Status::Failed;
                    *ended = Some(Instant::now());
                    item.touch();
                }
            }
        }
    }

    fn turn_done(&mut self) {
        let was_busy = self.busy;
        self.busy = false;
        self.approvals.clear();
        self.finish_running();
        self.save();
        let reply = self.items.iter().rev().find_map(|i| match &i.block {
            Block::Assistant { text, .. } => Some(text.clone()),
            _ => None,
        });
        if was_busy {
            hook("turn_done", json!({ "session_id": self.saved.id, "last_assistant_message": reply.unwrap_or_default() }));
        }
        if !self.queue.is_empty() {
            let msg = self.queue.remove(0);
            self.dispatch(msg);
        } else if was_busy && self.kind != Kind::ChatGpt && self.needs_compact() {
            // The ChatGPT backend compacts itself; the bridges are told to at the threshold.
            self.notice(format!("Context at {}%; compacting…", self.usage.context * 100 / self.usage.window.max(1)), NoticeKind::Info);
            self.busy = true;
            self.send(Cmd::Compact);
        }
    }

    fn needs_compact(&self) -> bool {
        let p = self.config.compact_at as u64;
        p > 0 && self.usage.window > 0 && self.usage.context * 100 > self.usage.window * p
    }

    fn save(&mut self) {
        let mut transcript = Vec::new();
        for item in &self.items {
            let (role, text) = match &item.block {
                Block::User { text } => ("user", text.clone()),
                Block::Assistant { text, .. } => ("assistant", text.clone()),
                Block::Tool { name, title, .. } => ("tool", format!("{name}\t{title}")),
                Block::Notice { text, kind: NoticeKind::Info } => ("notice", text.clone()),
                _ => continue,
            };
            transcript.push(Entry { role: role.into(), text });
        }
        if transcript.iter().all(|e| e.role != "user") {
            return;
        }
        self.saved.transcript = transcript;
        self.saved.updated = session::now();
        self.saved.model = self.spec();
        session::save(&self.saved);
    }

    fn restore(&mut self, saved: Saved) {
        let (kind, model) = backend::parse_spec(&saved.model);
        self.kind = kind;
        self.model = model;
        self.items.clear();
        for e in &saved.transcript {
            let block = match e.role.as_str() {
                "user" => Block::User { text: e.text.clone() },
                "assistant" => Block::Assistant { id: String::new(), text: e.text.clone() },
                "tool" => {
                    let (name, title) = e.text.split_once('\t').unwrap_or((&e.text, ""));
                    Block::Tool {
                        id: String::new(),
                        name: name.into(),
                        title: title.into(),
                        output: String::new(),
                        diff: None,
                        status: Status::Ok,
                        started: Instant::now(),
                        ended: Some(Instant::now()),
                    }
                }
                _ => Block::Notice { text: e.text.clone(), kind: NoticeKind::Info },
            };
            self.items.push(Item::new(block));
        }
        let resume = saved.backend_session.clone();
        self.saved = saved;
        self.notice(format!("Resumed session · {}", session::ago(self.saved.updated)), NoticeKind::Success);
        self.start_backend(resume);
    }

    // ---- sending ----

    pub fn submit(&mut self) {
        let (text, shown, mut images) = self.composer.take();
        self.completion = None;
        let trimmed = text.trim();
        if trimmed.is_empty() && images.is_empty() {
            return;
        }
        self.config.push_history(shown.trim());
        let entry = shown.trim().to_string();
        Config::update(|c| c.push_history(&entry));
        if trimmed.starts_with('/') && !trimmed.starts_with("//") && !trimmed.contains('\n') {
            self.command(trimmed);
            return;
        }
        if let Some(cmd) = trimmed.strip_prefix('!') {
            self.shell(cmd.trim().to_string());
            return;
        }
        let mut full = text.clone();
        let attached = self.mentions(&text, &mut images);
        if !attached.is_empty() {
            full.push_str(&attached);
        }
        if !self.shell_context.is_empty() {
            full = format!("{}\n\n{full}", std::mem::take(&mut self.shell_context).join("\n\n"));
        }
        self.items.push(Item::new(Block::User { text: shown.trim_end().to_string() }));
        if self.saved.title.is_empty() {
            self.saved.title = shown.lines().next().unwrap_or("").chars().take(80).collect();
        }
        self.scroll = 0;
        let msg = UserMsg { text: full, images };
        if self.busy {
            self.queue.push(msg);
        } else {
            self.dispatch(msg);
        }
    }

    fn dispatch(&mut self, msg: UserMsg) {
        // Compaction levels changed in the app's Settings apply from the next turn.
        let saved = Config::load();
        if (saved.trim_at, saved.compact_at) != (self.config.trim_at, self.config.compact_at) {
            self.config.trim_at = saved.trim_at;
            self.config.compact_at = saved.compact_at;
            self.send(Cmd::SetCompaction { trim: saved.trim_at, compact: saved.compact_at });
        }
        if !self.alive {
            self.start_backend(self.saved.backend_session.clone());
        }
        self.busy = true;
        self.turn_started = Instant::now();
        self.verb = VERBS[rand::random::<u32>() as usize % VERBS.len()];
        self.send(Cmd::Send(msg));
    }

    /// `@path` mentions: images are attached, small text files inlined.
    fn mentions(&self, text: &str, images: &mut Vec<PathBuf>) -> String {
        let mut out = String::new();
        for word in text.split_whitespace() {
            let Some(p) = word.strip_prefix('@') else { continue };
            let p = p.trim_end_matches(|c: char| ",.;:)!?".contains(c));
            let path = tools::resolve(&self.cwd, p);
            if !path.is_file() {
                continue;
            }
            if tools::image_base64(&path).is_some() {
                images.push(path);
                continue;
            }
            // Claude Code and Codex read `@path` themselves; inlining would pay for it twice.
            if self.kind != Kind::ChatGpt {
                continue;
            }
            let Ok(meta) = std::fs::metadata(&path) else { continue };
            if meta.len() > 100_000 {
                continue;
            }
            if let Ok(content) = std::fs::read_to_string(&path) {
                out.push_str(&format!("\n\n<file path=\"{p}\">\n{content}\n</file>"));
            }
        }
        out
    }

    fn shell(&mut self, command: String) {
        if command.is_empty() {
            return;
        }
        self.shell_seq += 1;
        let id = format!("shell-{}", self.shell_seq);
        self.items.push(Item::new(Block::Tool {
            id: id.clone(),
            name: "Shell".into(),
            title: command.clone(),
            output: String::new(),
            diff: None,
            status: Status::Running,
            started: Instant::now(),
            ended: None,
        }));
        self.scroll = 0;
        let tx = self.tx.clone();
        let cwd = self.cwd.clone();
        std::thread::spawn(move || {
            let stop = std::sync::atomic::AtomicBool::new(false);
            let out = tools::run_shell(&command, &cwd, Duration::from_secs(600), &stop, &|_| {});
            let _ = tx.send(Msg::Shell { id, command, output: out.output, ok: out.ok });
        });
    }

    pub fn on_shell(&mut self, id: String, command: String, output: String, ok: bool) {
        if let Some(item) = self.find(|b| matches!(b, Block::Tool { id: i, .. } if *i == id)) {
            if let Block::Tool { output: o, status, ended, .. } = &mut item.block {
                *o = output.clone();
                *status = if ok { Status::Ok } else { Status::Failed };
                *ended = Some(Instant::now());
            }
            item.touch();
        }
        self.shell_context.push(format!("<shell command=\"{command}\">\n{}\n</shell>", tools::clip(&output, 3000)));
    }

    // ---- commands ----

    fn command(&mut self, line: &str) {
        let (cmd, arg) = line.split_once(' ').map(|(c, a)| (c, a.trim())).unwrap_or((line, ""));
        match cmd {
            "/help" | "/?" => self.help(),
            "/model" | "/models" => {
                if arg.is_empty() {
                    self.open_models();
                } else {
                    self.switch_model(arg);
                }
            }
            "/effort" => {
                if arg.is_empty() {
                    self.open_effort();
                } else {
                    self.set_effort(arg);
                }
            }
            "/mode" | "/permissions" => match arg {
                "" => self.open_modes(),
                m => match serde_json::from_value::<Mode>(json!(m)) {
                    Ok(m) => self.set_mode(m),
                    Err(_) => self.toast("Modes: ask, auto-edit, plan, yolo", NoticeKind::Error),
                },
            },
            "/new" | "/clear" | "/reset" => self.new_session(),
            "/resume" | "/sessions" => self.open_sessions(),
            "/compact" => {
                if self.busy {
                    self.toast("Wait for the turn to finish.", NoticeKind::Error);
                } else {
                    self.send(Cmd::Compact);
                }
            }
            "/compaction" | "/autocompact" => self.compaction_command(arg),
            "/undo" => self.send(Cmd::Undo),
            "/init" => {
                let prompt = "Analyze this codebase and create an AGENTS.md file at the repository root with concise guidance for coding agents: what the project is, how it's laid out, how to build, run, test and lint it, code style and conventions, and anything non-obvious. If one exists, improve it instead.";
                self.composer.set(prompt);
                self.submit();
            }
            "/review" => {
                let prompt = "Review the uncommitted changes in this repository (git diff, including staged). Look for bugs, regressions, missing edge cases and unclear code. Report findings ranked by severity with file:line references. Don't change any files.";
                self.composer.set(prompt);
                self.submit();
            }
            "/diff" => self.diff(),
            "/cost" | "/usage" => self.cost(),
            "/status" => self.status(),
            "/copy" => self.copy_last(),
            "/export" => self.export(),
            "/mcp" => {
                let servers = config::mcp_servers(&self.config);
                let text = if servers.is_empty() {
                    "No MCP servers. Add them under \"mcpServers\" in ~/Library/Application Support/dev.winfer.app/harness/config.json (inside winfer, its own server is added automatically).".to_string()
                } else {
                    format!("MCP servers: {}", servers.keys().cloned().collect::<Vec<_>>().join(", "))
                };
                self.notice(text, NoticeKind::Info);
            }
            "/login" => self.notice(
                "ChatGPT: winfer → Settings → AI → Sign in with ChatGPT.\nClaude Code: run `claude` once and /login (its own subscription or API key).\nCodex: run `codex login`.",
                NoticeKind::Info,
            ),
            "/quit" | "/exit" | "/q" => self.quit = true,
            _ => self.toast(format!("Unknown command {cmd} — try /help"), NoticeKind::Error),
        }
    }

    fn help(&mut self) {
        let mut s = String::from("Keys\n  enter send · shift/alt+enter or ctrl+j newline · esc interrupt · ctrl+c clear/quit\n  shift+tab permission mode · ctrl+o expand details · ctrl+p command palette · ctrl+t model\n  ↑/↓ history · pgup/pgdn or wheel scroll · @ attach file · ctrl+v paste image · ! run shell command\n  drag to select and copy · double-click a word · triple-click a line · shift+drag for the terminal's own selection\n\nCommands\n");
        for (c, d) in COMMANDS {
            s.push_str(&format!("  {c:<10} {d}\n"));
        }
        self.notice(s.trim_end().to_string(), NoticeKind::Info);
    }

    pub fn open_models(&mut self) {
        let current = self.spec();
        let mut items: Vec<PickItem> = self
            .models
            .iter()
            .map(|m| {
                let group = match m.spec.split('/').next() {
                    Some("chatgpt") => "ChatGPT account",
                    Some("claude") => "Claude Code",
                    _ => "Codex",
                };
                PickItem::new(m.label.clone(), m.detail.clone(), m.spec.clone()).group(group).current(m.spec == current)
            })
            .collect();
        if items.is_empty() {
            items.push(PickItem::new(current.clone(), "current", current.clone()).current(true));
        }
        let mut p = Picker::new(PickKind::Model, "Model", items);
        p.loading = self.models.is_empty();
        self.open(p);
    }

    fn open_effort(&mut self) {
        let cur = self.config.effort.clone().unwrap_or_default();
        let items = [("", "Default", "the model's default"), ("minimal", "Minimal", "fastest"), ("low", "Low", ""), ("medium", "Medium", ""), ("high", "High", "deeper reasoning"), ("xhigh", "Extra high", "hardest problems"), ("max", "Max", "Claude only")]
            .iter()
            .map(|(v, l, d)| PickItem::new(*l, *d, *v).current(*v == cur))
            .collect();
        self.open(Picker::new(PickKind::Effort, "Reasoning effort", items));
    }

    fn open_modes(&mut self) {
        let cur = self.config.mode;
        let items = [
            (Mode::Ask, "Ask", "ask before edits, commands and network"),
            (Mode::AutoEdit, "Auto-edit", "file edits go through; commands ask"),
            (Mode::Plan, "Plan", "read-only: explore and plan, no changes"),
            (Mode::Yolo, "Yolo", "nothing asks — be careful"),
        ]
        .iter()
        .map(|(m, l, d)| PickItem::new(*l, *d, m.label()).current(*m == cur))
        .collect();
        self.open(Picker::new(PickKind::Mode, "Permission mode", items));
    }

    fn open_sessions(&mut self) {
        let cwd = self.saved.cwd.clone();
        let mut list = session::list();
        list.sort_by_key(|s| (s.cwd != cwd, std::cmp::Reverse(s.updated)));
        let items: Vec<PickItem> = list
            .into_iter()
            .take(200)
            .map(|s| {
                let title = if s.title.is_empty() { "(untitled)".to_string() } else { s.title.clone() };
                let here = if s.cwd == cwd { String::new() } else { format!(" · {}", tools::short_path(&s.cwd)) };
                PickItem::new(title, format!("{} · {}{here}", session::ago(s.updated), s.model), s.id.clone())
                    .group(if s.cwd == cwd { "This directory" } else { "Elsewhere" })
            })
            .collect();
        if items.is_empty() {
            self.toast("No saved sessions yet.", NoticeKind::Info);
            return;
        }
        self.open(Picker::new(PickKind::Session, "Resume session", items));
    }

    /// `/compaction`, `/compaction trim 50`, `/compaction full off`.
    fn compaction_command(&mut self, arg: &str) {
        let mut parts = arg.split_whitespace();
        match (parts.next(), parts.next()) {
            (None, _) => self.open_compaction(),
            (Some(which @ ("trim" | "micro" | "full" | "compact" | "auto")), Some(value)) => {
                let percent = match value.trim_end_matches('%') {
                    "off" | "0" => Some(0),
                    v => v.parse::<u8>().ok().filter(|p| (10..=99).contains(p)),
                };
                match percent {
                    Some(p) => self.set_compaction(matches!(which, "trim" | "micro"), p),
                    None => self.toast("Use a percentage from 10 to 99, or off", NoticeKind::Error),
                }
            }
            (Some("trim" | "micro"), None) => self.open_percent(PickKind::TrimAt),
            (Some("full" | "compact" | "auto"), None) => self.open_percent(PickKind::CompactAt),
            _ => self.toast("Usage: /compaction [trim|full] [percent|off]", NoticeKind::Error),
        }
    }

    fn open_compaction(&mut self) {
        let pct = |p: u8| if p == 0 { "off".to_string() } else { format!("at {p}% of context") };
        let items = vec![
            PickItem::new("Micro-compact", format!("{} · trims old tool output (ChatGPT)", pct(self.config.trim_at)), "trim"),
            PickItem::new("Full compact", format!("{} · summarizes the conversation", pct(self.config.compact_at)), "full"),
        ];
        self.open(Picker::new(PickKind::Compaction, "Compaction", items));
    }

    fn open_percent(&mut self, kind: PickKind) {
        let (title, current) = match kind {
            PickKind::TrimAt => ("Micro-compact at", self.config.trim_at),
            _ => ("Full compact at", self.config.compact_at),
        };
        let mut items = vec![PickItem::new("Off", "never", "0").current(current == 0)];
        for p in (30..=95).step_by(5) {
            let note = match (kind, p) {
                (PickKind::TrimAt, 60) | (PickKind::CompactAt, 85) => "default",
                _ => "",
            };
            items.push(PickItem::new(format!("{p}%"), note, p.to_string()).current(current == p));
        }
        self.open(Picker::new(kind, title, items));
    }

    fn set_compaction(&mut self, trim: bool, percent: u8) {
        let set = move |c: &mut Config| if trim { c.trim_at = percent } else { c.compact_at = percent };
        set(&mut self.config);
        Config::update(set);
        self.send(Cmd::SetCompaction { trim: self.config.trim_at, compact: self.config.compact_at });
        let what = if trim { "Micro-compact" } else { "Full compact" };
        let when = if percent == 0 { "off".to_string() } else { format!("at {percent}%") };
        let mut msg = format!("{what} {when}");
        let (t, c) = (self.config.trim_at, self.config.compact_at);
        if t > 0 && c > 0 && t >= c {
            msg.push_str(" — micro-compact won't run unless it's below full compact");
        }
        self.toast(msg, NoticeKind::Success);
    }

    pub fn open_palette(&mut self) {
        let items = COMMANDS.iter().map(|(c, d)| PickItem::new(*c, *d, *c)).collect();
        self.open(Picker::new(PickKind::Palette, "Commands", items));
    }

    fn open(&mut self, p: Picker) {
        self.picker = Some(p);
        self.overlay = Tween::new(0.0);
        self.overlay.set(1.0, 180);
    }

    fn pick(&mut self) {
        let Some(p) = self.picker.take() else { return };
        let Some(item) = p.chosen().cloned() else { return };
        match p.kind {
            PickKind::Model => self.switch_model(&item.value),
            PickKind::Effort => self.set_effort(&item.value),
            PickKind::Mode => {
                if let Ok(m) = serde_json::from_value::<Mode>(json!(item.value)) {
                    self.set_mode(m);
                }
            }
            PickKind::Palette => self.command(&item.value),
            PickKind::Compaction => self.open_percent(if item.value == "trim" { PickKind::TrimAt } else { PickKind::CompactAt }),
            PickKind::TrimAt | PickKind::CompactAt => {
                if let Ok(percent) = item.value.parse::<u8>() {
                    self.set_compaction(p.kind == PickKind::TrimAt, percent);
                }
            }
            PickKind::Session => {
                if self.busy {
                    self.send(Cmd::Interrupt);
                }
                if let Some(saved) = session::load(&item.value) {
                    self.todos.clear();
                    self.usage = Usage::default();
                    self.restore(saved);
                }
            }
        }
    }

    fn switch_model(&mut self, spec: &str) {
        let (kind, model) = backend::parse_spec(spec);
        let label = if model.is_empty() { kind.label().to_string() } else { format!("{} · {model}", kind.label()) };
        let spec = format!("{}/{}", kind.id(), model);
        self.config.model = spec.clone();
        Config::update(|c| c.model = spec);
        if kind == self.kind && self.alive {
            self.model = model.clone();
            self.actual_model = model.clone();
            self.send(Cmd::SetModel(model));
            self.toast(format!("Model: {label}"), NoticeKind::Success);
        } else {
            let fresh = self.saved.backend_session.is_some();
            self.kind = kind;
            self.model = model;
            self.actual_model.clear();
            self.saved.backend_session = None;
            self.usage = Usage::default();
            self.ctx_shown.set(0.0, 300);
            self.start_backend(None);
            if fresh {
                self.notice(format!("Switched to {label}. It starts with a fresh context."), NoticeKind::Info);
            } else {
                self.toast(format!("Model: {label}"), NoticeKind::Success);
            }
        }
        self.saved.model = self.spec();
    }

    fn set_effort(&mut self, e: &str) {
        let e = Some(e.to_string()).filter(|e| !e.is_empty() && e != "default");
        self.config.effort = e.clone();
        let saved = e.clone();
        Config::update(|c| c.effort = saved);
        self.send(Cmd::SetEffort(e.clone()));
        self.toast(format!("Effort: {}", e.as_deref().unwrap_or("default")), NoticeKind::Success);
    }

    pub fn set_mode(&mut self, m: Mode) {
        self.config.mode = m;
        Config::update(|c| c.mode = m);
        self.send(Cmd::SetMode(m));
        self.toast(format!("Mode: {}", m.label()), if m == Mode::Yolo { NoticeKind::Error } else { NoticeKind::Success });
    }

    fn new_session(&mut self) {
        if self.busy {
            self.send(Cmd::Interrupt);
        }
        self.items.clear();
        self.todos.clear();
        self.queue.clear();
        self.usage = Usage::default();
        self.ctx_shown = Tween::new(0.0);
        self.saved = Saved { id: session::new_id(), cwd: self.cwd.to_string_lossy().into_owned(), model: self.spec(), ..Default::default() };
        self.hooked = false;
        self.start_backend(None);
        self.epoch = Instant::now();
    }

    fn diff(&mut self) {
        let out = std::process::Command::new("git").args(["diff", "HEAD", "--no-color"]).current_dir(&self.cwd).output();
        match out {
            Ok(o) if o.status.success() => {
                let d = String::from_utf8_lossy(&o.stdout).into_owned();
                if d.trim().is_empty() {
                    self.toast("No uncommitted changes.", NoticeKind::Info);
                    return;
                }
                let files = d.matches("\ndiff --git").count() + usize::from(d.starts_with("diff --git"));
                let clean: String = d.lines().filter(|l| !l.starts_with("diff --git") && !l.starts_with("index ")).collect::<Vec<_>>().join("\n");
                self.items.push(Item::new(Block::Tool {
                    id: String::new(),
                    name: "Diff".into(),
                    title: format!("{files} file{}", if files == 1 { "" } else { "s" }),
                    output: String::new(),
                    diff: Some(clean),
                    status: Status::Ok,
                    started: Instant::now(),
                    ended: Some(Instant::now()),
                }));
                self.scroll = 0;
            }
            _ => self.toast("Not a git repository.", NoticeKind::Error),
        }
    }

    fn cost(&mut self) {
        let u = &self.usage;
        let mut s = format!(
            "Tokens: {} in ({} cached) · {} out\nContext: {} of {} ({:.0}%)",
            super::anim::tokens(u.input),
            super::anim::tokens(u.cached),
            super::anim::tokens(u.output),
            super::anim::tokens(u.context),
            super::anim::tokens(u.window),
            if u.window > 0 { u.context as f64 * 100.0 / u.window as f64 } else { 0.0 }
        );
        let pct = |p: u8| if p == 0 { "off".to_string() } else { format!("{p}%") };
        s.push_str(&format!("\nMicro-compact {} · full compact {} (/compaction)", pct(self.config.trim_at), pct(self.config.compact_at)));
        if u.input > 0 {
            s.push_str(&format!("\nCache hits: {:.0}% of input", u.cached as f64 * 100.0 / u.input as f64));
        }
        match (u.cost, self.kind) {
            (Some(c), _) => s.push_str(&format!("\nCost: ${c:.4} (API pricing; your plan may cover it)")),
            (None, Kind::ChatGpt) => {
                let tools = serde_json::to_string(&tools::definitions()).map(|t| t.len()).unwrap_or(0);
                s.push_str(&format!("\nBilled to your ChatGPT plan · tools ≈{} tokens per request", tools / 4));
            }
            (None, _) => {}
        }
        self.notice(s, NoticeKind::Info);
    }

    fn status(&mut self) {
        let s = format!(
            "Session {} · {}\nModel {} ({})\nMode {} · effort {}\nDirectory {}{}",
            self.saved.id,
            self.saved.backend_session.as_deref().unwrap_or("not started"),
            self.spec(),
            if self.actual_model.is_empty() { "…" } else { &self.actual_model },
            self.config.mode.label(),
            self.config.effort.as_deref().unwrap_or("default"),
            self.cwd.display(),
            self.branch.as_ref().map(|b| format!(" ({b})")).unwrap_or_default(),
        );
        self.notice(s, NoticeKind::Info);
    }

    fn copy_last(&mut self) {
        let Some(text) = self.items.iter().rev().find_map(|i| match &i.block {
            Block::Assistant { text, .. } => Some(text.clone()),
            _ => None,
        }) else {
            self.toast("Nothing to copy yet.", NoticeKind::Info);
            return;
        };
        copy(&text);
        self.toast("Copied the last reply.", NoticeKind::Success);
    }

    fn export(&mut self) {
        let mut md = format!("# {}\n\n", if self.saved.title.is_empty() { "winfer code session" } else { &self.saved.title });
        for item in &self.items {
            match &item.block {
                Block::User { text } => md.push_str(&format!("## You\n\n{text}\n\n")),
                Block::Assistant { text, .. } => md.push_str(&format!("## Assistant\n\n{text}\n\n")),
                Block::Tool { name, title, output, diff, .. } => {
                    md.push_str(&format!("**{name}** `{title}`\n\n"));
                    if let Some(d) = diff {
                        md.push_str(&format!("```diff\n{d}\n```\n\n"));
                    } else if !output.trim().is_empty() {
                        md.push_str(&format!("```\n{}\n```\n\n", tools::clip(output.trim(), 4000)));
                    }
                }
                _ => {}
            }
        }
        let path = self.cwd.join(format!("winfer-session-{}.md", self.saved.id));
        match std::fs::write(&path, md) {
            Ok(_) => self.notice(format!("Saved {}", tools::short_path(&path.to_string_lossy())), NoticeKind::Success),
            Err(e) => self.notice(format!("Couldn't save: {e}"), NoticeKind::Error),
        }
    }

    // ---- input ----

    pub fn on_term(&mut self, ev: Event) {
        if let Ok(path) = std::env::var("WINFER_CODE_DEBUG") {
            if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
                use std::io::Write;
                let _ = writeln!(f, "{ev:?}");
            }
        }
        match ev {
            Event::Key(k) if k.kind != KeyEventKind::Release => self.on_key(k),
            Event::Paste(s) => {
                if let Some(p) = &mut self.picker {
                    let f = format!("{}{}", p.filter, s.trim());
                    p.set_filter(f);
                } else if s.trim().is_empty() {
                    // Terminals paste nothing when the clipboard holds only an image.
                    self.paste_image();
                } else {
                    self.composer.paste(&s);
                    self.update_completion();
                }
            }
            Event::Mouse(m) => self.on_mouse(m),
            _ => {}
        }
    }

    fn on_mouse(&mut self, m: MouseEvent) {
        match m.kind {
            MouseEventKind::ScrollUp => self.scroll += 3,
            MouseEventKind::ScrollDown => self.scroll = self.scroll.saturating_sub(3),
            MouseEventKind::Down(MouseButton::Left) => {
                if self.picker.is_some() || !self.view.contains(m.row) {
                    self.selection = None;
                    return;
                }
                let p = self.view.point(m.column, m.row);
                match self.clicks.click(m.column, m.row) {
                    2 => {
                        self.selection = Some(self.view.word(p));
                        self.copy_selection();
                    }
                    3 => {
                        self.selection = Some(self.view.line(p));
                        self.copy_selection();
                    }
                    _ => self.selection = Some(Selection::at(p)),
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                let Some(sel) = self.selection.as_mut().filter(|s| s.dragging) else { return };
                // Dragging past the edges scrolls the transcript.
                if m.row < self.view.top {
                    self.scroll += 1;
                } else if m.row >= self.view.bottom {
                    self.scroll = self.scroll.saturating_sub(1);
                }
                sel.head = self.view.point(m.column, m.row);
                sel.moved = true;
            }
            MouseEventKind::Up(MouseButton::Left) => {
                let Some(sel) = self.selection.as_mut().filter(|s| s.dragging) else { return };
                sel.dragging = false;
                if sel.moved {
                    self.copy_selection();
                } else {
                    // A plain click clears the mark.
                    self.selection = None;
                }
            }
            _ => {}
        }
    }

    /// Copies the marked text to the clipboard; the mark stays until the next click or key.
    fn copy_selection(&mut self) {
        let Some(sel) = &self.selection else { return };
        let text = self.view.text(sel);
        if text.trim().is_empty() {
            self.selection = None;
            return;
        }
        copy(&text);
        let lines = text.lines().count();
        let what = if lines > 1 { format!("{lines} lines") } else { format!("{} characters", text.chars().count()) };
        self.toast(format!("Copied {what}"), NoticeKind::Success);
    }

    /// Attaches the clipboard's image (a screenshot, a copied picture) to the prompt.
    fn paste_image(&mut self) {
        match super::clipboard::save_image() {
            Some(path) => self.composer.attach_image(path),
            None => self.notice("No image on the clipboard", NoticeKind::Info),
        }
    }

    fn on_key(&mut self, k: KeyEvent) {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let alt = k.modifiers.contains(KeyModifiers::ALT);
        let shift = k.modifiers.contains(KeyModifiers::SHIFT);

        // Any key drops the mouse mark; ctrl+c (or esc) with a mark only copies/clears it.
        if let Some(sel) = self.selection.take() {
            if ctrl && k.code == KeyCode::Char('c') {
                self.selection = Some(sel);
                self.copy_selection();
                self.selection = None;
                return;
            }
            if k.code == KeyCode::Esc {
                return;
            }
        }

        if ctrl && k.code == KeyCode::Char('c') {
            if self.picker.is_some() {
                self.picker = None;
            } else if !self.composer.is_empty() {
                self.composer.clear();
                self.completion = None;
            } else if self.busy {
                self.interrupt();
            } else if self.quit_armed.is_some_and(|t| t.elapsed() < Duration::from_millis(1500)) {
                self.quit = true;
            } else {
                self.quit_armed = Some(Instant::now());
            }
            return;
        }
        if ctrl && k.code == KeyCode::Char('d') && self.composer.is_empty() {
            self.quit = true;
            return;
        }

        if let Some(p) = &mut self.picker {
            match k.code {
                KeyCode::Esc => self.picker = None,
                KeyCode::Enter => self.pick(),
                KeyCode::Up => p.move_by(-1),
                KeyCode::Down | KeyCode::Tab => p.move_by(1),
                KeyCode::Char('p') | KeyCode::Char('k') if ctrl => p.move_by(-1),
                KeyCode::Char('n') | KeyCode::Char('j') if ctrl => p.move_by(1),
                KeyCode::PageUp => p.move_by(-8),
                KeyCode::PageDown => p.move_by(8),
                KeyCode::Backspace => {
                    let mut f = p.filter.clone();
                    f.pop();
                    p.set_filter(f);
                }
                KeyCode::Char(c) if !ctrl => {
                    let f = format!("{}{c}", p.filter);
                    p.set_filter(f);
                }
                _ => {}
            }
            return;
        }

        if let Some(a) = self.approvals.front().cloned() {
            let decide = |app: &mut App, d: Decision| {
                app.approvals.pop_front();
                app.approval_choice = 0;
                app.send(Cmd::Approve(a.id.clone(), d));
                if d == Decision::Deny {
                    app.toast("Declined", NoticeKind::Info);
                }
                hook("approved", json!({ "session_id": app.saved.id }));
            };
            match k.code {
                KeyCode::Char('y') | KeyCode::Char('1') => decide(self, Decision::Allow),
                KeyCode::Char('a') | KeyCode::Char('2') => decide(self, Decision::Always),
                KeyCode::Char('n') | KeyCode::Char('3') | KeyCode::Esc => decide(self, Decision::Deny),
                KeyCode::Left | KeyCode::Up => self.approval_choice = (self.approval_choice + 2) % 3,
                KeyCode::Right | KeyCode::Down | KeyCode::Tab => self.approval_choice = (self.approval_choice + 1) % 3,
                KeyCode::Enter => {
                    let d = [Decision::Allow, Decision::Always, Decision::Deny][self.approval_choice];
                    decide(self, d);
                }
                KeyCode::Char('o') if ctrl => self.toggle_expanded(),
                KeyCode::PageUp => self.scroll += 10,
                KeyCode::PageDown => self.scroll = self.scroll.saturating_sub(10),
                _ => {}
            }
            return;
        }

        if let Some(c) = &mut self.completion {
            let n = c.items.len();
            match k.code {
                KeyCode::Up => {
                    c.selected = (c.selected + n - 1) % n;
                    return;
                }
                KeyCode::Down => {
                    c.selected = (c.selected + 1) % n;
                    return;
                }
                KeyCode::Tab => {
                    self.accept_completion(false);
                    return;
                }
                KeyCode::Enter if !shift && !alt => {
                    let slash = c.slash;
                    self.accept_completion(slash);
                    return;
                }
                KeyCode::Esc => {
                    self.completion = None;
                    return;
                }
                _ => {}
            }
        }

        match k.code {
            KeyCode::Enter if shift || alt => self.composer.insert("\n"),
            KeyCode::Char('j') if ctrl => self.composer.insert("\n"),
            KeyCode::Char('v') if ctrl => self.paste_image(),
            KeyCode::Enter => {
                // A trailing backslash continues the line.
                if self.composer.text[..self.composer.cursor].ends_with('\\') {
                    self.composer.backspace();
                    self.composer.insert("\n");
                } else {
                    self.submit();
                }
            }
            KeyCode::Esc => {
                if self.busy {
                    self.interrupt();
                } else if !self.composer.is_empty() {
                    self.composer.clear();
                }
            }
            KeyCode::BackTab => self.set_mode(self.config.mode.next()),
            KeyCode::Tab if shift => self.set_mode(self.config.mode.next()),
            KeyCode::Char('o') if ctrl => self.toggle_expanded(),
            KeyCode::Char('p') if ctrl => self.open_palette(),
            KeyCode::Char('t') if ctrl => self.open_models(),
            KeyCode::Char('l') if ctrl => self.scroll = 0,
            KeyCode::Char('a') if ctrl => self.composer.home(),
            KeyCode::Char('e') if ctrl => self.composer.end(),
            KeyCode::Char('u') if ctrl => self.composer.kill_to_start(),
            KeyCode::Char('k') if ctrl => self.composer.kill_to_end(),
            KeyCode::Char('w') if ctrl => self.composer.delete_word(),
            KeyCode::Backspace if alt || ctrl => self.composer.delete_word(),
            KeyCode::Char('b') if alt => self.composer.word_left(),
            KeyCode::Char('f') if alt => self.composer.word_right(),
            KeyCode::Left if alt || ctrl => self.composer.word_left(),
            KeyCode::Right if alt || ctrl => self.composer.word_right(),
            KeyCode::Char(c) if !ctrl => {
                let mut buf = [0u8; 4];
                self.composer.insert(c.encode_utf8(&mut buf));
            }
            KeyCode::Backspace => self.composer.backspace(),
            KeyCode::Delete => self.composer.delete(),
            KeyCode::Left => self.composer.left(),
            KeyCode::Right => self.composer.right(),
            KeyCode::Home => self.composer.home(),
            KeyCode::End => self.composer.end(),
            KeyCode::Up if shift => self.scroll += 1,
            KeyCode::Down if shift => self.scroll = self.scroll.saturating_sub(1),
            KeyCode::Up => {
                if !self.composer.up() {
                    let h = self.config.history.clone();
                    self.composer.history_prev(&h);
                }
            }
            KeyCode::Down => {
                if !self.composer.down() {
                    let h = self.config.history.clone();
                    self.composer.history_next(&h);
                }
            }
            KeyCode::PageUp => self.scroll += 10,
            KeyCode::PageDown => self.scroll = self.scroll.saturating_sub(10),
            _ => {}
        }
        self.update_completion();
    }

    fn toggle_expanded(&mut self) {
        self.expanded = !self.expanded;
        self.toast(if self.expanded { "Showing details" } else { "Hiding details" }, NoticeKind::Info);
    }

    fn interrupt(&mut self) {
        self.send(Cmd::Interrupt);
        self.queue.clear();
        self.approvals.clear();
        self.notice("Interrupted", NoticeKind::Error);
        hook("interrupted", json!({ "session_id": self.saved.id }));
    }

    fn update_completion(&mut self) {
        let text = &self.composer.text;
        if text.starts_with('/') && !text.contains(' ') && !text.contains('\n') {
            let q = &text[1..];
            let mut items: Vec<(i64, PickItem)> = COMMANDS
                .iter()
                .filter_map(|(c, d)| super::picker::score(q, &c[1..]).map(|s| (s, PickItem::new(*c, *d, *c))))
                .collect();
            items.sort_by(|a, b| b.0.cmp(&a.0));
            let items: Vec<PickItem> = items.into_iter().map(|(_, i)| i).take(8).collect();
            self.completion = (!items.is_empty()).then(|| Completion { items, selected: 0, start: 0, slash: true });
            return;
        }
        let (start, word) = self.composer.current_word();
        if let Some(q) = word.strip_prefix('@') {
            let mut scored: Vec<(i64, &String)> = self
                .files
                .iter()
                .filter_map(|f| {
                    let name = f.rsplit('/').next().unwrap_or(f);
                    let s = super::picker::score(q, f)?;
                    let bonus = if super::picker::score(q, name).is_some() { 30 } else { 0 };
                    Some((s + bonus, f))
                })
                .collect();
            scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.len().cmp(&b.1.len())));
            let items: Vec<PickItem> = scored.into_iter().take(8).map(|(_, f)| PickItem::new(f.clone(), "", f.clone())).collect();
            self.completion = (!items.is_empty()).then(|| Completion { items, selected: 0, start, slash: false });
            return;
        }
        self.completion = None;
    }

    fn accept_completion(&mut self, run: bool) {
        let Some(c) = self.completion.take() else { return };
        let Some(item) = c.items.get(c.selected) else { return };
        if c.slash {
            self.composer.set(&item.value);
            if run {
                self.submit();
            } else {
                self.composer.insert(" ");
            }
        } else {
            self.composer.replace_word(c.start, &format!("@{} ", item.value));
        }
    }
}

fn git_branch(cwd: &std::path::Path) -> Option<String> {
    std::process::Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(cwd)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
}

/// Reports status to winfer when running in one of its panes (no-op otherwise).
pub fn hook(event: &str, body: serde_json::Value) {
    let var = |k: &str| std::env::var(k).unwrap_or_default();
    let (addr, token, pane) = (var("WINFER_HOOK"), var("WINFER_TOKEN"), var("WINFER_PANE_ID"));
    if addr.is_empty() || pane.is_empty() {
        return;
    }
    let event = event.to_string();
    std::thread::spawn(move || crate::hooks::send_hook(&addr, &token, &pane, "winfer", &event, body.to_string()));
}

/// Sends context fill and spend to winfer for the pane header's meter and cost chip.
fn report_usage(u: &Usage) {
    if u.window == 0 {
        return;
    }
    let at = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0);
    hook(
        "usage",
        json!({
            "context": { "usedPercent": u.context as f64 * 100.0 / u.window as f64, "tokens": u.context, "window": u.window, "at": at },
            "spend": { "cost": u.cost, "input": u.input, "output": u.output, "cached": u.cached, "at": at },
        }),
    );
}

fn bell() {
    use std::io::Write;
    let _ = std::io::stdout().write_all(b"\x07");
    let _ = std::io::stdout().flush();
}

/// Copies text to the clipboard: the OS tool when there is one, else OSC 52 (works over SSH).
pub fn copy(text: &str) {
    use base64::Engine;
    use std::io::Write;
    let tools: &[(&str, &[&str])] = &[("pbcopy", &[]), ("wl-copy", &[]), ("xclip", &["-selection", "clipboard"]), ("clip", &[])];
    for (cmd, args) in tools {
        if crate::env::find_command(cmd).is_some() {
            if let Ok(mut child) = std::process::Command::new(cmd).args(*args).stdin(std::process::Stdio::piped()).spawn() {
                if let Some(mut stdin) = child.stdin.take() {
                    let _ = stdin.write_all(text.as_bytes());
                }
                let _ = child.wait();
                return;
            }
        }
    }
    let b64 = base64::engine::general_purpose::STANDARD.encode(text);
    let _ = write!(std::io::stdout(), "\x1b]52;c;{b64}\x07");
    let _ = std::io::stdout().flush();
}

/// Reads terminal input on its own thread.
pub fn spawn_input(tx: Sender<Msg>) {
    std::thread::spawn(move || {
        loop {
            match event::poll(Duration::from_millis(250)) {
                Ok(true) => match event::read() {
                    Ok(ev) => {
                        if tx.send(Msg::Term(ev)).is_err() {
                            return;
                        }
                    }
                    Err(_) => return,
                },
                Ok(false) => {}
                Err(_) => return,
            }
        }
    });
}

pub fn drain(app: &mut App, rx: &Receiver<Msg>, first: Msg) {
    let handle = |app: &mut App, m: Msg| match m {
        Msg::Term(e) => app.on_term(e),
        Msg::Agent(g, e) => app.on_agent(g, e),
        Msg::Models(m) => {
            app.models = m;
            if let Some(p) = &app.picker {
                if p.kind == PickKind::Model {
                    let filter = p.filter.clone();
                    app.open_models();
                    if let Some(p) = &mut app.picker {
                        p.set_filter(filter);
                        p.opened -= Duration::from_secs(1);
                    }
                    app.overlay = Tween::new(1.0);
                }
            }
        }
        Msg::Files(f) => app.files = f,
        Msg::Shell { id, command, output, ok } => app.on_shell(id, command, output, ok),
    };
    handle(app, first);
    while let Ok(m) = rx.try_recv() {
        handle(app, m);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn app() -> App {
        let (tx, _rx) = channel();
        let launch = Launch { model: "claude/opus".into(), resume: None, cont: false, prompt: None, mode: Some(Mode::Ask), system: None };
        let mut a = App::blank(tx, &launch);
        a.config = Config::default();
        a
    }

    #[test]
    fn compaction_thresholds_parse_and_pick() {
        let mut a = app();
        a.compaction_command("trim 45");
        assert_eq!(a.config.trim_at, 45);
        a.compaction_command("full off");
        assert_eq!(a.config.compact_at, 0);
        a.compaction_command("full 120");
        assert_eq!(a.config.compact_at, 0);
        a.compaction_command("");
        assert_eq!(a.picker.as_ref().unwrap().kind, PickKind::Compaction);
        key(&mut a, KeyCode::Down);
        key(&mut a, KeyCode::Enter);
        assert_eq!(a.picker.as_ref().unwrap().kind, PickKind::CompactAt);
        for c in "80".chars() {
            key(&mut a, KeyCode::Char(c));
        }
        key(&mut a, KeyCode::Enter);
        assert_eq!(a.config.compact_at, 80);
        a.usage = Usage { context: 90, window: 100, ..Default::default() };
        assert!(a.needs_compact());
    }

    fn screen(app: &mut App, w: u16, h: u16) -> String {
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| super::super::view::draw(f, app)).unwrap();
        let buf = term.backend().buffer().clone();
        (0..h).map(|y| (0..w).map(|x| buf[(x, y)].symbol().to_string()).collect::<String>()).collect::<Vec<_>>().join("\n")
    }

    fn key(app: &mut App, code: KeyCode) {
        app.on_term(Event::Key(KeyEvent::new(code, KeyModifiers::NONE)));
    }

    #[test]
    fn slash_completion_opens_the_model_picker() {
        let mut a = app();
        a.models = vec![ModelInfo { spec: "claude/opus".into(), label: "Claude Opus".into(), detail: "d".into() }];
        for c in "/model".chars() {
            key(&mut a, KeyCode::Char(c));
        }
        assert!(a.completion.is_some());
        key(&mut a, KeyCode::Enter);
        assert!(a.picker.is_some());
        let s = screen(&mut a, 100, 30);
        assert!(s.contains("Claude Opus"), "{s}");
    }

    #[test]
    fn renders_the_welcome_and_a_conversation() {
        let mut a = app();
        let s = screen(&mut a, 90, 30);
        assert!(s.contains("winfer code"), "{s}");
        a.items.push(Item::new(Block::User { text: "hello".into() }));
        a.items.push(Item::new(Block::Assistant { id: "1".into(), text: "**Hi** there\n\n- a\n- b".into() }));
        let s = screen(&mut a, 90, 30);
        assert!(s.contains("Hi there") && s.contains("• a"), "{s}");
    }

    #[test]
    fn renders_an_approval() {
        let mut a = app();
        a.approvals.push_back(Approval { id: "x".into(), tool: "Bash".into(), title: "rm -rf build".into(), detail: String::new(), diff: None });
        let s = screen(&mut a, 90, 30);
        assert!(s.contains("Allow Bash?") && s.contains("rm -rf build"), "{s}");
        key(&mut a, KeyCode::Char('n'));
        assert!(a.approvals.is_empty());
    }

    #[test]
    fn mouse_drag_marks_transcript_text() {
        use ratatui::crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
        let mut a = app();
        a.items.push(Item::new(Block::Assistant { id: "1".into(), text: "alpha beta\n\ngamma delta".into() }));
        let s = screen(&mut a, 60, 20);
        let rows: Vec<&str> = s.lines().collect();
        let y0 = rows.iter().position(|r| r.contains("alpha")).unwrap() as u16;
        let y1 = rows.iter().position(|r| r.contains("gamma")).unwrap() as u16;
        let col = |row: &str, word: &str| row[..row.find(word).unwrap()].chars().count() as u16;
        let x0 = col(rows[y0 as usize], "beta");
        let x1 = col(rows[y1 as usize], "gamma") + 4;
        let mouse = |kind, column, row| Event::Mouse(MouseEvent { kind, column, row, modifiers: KeyModifiers::NONE });
        a.on_term(mouse(MouseEventKind::Down(MouseButton::Left), x0, y0));
        a.on_term(mouse(MouseEventKind::Drag(MouseButton::Left), x1, y1));
        let sel = a.selection.unwrap();
        assert_eq!(a.view.text(&sel), "beta\n\ngamma");
        let mut term = Terminal::new(TestBackend::new(60, 20)).unwrap();
        term.draw(|f| super::super::view::draw(f, &mut a)).unwrap();
        assert_eq!(term.backend().buffer()[(x0, y0)].bg, super::super::theme::SELECTION);
        // A key press drops the mark.
        key(&mut a, KeyCode::Esc);
        assert!(a.selection.is_none());
    }
}
