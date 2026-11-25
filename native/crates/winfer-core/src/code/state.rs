//! One winfer code pane's transcript and session state (port of `features/code/store.ts`).
//! `apply` is the reducer the events go through.

use super::names::{is_spec, remember_resolved};
use serde_json::Value;
use super::protocol::*;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

pub fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

static SEQ: AtomicU64 = AtomicU64::new(0);

fn key() -> String {
    format!("k{}", SEQ.fetch_add(1, Ordering::Relaxed) + 1)
}

#[derive(Clone, Debug)]
pub struct Attachment {
    /// Base64 data (no `data:` prefix); empty for a desktop path.
    pub data: String,
    pub mime: String,
    pub name: String,
    /// Local drop: the backend can read the original file directly.
    pub path: Option<String>,
}

/// How long the subagent tree stays after the last subagent finished.
pub const TREE_LINGER_MS: u64 = 5000;

/// Session-local task activity, recorded even while its popover is closed.
#[derive(Clone, Debug, Default)]
pub struct CodeTask {
    pub ev: TaskEv,
    pub started_at: u64,
    /// When it left `running` (done, failed or stopped), so its time stops counting.
    pub ended_at: Option<u64>,
    pub recent_lines: Vec<String>,
}

impl CodeTask {
    pub fn running(&self) -> bool {
        self.ev.status.as_deref().is_none_or(|s| s == "running")
    }

    /// How long it ran, or has been running so far.
    pub fn elapsed_ms(&self, now: u64) -> u64 {
        self.ended_at.unwrap_or(now).saturating_sub(self.started_at)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sheet {
    TaskLog,
    Usage,
    Status,
    Help,
    Mcp,
    Login,
    Agents,
    Sessions,
    Btw,
    Compaction,
    Model,
    Effort,
    Mode,
    SubagentModel,
}

#[derive(Clone, Debug)]
pub struct Btw {
    pub id: String,
    pub question: String,
    pub answer: Option<String>,
    pub error: Option<String>,
}

#[derive(Clone, Debug)]
pub enum Item {
    User { key: String, text: String, at: u64, images: Vec<String> },
    Text { key: String, text: String, streaming: bool },
    Reasoning { key: String, text: String, streaming: bool, started: u64, ended: Option<u64> },
    Tool { key: String, name: String, title: String, input: Value, output: String, ok: Option<bool>, diff: Option<String>, started: u64, ended: Option<u64> },
    Notice { key: String, text: String, error: bool },
    /// Context handed off from one model/backend to another within the same conversation.
    Handoff { key: String, from_model: String, from_actual: Option<String>, to_model: String },
    /// Total time the agent worked since the last user message, shown under its last response.
    Worked { key: String, ms: u64 },
}

impl Item {
    pub fn key(&self) -> &str {
        match self {
            Item::User { key, .. } | Item::Text { key, .. } | Item::Reasoning { key, .. } | Item::Tool { key, .. } | Item::Notice { key, .. } | Item::Handoff { key, .. } | Item::Worked { key, .. } => key,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Rate {
    pub last_tokens: u64,
    pub last_at: u64,
    pub tokens: u64,
    pub active_ms: u64,
}

impl Rate {
    /// Gaps longer than this between streamed counts (tool runs, waiting on the next request)
    /// don't count as generation time, matching `harness::event::Tps::IDLE`.
    const IDLE_MS: u64 = 3_000;

    pub fn sample(&mut self, at: u64, count: u64) {
        if self.last_at > 0 {
            let gap = at.saturating_sub(self.last_at);
            if count > self.last_tokens && gap <= Self::IDLE_MS {
                self.tokens += count - self.last_tokens;
                self.active_ms += gap;
            }
        }
        self.last_at = at;
        self.last_tokens = count;
    }

    pub fn rate(&self) -> Option<f32> {
        (self.tokens > 0 && self.active_ms >= 500)
            .then(|| self.tokens as f32 / (self.active_ms as f32 / 1000.0))
    }
}

pub struct CodeState {
    pub session: Option<String>,
    pub model: String,
    /// The model actually answering, when the backend resolved an alias.
    pub actual_model: Option<String>,
    pub models: Vec<ModelInfo>,
    /// The `@` mention list; None until the composer first asks for it.
    pub files: Option<Vec<String>>,
    pub cwd: Option<String>,
    pub backend_session: Option<String>,
    pub title: Option<String>,
    pub branch: Option<String>,
    pub trim_at: u64,
    pub compact_at: u64,
    pub mcp: Vec<String>,
    pub sessions: Option<Vec<SessionInfo>>,
    /// The `/cost` sheet's context breakdown: None while it's read.
    pub breakdown: Option<(u64, Option<Vec<ContextPart>>)>,
    pub btw: Vec<Btw>,
    pub sheet: Option<Sheet>,
    pub rate: Option<Rate>,
    pub last_rate: Option<f32>,
    pub turn_out_base: u64,
    pub log_task: Option<String>,
    pub task_log: Option<(String, Option<String>, Option<String>)>,
    pub mode: Mode,
    pub effort: String,
    pub ultracode: bool,
    pub sub_model: Option<String>,
    pub sub_effort: Option<String>,
    pub items: Vec<Item>,
    pub approvals: Vec<Approval>,
    pub tasks: Vec<CodeTask>,
    pub todos: Vec<Todo>,
    pub plan_done_at: Option<u64>,
    pub skills: Vec<String>,
    pub usage: Usage,
    pub busy: bool,
    pub turn_started: Option<u64>,
    pub exited: Option<String>,
    /// Out of usage: when the session continues by itself (unix ms).
    pub paused_until: Option<u64>,
    /// Messages sent while out of usage that wait to go.
    pub queued: usize,
    /// Commands queued for the transport (drained by the view).
    pub outbox: Vec<Cmd>,
    /// A plan handed over for approval, to open in the file viewer (taken by the app).
    pub show_plan: Option<String>,
}

impl Default for CodeState {
    fn default() -> Self {
        Self {
            session: None, model: String::new(), actual_model: None, models: Vec::new(), files: None, cwd: None, backend_session: None,
            title: None, branch: None, trim_at: 0, compact_at: 0, mcp: Vec::new(), sessions: None, breakdown: None, btw: Vec::new(),
            sheet: None, rate: None, last_rate: None, turn_out_base: 0, log_task: None, task_log: None, mode: Mode::Ask, effort: "high".into(), ultracode: false, sub_model: None, sub_effort: None,
            items: Vec::new(), approvals: Vec::new(), tasks: Vec::new(), todos: Vec::new(), plan_done_at: None, skills: Vec::new(),
            usage: Usage { window: 200_000, ..Default::default() }, busy: false, turn_started: None, exited: None, paused_until: None, queued: 0, outbox: Vec::new(),
            show_plan: None,
        }
    }
}

fn close_reasoning(items: &mut [Item]) {
    let now = now_ms();
    for i in items {
        if let Item::Reasoning { streaming, ended, .. } = i {
            if *streaming {
                *streaming = false;
                *ended = Some(now);
            }
        }
    }
}

fn find_last(items: &[Item], f: impl Fn(&Item) -> bool) -> Option<usize> {
    items.iter().rposition(f)
}

impl CodeState {
    pub fn notice(&mut self, text: impl Into<String>) {
        self.items.push(Item::Notice { key: key(), text: text.into(), error: false });
    }

    pub fn set_sheet(&mut self, sheet: Option<Sheet>) {
        if sheet == Some(Sheet::Btw) {
            self.btw.clear();
        }
        self.sheet = sheet;
    }

    /// The subagents the live tree above the composer shows, in the order they started: the running
    /// ones, and while a turn runs also those it started that already finished. Shells aren't in it.
    /// Once none runs any more, the tree lingers `TREE_LINGER_MS` after the last one finished, then hides.
    pub fn tree_agents(&self, now: u64) -> Vec<&CodeTask> {
        let turn = self.busy.then_some(self.turn_started).flatten();
        let mut out: Vec<&CodeTask> = self.tasks.iter().filter(|t| t.ev.agent.is_some() && (t.running() || turn.is_some_and(|at| t.started_at >= at))).collect();
        if !out.iter().any(|t| t.running()) {
            let last_end = out.iter().filter_map(|t| t.ended_at).max().unwrap_or(0);
            if now.saturating_sub(last_end) >= TREE_LINGER_MS {
                out.clear();
            }
        }
        out.sort_by_key(|t| t.started_at);
        out
    }

    /// The task whose output sheet is open, while it still runs (its log is polled then).
    pub fn live_log_task(&self) -> Option<&CodeTask> {
        let id = self.log_task.as_deref().filter(|_| self.sheet == Some(Sheet::TaskLog))?;
        self.tasks.iter().find(|t| t.ev.id == id && t.running())
    }

    /// What a task's output sheet shows: its log file's tail, or, for subagents (which have no log
    /// file), the output of the tool call that started it.
    pub fn task_output(&self, id: &str) -> Option<String> {
        let logged = self.task_log.as_ref().filter(|(i, ..)| i == id).and_then(|(_, text, _)| text.clone()).filter(|t| !t.trim().is_empty());
        logged.or_else(|| {
            let tool_id = self.tasks.iter().find(|t| t.ev.id == id)?.ev.tool_id.clone()?;
            self.items.iter().rev().find_map(|i| match i {
                Item::Tool { key, output, .. } if *key == tool_id && !output.trim().is_empty() => Some(output.clone()),
                _ => None,
            })
        })
    }

    pub fn open_task_log(&mut self, id: &str) {
        self.log_task = Some(id.to_string());
        self.task_log = None;
        self.sheet = Some(Sheet::TaskLog);
        self.outbox.push(Cmd::TaskLog(id.to_string()));
    }

    /// Applies a command's local effect and queues it for the process.
    pub fn send(&mut self, cmd: Cmd) {
        match &cmd {
            Cmd::Send { steer: false, .. } => {
                if let Some(h) = crate::hooks::get() {
                    h.turn_sent(&self.model, &self.effort, self.ultracode);
                }
            }
            Cmd::SetMode(m) => self.mode = *m,
            Cmd::SetModel(m) => {
                self.model = m.clone();
                self.actual_model = None;
            }
            Cmd::SetEffort(e) => self.effort = e.clone().unwrap_or_else(|| "default".into()),
            Cmd::SetUltracode(on) => self.ultracode = *on,
            Cmd::SetSubagent { model, effort } => {
                self.sub_model = model.clone();
                self.sub_effort = if model.is_some() { effort.clone() } else { None };
            }
            Cmd::Undo { .. } | Cmd::New => self.approvals.clear(),
            Cmd::Btw { id, question } => {
                self.btw = vec![Btw { id: id.clone(), question: question.clone(), answer: None, error: None }];
                self.sheet = Some(Sheet::Btw);
            }
            Cmd::Sessions => self.sessions = None,
            Cmd::ContextBreakdown { .. } => self.breakdown = None,
            Cmd::SetCompaction { trim, compact } => {
                self.trim_at = trim.unwrap_or(self.trim_at);
                self.compact_at = compact.unwrap_or(self.compact_at);
            }
            Cmd::Approve { id, .. } | Cmd::Answer { id, .. } => self.approvals.retain(|a| a.id != *id),
            _ => {}
        }
        self.outbox.push(cmd);
    }

    /// Sends a message (or steers a running turn). `images` and `files` are the attachments.
    pub fn submit(&mut self, text: &str, attachments: &[Attachment], skill: Option<&str>) {
        let t = text.trim();
        if t.is_empty() && attachments.is_empty() && skill.is_none() {
            return;
        }
        let is_image = |a: &&Attachment| a.path.is_none() && a.mime.starts_with("image/");
        let images: Vec<ImageData> = attachments.iter().filter(is_image).map(|a| ImageData { mime: a.mime.clone(), data: a.data.clone() }).collect();
        let files: Vec<FileData> = attachments.iter().filter(|a| !is_image(a)).map(|a| FileData { name: a.name.clone(), data: a.path.is_none().then(|| a.data.clone()), path: a.path.clone() }).collect();
        let mut shown = match skill {
            Some(s) => format!("/{s}{}", if t.is_empty() { String::new() } else { format!(" {t}") }),
            None => t.to_string(),
        };
        for f in &files {
            shown.push_str(&format!("\n[File: {}]", f.name));
        }
        let urls: Vec<String> = images.iter().map(|i| format!("data:{};base64,{}", i.mime, i.data)).collect();
        self.items.push(Item::User { key: key(), text: shown, at: now_ms(), images: urls });
        let steer = self.busy;
        self.send(Cmd::Send { text: t.to_string(), images, files, skill: skill.map(str::to_string), steer });
    }

    pub fn apply(&mut self, ev: Ev) {
        match ev {
            Ev::Session { id } => self.backend_session = Some(id),
            Ev::Model { model } => {
                // `--serve` reports a switch as the `backend/model` spec; backends report the model
                // actually answering (an alias resolved: claude/opus → claude-opus-5-5).
                if is_spec(&model) {
                    self.model = model;
                    self.actual_model = None;
                } else {
                    if !self.model.is_empty() {
                        remember_resolved(&self.model, &model);
                    }
                    self.actual_model = Some(model);
                }
            }
            Ev::Mode { mode } => self.mode = mode,
            Ev::TextDelta { id, text } => match find_last(&self.items, |i| matches!(i, Item::Text { key, .. } if *key == id)) {
                Some(i) => {
                    if let Item::Text { text: t, .. } = &mut self.items[i] {
                        t.push_str(&text);
                    }
                }
                None => {
                    close_reasoning(&mut self.items);
                    self.items.push(Item::Text { key: id, text, streaming: true });
                }
            },
            Ev::Text { id, text } => match find_last(&self.items, |i| matches!(i, Item::Text { key, .. } if *key == id)) {
                Some(i) => self.items[i] = Item::Text { key: id, text, streaming: false },
                None => {
                    close_reasoning(&mut self.items);
                    self.items.push(Item::Text { key: id, text, streaming: false });
                }
            },
            Ev::ReasoningDelta { id, text } => match find_last(&self.items, |i| matches!(i, Item::Reasoning { key, .. } if *key == id)) {
                Some(i) => {
                    if let Item::Reasoning { text: t, .. } = &mut self.items[i] {
                        t.push_str(&text);
                    }
                }
                None => self.items.push(Item::Reasoning { key: id, text, streaming: true, started: now_ms(), ended: None }),
            },
            Ev::ReasoningEnd { id } => {
                if !id.is_empty() {
                    if let Some(i) = find_last(&self.items, |i| matches!(i, Item::Reasoning { key, .. } if *key == id)) {
                        if let Item::Reasoning { streaming, ended, .. } = &mut self.items[i] {
                            *streaming = false;
                            if ended.is_none() {
                                *ended = Some(now_ms());
                            }
                        }
                        return;
                    }
                }
                close_reasoning(&mut self.items);
            }
            Ev::ToolStart { id, name, title, input } => {
                // Upsert so early streaming tool starts refine their name/title/input in place
                // without duplicating the card when a second reporting path or final payload arrives.
                if let Some(i) = find_last(&self.items, |i| matches!(i, Item::Tool { key, .. } if *key == id)) {
                    if let Item::Tool { name: n, title: t, input: inp, .. } = &mut self.items[i] {
                        if !name.is_empty() {
                            *n = name;
                        }
                        if !title.is_empty() {
                            *t = title;
                        }
                        let has_input = !input.is_null() && !input.as_object().is_some_and(serde_json::Map::is_empty);
                        if has_input {
                            *inp = input;
                        }
                    }
                    return;
                }
                close_reasoning(&mut self.items);
                self.items.push(Item::Tool { key: id, name, title, input, output: String::new(), ok: None, diff: None, started: now_ms(), ended: None });
            }
            Ev::ToolOutput { id, chunk } => {
                if let Some(i) = find_last(&self.items, |i| matches!(i, Item::Tool { key, .. } if *key == id)) {
                    if let Item::Tool { output, .. } = &mut self.items[i] {
                        output.push_str(&chunk);
                    }
                }
            }
            Ev::ToolEnd { id, ok, output, diff } => {
                if let Some(i) = find_last(&self.items, |i| matches!(i, Item::Tool { key, .. } if *key == id)) {
                    if let Item::Tool { output: o, ok: k, diff: d, ended, .. } = &mut self.items[i] {
                        *k = Some(ok);
                        if !output.is_empty() {
                            *o = output;
                        }
                        *d = diff;
                        *ended = Some(now_ms());
                    }
                }
            }
            Ev::Approval(a) => {
                if let Some(file) = &a.plan_file {
                    self.show_plan = Some(file.clone());
                }
                self.approvals.push(a);
            }
            Ev::Task(t) => {
                let now = now_ms();
                let i = match self.tasks.iter().position(|x| x.ev.id == t.id) {
                    Some(i) => i,
                    None => {
                        self.tasks.push(CodeTask { ev: TaskEv { id: t.id.clone(), ..Default::default() }, started_at: now, ended_at: None, recent_lines: Vec::new() });
                        self.tasks.len() - 1
                    }
                };
                let task = &mut self.tasks[i];
                macro_rules! merge {
                    ($($f:ident),*) => { $( if t.$f.is_some() { task.ev.$f = t.$f.clone(); } )* };
                }
                merge!(tool_id, title, description, agent, tools, last, status, output_file, exit_code, model);
                if task.running() {
                    task.ended_at = None;
                } else if task.ended_at.is_none() {
                    task.ended_at = Some(now);
                }
                if let Some(last) = &t.last {
                    if task.recent_lines.last() != Some(last) {
                        task.recent_lines.push(last.clone());
                        let n = task.recent_lines.len();
                        if n > 8 {
                            task.recent_lines.drain(..n - 8);
                        }
                    }
                }
            }
            Ev::Todos { todos } => {
                let all_done = !todos.is_empty() && todos.iter().all(|t| t.status == "completed");
                self.plan_done_at = if all_done { self.plan_done_at.or(Some(now_ms())) } else { None };
                self.todos = todos;
            }
            Ev::Skills { skills } => self.skills = skills,
            Ev::Usage(u) => {
                let streamed = u.turn_streamed.unwrap_or(u.output);
                let now = now_ms();
                if self.busy {
                    self.rate.get_or_insert_with(Rate::default).sample(now, streamed);
                }
                let prev = &self.usage;
                self.usage = Usage {
                    context: u.context,
                    window: if u.window > 0 { u.window } else { prev.window },
                    input: u.input,
                    cached: u.cached,
                    output: u.output,
                    cost: u.cost.or(prev.cost),
                    turn: u.turn.or(prev.turn),
                    turn_input: u.turn_input.or(prev.turn_input),
                    turn_cached: u.turn_cached.or(prev.turn_cached),
                    turn_streamed: u.turn_streamed.or(prev.turn_streamed),
                    turn_active_ms: u.turn_active_ms.or(prev.turn_active_ms),
                };
                if let Some(r) = self.tokens_per_second() {
                    self.last_rate = Some(r);
                }
            }
            Ev::Notice { text } | Ev::BgNote { text } => self.items.push(Item::Notice { key: key(), text, error: false }),
            Ev::Error { text } => self.items.push(Item::Notice { key: key(), text, error: true }),
            Ev::TurnStart => {
                self.busy = true;
                self.turn_started = Some(now_ms());
                self.rate = None;
                self.turn_out_base = self.usage.output;
                self.usage.turn = Some(0);
                self.usage.turn_input = Some(0);
                self.usage.turn_cached = Some(0);
                self.usage.turn_streamed = Some(0);
                self.usage.turn_active_ms = Some(0);
            }
            Ev::TurnDone => {
                if let Some(r) = self.tokens_per_second() {
                    self.last_rate = Some(r);
                }
                close_reasoning(&mut self.items);
                for i in &mut self.items {
                    if let Item::Text { streaming, .. } = i {
                        *streaming = false;
                    }
                }
                // Follow-up turns without a new user message (queued steers, background task
                // wake-ups) add up into one total under the latest response.
                let last_user = find_last(&self.items, |i| matches!(i, Item::User { .. }));
                let mut ms = self.turn_started.map_or(0, |s| now_ms().saturating_sub(s));
                let mut idx = 0;
                self.items.retain(|i| {
                    let drop = match i {
                        Item::Worked { ms: m, .. } if last_user.is_none_or(|l| idx >= l) => {
                            ms += m;
                            true
                        }
                        _ => false,
                    };
                    idx += 1;
                    !drop
                });
                // Dropped rows sit after the user's message, so its index is unchanged.
                let worked = self.items[last_user.map_or(0, |i| i + 1)..].iter().any(|i| matches!(i, Item::Text { .. } | Item::Tool { .. } | Item::Reasoning { .. }));
                if worked && ms > 0 {
                    self.items.push(Item::Worked { key: key(), ms });
                }
                self.busy = false;
                self.approvals.clear();
            }
            Ev::Exited { text } => {
                self.busy = false;
                self.exited = Some(text);
                (self.paused_until, self.queued) = (None, 0);
            }
            Ev::LimitWait { until, queued } => (self.paused_until, self.queued) = (until, queued),
            Ev::Ready { session, backend_session, title, model, mode, effort, ultracode, subagent_model, subagent_effort, cwd, branch, trim_at, compact_at, mcp } => {
                self.actual_model = if model == self.model { self.actual_model.take() } else { None };
                // A new process: whatever the old one held for the usage limit is gone.
                (self.paused_until, self.queued) = (None, 0);
                self.session = Some(session);
                self.backend_session = backend_session;
                self.title = title;
                self.model = model;
                self.mode = mode;
                self.effort = effort.unwrap_or_else(|| "default".into());
                self.ultracode = ultracode;
                self.sub_model = subagent_model;
                self.sub_effort = subagent_effort;
                self.cwd = Some(cwd);
                self.branch = branch;
                self.trim_at = trim_at.unwrap_or(self.trim_at);
                self.compact_at = compact_at.unwrap_or(self.compact_at);
                self.mcp = mcp.unwrap_or_else(|| std::mem::take(&mut self.mcp));
                self.exited = None;
            }
            Ev::Sessions { sessions } => self.sessions = Some(sessions),
            Ev::ContextBreakdown { tokens, parts } => self.breakdown = Some((tokens, parts)),
            Ev::TaskLog { id, text, file } => {
                if self.log_task.as_deref() == Some(&id) {
                    self.task_log = Some((id, text, file));
                }
            }
            Ev::Btw { id, answer, error } => {
                for b in &mut self.btw {
                    if b.id == id {
                        b.answer = answer.clone();
                        b.error = error.clone();
                    }
                }
            }
            Ev::Models { models } => self.models = models,
            Ev::Files { files } => self.files = Some(files),
            Ev::User { text } => self.items.push(Item::User { key: key(), text, at: now_ms(), images: Vec::new() }),
            Ev::Answered { id } => self.approvals.retain(|a| a.id != id),
            Ev::Cleared => {
                self.items.clear();
                self.approvals.clear();
                self.tasks.clear();
                self.todos.clear();
                self.plan_done_at = None;
                self.busy = false;
                self.usage = Usage { window: self.usage.window, ..Default::default() };
                self.rate = None;
                self.last_rate = None;
                self.turn_out_base = 0;
            }
            Ev::Restore { transcript } => {
                for e in transcript {
                    self.items.push(match e.role.as_str() {
                        "user" => Item::User { key: key(), text: e.text, at: 0, images: Vec::new() },
                        "assistant" => Item::Text { key: key(), text: e.text, streaming: false },
                        "tool" => {
                            let (name, title) = e.text.split_once('\t').unwrap_or((&e.text, ""));
                            Item::Tool { key: key(), name: name.to_string(), title: title.to_string(), input: Value::Null, output: String::new(), ok: Some(e.state.as_deref() != Some("failed")), diff: None, started: 0, ended: Some(0) }
                        }
                        "handoff" => match (e.from_model, e.to_model) {
                            (Some(from_model), Some(to_model)) => Item::Handoff { key: key(), from_model, from_actual: e.from_actual, to_model },
                            _ => Item::Notice { key: key(), text: e.text, error: false },
                        },
                        _ => Item::Notice { key: key(), text: e.text, error: false },
                    });
                }
                self.notice("Resumed session");
            }
            Ev::Handoff { from_model, from_actual, to_model, replace } => {
                self.busy = false;
                self.approvals.clear();
                self.usage.context = 0;
                close_reasoning(&mut self.items);
                for i in &mut self.items {
                    if let Item::Text { streaming, .. } = i {
                        *streaming = false;
                    }
                }
                if replace {
                    if let Some(Item::Handoff { from_model: f, from_actual: fa, to_model: t, .. }) = self.items.last_mut() {
                        *f = from_model;
                        *fa = from_actual;
                        *t = to_model;
                        return;
                    }
                }
                self.items.push(Item::Handoff { key: key(), from_model, from_actual, to_model });
            }
            Ev::HandoffCancel => {
                if matches!(self.items.last(), Some(Item::Handoff { .. })) {
                    self.items.pop();
                }
            }
            Ev::Undone { files, error } => {
                if let Some(i) = find_last(&self.items, |i| matches!(i, Item::User { .. })) {
                    self.items.truncate(i);
                }
                let files = files.unwrap_or_default();
                match error {
                    Some(e) => self.items.push(Item::Notice { key: key(), text: format!("Undo failed: {e}"), error: true }),
                    None => {
                        let extra = if files.is_empty() { String::new() } else { format!(" · reverted {}", files.join(", ")) };
                        self.notice(format!("Undid last turn{extra}"));
                    }
                }
            }
            Ev::Account { .. } | Ev::Unknown => {}
        }
    }

    /// Context use as a fraction of the window.
    pub fn context_fraction(&self) -> f32 {
        if self.usage.window == 0 { 0.0 } else { (self.usage.context as f32 / self.usage.window as f32).clamp(0.0, 1.0) }
    }

    /// Input tokens for the running turn (0 if none reported yet).
    pub fn turn_input(&self) -> u64 {
        self.usage.turn_input.unwrap_or(0)
    }

    /// Cached input tokens for the running turn.
    pub fn turn_cached(&self) -> u64 {
        self.usage.turn_cached.unwrap_or(0)
    }

    /// Output tokens generated in the running turn.
    pub fn turn_output(&self) -> u64 {
        self.usage.turn.filter(|t| *t > 0).or(self.usage.turn_streamed.filter(|t| *t > 0)).unwrap_or_else(|| self.usage.output.saturating_sub(self.turn_out_base))
    }

    /// Tokens per second of the running turn (prefers backend-measured active time, else gap-filtered streamed rate).
    pub fn tokens_per_second(&self) -> Option<f32> {
        if let Some(ms) = self.usage.turn_active_ms.filter(|ms| *ms > 0) {
            if ms >= 500 {
                let toks = self.usage.turn_streamed.filter(|t| *t > 0)?;
                return Some(toks as f32 / (ms as f32 / 1000.0));
            }
            return None;
        }
        self.rate.as_ref().and_then(Rate::rate)
    }

    pub fn export_markdown(&self) -> String {
        let mut md = format!("# {}\n\n", self.title.as_deref().filter(|t| !t.is_empty()).unwrap_or("WinferCode session"));
        for it in &self.items {
            match it {
                Item::User { text, .. } => md.push_str(&format!("## You\n\n{text}\n\n")),
                Item::Text { text, .. } => md.push_str(&format!("## Assistant\n\n{text}\n\n")),
                Item::Tool { name, title, diff, output, .. } => {
                    md.push_str(&format!("**{name}** `{title}`\n\n"));
                    if let Some(d) = diff {
                        md.push_str(&format!("```diff\n{d}\n```\n\n"));
                    } else if !output.trim().is_empty() {
                        md.push_str(&format!("```\n{}\n```\n\n", output.trim().chars().take(4000).collect::<String>()));
                    }
                }
                Item::Handoff { from_model, from_actual, to_model, .. } => {
                    let from = super::names::model_name(from_model, from_actual.as_deref());
                    let to = super::names::model_name(to_model, None);
                    md.push_str(&format!("---\n\n_Context handoff: {from} → {to}_\n\n---\n\n"));
                }
                _ => {}
            }
        }
        md
    }
}

/// A plan task's glyph state for the strip above the composer.
pub fn plan_counts(todos: &[Todo]) -> (usize, usize) {
    (todos.iter().filter(|t| t.status == "completed").count(), todos.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    impl CodeState {
        /// The pane status the agent bar shows: asking, working, or idle.
        fn status(&self) -> &'static str {
            if !self.approvals.is_empty() { "needs-input" } else if self.busy { "working" } else { "idle" }
        }
    }

    fn ev(v: Value) -> Ev {
        serde_json::from_value(v).unwrap()
    }

    #[test]
    fn unknown_events_keep_the_state() {
        let mut s = CodeState::default();
        s.apply(ev(json!({ "type": "account", "id": "work", "label": "work@example.com" })));
        s.apply(ev(json!({ "type": "from_a_newer_harness" })));
        assert!(s.tasks.is_empty() && s.items.is_empty());
    }

    #[test]
    fn limit_wait_shows_until_it_resumes_and_the_queue_is_sent() {
        let mut s = CodeState::default();
        s.apply(ev(json!({ "type": "limit_wait", "until": 1_800_000_000_000u64, "queued": 0 })));
        assert_eq!((s.paused_until, s.queued), (Some(1_800_000_000_000), 0));
        s.apply(ev(json!({ "type": "limit_wait", "until": 1_800_000_000_000u64, "queued": 2 })));
        assert_eq!(s.queued, 2);
        s.apply(ev(json!({ "type": "limit_wait", "until": null, "queued": 1 })));
        assert_eq!((s.paused_until, s.queued), (None, 1));
        s.apply(ev(json!({ "type": "exited", "text": "Claude Code stopped" })));
        assert_eq!((s.paused_until, s.queued), (None, 0));
    }

    #[test]
    fn deltas_stream_into_one_row_and_the_final_text_replaces_them() {
        let mut s = CodeState::default();
        s.apply(ev(json!({ "type": "text_delta", "id": "a", "text": "par" })));
        s.apply(ev(json!({ "type": "text_delta", "id": "a", "text": "tial" })));
        assert!(matches!(&s.items[0], Item::Text { text, streaming: true, .. } if text == "partial"));
        s.apply(ev(json!({ "type": "text", "id": "a", "text": "complete" })));
        assert_eq!(s.items.len(), 1);
        assert!(matches!(&s.items[0], Item::Text { text, streaming: false, .. } if text == "complete"));
    }

    #[test]
    fn reasoning_ends_when_text_starts() {
        let mut s = CodeState::default();
        s.apply(ev(json!({ "type": "reasoning_delta", "id": "r", "text": "hmm" })));
        s.apply(ev(json!({ "type": "text_delta", "id": "t", "text": "ok" })));
        assert!(matches!(&s.items[0], Item::Reasoning { streaming: false, ended: Some(_), .. }));
    }

    #[test]
    fn reasoning_end_closes_thinking_and_tool_start_upserts_in_place() {
        let mut s = CodeState::default();
        s.apply(ev(json!({ "type": "reasoning_delta", "id": "r1", "text": "planning" })));
        s.apply(ev(json!({ "type": "reasoning_end", "id": "r1" })));
        assert!(matches!(&s.items[0], Item::Reasoning { streaming: false, ended: Some(_), .. }));

        // Streaming tool start with empty title, refined once path streams in, then finalized with full input.
        s.apply(ev(json!({ "type": "tool_start", "id": "w1", "name": "Write", "title": "", "input": {} })));
        let Item::Tool { started, .. } = &s.items[1] else { panic!("expected tool") };
        let orig_started = *started;
        s.apply(ev(json!({ "type": "tool_start", "id": "w1", "name": "Write", "title": "src/big.rs", "input": { "file_path": "src/big.rs" } })));
        s.apply(ev(json!({ "type": "tool_start", "id": "w1", "name": "Write", "title": "src/big.rs", "input": { "file_path": "src/big.rs", "content": "fn main() {}" } })));
        assert_eq!(s.items.len(), 2);
        assert!(matches!(
            &s.items[1],
            Item::Tool { name, title, input, started, .. }
                if name == "Write" && title == "src/big.rs" && input["content"] == "fn main() {}" && *started == orig_started
        ));
    }

    #[test]
    fn tools_run_and_finish_once() {
        let mut s = CodeState::default();
        s.apply(ev(json!({ "type": "tool_start", "id": "t1", "name": "Bash", "title": "ls", "input": {} })));
        s.apply(ev(json!({ "type": "tool_start", "id": "t1", "name": "Bash", "title": "ls", "input": {} })));
        s.apply(ev(json!({ "type": "tool_output", "id": "t1", "chunk": "a\n" })));
        s.apply(ev(json!({ "type": "tool_end", "id": "t1", "ok": true, "output": "", "diff": null })));
        assert_eq!(s.items.len(), 1);
        assert!(matches!(&s.items[0], Item::Tool { output, ok: Some(true), ended: Some(_), .. } if output == "a\n"));
    }

    #[test]
    fn a_turn_adds_up_the_time_worked() {
        let mut s = CodeState::default();
        s.submit("hi", &[], None);
        s.apply(Ev::TurnStart);
        s.turn_started = Some(now_ms() - 2000);
        s.apply(ev(json!({ "type": "text", "id": "a", "text": "hello" })));
        s.apply(Ev::TurnDone);
        let worked: Vec<_> = s.items.iter().filter(|i| matches!(i, Item::Worked { .. })).collect();
        assert_eq!(worked.len(), 1);
        // A follow-up turn merges into the same row.
        s.apply(Ev::TurnStart);
        s.apply(ev(json!({ "type": "text", "id": "b", "text": "more" })));
        s.apply(Ev::TurnDone);
        assert_eq!(s.items.iter().filter(|i| matches!(i, Item::Worked { .. })).count(), 1);
        assert!(!s.busy);
    }

    #[test]
    fn a_turn_without_output_shows_no_worked_row() {
        let mut s = CodeState::default();
        s.submit("hi", &[], None);
        s.apply(Ev::TurnStart);
        s.apply(Ev::TurnDone);
        assert!(!s.items.iter().any(|i| matches!(i, Item::Worked { .. })));
    }

    #[test]
    fn submit_steers_while_busy_and_sends_attachments() {
        let mut s = CodeState::default();
        let att = vec![
            Attachment { name: "shot.png".into(), mime: "image/png".into(), data: "aW1hZ2U=".into(), path: None },
            Attachment { name: "notes.txt".into(), mime: "text/plain".into(), data: "aGVsbG8=".into(), path: None },
            Attachment { name: "local.png".into(), mime: String::new(), data: String::new(), path: Some("C:\\My Files\\local.png".into()) },
        ];
        s.submit("review", &att, None);
        let v = s.outbox.last().unwrap().json();
        assert_eq!(v["type"], "send");
        assert_eq!(v["images"], json!([{ "mime": "image/png", "data": "aW1hZ2U=" }]));
        assert_eq!(v["files"], json!([{ "name": "notes.txt", "data": "aGVsbG8=" }, { "name": "local.png", "path": "C:\\My Files\\local.png" }]));
        assert!(matches!(&s.items[0], Item::User { text, images, .. } if text == "review\n[File: notes.txt]\n[File: local.png]" && images.len() == 1));
        s.busy = true;
        s.submit("also", &[], None);
        assert_eq!(s.outbox.last().unwrap().json()["type"], "steer");
        s.submit("  ", &[], None);
        assert_eq!(s.outbox.len(), 2);
    }

    #[test]
    fn approvals_come_and_go() {
        let mut s = CodeState::default();
        s.apply(ev(json!({ "type": "approval", "id": "x", "tool": "Bash", "title": "rm", "detail": "", "questions": [] })));
        assert_eq!(s.status(), "needs-input");
        s.send(Cmd::Approve { id: "x".into(), decision: Decision::Allow });
        assert!(s.approvals.is_empty());
        assert_eq!(s.status(), "idle");
        s.apply(ev(json!({ "type": "approval", "id": "y", "tool": "Bash", "title": "", "detail": "", "questions": [] })));
        s.apply(ev(json!({ "type": "answered", "id": "y" })));
        assert!(s.approvals.is_empty());
    }

    #[test]
    fn tasks_merge_and_keep_recent_lines() {
        let mut s = CodeState::default();
        s.apply(ev(json!({ "type": "task", "id": "a", "title": "build", "status": "running", "last": "one" })));
        s.apply(ev(json!({ "type": "task", "id": "a", "last": "two" })));
        s.apply(ev(json!({ "type": "task", "id": "a", "last": "two", "status": "done" })));
        assert_eq!(s.tasks.len(), 1);
        let t = &s.tasks[0];
        assert_eq!(t.ev.title.as_deref(), Some("build"));
        assert_eq!(t.ev.status.as_deref(), Some("done"));
        assert_eq!(t.recent_lines, ["one", "two"]);
    }

    #[test]
    fn tree_shows_running_subagents_and_this_turns_finished_ones() {
        let mut s = CodeState::default();
        s.apply(ev(json!({ "type": "task", "id": "old", "agent": "Explore", "status": "done" })));
        s.apply(ev(json!({ "type": "task", "id": "sh", "title": "npm test", "status": "running" })));
        assert!(s.tree_agents(now_ms()).is_empty(), "a finished subagent from an earlier turn and a shell stay out");
        s.tasks[0].started_at -= 1000;
        s.busy = true;
        s.turn_started = Some(now_ms());
        s.apply(ev(json!({ "type": "task", "id": "a", "agent": "Explore", "status": "running" })));
        s.apply(ev(json!({ "type": "task", "id": "b", "agent": "general", "status": "running" })));
        s.apply(ev(json!({ "type": "task", "id": "a", "status": "done" })));
        let ids: Vec<&str> = s.tree_agents(now_ms()).iter().map(|t| t.ev.id.as_str()).collect();
        assert_eq!(ids, ["a", "b"]);
        let a = s.tasks.iter().find(|t| t.ev.id == "a").unwrap();
        assert!(!a.running() && a.ended_at.is_some());
        s.busy = false;
        let ids: Vec<&str> = s.tree_agents(now_ms()).iter().map(|t| t.ev.id.as_str()).collect();
        assert_eq!(ids, ["b"], "once the turn ends only running ones stay");
    }

    #[test]
    fn tree_hides_five_seconds_after_the_last_subagent_finished() {
        let mut s = CodeState::default();
        s.busy = true;
        s.turn_started = Some(now_ms());
        s.apply(ev(json!({ "type": "task", "id": "a", "agent": "Explore", "status": "running" })));
        s.apply(ev(json!({ "type": "task", "id": "a", "status": "completed" })));
        let ended = s.tasks[0].ended_at.unwrap();
        assert_eq!(s.tree_agents(ended + TREE_LINGER_MS - 1).len(), 1);
        assert!(s.tree_agents(ended + TREE_LINGER_MS).is_empty());
        s.apply(ev(json!({ "type": "task", "id": "b", "agent": "Explore", "status": "running" })));
        assert_eq!(s.tree_agents(ended + TREE_LINGER_MS + 1000).len(), 2, "a running one keeps the tree up");
    }

    #[test]
    fn subagent_output_falls_back_to_its_tool_card() {
        let mut s = CodeState::default();
        s.apply(ev(json!({ "type": "tool_start", "id": "call1", "name": "Agent", "title": "Explore auth", "input": {} })));
        s.apply(ev(json!({ "type": "task", "id": "a", "tool_id": "call1", "agent": "Explore", "status": "running" })));
        assert_eq!(s.task_output("a"), None);
        s.apply(ev(json!({ "type": "tool_end", "id": "call1", "ok": true, "output": "Found 3 call sites" })));
        assert_eq!(s.task_output("a").as_deref(), Some("Found 3 call sites"));
        s.open_task_log("a");
        assert_eq!(s.live_log_task().map(|t| t.ev.id.as_str()), Some("a"));
        s.apply(ev(json!({ "type": "task_log", "id": "a", "text": "log tail" })));
        assert_eq!(s.task_output("a").as_deref(), Some("log tail"), "a log file wins");
        s.apply(ev(json!({ "type": "task", "id": "a", "status": "done" })));
        assert!(s.live_log_task().is_none());
    }

    #[test]
    fn plan_completion_time_sticks() {
        let mut s = CodeState::default();
        s.apply(ev(json!({ "type": "todos", "todos": [{ "content": "a", "status": "completed" }] })));
        let at = s.plan_done_at;
        assert!(at.is_some());
        s.apply(ev(json!({ "type": "todos", "todos": [{ "content": "a", "status": "completed" }] })));
        assert_eq!(s.plan_done_at, at);
        s.apply(ev(json!({ "type": "todos", "todos": [{ "content": "a", "status": "pending" }] })));
        assert_eq!(s.plan_done_at, None);
        assert_eq!(plan_counts(&s.todos), (0, 1));
    }

    #[test]
    fn model_events_tell_specs_from_resolved_ids() {
        let mut s = CodeState::default();
        s.apply(Ev::Model { model: "claude/opus".into() });
        s.apply(Ev::Model { model: "claude-opus-5-6".into() });
        assert_eq!(s.model, "claude/opus");
        assert_eq!(s.actual_model.as_deref(), Some("claude-opus-5-6"));
        s.apply(Ev::Model { model: "claude/sonnet".into() });
        assert_eq!(s.actual_model, None);
    }

    #[test]
    fn undo_drops_the_last_turn() {
        let mut s = CodeState::default();
        s.submit("one", &[], None);
        s.apply(ev(json!({ "type": "text", "id": "a", "text": "r1" })));
        s.submit("two", &[], None);
        s.apply(ev(json!({ "type": "text", "id": "b", "text": "r2" })));
        s.apply(ev(json!({ "type": "undone", "files": ["a.rs"] })));
        assert_eq!(s.items.len(), 3);
        assert!(matches!(s.items.last(), Some(Item::Notice { text, error: false, .. }) if text == "Undid last turn · reverted a.rs"));
    }

    #[test]
    fn clearing_resets_the_session_but_not_the_window() {
        let mut s = CodeState::default();
        s.apply(ev(json!({ "type": "usage", "context": 5000, "window": 100000, "input": 10, "cached": 0, "output": 5, "cost": 0.5 })));
        s.submit("x", &[], None);
        s.apply(Ev::Cleared);
        assert!(s.items.is_empty());
        assert_eq!(s.usage.window, 100_000);
        assert_eq!(s.usage.context, 0);
        assert_eq!(s.usage.cost, None);
    }

    #[test]
    fn ready_and_restore() {
        let mut s = CodeState::default();
        s.apply(ev(json!({ "type": "ready", "session": "s", "model": "claude/opus", "mode": "plan", "cwd": "/x", "effort": "high", "trim_at": 60 })));
        assert_eq!((s.mode, s.trim_at, s.effort.as_str()), (Mode::Plan, 60, "high"));
        s.apply(ev(json!({ "type": "restore", "transcript": [{ "role": "user", "text": "hi" }, { "role": "assistant", "text": "yo" }, { "role": "tool", "text": "Bash\tls", "state": "failed" }] })));
        assert_eq!(s.items.len(), 4);
        assert!(matches!(&s.items[2], Item::Tool { name, title, ok: Some(false), .. } if name == "Bash" && title == "ls"));
    }

    #[test]
    fn sending_commands_updates_the_state_at_once() {
        let mut s = CodeState::default();
        s.send(Cmd::SetMode(Mode::Yolo));
        s.send(Cmd::SetEffort(None));
        s.send(Cmd::SetSubagent { model: None, effort: Some("low".into()) });
        s.send(Cmd::SetCompaction { trim: Some(50), compact: None });
        assert_eq!((s.mode, s.effort.as_str(), s.sub_effort.clone(), s.trim_at), (Mode::Yolo, "default", None, 50));
        assert_eq!(s.outbox.len(), 4);
    }

    #[test]
    fn token_metrics_track_in_out_and_rate() {
        let mut s = CodeState::default();
        s.apply(Ev::TurnStart);
        s.apply(ev(json!({
            "type": "usage",
            "context": 12000,
            "window": 200000,
            "input": 10000,
            "cached": 4000,
            "output": 600,
            "turn": 600,
            "turn_input": 10000,
            "turn_cached": 4000,
            "turn_streamed": 600,
            "turn_active_ms": 10000
        })));
        assert_eq!(s.turn_input(), 10_000);
        assert_eq!(s.turn_cached(), 4_000);
        assert_eq!(s.turn_output(), 600);
        let rate = s.tokens_per_second().expect("rate from turn_active_ms");
        assert!((rate - 60.0).abs() < 0.01, "expected ~60 tok/s, got {rate}");
        s.apply(Ev::TurnDone);
        assert!((s.last_rate.expect("preserved after turn") - 60.0).abs() < 0.01);
        s.apply(Ev::Cleared);
        assert_eq!(s.last_rate, None);

        // Untimed backends skip tool-run gaps (>3s) instead of averaging over wall time.
        let mut r = Rate::default();
        r.sample(1_000, 0);
        r.sample(1_500, 50);
        r.sample(2_000, 100);
        assert!((r.rate().unwrap() - 100.0).abs() < 0.01);
        // 15s tool run: gap is ignored.
        r.sample(17_000, 125);
        r.sample(17_500, 175);
        assert!((r.rate().unwrap() - 100.0).abs() < 0.01);

        // Subagent-only `turn` tokens must not divide by main-agent `turn_active_ms`.
        s.apply(Ev::TurnStart);
        s.apply(ev(json!({
            "type": "usage",
            "context": 12000,
            "window": 200000,
            "input": 10000,
            "cached": 0,
            "output": 500,
            "turn": 500,
            "turn_streamed": 0,
            "turn_active_ms": 1000
        })));
        assert_eq!(s.tokens_per_second(), None);
    }

    #[test]
    fn handoff_events_replace_cancel_restore_and_export() {
        let mut s = CodeState::default();
        s.apply(Ev::TurnStart);
        s.apply(ev(json!({ "type": "usage", "context": 45000, "window": 200000, "input": 45000, "cached": 0, "output": 500, "cost": 0.42 })));
        s.apply(ev(json!({ "type": "text_delta", "id": "a1", "text": "Working on it" })));
        s.apply(ev(json!({ "type": "handoff", "from_model": "claude/fable", "from_actual": "claude-fable-5-2", "to_model": "codex/gpt-6.1-sol", "replace": false })));
        assert!(!s.busy);
        assert_eq!(s.usage.context, 0);
        assert_eq!(s.usage.cost, Some(0.42));
        assert!(matches!(&s.items[0], Item::Text { streaming: false, .. }));
        assert!(matches!(&s.items[1], Item::Handoff { from_model, from_actual, to_model, .. }
            if from_model == "claude/fable" && from_actual.as_deref() == Some("claude-fable-5-2") && to_model == "codex/gpt-6.1-sol"));

        // Consecutive switch before sending replaces the trailing handoff in place.
        s.apply(ev(json!({ "type": "handoff", "from_model": "claude/fable", "from_actual": "claude-fable-5-2", "to_model": "agy/gemini-4-neon", "replace": true })));
        assert_eq!(s.items.len(), 2);
        assert!(matches!(&s.items[1], Item::Handoff { to_model, .. } if to_model == "agy/gemini-4-neon"));

        let md = s.export_markdown();
        assert!(md.contains("Context handoff: Fable 5.2 → Gemini 4 Neon"));

        // Canceling drops the trailing uncommitted handoff divider.
        s.apply(ev(json!({ "type": "handoff_cancel" })));
        assert_eq!(s.items.len(), 1);

        // Restoring a saved transcript rebuilds Handoff items.
        s.apply(Ev::Cleared);
        s.apply(ev(json!({
            "type": "restore",
            "transcript": [
                { "role": "user", "text": "hi" },
                { "role": "handoff", "text": "Context handoff", "from_model": "claude/fable", "from_actual": "claude-fable-5-2", "to_model": "agy/gemini-4-neon" }
            ]
        })));
        assert!(matches!(&s.items[1], Item::Handoff { from_model, to_model, .. } if from_model == "claude/fable" && to_model == "agy/gemini-4-neon"));
    }
}
