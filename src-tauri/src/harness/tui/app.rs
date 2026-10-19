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
use crate::harness::checkpoint::{self, Done, Job};
use crate::harness::config::{self, Config};
use crate::harness::event::{Approval, Cmd, Decision, Ev, Mode, ModelInfo, Skill, TaskEv, Todo, Usage, UserMsg};
use crate::harness::session::{self, Entry, Saved};
use crate::harness::tools;

pub enum Msg {
    Term(Event),
    Agent(u64, Ev),
    Models(Vec<ModelInfo>),
    Files(Vec<String>),
    Shell { id: String, command: String, output: String, ok: bool },
    Checkpoint(Done),
}

/// A user turn, for /undo: its message in the transcript and whether the workspace was
/// snapshotted before it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Turn {
    pub id: u64,
    /// The user's message in `items`.
    pub item: usize,
    /// Handed to the backend (queued and starting turns aren't yet).
    pub sent: bool,
    /// The checkpoint thread snapshotted the workspace before it.
    pub checkpoint: bool,
}

pub struct Launch {
    pub model: String,
    pub resume: Option<String>,
    pub cont: bool,
    pub prompt: Option<String>,
    pub mode: Option<Mode>,
    pub system: Option<String>,
}

pub const COMMANDS: [(&str, &str); 23] = [
    ("/model", "Switch model (ChatGPT, Claude, Codex)"),
    ("/effort", "Set reasoning effort (ctrl+r cycles)"),
    ("/mode", "Permission mode: ask, auto-edit, plan, yolo"),
    ("/new", "Start a new conversation"),
    ("/resume", "Resume a past session"),
    ("/compact", "Summarize the conversation to free context"),
    ("/compaction", "When to trim and auto-compact (% of context)"),
    ("/undo", "Undo the last turn: its messages and file changes"),
    ("/revert", "Same as /undo"),
    ("/init", "Create an AGENTS.md for this project"),
    ("/diff", "Show uncommitted git changes"),
    ("/cost", "Token usage and cost"),
    ("/copy", "Copy the last reply"),
    ("/export", "Save the conversation as Markdown"),
    ("/agents", "Subagents running and finished (ctrl+g)"),
    ("/mcp", "MCP servers in use"),
    ("/login", "How to sign in to each backend"),
    ("/help", "Keys and commands"),
    ("/clear", "Clear the screen and start over"),
    ("/review", "Review uncommitted changes"),
    ("/status", "Session, model and directory"),
    ("/sidebar", "Show the plan in a right sidebar when wide (on/off)"),
    ("/quit", "Exit"),
];

const VERBS: [&str; 10] = ["Orbiting", "Charting", "Drifting", "Calibrating", "Navigating", "Stargazing", "Launching", "Docking", "Transmitting", "Warping"];

pub struct Completion {
    pub items: Vec<PickItem>,
    pub selected: usize,
    pub start: usize,
    pub slash: bool,
}

/// A subagent as the status bar and `/agents` show it.
pub struct Subagent<'a> {
    pub id: &'a str,
    pub title: &'a str,
    pub status: Status,
    pub started: Instant,
    pub ended: Option<Instant>,
    /// Tool calls it has made so far and the latest one (while it runs).
    pub steps: usize,
    pub last: &'a str,
    /// Index of its card in `App::items`.
    pub item: usize,
}

/// A task the backend runs on its own, as its events describe it (see `Ev::Task`).
#[derive(Debug, Clone)]
pub struct Task {
    pub id: String,
    pub tool_id: Option<String>,
    pub title: String,
    pub agent: bool,
    pub status: Status,
    pub started: Instant,
    pub ended: Option<Instant>,
    pub tools: usize,
    pub last: String,
    pub output_file: Option<PathBuf>,
    pub exit_code: Option<i32>,
    polled: Option<Instant>,
}

/// Tool cards that are subagents: Claude Code's Task/Agent tool, Codex's subagents and
/// winfer code's `agent` tool all show as "Agent".
pub fn is_subagent(name: &str) -> bool {
    matches!(name, "Agent" | "Task")
}

/// Progress through an approval's questions (Claude Code's AskUserQuestion); `approval_choice`
/// is the highlighted row: an option, or the last row for a typed answer.
#[derive(Debug, Default)]
pub struct Answering {
    /// The question on screen.
    pub index: usize,
    /// Options ticked so far (multi-select questions).
    pub picked: Vec<usize>,
    /// The typed answer.
    pub other: String,
    /// Answers to the questions before this one.
    pub answers: Vec<String>,
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
    pub answering: Answering,
    pub picker: Option<Picker>,
    pub overlay: Tween,
    pub completion: Option<Completion>,
    pub todos: Vec<Todo>,
    /// When every step of the plan got done (it shows briefly, then hides).
    pub plan_done: Option<Instant>,
    /// The plan above the prompt shows every step (else one compact line); click or ctrl+x.
    pub plan_open: bool,
    /// Screen rows of the plan above the prompt, from the last draw (for clicks).
    pub plan_rows: Option<(u16, u16)>,
    /// Background tasks (subagents keep running after their Agent card's call returns).
    pub tasks: Vec<Task>,
    pub usage: Usage,
    pub ctx_shown: Tween,
    pub queue: Vec<UserMsg>,
    pub toast: Option<(String, Instant, NoticeKind)>,
    pub models: Vec<ModelInfo>,
    pub files: Vec<String>,
    /// Skills the backend can run, invoked as `/name args`.
    pub skills: Vec<Skill>,
    pub scroll: usize,
    /// A clicked card's item and the transcript row its header sat on: the next draw keeps it there,
    /// so the card opens downward and pushes what follows past the bottom.
    pub anchor: Option<(usize, usize)>,
    /// How far `scroll` may exceed the transcript's height for a held card (only ever shrinks).
    pub hold: usize,
    /// Text marked with the mouse in the transcript, and where the transcript was last drawn.
    pub selection: Option<Selection>,
    pub view: selection::View,
    /// The settled part of the transcript as last rendered.
    pub tcache: super::view::TranscriptCache,
    /// First transcript line of each item (usize::MAX if it drew nothing), from the last draw.
    pub item_rows: Vec<usize>,
    clicks: Clicks,
    pub expanded: bool,
    pub quit_armed: Option<Instant>,
    pub quit: bool,
    pub saved: Saved,
    system: Option<String>,
    shell_context: Vec<String>,
    shell_seq: u64,
    hooked: bool,
    /// The backend session and where it's kept (transcript or log), for winfer's context breakdown.
    usage_source: Option<(String, String)>,
    /// Whether winfer was last told this pane is working (a turn or background task runs).
    reported_working: bool,
    /// Snapshots the workspace around turns, for /undo (None in tests).
    checkpoints: Option<checkpoint::Worker>,
    /// User turns of this backend session, oldest first.
    pub turns: Vec<Turn>,
    next_turn: u64,
    /// The message waiting for its turn's snapshot before it goes to the backend.
    starting: Option<(u64, UserMsg)>,
    /// An undo was sent to the backend and not yet answered.
    undoing: bool,
}

impl App {
    pub fn new(tx: Sender<Msg>, launch: Launch) -> App {
        let mut app = App::blank(tx, &launch);
        let reply = app.tx.clone();
        app.checkpoints = Some(checkpoint::Worker::start(app.cwd.clone(), checkpoint::store(), session::new_id(), move |d| {
            let _ = reply.send(Msg::Checkpoint(d));
        }));
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
            answering: Answering::default(),
            picker: None,
            overlay: Tween::new(0.0),
            completion: None,
            todos: Vec::new(),
            plan_done: None,
            plan_open: false,
            plan_rows: None,
            tasks: Vec::new(),
            usage: Usage::default(),
            ctx_shown: Tween::new(0.0),
            queue: Vec::new(),
            toast: None,
            models: Vec::new(),
            files: Vec::new(),
            skills: Vec::new(),
            scroll: 0,
            anchor: None,
            hold: 0,
            selection: None,
            view: selection::View::default(),
            tcache: Default::default(),
            item_rows: Vec::new(),
            clicks: Clicks::default(),
            expanded: false,
            quit_armed: None,
            quit: false,
            saved: Saved { id: session::new_id(), cwd: cwd.to_string_lossy().into_owned(), ..Default::default() },
            system: launch.system.clone(),
            shell_context: Vec::new(),
            shell_seq: 0,
            hooked: false,
            usage_source: None,
            reported_working: false,
            checkpoints: None,
            turns: Vec::new(),
            next_turn: 0,
            starting: None,
            undoing: false,
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
        self.skills.clear();
        if let Some(h) = self.handle.take() {
            h.send(Cmd::Shutdown);
            for task in self.tasks.iter_mut().filter(|t| !t.agent && t.status == Status::Running) {
                task.status = Status::Failed;
                task.ended = Some(Instant::now());
            }
            self.sync_shell_cards();
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
            skills: self.config.lean_skills(),
        };
        self.handle = Some(backend::start(self.kind, opts, etx));
        self.alive = true;
        self.busy = false;
        self.approvals.clear();
        self.report_status();
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
        self.working()
            || self.items.is_empty()
            || self.overlay.moving()
            || self.picker.is_some()
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
        self.apply_agent(ev);
        self.report_status();
    }

    /// Whether the agent is doing anything: a turn runs, or a subagent or shell it started in
    /// the background still does (Claude Code ends the turn before those finish).
    pub fn working(&self) -> bool {
        self.busy || self.background_running()
    }

    pub fn background_running(&self) -> bool {
        self.running_subagents() > 0 || self.running_shells() > 0
    }

    /// Tells winfer when the pane starts or stops working, so it isn't "done" while background
    /// tasks still run.
    fn report_status(&mut self) {
        let working = self.working();
        if working == self.reported_working {
            return;
        }
        self.reported_working = working;
        if working {
            // Saved as the turn starts (and after each tool) too, so winfer's phone thread
            // shows the prompt and progress before the turn ends.
            self.save();
            hook("turn_start", json!({ "session_id": self.saved.id }));
        } else {
            let reply = self.items.iter().rev().find_map(|i| match &i.block {
                Block::Assistant { text, .. } => Some(text.clone()),
                _ => None,
            });
            hook("turn_done", json!({ "session_id": self.saved.id, "last_assistant_message": reply.unwrap_or_default() }));
        }
    }

    /// Where the backend keeps this session's conversation: Claude Code's transcript, Codex's
    /// session log, the ChatGPT backend's saved items. Looked up once per backend session.
    fn usage_source(&mut self) -> Option<String> {
        if std::env::var("WINFER_PANE_ID").map_or(true, |p| p.is_empty()) {
            return None;
        }
        let session = self.saved.backend_session.clone()?;
        if let Some((s, path)) = &self.usage_source {
            if *s == session {
                return Some(path.clone());
            }
        }
        let path = match self.kind {
            Kind::Claude => crate::usage::claude_transcript(&session, &self.saved.cwd),
            Kind::Codex => crate::usage::codex_log(Some(&session), &self.saved.cwd).filter(|p| p.to_string_lossy().contains(&session)),
            Kind::ChatGpt => Some(backend::chatgpt::session_path(&session)),
        }?;
        let path = path.to_string_lossy().into_owned();
        self.usage_source = Some((session, path.clone()));
        Some(path)
    }

    /// Sends context fill and spend to winfer for the pane header's meter and cost chip, with
    /// the backend and its session's log so winfer can break the context down and know whose
    /// plan limits apply.
    fn report_usage(&mut self, u: &Usage) {
        if u.window == 0 {
            return;
        }
        let at = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0);
        let source = self.usage_source();
        hook(
            "usage",
            json!({
                "backend": self.kind.id(),
                "context": { "usedPercent": u.context as f64 * 100.0 / u.window as f64, "tokens": u.context, "window": u.window, "at": at, "source": source, "provider": self.kind.id() },
                "spend": { "cost": u.cost, "input": u.input, "output": u.output, "cached": u.cached, "at": at },
            }),
        );
    }

    fn apply_agent(&mut self, ev: Ev) {
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
                if is_subagent(&name) {
                    for task in self.tasks.iter_mut().filter(|t| t.tool_id.as_deref() == Some(id.as_str())) {
                        task.agent = true;
                    }
                }
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
                self.sync_shell_cards();
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
                self.sync_shell_cards();
                self.save();
            }
            Ev::Approval(a) => {
                if self.approvals.is_empty() {
                    self.approval_choice = 0;
                    self.answering = Answering::default();
                    let message = if a.questions.is_empty() { format!("{}: {}", a.tool, a.title) } else { a.title.clone() };
                    hook("needs_input", json!({ "session_id": self.saved.id, "message": message }));
                    bell();
                }
                self.approvals.push_back(a);
                self.scroll = 0;
            }
            Ev::Task(t) => self.on_task(t),
            Ev::Skills(s) => self.skills = s,
            Ev::Todos(t) => {
                let done = !t.is_empty() && t.iter().all(|t| t.status == "completed");
                self.plan_done = if done { self.plan_done.or(Some(Instant::now())) } else { None };
                self.todos = t;
            }
            Ev::Usage(u) => {
                let pct = if u.window > 0 { u.context as f32 / u.window as f32 } else { 0.0 };
                self.ctx_shown.set(pct, 600);
                self.report_usage(&u);
                self.usage = u;
            }
            Ev::Notice(n) => self.notice(n, NoticeKind::Info),
            Ev::Error(e) => self.notice(e, NoticeKind::Error),
            Ev::TurnStart => {
                if !self.busy {
                    self.busy = true;
                    self.turn_started = Instant::now();
                    self.verb = VERBS[rand::random::<u32>() as usize % VERBS.len()];
                }
            }
            Ev::TurnDone => self.turn_done(),
            Ev::Undone(result) => self.undone(result),
            Ev::Exited(reason) => {
                self.alive = false;
                self.busy = false;
                self.approvals.clear();
                for task in &mut self.tasks {
                    if task.status == Status::Running {
                        task.status = Status::Failed;
                        task.ended = Some(Instant::now());
                    }
                }
                self.sync_shell_cards();
                self.finish_running();
                self.notice(format!("{reason}\nUse /model to pick another backend, or /new to retry."), NoticeKind::Error);
            }
        }
    }

    fn finish_running(&mut self) {
        self.end_reasoning();
        let background: Vec<_> = self.tasks.iter().filter(|t| t.status == Status::Running).filter_map(|t| t.tool_id.as_deref()).collect();
        for item in self.items.iter_mut().rev().take(50) {
            if let Block::Tool { id, status, ended, .. } = &mut item.block {
                if *status == Status::Running && !background.contains(&id.as_str()) {
                    *status = Status::Failed;
                    *ended = Some(Instant::now());
                    item.touch();
                }
            }
        }
    }

    fn turn_done(&mut self) {
        // The workspace as the turn left it (a later turn the backend starts on its own, e.g. on a
        // background task's report, counts as part of the last user turn).
        if let (Some(w), Some(t)) = (&self.checkpoints, self.turns.iter().rev().find(|t| t.sent && t.checkpoint)) {
            w.send(Job::End(t.id));
        }
        let was_busy = self.busy;
        self.busy = false;
        self.approvals.clear();
        self.finish_running();
        self.save();
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
                Block::Tool { id, name, title, status, .. } => {
                    // A background call returns at once; its task says how it really is.
                    let task = self.tasks.iter().find(|t| t.tool_id.as_deref() == Some(id.as_str()));
                    let state = match task.map_or(*status, |t| t.status) {
                        Status::Running => "running",
                        Status::Ok => "done",
                        Status::Failed => "failed",
                    };
                    let background = item.background || task.is_some();
                    transcript.push(Entry { role: "tool".into(), text: format!("{name}\t{title}"), state: Some(state.into()), background });
                    continue;
                }
                Block::Notice { text, kind: NoticeKind::Info } => ("notice", text.clone()),
                _ => continue,
            };
            transcript.push(Entry { role: role.into(), text, ..Default::default() });
        }
        // (A session whose only turn was undone is still saved, emptied.)
        if transcript.iter().all(|e| e.role != "user") && self.saved.transcript.is_empty() {
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
        self.forget_turns();
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
        let skill = self.skill_call(trimmed);
        if skill.is_none() && trimmed.starts_with('/') && !trimmed.starts_with("//") && !trimmed.contains('\n') {
            self.command(trimmed);
            return;
        }
        if let Some(cmd) = trimmed.strip_prefix('!') {
            self.shell(cmd.trim().to_string());
            return;
        }
        let text = match &skill {
            Some((_, args)) => args.clone(),
            None => text,
        };
        let mut full = text.clone();
        let attached = self.mentions(&text, &mut images);
        if !attached.is_empty() {
            full.push_str(&attached);
        }
        if !self.shell_context.is_empty() {
            full = format!("{}\n\n{full}", std::mem::take(&mut self.shell_context).join("\n\n"));
        }
        self.items.push(Item::new(Block::User { text: shown.trim_end().to_string() }));
        self.turns.push(Turn { id: self.next_turn, item: self.items.len() - 1, sent: false, checkpoint: false });
        self.next_turn += 1;
        if self.saved.title.is_empty() {
            self.saved.title = shown.lines().next().unwrap_or("").chars().take(80).collect();
        }
        self.scroll = 0;
        let msg = UserMsg { text: full, images, skill: skill.map(|(name, _)| name) };
        if self.busy {
            self.queue.push(msg);
        } else {
            self.dispatch(msg);
        }
    }

    /// `/name args` → (name, args) when `name` is one of the backend's skills and not a built-in
    /// command (those win).
    fn skill_call(&self, line: &str) -> Option<(String, String)> {
        let rest = line.strip_prefix('/')?;
        let (name, args) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
        if COMMANDS.iter().any(|(c, _)| c[1..] == *name) || !self.skills.iter().any(|s| s.name == name) {
            return None;
        }
        Some((name.to_string(), args.trim().to_string()))
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
            // The new backend doesn't know the old one's turns.
            self.turns.retain(|t| !t.sent);
            self.start_backend(self.saved.backend_session.clone());
        }
        self.busy = true;
        self.turn_started = Instant::now();
        self.verb = VERBS[rand::random::<u32>() as usize % VERBS.len()];
        // Snapshot the workspace first, so /undo can put back whatever the turn changes.
        match (&self.checkpoints, self.turns.iter().find(|t| !t.sent).map(|t| t.id)) {
            (Some(w), Some(id)) => {
                w.send(Job::Begin(id));
                self.starting = Some((id, msg));
            }
            (_, id) => {
                if let Some(t) = self.turns.iter_mut().find(|t| Some(t.id) == id) {
                    t.sent = true;
                }
                self.send(Cmd::Send(msg));
            }
        }
    }

    /// The checkpoint thread's answers.
    pub fn on_checkpoint(&mut self, done: Done) {
        match done {
            Done::Began(id, ok) => {
                let Some((_, msg)) = self.starting.take_if(|(s, _)| *s == id) else { return };
                if let Some(t) = self.turns.iter_mut().find(|t| t.id == id) {
                    t.sent = true;
                    t.checkpoint = ok;
                }
                self.send(Cmd::Send(msg));
            }
            Done::Restored(Ok(paths)) => self.notice(undone_text(&paths), NoticeKind::Success),
            Done::Restored(Err(e)) => self.notice(format!("Undid the last turn, but couldn't restore its files: {e}"), NoticeKind::Error),
        }
    }

    /// A fresh conversation: nothing to undo.
    fn forget_turns(&mut self) {
        self.turns.clear();
        self.starting = None;
        self.undoing = false;
        if let Some(w) = &self.checkpoints {
            w.send(Job::Clear);
        }
    }

    /// /undo: drops the last turn from the conversation (the backend first, then the
    /// transcript) and puts back the files it changed.
    fn undo(&mut self) {
        if self.busy || self.starting.is_some() {
            self.toast("Wait for the turn to finish (or press Esc) before /undo.", NoticeKind::Error);
        } else if self.undoing {
            self.toast("Already undoing…", NoticeKind::Info);
        } else if !self.alive {
            self.toast("The agent isn't running; nothing to undo.", NoticeKind::Error);
        } else if let Some(turn) = self.turns.last() {
            self.undoing = true;
            // Without a workspace snapshot, the backend reverts what file changes it tracked.
            self.send(Cmd::Undo { files: !turn.checkpoint });
        } else {
            self.toast("Nothing to undo in this session.", NoticeKind::Info);
        }
    }

    /// The backend's answer to /undo.
    fn undone(&mut self, result: Result<Vec<String>, String>) {
        self.undoing = false;
        let files = match result {
            Ok(files) => files,
            Err(e) => return self.notice(format!("Couldn't undo: {e}"), NoticeKind::Error),
        };
        let Some(turn) = self.turns.pop() else { return };
        // The transcript goes back to before the message, which returns to the prompt to edit.
        if let Some(Block::User { text }) = self.items.get(turn.item).map(|i| &i.block) {
            let text = text.clone();
            self.items.truncate(turn.item);
            if self.composer.text.trim().is_empty() {
                self.composer.set(&text);
            }
        }
        self.todos.clear();
        self.approvals.clear();
        self.scroll = 0;
        self.selection = None;
        match &self.checkpoints {
            Some(w) if turn.checkpoint => w.send(Job::Restore(turn.id)),
            _ => self.notice(undone_text(&files), NoticeKind::Success),
        }
        self.save();
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
            name: tools::shell_display().into(),
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
            "/undo" | "/revert" => self.undo(),
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
            "/sidebar" => {
                let on = match arg {
                    "on" => true,
                    "off" => false,
                    _ => !self.config.plan_sidebar,
                };
                self.config.plan_sidebar = on;
                config::Config::update(|c| c.plan_sidebar = on);
                self.toast(if on { "Plan sidebar on (shows when the pane is wide enough)" } else { "Plan sidebar off" }, NoticeKind::Success);
            }
            "/agents" | "/subagents" | "/tasks" => self.open_agents(),
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
        let mut s = String::from("Keys\n  enter send · shift/alt+enter or ctrl+j newline · esc interrupt · ctrl+c clear/quit\n  shift+tab permission mode · ctrl+o expand details · ctrl+p command palette · ctrl+t model · ctrl+r effort · ctrl+g subagents (ctrl+x there stops one) · ctrl+x plan (or click it)\n  ↑/↓ history · pgup/pgdn or wheel scroll · @ attach file · ctrl+v paste image · ! run shell command\n  drag to select and copy · double-click a word · triple-click a line · shift+drag for the terminal's own selection\n\nCommands\n");
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

    /// Subagents of this conversation (Claude Code Task/Agent calls, Codex collab agents), read
    /// off their tool cards: running ones first, oldest first, then finished ones, newest first.
    pub fn subagents(&self) -> Vec<Subagent<'_>> {
        let mut out: Vec<Subagent> = self
            .items
            .iter()
            .enumerate()
            .filter_map(|(item, i)| match &i.block {
                Block::Tool { id, name, title, output, status, started, ended, .. } if is_subagent(name) && !id.is_empty() => {
                    // A background subagent's call returns at once; its task says how it really is.
                    let task = self.tasks.iter().find(|t| t.tool_id.as_deref() == Some(id.as_str()));
                    let call = *status;
                    let status = task.map_or(call, |t| t.status);
                    let running = status == Status::Running;
                    let ended = if running { None } else { task.and_then(|t| t.ended).or(*ended) };
                    // While its call runs (a foreground agent), Claude Code's bridge streams one
                    // line per tool the subagent calls; a backgrounded call's output is just a receipt.
                    let steps: Vec<&str> = if running && call == Status::Running { output.lines().filter(|l| !l.trim().is_empty()).collect() } else { Vec::new() };
                    let last = match task.filter(|t| !t.last.is_empty()) {
                        Some(t) => t.last.as_str(),
                        None => steps.last().copied().unwrap_or(""),
                    };
                    Some(Subagent { id, title, status, started: *started, ended, steps: steps.len().max(task.map_or(0, |t| t.tools)), last, item })
                }
                _ => None,
            })
            .collect();
        // Subagent tasks with no card (e.g. started before this session was resumed).
        for t in &self.tasks {
            let carded = t.tool_id.as_deref().is_some_and(|id| out.iter().any(|a| a.id == id));
            if t.agent && !carded {
                out.push(Subagent { id: &t.id, title: &t.title, status: t.status, started: t.started, ended: t.ended, steps: t.tools, last: &t.last, item: usize::MAX });
            }
        }
        out.sort_by_key(|a| (a.status != Status::Running, if a.status == Status::Running { a.item as i64 } else { -(a.item as i64) }));
        out
    }

    pub fn running_subagents(&self) -> usize {
        self.subagents().iter().filter(|a| a.status == Status::Running).count()
    }

    #[cfg(test)]
    pub fn on_agent_task(&mut self, t: TaskEv) {
        self.on_task(t);
    }

    fn on_task(&mut self, t: TaskEv) {
        let i = match self.tasks.iter().position(|x| x.id == t.id) {
            Some(i) => i,
            None => {
                self.tasks.push(Task {
                    id: t.id.clone(),
                    tool_id: None,
                    title: String::new(),
                    agent: false,
                    status: Status::Running,
                    started: Instant::now(),
                    ended: None,
                    tools: 0,
                    last: String::new(),
                    output_file: None,
                    exit_code: None,
                    polled: None,
                });
                self.tasks.len() - 1
            }
        };
        let task = &mut self.tasks[i];
        if t.tool_id.is_some() {
            task.tool_id = t.tool_id;
        }
        if let Some(title) = t.title {
            task.title = title;
        }
        task.agent |= t.agent.is_some();
        task.agent |= task.tool_id.as_deref().is_some_and(|id| self.items.iter().any(|i| matches!(&i.block, Block::Tool { id: card, name, .. } if card == id && is_subagent(name))));
        if let Some(path) = t.output_file {
            task.output_file = Some(path.into());
        }
        if t.exit_code.is_some() {
            task.exit_code = t.exit_code;
        }
        if let Some(n) = t.tools {
            task.tools = n as usize;
        }
        if let Some(l) = t.last {
            task.last = l;
        }
        match t.status.as_deref() {
            Some("running" | "pending") if task.ended.is_none() => {
                task.status = Status::Running;
                task.ended = None;
            }
            Some("completed") => {
                task.status = Status::Ok;
                task.ended.get_or_insert_with(Instant::now);
            }
            Some("running" | "pending") => {}
            Some(_) => {
                task.status = Status::Failed;
                task.ended.get_or_insert_with(Instant::now);
            }
            None => {}
        }
        self.sync_shell_cards();
        self.poll_shell_output();
        // Background work starting or ending shows on winfer's phone thread.
        if t.status.is_some() && !self.saved.transcript.is_empty() {
            self.save();
        }
    }

    pub fn running_shells(&self) -> usize {
        self.tasks.iter().filter(|t| !t.agent && t.status == Status::Running).count()
    }

    fn sync_shell_cards(&mut self) {
        for task in self.tasks.iter().filter(|t| !t.agent) {
            if let Some(item) = self.items.iter_mut().find(|i| matches!(&i.block, Block::Tool { id, name, .. } if task.tool_id.as_deref() == Some(id) && matches!(super::transcript::tool_kind(name).1, super::transcript::ToolKind::Shell))) {
                if let Block::Tool { status, ended, title, .. } = &mut item.block {
                    if !item.background || *status != task.status || *ended != task.ended || item.exit_code != task.exit_code {
                        *status = task.status;
                        *ended = task.ended;
                        if !task.title.is_empty() {
                            *title = task.title.clone();
                        }
                        item.background = true;
                        item.exit_code = task.exit_code;
                        item.touch();
                    }
                }
            }
        }
    }

    /// Read a bounded tail at most once a second, including one final read on completion.
    pub fn poll_shell_output(&mut self) {
        use std::io::{Read, Seek, SeekFrom};
        for task in self.tasks.iter_mut().filter(|t| !t.agent && t.output_file.is_some()) {
            if task.polled.is_some_and(|at| if task.status == Status::Running { at.elapsed() < Duration::from_secs(1) } else { task.ended.is_none_or(|end| at >= end) }) {
                continue;
            }
            task.polled = Some(Instant::now());
            let Some(item) = self.items.iter_mut().find(|i| matches!(&i.block, Block::Tool { id, .. } if task.tool_id.as_deref() == Some(id))) else { continue };
            let read = || -> std::io::Result<String> {
                let path = task.output_file.as_ref().unwrap();
                if !std::fs::metadata(path)?.is_file() {
                    return Err(std::io::Error::other("not a regular file"));
                }
                let mut file = std::fs::File::open(path)?;
                let len = file.metadata()?.len();
                let offset = len.saturating_sub(16_384);
                file.seek(SeekFrom::Start(offset))?;
                let mut bytes = Vec::new();
                file.take(16_384).read_to_end(&mut bytes)?;
                let text = String::from_utf8_lossy(&bytes);
                Ok(if offset > 0 { text.split_once('\n').map_or(text.as_ref(), |(_, tail)| tail) } else { &text }.to_string())
            };
            if let Ok(text) = read() {
                if let Block::Tool { output, .. } = &mut item.block {
                    if *output != text {
                        *output = text;
                        item.touch();
                    }
                }
            }
        }
    }

    fn agent_items(&self) -> Vec<PickItem> {
        let now = Instant::now();
        self.subagents()
            .into_iter()
            .take(60)
            .map(|a| {
                let took = super::anim::elapsed(a.ended.unwrap_or(now).duration_since(a.started));
                let detail = match a.status {
                    Status::Running if a.steps > 0 => format!("{took} · {} tool{} · {}", a.steps, if a.steps == 1 { "" } else { "s" }, a.last),
                    Status::Running => format!("{took} · starting"),
                    Status::Ok => format!("done in {took}"),
                    Status::Failed => format!("stopped after {took}"),
                };
                let label = if a.title.trim().is_empty() { "(subagent)".to_string() } else { a.title.to_string() };
                PickItem::new(label, detail, a.id).group(if a.status == Status::Running { "Running" } else { "Finished" })
            })
            .collect()
    }

    pub fn open_agents(&mut self) {
        let items = self.agent_items();
        if items.is_empty() {
            self.toast("No subagents in this conversation yet.", NoticeKind::Info);
            return;
        }
        self.open(Picker::new(PickKind::Agents, "Subagents", items));
    }

    /// Stops the running subagent picked in `/agents` (by its card id or task id).
    fn stop_agent(&mut self, value: &str) {
        let task = self.tasks.iter().find(|t| t.id == value || t.tool_id.as_deref() == Some(value));
        match task {
            Some(t) if t.status == Status::Running => {
                let id = t.id.clone();
                self.send(Cmd::StopTask(id));
                self.toast("Stopping subagent…", NoticeKind::Info);
            }
            Some(_) => self.toast("That subagent already finished.", NoticeKind::Info),
            // A foreground agent with no task: only interrupting the turn stops it.
            None => self.toast("Can't stop that one on its own; esc interrupts the turn.", NoticeKind::Info),
        }
    }

    /// Keeps an open subagent list live: new agents, progress and elapsed times.
    pub fn refresh_agents(&mut self) {
        if !self.picker.as_ref().is_some_and(|p| p.kind == PickKind::Agents) {
            return;
        }
        let items = self.agent_items();
        let Some(p) = &mut self.picker else { return };
        let chosen = p.chosen().map(|i| i.value.clone());
        p.items = items;
        let visible = p.visible();
        p.selected = chosen.and_then(|c| visible.iter().position(|&i| p.items[i].value == c)).unwrap_or(0).min(visible.len().saturating_sub(1));
    }

    /// Scrolls the transcript so the tool card with this id is at the top.
    fn jump_to_tool(&mut self, id: &str) {
        let Some(idx) = self.items.iter().position(|i| matches!(&i.block, Block::Tool { id: t, .. } if t == id)) else { return };
        let Some(&row) = self.item_rows.get(idx).filter(|r| **r != usize::MAX) else { return };
        let h = self.view.bottom.saturating_sub(self.view.top) as usize;
        self.scroll = self.view.lines.len().saturating_sub(row + h);
        self.selection = None;
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
            PickKind::Agents => self.jump_to_tool(&item.value),
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
            self.forget_turns();
            self.start_backend(None);
            if fresh {
                self.notice(format!("Switched to {label}. It starts with a fresh context."), NoticeKind::Info);
            } else {
                self.toast(format!("Model: {label}"), NoticeKind::Success);
            }
        }
        self.saved.model = self.spec();
    }

    /// The effort levels ctrl+r steps through for this backend ("" = the model's default).
    fn effort_levels(&self) -> &'static [&'static str] {
        match self.kind {
            Kind::Claude => &["", "low", "medium", "high", "xhigh", "max"],
            _ => &["", "minimal", "low", "medium", "high", "xhigh"],
        }
    }

    /// ctrl+r: next effort level, applied to the running session.
    fn cycle_effort(&mut self) {
        let levels = self.effort_levels();
        let cur = self.config.effort.as_deref().unwrap_or("");
        let next = levels.iter().position(|l| *l == cur).map_or(0, |i| (i + 1) % levels.len());
        self.set_effort(levels[next]);
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
        self.tasks.clear();
        self.queue.clear();
        self.usage = Usage::default();
        self.ctx_shown = Tween::new(0.0);
        self.saved = Saved { id: session::new_id(), cwd: self.cwd.to_string_lossy().into_owned(), model: self.spec(), ..Default::default() };
        self.hooked = false;
        self.forget_turns();
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
                let tools = serde_json::to_string(&crate::harness::backend::chatgpt::tool_definitions()).map(|t| t.len()).unwrap_or(0);
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
                } else if let Some(n) = self.approvals.front().and_then(|a| a.questions.get(self.answering.index)).map(|q| q.options.len()) {
                    // A pasted answer goes to the typed-answer row.
                    self.approval_choice = n;
                    self.answering.other.push_str(&s.split_whitespace().collect::<Vec<_>>().join(" "));
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
                if self.picker.is_none() && self.plan_rows.is_some_and(|(top, bottom)| m.row >= top && m.row < bottom) {
                    self.plan_open = !self.plan_open;
                    self.selection = None;
                    return;
                }
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
                    // A plain click clears the mark; on a tool card's header it opens or collapses it.
                    let line = sel.head.line;
                    self.selection = None;
                    self.toggle_card_at(line);
                }
            }
            _ => {}
        }
    }

    /// Opens or collapses the tool card whose header is this transcript line, like the plan.
    fn toggle_card_at(&mut self, line: usize) {
        let Some(idx) = self.item_rows.iter().position(|&r| r == line) else { return };
        let Some(item) = self.items.get_mut(idx).filter(|i| matches!(i.block, Block::Tool { .. })) else { return };
        item.open = !item.open;
        item.touch();
        let row = (self.view.y - self.view.top) as usize + line.saturating_sub(self.view.start);
        self.anchor = Some((idx, row));
    }

    /// Keys while the agent's questions are on screen: ↑↓ pick a row, space ticks an option of a
    /// multi-select question, digits pick an option, typing goes to the last row, enter answers
    /// and moves on (the last answer sends them all), esc declines.
    fn on_question_key(&mut self, a: &Approval, code: KeyCode, ctrl: bool) {
        let q = &a.questions[self.answering.index.min(a.questions.len() - 1)];
        let n = q.options.len();
        let typing = self.approval_choice >= n;
        let toggle = |app: &mut App, i: usize| match app.answering.picked.iter().position(|&p| p == i) {
            Some(at) => {
                app.answering.picked.remove(at);
            }
            None => app.answering.picked.push(i),
        };
        let mut answer = None;
        match code {
            KeyCode::Esc => {
                self.approvals.pop_front();
                self.approval_choice = 0;
                self.answering = Answering::default();
                self.send(Cmd::Approve(a.id.clone(), Decision::Deny));
                self.toast("Declined", NoticeKind::Info);
                hook("approved", json!({ "session_id": self.saved.id }));
                return;
            }
            KeyCode::Up | KeyCode::BackTab => self.approval_choice = (self.approval_choice + n) % (n + 1),
            KeyCode::Down | KeyCode::Tab => self.approval_choice = (self.approval_choice + 1) % (n + 1),
            KeyCode::PageUp => self.scroll += 10,
            KeyCode::PageDown => self.scroll = self.scroll.saturating_sub(10),
            KeyCode::Backspace if typing => {
                self.answering.other.pop();
            }
            KeyCode::Char(c) if typing && !ctrl => self.answering.other.push(c),
            KeyCode::Char(' ') if q.multi => toggle(self, self.approval_choice),
            KeyCode::Char(c @ '1'..='9') if !ctrl && (c as usize - '1' as usize) < n => {
                let i = c as usize - '1' as usize;
                self.approval_choice = i;
                if q.multi {
                    toggle(self, i);
                } else {
                    answer = Some(q.options[i].0.clone());
                }
            }
            KeyCode::Enter => {
                let other = self.answering.other.trim().to_string();
                answer = if q.multi {
                    let mut picked = self.answering.picked.clone();
                    if picked.is_empty() && !typing {
                        picked.push(self.approval_choice);
                    }
                    picked.sort_unstable();
                    let mut parts: Vec<String> = picked.iter().map(|&i| q.options[i].0.clone()).collect();
                    if !other.is_empty() {
                        parts.push(other);
                    }
                    Some(parts.join(", "))
                } else if typing {
                    Some(other)
                } else {
                    Some(q.options[self.approval_choice].0.clone())
                };
            }
            _ => {}
        }
        let Some(answer) = answer.filter(|a| !a.is_empty()) else { return };
        let done = &mut self.answering;
        done.answers.push(answer);
        done.index += 1;
        done.picked.clear();
        done.other.clear();
        self.approval_choice = 0;
        if self.answering.index >= a.questions.len() {
            let answers = std::mem::take(&mut self.answering).answers;
            self.approvals.pop_front();
            self.send(Cmd::Answer(a.id.clone(), answers));
            hook("approved", json!({ "session_id": self.saved.id }));
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
            } else if self.working() {
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
                KeyCode::Char('x') if ctrl && p.kind == PickKind::Agents => {
                    if let Some(value) = p.chosen().map(|i| i.value.clone()) {
                        self.stop_agent(&value);
                    }
                }
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

        if let Some(a) = self.approvals.front().filter(|a| !a.questions.is_empty()).cloned() {
            self.on_question_key(&a, k.code, ctrl);
            return;
        }

        if let Some(a) = self.approvals.front().cloned() {
            let decide = |app: &mut App, d: Decision| {
                app.approvals.pop_front();
                app.approval_choice = 0;
                app.answering = Answering::default();
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
                if self.working() {
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
            KeyCode::Char('r') if ctrl => self.cycle_effort(),
            KeyCode::Char('g') if ctrl => self.open_agents(),
            KeyCode::Char('x') if ctrl => self.plan_open = !self.plan_open,
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

    /// Stops all work: the running turn and every background subagent and shell.
    fn interrupt(&mut self) {
        if self.starting.take().is_some() {
            // The backend never got the message.
            self.busy = false;
        } else if self.busy {
            self.send(Cmd::Interrupt);
        }
        self.turns.retain(|t| t.sent);
        let running: Vec<String> = self.tasks.iter().filter(|t| t.status == Status::Running).map(|t| t.id.clone()).collect();
        for id in &running {
            self.send(Cmd::StopTask(id.clone()));
        }
        self.queue.clear();
        self.approvals.clear();
        let what = match (self.busy, running.len()) {
            (_, 0) => "Interrupted".to_string(),
            (true, n) => format!("Interrupted · stopping {n} background task{}", if n == 1 { "" } else { "s" }),
            (false, n) => format!("Stopping {n} background task{}", if n == 1 { "" } else { "s" }),
        };
        self.notice(what, NoticeKind::Error);
        hook("interrupted", json!({ "session_id": self.saved.id }));
        self.reported_working = false;
    }

    fn update_completion(&mut self) {
        let text = &self.composer.text;
        if text.starts_with('/') && !text.contains(' ') && !text.contains('\n') {
            let q = &text[1..];
            let mut items: Vec<(i64, PickItem)> = COMMANDS
                .iter()
                .filter_map(|(c, d)| super::picker::score(q, &c[1..]).map(|s| (s, PickItem::new(*c, *d, *c))))
                .chain(self.skills.iter().filter_map(|sk| {
                    let c = format!("/{}", sk.name);
                    let d: String = sk.description.lines().next().unwrap_or("").chars().take(60).collect();
                    super::picker::score(q, &sk.name).map(|s| (s - 1, PickItem::new(c.clone(), format!("skill · {d}"), c)))
                }))
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

/// What /undo did, for the transcript.
fn undone_text(paths: &[String]) -> String {
    match paths.len() {
        0 => "Undid the last turn (no file changes to revert).".into(),
        n if n <= 8 => format!("Undid the last turn · reverted {}", paths.join(", ")),
        n => format!("Undid the last turn · reverted {} and {} more", paths[..6].join(", "), n - 6),
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
        Msg::Checkpoint(d) => app.on_checkpoint(d),
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
    fn undo_drops_the_last_turn_and_returns_its_message() {
        let mut a = app();
        a.alive = true;
        let g = a.generation;
        let turn = |a: &mut App, text: &str, reply: &str| {
            a.composer.set(text);
            a.submit();
            a.on_agent(g, Ev::TurnStart);
            a.on_agent(g, Ev::Text { id: format!("m-{text}"), text: reply.into() });
            a.on_agent(g, Ev::TurnDone);
        };
        turn(&mut a, "first", "one");
        turn(&mut a, "second", "two");
        assert_eq!(a.turns.len(), 2);
        assert!(a.turns.iter().all(|t| t.sent && !t.checkpoint));
        let users = |a: &App| a.items.iter().filter(|i| matches!(i.block, Block::User { .. })).count();

        // Busy: refused.
        a.busy = true;
        a.command("/undo");
        assert!(!a.undoing);
        a.busy = false;

        a.command("/undo");
        assert!(a.undoing);
        // The backend couldn't: the transcript stays.
        a.on_agent(g, Ev::Undone(Err("nope".into())));
        assert!(!a.undoing && users(&a) == 2 && a.turns.len() == 2);

        a.command("/undo");
        a.on_agent(g, Ev::Undone(Ok(vec!["a.txt".into()])));
        assert_eq!(users(&a), 1);
        assert_eq!(a.turns.len(), 1);
        assert_eq!(a.composer.text, "second");
        assert!(!a.items.iter().any(|i| matches!(&i.block, Block::Assistant { text, .. } if text == "two")));
        assert!(matches!(&a.items.last().unwrap().block, Block::Notice { text, .. } if text.contains("reverted a.txt")));

        // A message typed meanwhile isn't overwritten.
        a.composer.set("draft");
        a.command("/undo");
        a.on_agent(g, Ev::Undone(Ok(Vec::new())));
        assert_eq!((users(&a), a.composer.text.as_str()), (0, "draft"));
        a.command("/undo");
        assert!(!a.undoing);
    }

    #[test]
    fn a_turn_waits_for_its_snapshot_and_esc_cancels_it() {
        let mut a = app();
        a.alive = true;
        let (tx, rx) = channel();
        a.checkpoints = Some(checkpoint::Worker::start(PathBuf::from("/nonexistent-winfer-dir"), std::env::temp_dir(), "t".into(), move |d| {
            let _ = tx.send(d);
        }));
        a.composer.set("go");
        a.submit();
        assert!(a.busy && a.starting.is_some() && !a.turns[0].sent);
        // No usable folder: the turn starts without a checkpoint.
        let done = rx.recv_timeout(Duration::from_secs(10)).unwrap();
        a.on_checkpoint(done);
        assert!(a.starting.is_none() && a.turns[0].sent && !a.turns[0].checkpoint);

        a.busy = false;
        a.composer.set("again");
        a.submit();
        assert!(a.starting.is_some());
        a.interrupt();
        assert!(!a.busy && a.starting.is_none() && a.turns.len() == 1);
        // Its late answer is ignored.
        a.on_checkpoint(rx.recv_timeout(Duration::from_secs(10)).unwrap());
        assert_eq!(a.turns.len(), 1);
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
    fn ctrl_r_cycles_effort() {
        let mut a = app();
        let ctrl_r = |a: &mut App| a.on_term(Event::Key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL)));
        ctrl_r(&mut a);
        assert_eq!(a.config.effort.as_deref(), Some("low"));
        for _ in 0..4 {
            ctrl_r(&mut a);
        }
        assert_eq!(a.config.effort.as_deref(), Some("max"));
        ctrl_r(&mut a);
        assert_eq!(a.config.effort, None);
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
    fn clicking_a_tool_card_opens_and_collapses_it() {
        let mut a = app();
        let now = Instant::now();
        let diff = crate::harness::diff::unified("a.txt", "a\nb\nc\nd\ne\n", "a\nB\nc\nD\nE\n");
        a.items.push(Item::new(Block::Tool { id: "1".into(), name: "Edit".into(), title: "a.txt".into(), output: String::new(), diff: Some(diff), status: Status::Ok, started: now, ended: Some(now) }));
        a.items[0].born = now - Duration::from_secs(5);
        let s = screen(&mut a, 100, 30);
        assert!(s.contains("▸") && s.contains("+ B") && !s.contains("+ E"), "{s}");
        let click = |a: &mut App| {
            let row = a.view.y + (a.item_rows[0] - a.view.start) as u16;
            for kind in [MouseEventKind::Down(MouseButton::Left), MouseEventKind::Up(MouseButton::Left)] {
                a.on_term(Event::Mouse(MouseEvent { kind, column: 10, row, modifiers: KeyModifiers::NONE }));
            }
        };
        click(&mut a);
        let s = screen(&mut a, 100, 30);
        assert!(s.contains("▾") && s.contains("+ E") && s.contains("  c"), "{s}");
        click(&mut a);
        let s = screen(&mut a, 100, 30);
        assert!(s.contains("▸") && !s.contains("+ E"), "{s}");
    }

    #[test]
    fn an_opened_tool_card_keeps_its_header_in_place_and_pushes_the_rest_down() {
        // Short transcripts (bottom-aligned) and long ones (following the bottom) alike.
        for before in [0, 40] {
            let mut a = app();
            let now = Instant::now();
            for i in 0..before {
                a.items.push(Item::new(Block::User { text: format!("message {i}") }));
            }
            let output: String = (1..=8).map(|i| format!("line {i}\n")).collect();
            a.items.push(Item::new(Block::Tool { id: "1".into(), name: "Bash".into(), title: "seq 8".into(), output, diff: None, status: Status::Ok, started: now, ended: Some(now) }));
            a.items.push(Item::new(Block::Assistant { id: "a".into(), text: "after the card\n\none\n\ntwo\n\nthree\n\nfour".into() }));
            for i in &mut a.items {
                i.born = now - Duration::from_secs(5);
            }
            let card = before;
            let header = |a: &mut App| {
                screen(a, 100, 30);
                a.view.y + (a.item_rows[card] - a.view.start) as u16
            };
            let row = header(&mut a);
            // Different columns each time, so the clicks don't count as a double-click.
            let click = |a: &mut App, row: u16, column: u16| {
                for kind in [MouseEventKind::Down(MouseButton::Left), MouseEventKind::Up(MouseButton::Left)] {
                    a.on_term(Event::Mouse(MouseEvent { kind, column, row, modifiers: KeyModifiers::NONE }));
                }
            };
            click(&mut a, row, 10);
            assert!(a.items[card].open);
            assert_eq!(header(&mut a), row, "opening moved the header (before={before})");
            let s = screen(&mut a, 100, 30);
            assert!(s.contains("line 8") && !s.contains("four") && a.scroll > 0, "{s}");
            click(&mut a, row, 30);
            assert!(!a.items[card].open);
            assert_eq!(header(&mut a), row, "collapsing moved the header (before={before})");
            let s = screen(&mut a, 100, 30);
            assert!(s.contains("four") && a.scroll == 0, "{s}");
        }
    }

    #[test]
    fn clicking_the_plan_opens_and_collapses_it() {
        let mut a = app();
        a.items.push(Item::new(Block::User { text: "go".into() }));
        a.busy = true;
        a.todos = vec![
            Todo { content: "Read the code".into(), status: "completed".into() },
            Todo { content: "Fix the pager".into(), status: "in_progress".into() },
            Todo { content: "Write a test".into(), status: "pending".into() },
        ];
        let s = screen(&mut a, 100, 30);
        assert!(s.contains("▸ plan") && !s.contains("Write a test"), "{s}");
        let (top, _) = a.plan_rows.expect("plan drawn");
        let click = |a: &mut App| a.on_term(Event::Mouse(MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: 5, row: top, modifiers: KeyModifiers::NONE }));
        click(&mut a);
        let s = screen(&mut a, 100, 30);
        assert!(s.contains("▾ plan") && s.contains("Read the code") && s.contains("Write a test"), "{s}");
        let (top, _) = a.plan_rows.unwrap();
        let click = |a: &mut App| a.on_term(Event::Mouse(MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: 5, row: top, modifiers: KeyModifiers::NONE }));
        click(&mut a);
        assert!(!a.plan_open);
        // ctrl+x does the same.
        a.on_term(Event::Key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL)));
        assert!(a.plan_open);
    }

    #[test]
    fn finished_plan_clears_away() {
        let mut a = app();
        let g = a.generation;
        a.items.push(Item::new(Block::User { text: "go".into() }));
        a.busy = true;
        let todo = |s: &str| Todo { content: "Ship it".into(), status: s.into() };
        a.on_agent(g, Ev::Todos(vec![todo("in_progress")]));
        assert!(screen(&mut a, 100, 30).contains("Ship it"));
        a.on_agent(g, Ev::Todos(vec![todo("completed")]));
        // Still up for a moment, even while the agent keeps working…
        let s = screen(&mut a, 100, 30);
        assert!(s.contains("1/1") && s.contains("all done"), "{s}");
        // …then gone.
        a.plan_done = Some(Instant::now() - Duration::from_secs(10));
        assert!(!screen(&mut a, 100, 30).contains("1/1"));
    }

    #[test]
    fn plan_goes_to_a_sidebar_when_wide() {
        let mut a = app();
        a.items.push(Item::new(Block::User { text: "go".into() }));
        a.busy = true;
        a.todos = vec![
            Todo { content: "Read the code".into(), status: "completed".into() },
            Todo { content: "Fix the off-by-one in the pager".into(), status: "in_progress".into() },
        ];
        a.config.plan_sidebar = true;
        let wide = screen(&mut a, 140, 30);
        let row = |s: &str, needle: &str| s.lines().find(|l| l.contains(needle)).map(|l| l.find(needle).unwrap()).unwrap_or(0);
        // On the right, next to the conversation.
        assert!(row(&wide, "Fix the off-by-one") > 90, "{wide}");
        assert!(wide.contains("plan") && wide.contains("1/2"), "{wide}");
        // Narrow: above the prompt instead.
        let narrow = screen(&mut a, 100, 30);
        // Narrow: one compact line above the prompt (the finished step isn't listed).
        assert!(narrow.lines().any(|l| l.contains("plan") && l.contains("1/2") && l.contains("Fix the off-by-one")), "{narrow}");
        assert!(!narrow.contains("Read the code"), "{narrow}");
        // Off: never on the side.
        a.config.plan_sidebar = false;
        let off = screen(&mut a, 140, 30);
        assert!(off.lines().any(|l| l.contains("1/2") && l.contains("Fix the off-by-one")), "{off}");
    }

    #[test]
    fn background_shell_output_is_bounded_and_missing_files_are_safe() {
        let mut a = app();
        let g = a.generation;
        let path = std::env::temp_dir().join(format!("winfer-bg-{}.output", session::new_id()));
        a.on_agent(g, Ev::ToolStart { id: "bash1".into(), name: "Bash".into(), title: "watch".into(), input: json!({}) });
        a.on_agent(g, Ev::Task(TaskEv {
            id: "bg1".into(), tool_id: Some("bash1".into()), status: Some("running".into()),
            output_file: Some(path.to_string_lossy().into_owned()), ..Default::default()
        }));
        assert_eq!(a.running_shells(), 1);
        std::fs::write(&path, format!("{}\nlatest output\n", "x".repeat(40_000))).unwrap();
        a.tasks[0].polled = None;
        a.poll_shell_output();
        assert!(matches!(&a.items[0].block, Block::Tool { output, .. } if output == "latest output\n"));
        a.on_agent(g, Ev::Task(TaskEv { id: "bg1".into(), status: Some("completed".into()), exit_code: Some(0), ..Default::default() }));
        let s = screen(&mut a, 80, 20);
        assert!(s.contains("exit 0") && !s.contains("⇢ 1 bg"), "{s}");
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn codex_commands_outliving_the_turn_stay_running_until_they_exit() {
        // Codex: the card has no result yet at turn end; output streams in after it.
        let mut a = app();
        let g = a.generation;
        a.on_agent(g, Ev::TurnStart);
        a.on_agent(g, Ev::ToolStart { id: "c1".into(), name: "PowerShell".into(), title: "watch #811".into(), input: json!({}) });
        a.on_agent(g, Ev::Task(TaskEv { id: "c1".into(), tool_id: Some("c1".into()), title: Some("watch #811".into()), status: Some("running".into()), ..Default::default() }));
        a.on_agent(g, Ev::TurnDone);
        a.on_agent(g, Ev::ToolOutput { id: "c1".into(), chunk: "#811: inProgress\n".into() });
        assert!(matches!(&a.items[0].block, Block::Tool { status: Status::Running, ended: None, .. }));
        assert_eq!(a.running_shells(), 1);
        let s = screen(&mut a, 100, 20);
        assert!(s.contains("⇢ bg") && !s.contains("failed"), "{s}");
        a.on_agent(g, Ev::Task(TaskEv { id: "c1".into(), status: Some("completed".into()), exit_code: Some(0), ..Default::default() }));
        a.on_agent(g, Ev::ToolEnd { id: "c1".into(), ok: true, output: "#811: succeeded\n".into(), diff: None });
        assert!(matches!(&a.items[0].block, Block::Tool { status: Status::Ok, output, .. } if output.contains("succeeded")));
        assert_eq!(a.running_shells(), 0);
        let s = screen(&mut a, 100, 20);
        assert!(s.contains("exit 0") && !s.contains("⇢ 1 bg"), "{s}");
    }

    #[test]
    fn background_shell_cards_survive_turn_end_and_show_output_and_exit() {
        let mut a = app();
        let g = a.generation;
        let path = std::env::temp_dir().join(format!("winfer-bg-{}.output", session::new_id()));
        std::fs::write(&path, "Watching CI\n").unwrap();
        a.items.push(Item::new(Block::User { text: "watch CI".into() }));
        a.on_agent(g, Ev::ToolStart { id: "bash1".into(), name: "Bash".into(), title: "gh run watch 123".into(), input: json!({}) });
        let task = |status: &str, exit_code| Ev::Task(TaskEv {
            id: "bg1".into(), tool_id: Some("bash1".into()), status: Some(status.into()),
            output_file: Some(path.to_string_lossy().into_owned()), exit_code, ..Default::default()
        });
        a.on_agent(g, task("running", None));
        a.on_agent(g, Ev::ToolEnd { id: "bash1".into(), ok: true, output: "background receipt".into(), diff: None });
        a.finish_running();
        assert_eq!(a.running_shells(), 1);
        assert_eq!(a.running_subagents(), 0);
        a.tasks[0].polled = None;
        let s = screen(&mut a, 100, 25);
        assert!(s.contains("⇢ bg") && s.contains("⇢ 1 bg") && s.contains("Watching CI"), "{s}");
        assert!(matches!(&a.items[1].block, Block::Tool { status: Status::Running, ended: None, .. }));
        std::fs::write(&path, "CI failed\n").unwrap();
        a.on_agent(g, task("failed", Some(1)));
        // A late launch receipt must not revive a finished task.
        a.on_agent(g, task("running", None));
        let s = screen(&mut a, 100, 25);
        assert_eq!(a.running_shells(), 0);
        assert!(!s.contains("⇢ 1 bg") && s.contains("exit 1") && s.contains("CI failed"), "{s}");
        assert!(matches!(&a.items[1].block, Block::Tool { status: Status::Failed, ended: Some(_), .. }));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn background_subagents_stay_running_until_their_task_ends() {
        let mut a = app();
        let g = a.generation;
        a.items.push(Item::new(Block::User { text: "go".into() }));
        a.on_agent(g, Ev::ToolStart { id: "t1".into(), name: "Agent".into(), title: "Map the repo".into(), input: json!({}) });
        // Claude Code returns a backgrounded agent's call at once.
        a.on_agent(g, Ev::ToolEnd { id: "t1".into(), ok: true, output: "Launched in the background".into(), diff: None });
        assert_eq!(a.running_subagents(), 0);
        let task = |status: Option<&str>, tools: Option<u64>| Ev::Task(TaskEv { id: "k1".into(), tool_id: Some("t1".into()), agent: Some("Explore".into()), status: status.map(str::to_string), tools, last: Some("Grep".into()), ..Default::default() });
        a.on_agent(g, task(Some("running"), None));
        a.on_agent(g, task(None, Some(4)));
        assert_eq!(a.running_subagents(), 1);
        let s = screen(&mut a, 120, 30);
        assert!(s.contains("1 agent · Map the repo · 4 tools · Grep"), "{s}");
        // The turn ends but the agent keeps going: still working, the rule says why.
        a.on_agent(g, Ev::TurnStart);
        a.on_agent(g, Ev::TurnDone);
        assert!(!a.busy && a.working() && a.animating() && a.reported_working);
        let s = screen(&mut a, 120, 30);
        assert!(s.contains("1 background agent running…"), "{s}");
        a.on_agent(g, task(Some("completed"), None));
        assert_eq!(a.running_subagents(), 0);
        assert!(!a.working() && !a.reported_working);
        let s = screen(&mut a, 120, 30);
        assert!(!s.contains("1 agent") && !s.contains("background agent"), "{s}");
    }

    #[test]
    fn esc_stops_background_tasks_after_the_turn_ends() {
        let mut a = app();
        let g = a.generation;
        a.items.push(Item::new(Block::User { text: "go".into() }));
        a.on_agent(g, Ev::TurnStart);
        a.on_agent(g, Ev::Task(TaskEv { id: "k1".into(), agent: Some("Explore".into()), status: Some("running".into()), ..Default::default() }));
        a.on_agent(g, Ev::Task(TaskEv { id: "b1".into(), title: Some("npm run dev".into()), status: Some("running".into()), ..Default::default() }));
        a.on_agent(g, Ev::TurnDone);
        assert!(!a.busy && a.working());
        let s = screen(&mut a, 120, 30);
        assert!(s.contains("esc to stop"), "{s}");
        key(&mut a, KeyCode::Esc);
        assert!(a.items.iter().any(|i| matches!(&i.block, Block::Notice { text, .. } if text == "Stopping 2 background tasks")));
        assert!(!a.reported_working);
        // The backend reports them stopped: nothing runs any more.
        a.on_agent(g, Ev::Task(TaskEv { id: "k1".into(), status: Some("stopped".into()), ..Default::default() }));
        a.on_agent(g, Ev::Task(TaskEv { id: "b1".into(), status: Some("killed".into()), ..Default::default() }));
        assert!(!a.working());
        // With nothing running, esc only clears the prompt.
        let n = a.items.len();
        key(&mut a, KeyCode::Esc);
        assert_eq!(a.items.len(), n);
    }

    #[test]
    fn subagents_show_in_the_status_bar_and_their_picker() {
        let mut a = app();
        let g = a.generation;
        let start = |a: &mut App, id: &str, title: &str| a.on_agent(g, Ev::ToolStart { id: id.into(), name: "Agent".into(), title: title.into(), input: json!({}) });
        a.items.push(Item::new(Block::User { text: "go".into() }));
        start(&mut a, "t1", "Find auth code");
        start(&mut a, "t2", "Review tests");
        a.on_agent(g, Ev::ToolOutput { id: "t1".into(), chunk: "Grep login src\nRead auth.rs\n".into() });
        a.on_agent(g, Ev::ToolEnd { id: "t2".into(), ok: true, output: "fine".into(), diff: None });
        assert_eq!(a.running_subagents(), 1);
        let s = screen(&mut a, 120, 30);
        assert!(s.contains("1 agent · Find auth code"), "{s}");

        start(&mut a, "t3", "Check docs");
        let s = screen(&mut a, 120, 30);
        assert!(s.contains("2 agents"), "{s}");

        a.on_term(Event::Key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::CONTROL)));
        assert_eq!(a.picker.as_ref().unwrap().kind, PickKind::Agents);
        a.overlay = Tween::new(1.0);
        let s = screen(&mut a, 120, 30);
        assert!(s.contains("Subagents") && s.contains("Running") && s.contains("Finished") && s.contains("ctrl+x stop"), "{s}");
        assert!(s.contains("2 tools · Read auth.rs") && s.contains("done in"), "{s}");

        // The list stays live while open.
        a.on_agent(g, Ev::ToolEnd { id: "t1".into(), ok: true, output: "found".into(), diff: None });
        screen(&mut a, 120, 30);
        let p = a.picker.as_ref().unwrap();
        assert_eq!(p.items.iter().filter(|i| i.group == "Running").count(), 1);

        // Enter jumps to the chosen agent's card.
        for _ in 0..30 {
            a.items.push(Item::new(Block::Assistant { id: format!("m{}", a.items.len()), text: "filler".into() }));
        }
        screen(&mut a, 120, 30);
        key(&mut a, KeyCode::Enter);
        assert!(a.picker.is_none());
        assert!(a.scroll > 0);
        let s = screen(&mut a, 120, 30);
        assert!(s.contains("Check docs"), "{s}");
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
    fn answers_the_agents_questions_one_by_one() {
        use crate::harness::event::Question;
        let mut a = app();
        let opts = |l: &[&str]| l.iter().map(|o| (o.to_string(), format!("about {o}"))).collect();
        let questions = vec![
            Question { header: "Lib".into(), question: "Which library?".into(), options: opts(&["serde", "nom"]), multi: false },
            Question { header: "Targets".into(), question: "Which targets?".into(), options: opts(&["mac", "linux", "windows"]), multi: true },
            Question { header: "Name".into(), question: "Crate name?".into(), options: Vec::new(), multi: false },
        ];
        let g = a.generation;
        a.on_agent(g, Ev::Approval(Approval { id: "q".into(), tool: "AskUserQuestion".into(), title: "Which library?".into(), questions, ..Default::default() }));
        let s = screen(&mut a, 90, 30);
        assert!(s.contains("Which library?") && s.contains("1. serde") && s.contains("about nom") && s.contains("1/3") && s.contains("waiting for your answer"), "{s}");
        key(&mut a, KeyCode::Char('2'));
        assert_eq!(a.answering.answers, ["nom"]);
        // Multi-select: tick two, enter.
        key(&mut a, KeyCode::Char(' '));
        key(&mut a, KeyCode::Down);
        key(&mut a, KeyCode::Down);
        key(&mut a, KeyCode::Char(' '));
        assert!(screen(&mut a, 90, 30).contains("[x] windows"));
        key(&mut a, KeyCode::Enter);
        assert_eq!(a.answering.answers, ["nom", "mac, windows"]);
        // No options: typing goes straight to the answer; enter with nothing typed does nothing.
        key(&mut a, KeyCode::Enter);
        assert_eq!(a.answering.answers.len(), 2);
        for c in "fastx".chars() {
            key(&mut a, KeyCode::Char(c));
        }
        key(&mut a, KeyCode::Backspace);
        assert!(screen(&mut a, 90, 30).contains("fast▏"));
        key(&mut a, KeyCode::Enter);
        assert!(a.approvals.is_empty() && a.answering.answers.is_empty());
    }

    #[test]
    fn renders_an_approval() {
        let mut a = app();
        a.approvals.push_back(Approval { id: "x".into(), tool: "Bash".into(), title: "rm -rf build".into(), detail: String::new(), diff: None, ..Default::default() });
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
        assert_eq!(term.backend().buffer()[(x0, y0)].bg, super::super::theme::SELECTION());
        // A key press drops the mark.
        key(&mut a, KeyCode::Esc);
        assert!(a.selection.is_none());
    }
}
