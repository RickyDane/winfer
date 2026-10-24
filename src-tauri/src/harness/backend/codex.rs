//! Codex bridge: `codex app-server` (JSON-RPC over stdio), with Codex's own tools, sandbox and
//! login. Approval requests from the server are answered by the TUI.

use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Duration;

use serde_json::{Value, json};

use super::microcompact::{self, Format};
use super::{In, LineProcess, Opts};
use crate::harness::event::{Approval, Cmd, Decision, Ev, Events, Mode, ModelInfo, Question, Skill, TaskEv, Todo, TurnTokens, Usage, UserMsg};

fn approval_policy(mode: Mode) -> &'static str {
    match mode {
        // Yolo-plan runs in the read-only sandbox, so not asking can't change anything.
        Mode::Yolo | Mode::YoloPlan => "never",
        _ => "on-request",
    }
}

/// Ask mode runs in the read-only sandbox too, like Codex's own "ask before changes": every
/// edit and every command that writes then needs the user's approval. (A workspace-write
/// sandbox lets Codex change the workspace unasked, whatever the approval policy says.)
fn sandbox(mode: Mode) -> &'static str {
    match mode {
        Mode::Yolo => "danger-full-access",
        Mode::Ask | Mode::Plan | Mode::YoloPlan => "read-only",
        _ => "workspace-write",
    }
}

fn sandbox_policy(mode: Mode, cwd: &str) -> Value {
    match mode {
        Mode::Yolo => json!({ "type": "dangerFullAccess" }),
        Mode::Ask | Mode::Plan | Mode::YoloPlan => json!({ "type": "readOnly", "networkAccess": false }),
        _ => json!({ "type": "workspaceWrite", "writableRoots": [cwd], "networkAccess": false, "excludeTmpdirEnvVar": false, "excludeSlashTmp": false }),
    }
}

/// `-c` flags that register MCP servers (`{ name: { command, args, env } }`) with Codex.
fn mcp_args(servers: &serde_json::Map<String, Value>) -> Vec<String> {
    let mut args = Vec::new();
    for (name, s) in servers {
        let key = name.replace('-', "_");
        args.extend(["-c".to_string(), format!("mcp_servers.{key}.command={}", s["command"])]);
        args.extend(["-c".to_string(), format!("mcp_servers.{key}.args={}", s["args"])]);
        if let Some(env) = s["env"].as_object().filter(|e| !e.is_empty()) {
            let pairs: Vec<String> = env.iter().map(|(k, v)| format!("{k}={v}")).collect();
            args.extend(["-c".to_string(), format!("mcp_servers.{key}.env={{{}}}", pairs.join(", "))]);
        }
    }
    args
}

/// Questions waiting on the user's answers, by approval id.
enum Ask {
    /// winfer code's own `ask_user` (a dynamic tool call).
    Tool { rpc: Value, questions: Vec<Question> },
    /// Codex's `request_user_input` (plan mode), answered by question id.
    Input { rpc: Value, ids: Vec<String> },
}

/// winfer code's plan tool for Codex, whose app-server has none: `update_plan`, named and
/// shaped like the plan tool GPT models know. Its steps become the TUI's plan.
const UPDATE_PLAN: &str = "update_plan";

fn update_plan_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "explanation": { "type": "string" },
            "plan": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": { "step": { "type": "string" }, "status": { "type": "string", "enum": ["pending", "in_progress", "completed"] } },
                    "required": ["step", "status"],
                },
            },
        },
        "required": ["plan"],
    })
}

/// `update_plan` arguments as todos.
fn plan_todos(args: &Value) -> Vec<Todo> {
    args["plan"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|step| Todo { content: step["step"].as_str().unwrap_or("").to_string(), status: step["status"].as_str().unwrap_or("pending").to_string() })
        .filter(|t| !t.content.is_empty())
        .collect()
}

/// winfer code's dynamic tools for Codex: `ask_user` and `update_plan`.
fn dynamic_tools() -> Value {
    use crate::harness::tools;
    json!([
        { "type": "function", "name": tools::ASK_USER, "description": tools::ASK_USER_DESCRIPTION, "inputSchema": tools::ask_user_schema() },
        {
            "type": "function",
            "name": UPDATE_PLAN,
            "description": "Keep a short plan for multi-step work: send the whole list of steps each time, with at most one in_progress, and mark steps completed as you finish them. The user sees it live.",
            "inputSchema": update_plan_schema(),
        },
    ])
}

/// Codex's `request_user_input` questions (`{ id, header, question, options: [{ label, description }] }`).
fn input_questions(p: &Value) -> (Vec<String>, Vec<Question>) {
    let text = |v: &Value| v.as_str().unwrap_or("").trim().to_string();
    p["questions"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|q| {
            let options = q["options"].as_array().into_iter().flatten().map(|o| (text(&o["label"]), text(&o["description"]))).collect();
            (text(&q["id"]), Question { header: text(&q["header"]), question: text(&q["question"]), options, multi: false })
        })
        .unzip()
}

enum Pending {
    Init,
    Thread,
    Turn,
    Skills,
    /// Stopping these background shells: one terminal (`thread/backgroundTerminals/terminate`),
    /// else all of the thread's (`clean`).
    StopShells { ids: Vec<String>, all: bool },
    /// `thread/revert` for /undo; whether to revert the turn's file changes too.
    Revert { files: bool },
    /// `turn/steer`: the message, to start a turn with if the one it was for already ended.
    Steer(UserMsg),
    Ignore,
}

/// A user turn, for /undo.
#[derive(Default)]
struct UserTurn {
    /// Codex's turn id, once `turn/start` answered.
    id: Option<String>,
    /// File changes Codex applied in it, in order.
    changes: Vec<Value>,
}

struct Bridge {
    events: Events,
    proc: LineProcess,
    next: u64,
    pending: HashMap<u64, Pending>,
    thread: Option<String>,
    turn: Option<String>,
    queued: Vec<UserMsg>,
    mode: Mode,
    bg_owner: String,
    model: Option<String>,
    effort: Option<String>,
    /// Whether we put the thread on Codex's fast tier, so a later non-fast model turns it back.
    fast: bool,
    cwd: String,
    /// Approval requests waiting on the user: our id → (JSON-RPC id, kind).
    approvals: HashMap<String, (Value, &'static str)>,
    /// Questions waiting on the user.
    asks: HashMap<String, Ask>,
    /// Diffs of file-change items, for their result card.
    diffs: HashMap<String, String>,
    /// Subagent threads by thread id. Their events arrive on the same stream as the main
    /// thread's and become progress on the subagent's task instead of transcript items.
    children: HashMap<String, Child>,
    /// The main thread's own token usage, before the subagents' is added.
    usage: Usage,
    /// The running turn's output tokens, and the session's output total when it started.
    turn_tokens: TurnTokens,
    turn_base: u64,
    /// Input tokens of the main thread and its subagents when the running turn started.
    turn_input_base: u64,
    turn_cached_base: u64,
    /// Skills Codex found (`skills/list`): name → SKILL.md path.
    skills: HashMap<String, String>,
    /// The user turns, newest last, for /undo.
    turns: Vec<UserTurn>,
    /// The main thread's commands still running: item id → card title.
    commands: HashMap<String, String>,
    /// Commands that outlived their turn (Codex keeps them running as background terminals),
    /// shown as background shells until they exit.
    background: HashSet<String>,
    /// Running commands' PTY process ids (item id → process id), to stop background shells.
    processes: HashMap<String, String>,
    /// Context fill (%) that triggers trimming / full compaction; 0 = off.
    trim_at: u8,
    compact_at: u8,
    /// A turn just finished: time to see whether to micro-compact.
    settled: bool,
}

#[derive(Default)]
struct Child {
    /// The running turn, for stopping it.
    turn: Option<String>,
    tools: u64,
    /// Token totals of this thread.
    input: u64,
    output: u64,
    cached: u64,
}

pub fn run(opts: Opts, rx: Receiver<In>, me: Sender<In>, events: Events) {
    let mut args = vec!["app-server".to_string()];
    args.extend(mcp_args(&opts.mcp));
    // Lean Codex (Settings → winfer code → Lean setup): on by default for winfer code; Codex CLI
    // panes have their own switch.
    let config = crate::harness::config::Config::load();
    if config.lean_codex {
        args.extend(crate::pty::codex_output_cap());
        args.extend(crate::pty::lean_codex_args(config.lean_codex_prompt));
    }
    let proc = match LineProcess::spawn("codex", &args, &[], &opts.cwd, me.clone()) {
        Ok(p) => p,
        Err(e) => {
            let _ = events.send(Ev::Exited(e));
            return;
        }
    };
    let cwd = opts.cwd.to_string_lossy().into_owned();
    let mut b = Bridge {
        events,
        proc,
        next: 0,
        pending: HashMap::new(),
        thread: None,
        turn: None,
        queued: Vec::new(),
        mode: opts.mode,
        bg_owner: opts.bg_owner.clone(),
        model: Some(opts.model.clone()).filter(|m| !m.is_empty()),
        effort: opts.effort.clone(),
        fast: false,
        cwd: cwd.clone(),
        approvals: HashMap::new(),
        asks: HashMap::new(),
        diffs: HashMap::new(),
        children: HashMap::new(),
        usage: Usage::default(),
        turn_tokens: TurnTokens::default(),
        turn_base: 0,
        turn_input_base: 0,
        turn_cached_base: 0,
        skills: HashMap::new(),
        turns: Vec::new(),
        commands: HashMap::new(),
        background: HashSet::new(),
        processes: HashMap::new(),
        trim_at: opts.trim_at,
        compact_at: opts.compact_at,
        settled: false,
    };
    crate::harness::bg::set_mode(&b.bg_owner, b.mode);
    b.initialize();
    let mut thread_params = json!({
        "cwd": cwd,
        "approvalPolicy": approval_policy(opts.mode),
        "sandbox": sandbox(opts.mode),
    });
    if let Some(m) = &b.model {
        let (model, fast) = split_fast(m);
        thread_params["model"] = json!(model);
        if fast {
            thread_params["serviceTier"] = json!(FAST_TIER);
            b.fast = true;
        }
    }
    if let Some(system) = &opts.system {
        thread_params["developerInstructions"] = json!(system);
    }
    let mut resume = opts.resume.clone();
    // Exits of processes replaced by a micro-compact that ended late.
    let mut stale_exits = 0;
    // New threads keep paginated history, which `thread/revert` (/undo) needs.
    let mut start_params = thread_params.clone();
    start_params["historyMode"] = json!("paginated");
    // winfer code's question tool (Codex's own `request_user_input` only works in its plan mode).
    start_params["dynamicTools"] = dynamic_tools();

    while let Some(msg) = super::recv(&rx) {
        match msg {
            In::Line(line) => {
                let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
                if let Some(id) = v.get("id").filter(|_| v.get("method").is_none()) {
                    // A response to one of our requests.
                    let kind = id.as_u64().and_then(|id| b.pending.remove(&id));
                    if let Some(err) = v.get("error") {
                        let message = err["message"].as_str().unwrap_or("request failed").to_string();
                        if matches!(kind, Some(Pending::Turn)) {
                            b.emit(Ev::Error(format!("Codex: {message}")));
                            b.emit(Ev::TurnDone);
                        } else if let Some(Pending::Steer(msg)) = kind {
                            // The turn ended (or can't be steered) meanwhile: the message gets its own.
                            b.emit(Ev::TurnStart);
                            b.start_turn(msg);
                        } else if let Some(Pending::StopShells { ids, all: false }) = kind {
                            // Its process id didn't work: clean the thread's terminals instead.
                            b.clean_shells(ids);
                        } else if matches!(kind, Some(Pending::Revert { .. })) {
                            b.emit(Ev::Undone(Err(format!("Codex couldn't drop the turn: {message}"))));
                        } else if matches!(kind, Some(Pending::StopShells { .. })) {
                            b.emit(Ev::Notice(format!("Couldn't stop the background shells: {message}")));
                        } else if matches!(kind, Some(Pending::Thread)) && resume.is_some() {
                            b.emit(Ev::Notice(format!("Couldn't resume that Codex thread ({message}); starting a new one.")));
                            b.request("thread/start", start_params.clone(), Pending::Thread);
                        } else {
                            b.emit(Ev::Error(format!("Codex: {message}")));
                        }
                        continue;
                    }
                    let result = &v["result"];
                    match kind {
                        Some(Pending::Init) => {
                            let _ = b.proc.write(&json!({ "method": "initialized" }));
                            b.request("skills/list", json!({ "cwds": [b.cwd.clone()] }), Pending::Skills);
                            match &resume {
                                Some(thread) => {
                                    let mut p = thread_params.clone();
                                    p["threadId"] = json!(thread);
                                    p["excludeTurns"] = json!(true);
                                    b.request("thread/resume", p, Pending::Thread);
                                }
                                None => b.request("thread/start", start_params.clone(), Pending::Thread),
                            }
                        }
                        Some(Pending::Thread) => {
                            let id = result["thread"]["id"].as_str().unwrap_or("").to_string();
                            b.emit(Ev::Session(id.clone()));
                            if let Some(m) = result["model"].as_str() {
                                let fast = result["serviceTier"].as_str() == Some(FAST_TIER);
                                b.emit(Ev::Model(if fast { format!("{m}{FAST_SUFFIX}") } else { m.to_string() }));
                            }
                            b.thread = Some(id);
                            for msg in std::mem::take(&mut b.queued) {
                                b.start_turn(msg);
                            }
                        }
                        Some(Pending::Turn) => {
                            if let Some(id) = result["turn"]["id"].as_str() {
                                b.turn = Some(id.to_string());
                                if let Some(t) = b.turns.last_mut().filter(|t| t.id.is_none()) {
                                    t.id = Some(id.to_string());
                                }
                            }
                        }
                        Some(Pending::Revert { files }) => b.reverted(files),
                        Some(Pending::Skills) => {
                            let list = b.take_skills(result);
                            b.emit(Ev::Skills(list));
                        }
                        Some(Pending::StopShells { ids, .. }) => b.shells_stopped(&ids),
                        _ => {}
                    }
                } else if let Some(method) = v["method"].as_str() {
                    match v.get("id") {
                        Some(id) => b.on_request(method, id.clone(), &v["params"]),
                        None => b.on_notification(method, &v["params"]),
                    }
                }
                if let Some((thread, path)) = b.micro_compact_file() {
                    // Stop Codex, trim the rollout, and resume the thread in a new app-server;
                    // what's sent meanwhile waits for the thread (`queued`).
                    let _ = b.proc.child.kill();
                    let _ = b.proc.child.wait();
                    let mut held = Vec::new();
                    stale_exits += usize::from(!super::drain_closed(&rx, &mut held));
                    let saved = microcompact::trim_file(&path, Format::Codex, 1, true).unwrap_or(0);
                    b.proc = match LineProcess::spawn("codex", &args, &[], &opts.cwd, me.clone()) {
                        Ok(p) => p,
                        Err(e) => {
                            b.emit(Ev::Exited(format!("Codex didn't restart after trimming its context: {e}")));
                            return;
                        }
                    };
                    b.pending.clear();
                    b.thread = None;
                    resume = Some(thread);
                    b.initialize();
                    if saved > 0 {
                        b.usage.context = b.usage.context.saturating_sub(saved as u64 / 4);
                        b.emit_usage();
                        b.emit(Ev::Notice(format!("Micro-compacted: trimmed {} KB of old tool output.", saved / 1024)));
                    }
                    for cmd in held {
                        let _ = me.send(In::Cmd(cmd));
                    }
                }
            }
            In::Closed if stale_exits > 0 => stale_exits -= 1,
            In::Closed => {
                let reason = b.proc.exit_reason();
                b.emit(Ev::Exited(format!("Codex stopped: {reason}")));
                return;
            }
            In::Cmd(cmd) => match cmd {
                Cmd::Send(msg) => {
                    b.emit(Ev::TurnStart);
                    if b.thread.is_some() {
                        b.start_turn(msg);
                    } else {
                        b.queued.push(msg);
                    }
                }
                Cmd::Steer(msg) => match (b.thread.clone(), b.turn.clone()) {
                    (Some(thread), Some(turn)) => {
                        let input = b.input(&msg);
                        b.request("turn/steer", json!({ "threadId": thread, "expectedTurnId": turn, "input": input }), Pending::Steer(msg));
                    }
                    _ => {
                        b.emit(Ev::TurnStart);
                        match b.thread {
                            Some(_) => b.start_turn(msg),
                            None => b.queued.push(msg),
                        }
                    }
                },
                Cmd::Interrupt => {
                    if let (Some(thread), Some(turn)) = (b.thread.clone(), b.turn.clone()) {
                        b.request("turn/interrupt", json!({ "threadId": thread, "turnId": turn }), Pending::Ignore);
                    }
                }
                Cmd::Approve(id, _) if b.asks.contains_key(&id) => b.answer(&id, None),
                Cmd::Answer(id, answers) => b.answer(&id, Some(answers)),
                Cmd::Approve(id, decision) => {
                    let Some((rpc, _kind)) = b.approvals.remove(&id) else { continue };
                    let decision = match decision {
                        Decision::Allow => "accept",
                        Decision::Always => "acceptForSession",
                        Decision::Deny => "decline",
                    };
                    let _ = b.proc.write(&json!({ "id": rpc, "result": { "decision": decision } }));
                }
                Cmd::SetModel(m) => b.model = Some(m),
                Cmd::SetEffort(e) => b.effort = e,
                Cmd::SetMode(m) => {
                    b.mode = m;
                    crate::harness::bg::set_mode(&b.bg_owner, m);
                }
                Cmd::Compact => {
                    if let Some(thread) = b.thread.clone() {
                        b.emit(Ev::TurnStart);
                        b.request("thread/compact/start", json!({ "threadId": thread }), Pending::Ignore);
                    }
                }
                Cmd::SetCompaction { trim, compact } => {
                    b.trim_at = trim;
                    b.compact_at = compact;
                }
                Cmd::Undo { files } => b.undo(files),
                Cmd::StopTask(id) if b.background.contains(&id) => b.stop_shell(id),
                Cmd::StopTask(id) if crate::harness::bg::stop(&id) => {}
                Cmd::StopTask(thread) => {
                    match b.children.get(&thread).and_then(|c| c.turn.clone()) {
                        Some(turn) => b.request("turn/interrupt", json!({ "threadId": thread, "turnId": turn }), Pending::Ignore),
                        None => b.emit(Ev::Notice("That subagent isn't running.".into())),
                    }
                }
                Cmd::Shutdown => return,
            },
        }
    }
}

impl Bridge {
    fn initialize(&mut self) {
        self.request("initialize", json!({ "clientInfo": { "name": "winfer", "title": "winfer code", "version": env!("CARGO_PKG_VERSION") }, "capabilities": { "experimentalApi": true } }), Pending::Init);
    }

    /// The thread and its rollout, when it's time to micro-compact: the context is past
    /// `trim_at`, the thread is idle with nothing a restart would cut short (approvals, background
    /// terminals, subagents, requests in flight), and trimming saves enough.
    fn micro_compact_file(&mut self) -> Option<(String, std::path::PathBuf)> {
        if !std::mem::take(&mut self.settled)
            || self.turn.is_some()
            || !self.queued.is_empty()
            || !self.approvals.is_empty()
            || !self.asks.is_empty()
            || !self.background.is_empty()
            || self.children.values().any(|c| c.turn.is_some())
            || self.pending.values().any(|p| !matches!(p, Pending::Skills | Pending::Ignore))
            || !microcompact::due(&self.usage, self.trim_at, self.compact_at)
        {
            return None;
        }
        let thread = self.thread.clone()?;
        let path = microcompact::codex_rollout(&thread)?;
        (microcompact::trim_file(&path, Format::Codex, microcompact::MIN_SAVING, false).ok()? > 0).then_some((thread, path))
    }

    fn emit(&self, ev: Ev) {
        let _ = self.events.send(ev);
    }

    fn request(&mut self, method: &str, params: Value, kind: Pending) {
        self.next += 1;
        self.pending.insert(self.next, kind);
        let _ = self.proc.write(&json!({ "id": self.next, "method": method, "params": params }));
    }

    /// Kills a command that outlived its turn: its own terminal when Codex told us its process,
    /// else every background terminal of the thread.
    fn stop_shell(&mut self, id: String) {
        let Some(thread) = self.thread.clone() else { return };
        match self.processes.get(&id).cloned() {
            Some(pid) => self.request("thread/backgroundTerminals/terminate", json!({ "threadId": thread, "processId": pid }), Pending::StopShells { ids: vec![id], all: false }),
            None => self.clean_shells(vec![id]),
        }
    }

    /// Kills every background terminal of the thread; `ids` (and every other background shell)
    /// end as stopped.
    fn clean_shells(&mut self, mut ids: Vec<String>) {
        let Some(thread) = self.thread.clone() else { return };
        ids.extend(self.background.iter().filter(|b| !ids.contains(b)).cloned().collect::<Vec<_>>());
        self.request("thread/backgroundTerminals/clean", json!({ "threadId": thread }), Pending::StopShells { ids, all: true });
    }

    /// Codex killed these background shells; their tasks end as stopped (their command items may
    /// never complete once the process is gone).
    fn shells_stopped(&mut self, ids: &[String]) {
        for id in ids {
            self.processes.remove(id);
            if self.background.remove(id) {
                self.task(TaskEv { id: id.clone(), status: Some("stopped".into()), ..Default::default() });
            }
        }
    }

    fn start_turn(&mut self, msg: UserMsg) {
        let Some(thread) = self.thread.clone() else { return };
        self.turns.push(UserTurn::default());
        let input = self.input(&msg);
        let mut params = json!({
            "threadId": thread,
            "input": input,
            "approvalPolicy": approval_policy(self.mode),
            "sandboxPolicy": sandbox_policy(self.mode, &self.cwd),
        });
        if let Some(m) = &self.model {
            let (model, fast) = split_fast(m);
            params["model"] = json!(model);
            if fast {
                params["serviceTier"] = json!(FAST_TIER);
            } else if self.fast {
                params["serviceTier"] = json!("default");
            }
            self.fast = fast;
        }
        if let Some(e) = &self.effort {
            params["effort"] = json!(e);
        }
        self.request("turn/start", params, Pending::Turn);
    }

    /// A message as Codex's `UserInput` list. An explicit skill goes as Codex's own skill input
    /// (what `$name` does in its TUI), so the model gets the SKILL.md itself instead of finding
    /// and reading it.
    fn input(&self, msg: &UserMsg) -> Vec<Value> {
        let (text, skill) = match &msg.skill {
            Some(name) => (format!("${name} {}", msg.text).trim_end().to_string(), self.skills.get(name).map(|path| json!({ "type": "skill", "name": name, "path": path }))),
            None => (msg.text.clone(), None),
        };
        let mut input = vec![json!({ "type": "text", "text": text, "text_elements": [] })];
        input.extend(skill);
        for path in &msg.images {
            input.push(json!({ "type": "localImage", "path": path }));
        }
        input
    }

    /// Drops the last user turn from the thread (`thread/revert`, which leaves files alone);
    /// [`Self::reverted`] finishes once Codex has.
    fn undo(&mut self, files: bool) {
        if self.turn.is_some() {
            return self.emit(Ev::Undone(Err("Wait for the turn to finish (or press Esc) before /undo.".into())));
        }
        let (Some(thread), Some(turn)) = (self.thread.clone(), self.turns.last()) else {
            return self.emit(Ev::Undone(Err("Nothing to undo.".into())));
        };
        let Some(id) = turn.id.clone() else {
            return self.emit(Ev::Undone(Err("Codex never started that turn.".into())));
        };
        self.request("thread/revert", json!({ "threadId": thread, "beforeTurnId": id }), Pending::Revert { files });
    }

    /// Codex dropped the last user turn; with `files`, its patches are undone here, newest first
    /// (edits made through shell commands aren't tracked).
    fn reverted(&mut self, files: bool) {
        let Some(turn) = self.turns.pop() else { return };
        let cwd = std::path::PathBuf::from(&self.cwd);
        let mut names = Vec::new();
        let mut failed = Vec::new();
        for c in turn.changes.iter().rev().filter(|_| files) {
            let name = crate::harness::tools::short_path(c["path"].as_str().unwrap_or(""));
            match revert_change(&cwd, c) {
                Ok(()) => names.push(name),
                Err(e) => failed.push(format!("{name} ({e})")),
            }
        }
        names.sort();
        names.dedup();
        if !failed.is_empty() {
            self.emit(Ev::Error(format!("Couldn't revert {}", failed.join(", "))));
        }
        self.emit(Ev::Undone(Ok(names)));
    }

    /// Remembers the enabled skills of a `skills/list` result and returns them for the TUI.
    fn take_skills(&mut self, result: &Value) -> Vec<Skill> {
        let mut list = Vec::new();
        for entry in result["data"].as_array().into_iter().flatten() {
            for sk in entry["skills"].as_array().into_iter().flatten() {
                let name = s(sk, "name");
                if name.is_empty() || sk["enabled"] == false || self.skills.contains_key(&name) {
                    continue;
                }
                self.skills.insert(name.clone(), s(sk, "path"));
                let description = sk["shortDescription"].as_str().filter(|d| !d.is_empty()).map(str::to_string).unwrap_or_else(|| s(sk, "description"));
                list.push(Skill { name, description });
            }
        }
        list
    }

    /// Answers questions (`None`: the user dismissed them).
    fn answer(&mut self, id: &str, answers: Option<Vec<String>>) {
        use crate::harness::tools;
        let Some(ask) = self.asks.remove(id) else { return };
        let msg = match ask {
            Ask::Tool { rpc, questions } => {
                let (ok, text) = match answers {
                    Some(a) => (true, tools::ask_user_result(&questions, &a)),
                    None => (false, tools::ASK_USER_DECLINED.to_string()),
                };
                json!({ "id": rpc, "result": { "success": ok, "contentItems": [{ "type": "inputText", "text": text }] } })
            }
            Ask::Input { rpc, ids } => {
                let answers: serde_json::Map<String, Value> =
                    ids.into_iter().zip(answers.unwrap_or_default()).map(|(id, a)| (id, json!({ "answers": [a] }))).collect();
                json!({ "id": rpc, "result": { "answers": answers } })
            }
        };
        let _ = self.proc.write(&msg);
    }

    /// Questions for the user: winfer code's `ask_user` and Codex's `request_user_input`.
    fn on_ask(&mut self, method: &str, id: Value, p: &Value) {
        use crate::harness::tools;
        let key = format!("codex-{id}");
        let (ask, questions) = if method == "item/tool/call" {
            if p["tool"].as_str() == Some(UPDATE_PLAN) {
                // Only the main thread's plan shows; a subagent's steps would replace it.
                let main = p["threadId"].as_str().is_none_or(|t| !self.children.contains_key(t));
                let todos = plan_todos(&p["arguments"]);
                let ok = !todos.is_empty();
                if ok && main {
                    self.emit(Ev::Todos(todos));
                }
                let text = if ok { "Plan updated." } else { "The plan needs at least one step." };
                let _ = self.proc.write(&json!({ "id": id, "result": { "success": ok, "contentItems": [{ "type": "inputText", "text": text }] } }));
                return;
            }
            if p["tool"].as_str() != Some(tools::ASK_USER) {
                let text = format!("No tool {} here.", p["tool"].as_str().unwrap_or(""));
                let _ = self.proc.write(&json!({ "id": id, "result": { "success": false, "contentItems": [{ "type": "inputText", "text": text }] } }));
                return;
            }
            let questions = Question::parse_all(&p["arguments"]);
            (Ask::Tool { rpc: id.clone(), questions: questions.clone() }, questions)
        } else {
            let (ids, questions) = input_questions(p);
            (Ask::Input { rpc: id.clone(), ids }, questions)
        };
        if questions.is_empty() {
            self.asks.insert(key.clone(), ask);
            return self.answer(&key, None);
        }
        let title = questions[0].question.clone();
        let detail = if p["threadId"].as_str().is_some_and(|t| self.children.contains_key(t)) { "Asked by a subagent.".into() } else { String::new() };
        self.asks.insert(key.clone(), ask);
        self.emit(Ev::Approval(Approval { id: key, tool: "Question".into(), title, detail, questions, ..Default::default() }));
    }

    fn on_request(&mut self, method: &str, id: Value, p: &Value) {
        if matches!(method, "item/tool/call" | "item/tool/requestUserInput") {
            return self.on_ask(method, id, p);
        }
        let key = format!("codex-{id}");
        let approval = match method {
            "item/commandExecution/requestApproval" | "execCommandApproval" => {
                let raw = p["command"].as_str().unwrap_or("");
                let command = strip_shell(raw);
                Some(Approval {
                    id: key.clone(),
                    tool: shell_tool(raw).into(),
                    title: command,
                    detail: p["reason"].as_str().unwrap_or("").to_string(),
                    diff: None,
                    ..Default::default()
                })
            }
            "item/fileChange/requestApproval" | "applyPatchApproval" => {
                let item = p["itemId"].as_str().unwrap_or("");
                Some(Approval {
                    id: key.clone(),
                    tool: "Edit".into(),
                    title: p["grantRoot"].as_str().map(|r| format!("write access to {r}")).unwrap_or_else(|| "apply changes".into()),
                    detail: p["reason"].as_str().unwrap_or("").to_string(),
                    diff: self.diffs.get(item).cloned(),
                    ..Default::default()
                })
            }
            "item/permissions/requestApproval" => Some(Approval {
                id: key.clone(),
                tool: "Permissions".into(),
                title: p["reason"].as_str().unwrap_or("more access").to_string(),
                detail: String::new(),
                diff: None,
                ..Default::default()
            }),
            _ => None,
        };
        match approval {
            Some(mut a) => {
                if p["threadId"].as_str().is_some_and(|t| self.children.contains_key(t)) && a.detail.is_empty() {
                    a.detail = "Asked by a subagent.".into();
                }
                self.approvals.insert(key, (id, "approval"));
                self.emit(Ev::Approval(a));
            }
            None => {
                // Requests we don't support (MCP elicitation, auth refresh): refuse politely.
                let _ = self.proc.write(&json!({ "id": id, "error": { "code": -32601, "message": format!("winfer code doesn't support {method}") } }));
            }
        }
    }

    fn on_notification(&mut self, method: &str, p: &Value) {
        if let Some(thread) = p["threadId"].as_str().filter(|t| self.thread.as_deref().is_some_and(|main| main != *t)) {
            let thread = thread.to_string();
            return self.on_child(&thread, method, p);
        }
        match method {
            "turn/started" => {
                self.turn = p["turn"]["id"].as_str().map(str::to_string);
                self.turn_tokens = TurnTokens::default();
                self.turn_base = self.output_total();
                self.turn_input_base = self.usage.input + self.children.values().map(|c| c.input).sum::<u64>();
                self.turn_cached_base = self.usage.cached + self.children.values().map(|c| c.cached).sum::<u64>();
            }
            "turn/completed" => {
                let turn = &p["turn"];
                if turn["status"] == "failed" {
                    let message = turn["error"]["message"].as_str().unwrap_or("the turn failed");
                    self.emit(Ev::Error(format!("Codex: {message}")));
                }
                self.turn = None;
                // Codex ends the turn without waiting for long commands (watchers, servers): they
                // run on as background terminals, so their cards become background shells
                // instead of being failed with the turn. Codex gives them no name of their own,
                // so they're named by a short form of the command, not the whole line.
                for (id, command) in std::mem::take(&mut self.commands) {
                    let title = crate::harness::bg::short_command(&command);
                    self.task(TaskEv { id: id.clone(), tool_id: Some(id.clone()), title: Some(title), status: Some("running".into()), ..Default::default() });
                    self.background.insert(id);
                }
                self.settled = true;
                self.emit(Ev::TurnDone);
            }
            "error" => {
                if p["willRetry"].as_bool() != Some(true) {
                    let message = p["error"]["message"].as_str().unwrap_or("error");
                    self.emit(Ev::Error(format!("Codex: {message}")));
                }
            }
            "item/agentMessage/delta" => {
                self.streamed(p);
                self.emit(Ev::TextDelta { id: s(p, "itemId"), text: s(p, "delta") });
            }
            "item/reasoning/summaryTextDelta" | "item/reasoning/textDelta" => {
                self.streamed(p);
                self.emit(Ev::ReasoningDelta { id: s(p, "itemId"), text: s(p, "delta") });
            }
            "item/reasoning/summaryPartAdded" => self.emit(Ev::ReasoningDelta { id: s(p, "itemId"), text: "\n\n".into() }),
            "item/commandExecution/outputDelta" => self.emit(Ev::ToolOutput { id: s(p, "itemId"), chunk: s(p, "delta") }),
            "turn/plan/updated" => {
                let todos = p["plan"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|step| Todo {
                        content: step["step"].as_str().unwrap_or("").to_string(),
                        status: match step["status"].as_str() {
                            Some("inProgress") => "in_progress".into(),
                            Some(other) => other.to_string(),
                            None => "pending".into(),
                        },
                    })
                    .collect();
                self.emit(Ev::Todos(todos));
            }
            "thread/tokenUsage/updated" => {
                let u = &p["tokenUsage"];
                let n = |part: &str, k: &str| u[part][k].as_u64().unwrap_or(0);
                self.usage = Usage {
                    context: n("last", "inputTokens") + n("last", "outputTokens"),
                    window: u["modelContextWindow"].as_u64().unwrap_or(272_000),
                    input: n("total", "inputTokens"),
                    output: n("total", "outputTokens"),
                    cached: n("total", "cachedInputTokens"),
                    cost: None,
                    turn: 0,
                    turn_input: 0,
                    turn_cached: 0,
                    turn_streamed: 0,
                };
                self.counted();
            }
            "thread/compacted" => self.emit(Ev::Notice("Context compacted.".into())),
            "model/rerouted" => {
                if let Some(m) = p["toModel"].as_str().or_else(|| p["model"].as_str()) {
                    self.emit(Ev::Model(m.to_string()));
                }
            }
            "item/started" => self.item_started(&p["item"]),
            "item/completed" => self.item_completed(&p["item"]),
            _ => {}
        }
    }

    /// The main usage plus every subagent's tokens (their context isn't the main one's).
    fn emit_usage(&self) {
        let mut u = self.usage.clone();
        for c in self.children.values() {
            u.input += c.input;
            u.output += c.output;
            u.cached += c.cached;
        }
        u.turn = self.turn_tokens.now();
        u.turn_streamed = self.turn_tokens.streamed_total();
        u.turn_input = u.input.saturating_sub(self.turn_input_base);
        u.turn_cached = u.cached.saturating_sub(self.turn_cached_base);
        self.emit(Ev::Usage(u));
    }

    /// Output tokens of the main thread and its subagents.
    fn output_total(&self) -> u64 {
        self.usage.output + self.children.values().map(|c| c.output).sum::<u64>()
    }

    /// Codex counts tokens per request; the text streamed in between counts as an estimate.
    fn streamed(&mut self, p: &Value) {
        if self.turn_tokens.streamed(p["delta"].as_str().unwrap_or("")) {
            self.emit_usage();
        }
    }

    /// Codex's exact totals arrived: the turn's output is what grew since it started.
    fn counted(&mut self) {
        // Not only during the turn: the last count can land just after `turn/completed`.
        let n = self.output_total().saturating_sub(self.turn_base);
        self.turn_tokens.exact(n);
        self.emit_usage();
    }

    fn task(&self, t: TaskEv) {
        self.emit(Ev::Task(t));
    }

    /// An event of a subagent's thread: progress on its task.
    fn on_child(&mut self, thread: &str, method: &str, p: &Value) {
        let task = |status: Option<&str>| TaskEv { id: thread.to_string(), agent: Some("codex".into()), status: status.map(str::to_string), ..Default::default() };
        match method {
            "turn/started" => {
                self.children.entry(thread.to_string()).or_default().turn = p["turn"]["id"].as_str().map(str::to_string);
                self.task(task(Some("running")));
            }
            "turn/completed" => {
                self.children.entry(thread.to_string()).or_default().turn = None;
                let status = match p["turn"]["status"].as_str() {
                    Some("failed") => "failed",
                    Some("interrupted") => "stopped",
                    _ => "completed",
                };
                self.task(task(Some(status)));
            }
            "item/started" => {
                let item = &p["item"];
                // A subagent's own subagents.
                if item["type"] == "subAgentActivity" {
                    return self.sub_agent_activity(item, false);
                }
                let Some((name, _)) = self.tool_card(item) else { return };
                let child = self.children.entry(thread.to_string()).or_default();
                child.tools += 1;
                let tools = child.tools;
                self.task(TaskEv { tools: Some(tools), last: Some(name), ..task(None) });
            }
            "item/completed" => {
                self.diffs.remove(p["item"]["id"].as_str().unwrap_or(""));
            }
            "thread/tokenUsage/updated" => {
                let u = &p["tokenUsage"]["total"];
                let child = self.children.entry(thread.to_string()).or_default();
                child.input = u["inputTokens"].as_u64().unwrap_or(0);
                child.output = u["outputTokens"].as_u64().unwrap_or(0);
                child.cached = u["cachedInputTokens"].as_u64().unwrap_or(0);
                self.counted();
            }
            _ => {}
        }
    }

    /// `subAgentActivity` (Codex 0.16x): a subagent started, was messaged, stopped or finished.
    /// Starting one shows an Agent card that returns at once; the agent runs on as its task.
    /// `card`: show it in the transcript (it's the main thread's).
    fn sub_agent_activity(&mut self, item: &Value, card: bool) {
        let thread = s(item, "agentThreadId");
        if thread.is_empty() {
            return;
        }
        let name = agent_name(&s(item, "agentPath"));
        let status = match item["kind"].as_str() {
            Some("started") => {
                self.children.entry(thread.clone()).or_default();
                let id = s(item, "id");
                if card {
                    self.emit(Ev::ToolStart { id: id.clone(), name: "Agent".into(), title: name.clone(), input: item.clone() });
                    self.emit(Ev::ToolEnd { id: id.clone(), ok: true, output: format!("Started {name}."), diff: None });
                }
                self.task(TaskEv { id: thread, tool_id: Some(id).filter(|_| card), title: Some(name), agent: Some("codex".into()), status: Some("running".into()), ..Default::default() });
                return;
            }
            Some("completed") => "completed",
            Some("interrupted") => "stopped",
            _ => return,
        };
        self.task(TaskEv { id: thread, agent: Some("codex".into()), status: Some(status.into()), ..Default::default() });
    }

    /// The card a tool-like item shows as: (name, title).
    fn tool_card(&mut self, item: &Value) -> Option<(String, String)> {
        let id = s(item, "id");
        Some(match item["type"].as_str().unwrap_or("") {
            "commandExecution" => {
                let raw = item["command"].as_str().unwrap_or("");
                let command = strip_shell(raw);
                match skill_reads(&command) {
                    Some(names) => ("Skill".to_string(), names),
                    None => (shell_tool(raw).to_string(), command),
                }
            }
            "fileChange" => {
                let diff = changes_diff(item);
                self.diffs.insert(id, diff);
                ("Edit".into(), changed_paths(item))
            }
            "mcpToolCall" if s(item, "tool") == "bg" => (format!("{} · bg", s(item, "server")), crate::harness::tools::title("bg", &item["arguments"])),
            "mcpToolCall" => (format!("{} · {}", s(item, "server"), s(item, "tool")), first_arg(&item["arguments"])),
            "dynamicToolCall" if s(item, "tool") == crate::harness::tools::ASK_USER => {
                ("Question".into(), item["arguments"]["questions"][0]["question"].as_str().unwrap_or("").to_string())
            }
            // The plan shows as the TUI's plan, not as a card ("Todos" cards are skipped).
            "dynamicToolCall" if s(item, "tool") == UPDATE_PLAN => ("Todos".into(), "update plan".into()),
            "dynamicToolCall" => (s(item, "tool"), first_arg(&item["arguments"])),
            "webSearch" => ("WebSearch".into(), s(item, "query")),
            "collabAgentToolCall" => {
                let prompt = s(item, "prompt").lines().next().unwrap_or("").to_string();
                match item["tool"].as_str().unwrap_or("") {
                    "spawnAgent" => ("Agent".into(), prompt),
                    "wait" => ("WaitAgents".into(), agent_count(item)),
                    "sendInput" | "sendMessage" | "followupTask" => ("MessageAgent".into(), prompt),
                    "resumeAgent" => ("ResumeAgent".into(), agent_count(item)),
                    "closeAgent" => ("CloseAgent".into(), agent_count(item)),
                    "interruptAgent" => ("InterruptAgent".into(), agent_count(item)),
                    "listAgents" => ("ListAgents".into(), String::new()),
                    other => (other.to_string(), prompt),
                }
            }
            "imageView" => ("ViewImage".into(), crate::harness::tools::short_path(&s(item, "path"))),
            _ => return None,
        })
    }

    fn item_started(&mut self, item: &Value) {
        if item["type"] == "subAgentActivity" {
            return self.sub_agent_activity(item, true);
        }
        let Some((name, title)) = self.tool_card(item) else { return };
        if item["type"] == "commandExecution" {
            self.commands.insert(s(item, "id"), title.clone());
            if let Some(pid) = item["processId"].as_str() {
                self.processes.insert(s(item, "id"), pid.to_string());
            }
        }
        self.emit(Ev::ToolStart { id: s(item, "id"), name, title, input: item.clone() });
    }

    /// A finished collab call (Codex before 0.16x): what it says about each agent it touched.
    fn collab_done(&mut self, item: &Value) -> String {
        let id = s(item, "id");
        let spawn = item["tool"] == "spawnAgent";
        let mut lines = Vec::new();
        for thread in item["receiverThreadIds"].as_array().into_iter().flatten().filter_map(Value::as_str) {
            self.children.entry(thread.to_string()).or_default();
            let state = &item["agentsStates"][thread];
            let status = match state["status"].as_str() {
                Some("pendingInit" | "running") => Some("running"),
                Some("completed") => Some("completed"),
                Some("errored" | "notFound") => Some("failed"),
                Some("interrupted" | "shutdown") => Some("stopped"),
                _ if spawn => Some("running"),
                _ => None,
            };
            let title = spawn.then(|| s(item, "prompt").lines().next().unwrap_or("").to_string()).filter(|t| !t.is_empty());
            self.task(TaskEv {
                id: thread.to_string(),
                tool_id: Some(id.clone()).filter(|_| spawn),
                title,
                agent: Some("codex".into()),
                status: status.map(str::to_string),
                ..Default::default()
            });
            if let Some(m) = state["message"].as_str().filter(|m| !m.is_empty()) {
                lines.push(m.to_string());
            }
        }
        lines.join("\n\n")
    }

    fn item_completed(&mut self, item: &Value) {
        let id = s(item, "id");
        match item["type"].as_str().unwrap_or("") {
            "collabAgentToolCall" => {
                let output = self.collab_done(item);
                let ok = item["status"] != "failed";
                self.emit(Ev::ToolEnd { id, ok, output, diff: None });
            }
            "agentMessage" => self.emit(Ev::Text { id, text: s(item, "text") }),
            "commandExecution" => {
                let ok = item["exitCode"].as_i64() == Some(0) || item["status"] == "completed" && item["exitCode"].is_null();
                self.commands.remove(&id);
                self.processes.remove(&id);
                if self.background.remove(&id) {
                    let exit_code = item["exitCode"].as_i64().and_then(|n| i32::try_from(n).ok());
                    let status = if ok { "completed" } else { "failed" };
                    self.task(TaskEv { id: id.clone(), status: Some(status.into()), exit_code, ..Default::default() });
                }
                self.emit(Ev::ToolEnd { id, ok, output: s(item, "aggregatedOutput"), diff: None });
            }
            "fileChange" => {
                let diff = changes_diff(item);
                self.diffs.remove(&id);
                let ok = item["status"] == "completed";
                if let Some(turn) = self.turns.last_mut().filter(|_| ok) {
                    turn.changes.extend(item["changes"].as_array().into_iter().flatten().cloned());
                }
                self.emit(Ev::ToolEnd { id, ok, output: String::new(), diff: Some(diff).filter(|d| !d.is_empty()) });
            }
            "dynamicToolCall" => {
                let ok = item["success"].as_bool() != Some(false);
                let output = item["contentItems"].as_array().into_iter().flatten().filter_map(|c| c["text"].as_str()).collect::<Vec<_>>().join("\n");
                self.emit(Ev::ToolEnd { id, ok, output, diff: None });
            }
            "mcpToolCall" => {
                let ok = item["error"].is_null();
                let output = if ok {
                    item["result"]["content"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|c| c["text"].as_str())
                        .collect::<Vec<_>>()
                        .join("\n")
                } else {
                    item["error"]["message"].as_str().unwrap_or("failed").to_string()
                };
                self.emit(Ev::ToolEnd { id, ok, output, diff: None });
            }
            "webSearch" | "imageView" => self.emit(Ev::ToolEnd { id, ok: true, output: String::new(), diff: None }),
            "contextCompaction" => self.emit(Ev::Notice("Context compacted.".into())),
            _ => {}
        }
    }
}

// No lean mode for Codex: its default instructions and tool list are a prefix OpenAI's prompt
// cache already holds, so a shorter custom setup measured *more* full-price tokens (bench: 8.6k
// stock vs 26.1k with a lean prompt, 20.4k with just unused features off).

/// Codex loads a skill by reading its SKILL.md with the shell: `cat ~/.agents/skills/caveman/SKILL.md`
/// (or `Get-Content -LiteralPath 'C:\…\caveman\SKILL.md'` on Windows) → `caveman`. Commands that
/// do anything else stay Bash cards.
fn skill_reads(command: &str) -> Option<String> {
    let command = command.replace('\\', "/");
    let mut words = command.split_whitespace();
    let reader = words.next()?.to_ascii_lowercase();
    let readers = ["cat", "sed", "head", "nl", "less", "bat", "get-content", "gc", "type"];
    if !readers.contains(&reader.as_str()) || command.contains(['|', ';', '>', '&']) {
        return None;
    }
    let names: Vec<&str> = words
        .filter_map(|w| w.trim_matches(['\'', '"']).strip_suffix("/SKILL.md"))
        .filter_map(|dir| dir.trim_end_matches('/').rsplit('/').next())
        .filter(|n| !n.is_empty())
        .collect();
    (!names.is_empty()).then(|| names.join(", "))
}

fn s(v: &Value, k: &str) -> String {
    v[k].as_str().unwrap_or("").to_string()
}

/// `/root/auth_probe` → `auth probe`.
fn agent_name(path: &str) -> String {
    let last = path.rsplit('/').find(|p| !p.is_empty()).unwrap_or("subagent");
    last.replace(['_', '-'], " ")
}

/// "2 agents" for a collab call that targets agents.
fn agent_count(item: &Value) -> String {
    match item["receiverThreadIds"].as_array().map_or(0, Vec::len) {
        0 => String::new(),
        1 => "1 agent".into(),
        n => format!("{n} agents"),
    }
}

fn first_arg(args: &Value) -> String {
    args.as_object()
        .and_then(|o| o.values().find_map(|v| v.as_str().map(str::to_string)))
        .unwrap_or_default()
}

/// `/bin/zsh -lc 'ls -la'` → `ls -la`.
fn strip_shell(command: &str) -> String {
    let unquote = |rest: &str| {
        let rest = rest.trim();
        shell_word(rest).unwrap_or_else(|| rest.to_string())
    };
    for prefix in ["/bin/zsh -lc ", "/bin/bash -lc ", "bash -lc ", "zsh -lc ", "/bin/sh -c "] {
        if let Some(rest) = command.strip_prefix(prefix) {
            return unquote(rest);
        }
    }
    strip_powershell(command).map(unquote).unwrap_or_else(|| command.to_string())
}

/// The text of one quoted POSIX shell word, the way Codex quotes the script it runs:
/// `'it'"'"'s'` and `'$a='"'Stop'; "'$b'` (shlex mixes quote styles) → `it's`, `$a='Stop'; $b`.
/// None when it isn't a single quoted word.
fn shell_word(word: &str) -> Option<String> {
    if !word.starts_with(['\'', '"']) {
        return None;
    }
    let mut out = String::new();
    let mut chars = word.chars();
    while let Some(c) = chars.next() {
        match c {
            '\'' => loop {
                match chars.next()? {
                    '\'' => break,
                    c => out.push(c),
                }
            },
            '"' => loop {
                match chars.next()? {
                    '"' => break,
                    '\\' => match chars.next()? {
                        c @ ('"' | '\\' | '$' | '`') => out.push(c),
                        '\n' => {}
                        c => {
                            out.push('\\');
                            out.push(c);
                        }
                    },
                    c => out.push(c),
                }
            },
            '\\' => out.push(chars.next()?),
            c if c.is_whitespace() => return None,
            c => out.push(c),
        }
    }
    Some(out)
}

/// The card name for a command Codex ran: `PowerShell` when it went through pwsh/powershell
/// (Codex on Windows), else `Bash`.
fn shell_tool(command: &str) -> &'static str {
    if strip_powershell(command).is_some() { "PowerShell" } else { "Bash" }
}

/// On Windows Codex runs commands as `"C:\…\pwsh.exe" -Command "…"` (or powershell.exe, maybe
/// with -NoProfile/-NoLogo first): the part after `-Command`.
fn strip_powershell(command: &str) -> Option<&str> {
    let (exe, rest) = match command.strip_prefix('"') {
        Some(quoted) => quoted.split_once('"')?,
        None => command.split_once(' ')?,
    };
    let exe = exe.rsplit(['\\', '/']).next()?.to_ascii_lowercase();
    if !matches!(exe.as_str(), "pwsh" | "pwsh.exe" | "powershell" | "powershell.exe") {
        return None;
    }
    let mut rest = rest.trim_start();
    loop {
        let (flag, after) = rest.split_once(' ')?;
        match flag.to_ascii_lowercase().as_str() {
            "-command" | "-c" => return Some(after),
            "-noprofile" | "-nologo" | "-noninteractive" => rest = after.trim_start(),
            _ => return None,
        }
    }
}

fn changed_paths(item: &Value) -> String {
    item["changes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|c| c["path"].as_str())
        .map(crate::harness::tools::short_path)
        .collect::<Vec<_>>()
        .join(", ")
}

fn changes_diff(item: &Value) -> String {
    let mut out = String::new();
    for c in item["changes"].as_array().into_iter().flatten() {
        let path = crate::harness::tools::short_path(c["path"].as_str().unwrap_or(""));
        let raw = c["diff"].as_str().unwrap_or("");
        // New and deleted files carry their content, not a diff.
        let whole = match c["kind"]["type"].as_str() {
            Some("add") => Some(crate::harness::diff::unified(&path, "", raw)),
            Some("delete") => Some(crate::harness::diff::unified(&path, raw, "")),
            _ => None,
        };
        if let Some(d) = whole {
            out.push_str(&d);
            continue;
        }
        let diff = raw;
        if !diff.starts_with("---") {
            out.push_str(&format!("--- a/{path}\n+++ b/{path}\n"));
        }
        out.push_str(diff);
        if !out.ends_with('\n') {
            out.push('\n');
        }
    }
    out
}

/// Undoes one change of a Codex `fileChange` item: added files are removed, deleted ones
/// rewritten, and updates (and moves) reverse-applied.
fn revert_change(cwd: &std::path::Path, c: &Value) -> Result<(), String> {
    let path = crate::harness::tools::resolve(cwd, c["path"].as_str().unwrap_or(""));
    let raw = c["diff"].as_str().unwrap_or("");
    let io = |e: std::io::Error| e.to_string();
    match c["kind"]["type"].as_str() {
        Some("add") => std::fs::remove_file(&path).map_err(io),
        Some("delete") => {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir).map_err(io)?;
            }
            std::fs::write(&path, raw).map_err(io)
        }
        _ => {
            let moved = c["kind"]["move_path"].as_str().map(|m| crate::harness::tools::resolve(cwd, m));
            let now = moved.as_ref().unwrap_or(&path);
            let current = std::fs::read_to_string(now).map_err(io)?;
            let old = crate::harness::diff::reverse(&current, raw).ok_or("changed since")?;
            std::fs::write(&path, old).map_err(io)?;
            if let Some(m) = moved.filter(|m| *m != path) {
                std::fs::remove_file(m).map_err(io)?;
            }
            Ok(())
        }
    }
}

/// A `-fast` model (e.g. `gpt-6.1-sol-fast`) is the model on Codex's fast service tier, which
/// Codex itself sets apart from the model id.
const FAST_SUFFIX: &str = "-fast";
const FAST_TIER: &str = "priority";

fn split_fast(model: &str) -> (&str, bool) {
    match model.strip_suffix(FAST_SUFFIX) {
        Some(base) if !base.is_empty() => (base, true),
        _ => (model, false),
    }
}

/// Models from `model/list`, through a short-lived app server, each followed by its fast
/// variant when Codex offers the fast tier for it.
pub fn models() -> Vec<ModelInfo> {
    if crate::env::find_command("codex").is_none() {
        return Vec::new();
    }
    let (tx, rx) = channel();
    let cwd = std::env::temp_dir();
    let Ok(mut proc) = LineProcess::spawn("codex", &["app-server".to_string()], &[], &cwd, tx) else {
        return Vec::new();
    };
    let _ = proc.write(&json!({ "id": 1, "method": "initialize", "params": { "clientInfo": { "name": "winfer", "title": null, "version": env!("CARGO_PKG_VERSION") }, "capabilities": null } }));
    let deadline = std::time::Instant::now() + Duration::from_secs(8);
    while let Some(left) = deadline.checked_duration_since(std::time::Instant::now()) {
        let Ok(In::Line(line)) = rx.recv_timeout(left) else { break };
        let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
        match v["id"].as_u64() {
            Some(1) => {
                let _ = proc.write(&json!({ "method": "initialized" }));
                let _ = proc.write(&json!({ "id": 2, "method": "model/list", "params": {} }));
            }
            Some(2) => {
                return v["result"]["data"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|m| m["hidden"].as_bool() != Some(true))
                    .flat_map(model_infos)
                    .collect();
            }
            _ => {}
        }
    }
    Vec::new()
}

fn model_infos(m: &Value) -> Vec<ModelInfo> {
    let label = m["displayName"].as_str().unwrap_or(m["model"].as_str().unwrap_or("")).to_string();
    let efforts = m["supportedReasoningEfforts"]
        .as_array()
        .map(|a| a.iter().filter_map(|e| e["reasoningEffort"].as_str().map(String::from)).collect::<Vec<_>>());
    let mut out = vec![ModelInfo { spec: format!("codex/{}", s(m, "model")), label: label.clone(), detail: format!("Codex · {}", s(m, "description")), efforts: efforts.clone() }];
    let fast = m["serviceTiers"].as_array().into_iter().flatten().find(|t| t["id"].as_str() == Some(FAST_TIER));
    if let Some(t) = fast {
        out.push(ModelInfo {
            spec: format!("codex/{}{FAST_SUFFIX}", s(m, "model")),
            label: format!("{label} ({})", t["name"].as_str().unwrap_or("Fast")),
            detail: format!("Codex · {}", s(t, "description")),
            efforts,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_plan_becomes_todos() {
        let args = json!({ "plan": [{ "step": "Read the code", "status": "completed" }, { "step": "Fix it", "status": "in_progress" }, { "step": "", "status": "pending" }] });
        let todos = plan_todos(&args);
        assert_eq!(todos.len(), 2);
        assert_eq!((todos[1].content.as_str(), todos[1].status.as_str()), ("Fix it", "in_progress"));
        assert!(dynamic_tools().as_array().unwrap().iter().any(|t| t["name"] == UPDATE_PLAN));
    }

    #[test]
    fn ask_mode_asks_before_edits() {
        // Read-only sandbox + on-request: Codex has to ask for every write.
        assert_eq!((sandbox(Mode::Ask), approval_policy(Mode::Ask)), ("read-only", "on-request"));
        assert_eq!(sandbox_policy(Mode::Ask, "/w")["type"], "readOnly");
        assert_eq!(sandbox(Mode::AutoEdit), "workspace-write");
        assert_eq!(approval_policy(Mode::Yolo), "never");
    }

    #[test]
    fn skill_file_reads_become_skill_cards() {
        assert_eq!(skill_reads("cat /Users/a/.agents/skills/caveman/SKILL.md /Users/a/.agents/skills/surgical-patch/SKILL.md").as_deref(), Some("caveman, surgical-patch"));
        assert_eq!(skill_reads("sed -n 1,200p '/x/skills/graphify/SKILL.md'").as_deref(), Some("graphify"));
        assert_eq!(skill_reads("cat /x/skills/caveman/SKILL.md | wc -l"), None);
        assert_eq!(skill_reads("cat README.md"), None);
        let pwsh = r#""C:\Program Files\PowerShell\7\pwsh.exe" -Command "Get-Content -LiteralPath 'C:\Users\rp\.agents\skills\caveman\SKILL.md'""#;
        assert_eq!(skill_reads(&strip_shell(pwsh)).as_deref(), Some("caveman"));
        let escaped = r#""C:\\Program Files\\PowerShell\\7\\pwsh.exe" -Command "Get-Content -LiteralPath 'C:\\Users\\rp\\.agents\\skills\\caveman\\SKILL.md'""#;
        assert_eq!(skill_reads(&strip_shell(escaped)).as_deref(), Some("caveman"));
        assert_eq!(strip_shell(r#"powershell.exe -NoProfile -Command "git status""#), "git status");
        assert_eq!(strip_shell("pwsh.exe -File x.ps1"), "pwsh.exe -File x.ps1");
        assert_eq!(shell_tool(pwsh), "PowerShell");
        assert_eq!(shell_tool("/bin/zsh -lc 'ls'"), "Bash");
    }

    #[test]
    fn explicit_skills_go_as_skill_input() {
        let (mut b, _rx) = bridge();
        let list = b.take_skills(&json!({ "data": [{ "skills": [
            { "name": "caveman", "path": "/a/caveman/SKILL.md", "enabled": true, "description": "terse" },
            { "name": "caveman", "path": "/b/caveman/SKILL.md", "enabled": true, "description": "dup" },
            { "name": "off", "path": "/c/SKILL.md", "enabled": false, "description": "" },
        ] }] }));
        assert_eq!(list, vec![Skill { name: "caveman".into(), description: "terse".into() }]);
        assert_eq!(b.skills["caveman"], "/a/caveman/SKILL.md");
    }

    #[test]
    fn strips_shell_wrappers() {
        assert_eq!(strip_shell("/bin/zsh -lc 'curl -s x | head'"), "curl -s x | head");
        assert_eq!(strip_shell("/bin/zsh -lc \"printf 'hi' > a.txt\""), "printf 'hi' > a.txt");
        assert_eq!(strip_shell("ls"), "ls");
        // shlex mixes quote styles around the script's own quotes.
        let watch = r#"powershell.exe -NoProfile -Command '$ErrorActionPreference='"'Stop'; "'$old = @(Get-CimInstance Win32_Process)'"#;
        assert_eq!(strip_shell(watch), "$ErrorActionPreference='Stop'; $old = @(Get-CimInstance Win32_Process)");
        assert_eq!(strip_shell(r#"/bin/zsh -lc 'echo '"'"'hi'"'"''"#), "echo 'hi'");
        assert_eq!(strip_shell(r#"/bin/zsh -lc "echo \"\$HOME\" \\n""#), r#"echo "$HOME" \n"#);
        // Not one quoted word: left as it is.
        assert_eq!(strip_shell("pwsh -Command git status"), "git status");
        assert_eq!(strip_shell("/bin/zsh -lc 'a' 'b'"), "'a' 'b'");
        assert_eq!(strip_shell("/bin/zsh -lc 'unterminated"), "'unterminated");
    }

    #[test]
    fn bg_cards_show_the_tasks_name() {
        let (mut b, _rx) = bridge();
        let item = json!({ "type": "mcpToolCall", "id": "m1", "server": "winfer", "tool": "bg", "arguments": { "action": "start", "cmd": "pnpm dev", "name": "vite dev server" } });
        assert_eq!(b.tool_card(&item), Some(("winfer · bg".into(), "start vite dev server".into())));
    }

    #[test]
    fn commands_outliving_their_turn_become_background_shells() {
        let (mut b, rx) = bridge();
        let n = |b: &mut Bridge, method: &str, p: Value| b.on_notification(method, &p);
        let cmd = |status: &str, exit: Value| json!({ "type": "commandExecution", "id": "c1", "command": "/bin/zsh -lc 'cd app && gh run watch 811 | tail'", "status": status, "exitCode": exit });
        n(&mut b, "item/started", json!({ "threadId": "main", "item": cmd("inProgress", Value::Null) }));
        n(&mut b, "item/started", json!({ "threadId": "main", "item": { "type": "commandExecution", "id": "c2", "command": "ls" } }));
        n(&mut b, "item/completed", json!({ "threadId": "main", "item": { "type": "commandExecution", "id": "c2", "command": "ls", "status": "completed", "exitCode": 0 } }));
        n(&mut b, "turn/completed", json!({ "threadId": "main", "turn": { "id": "t0", "status": "completed" } }));
        let evs: Vec<Ev> = rx.try_iter().collect();
        let tasks: Vec<&TaskEv> = evs.iter().filter_map(|e| if let Ev::Task(t) = e { Some(t) } else { None }).collect();
        assert_eq!(tasks.len(), 1, "{evs:?}");
        assert!(tasks[0].id == "c1" && tasks[0].tool_id.as_deref() == Some("c1") && tasks[0].status.as_deref() == Some("running") && tasks[0].agent.is_none());
        assert_eq!(tasks[0].title.as_deref(), Some("gh run watch"));
        // The task is up before the turn ends, so the card isn't failed with the turn.
        let task_at = evs.iter().position(|e| matches!(e, Ev::Task(_))).unwrap();
        assert!(task_at < evs.iter().position(|e| matches!(e, Ev::TurnDone)).unwrap());

        n(&mut b, "item/completed", json!({ "threadId": "main", "item": cmd("failed", json!(3)) }));
        let evs: Vec<Ev> = rx.try_iter().collect();
        assert!(matches!(&evs[0], Ev::Task(t) if t.id == "c1" && t.status.as_deref() == Some("failed") && t.exit_code == Some(3)), "{evs:?}");
        assert!(matches!(&evs[1], Ev::ToolEnd { id, ok: false, .. } if id == "c1"));
        assert!(b.background.is_empty() && b.commands.is_empty());
    }

    #[test]
    fn stopping_background_shells_terminates_their_terminal() {
        let (mut b, rx) = bridge();
        let n = |b: &mut Bridge, method: &str, p: Value| b.on_notification(method, &p);
        let cmd = |id: &str, pid: Value| json!({ "type": "commandExecution", "id": id, "command": "/bin/zsh -lc 'npm run dev'", "status": "inProgress", "processId": pid });
        n(&mut b, "item/started", json!({ "threadId": "main", "item": cmd("c1", json!("p1")) }));
        n(&mut b, "item/started", json!({ "threadId": "main", "item": cmd("c2", Value::Null) }));
        n(&mut b, "turn/completed", json!({ "threadId": "main", "turn": { "id": "t0", "status": "completed" } }));
        let _ = rx.try_iter().count();
        // A known process is terminated on its own…
        b.stop_shell("c1".into());
        assert!(matches!(b.pending.get(&b.next), Some(Pending::StopShells { ids, all: false }) if ids == &["c1"]));
        b.shells_stopped(&["c1".into()]);
        let evs: Vec<Ev> = rx.try_iter().collect();
        assert!(matches!(&evs[..], [Ev::Task(t)] if t.id == "c1" && t.status.as_deref() == Some("stopped")), "{evs:?}");
        // …else the thread's background terminals are cleaned.
        b.stop_shell("c2".into());
        assert!(matches!(b.pending.get(&b.next), Some(Pending::StopShells { ids, all: true }) if ids == &["c2"]));
        b.shells_stopped(&["c2".into()]);
        assert_eq!(rx.try_iter().filter(|e| matches!(e, Ev::Task(t) if t.status.as_deref() == Some("stopped"))).count(), 1);
        assert!(b.background.is_empty() && b.processes.is_empty());
        // A late completion of a stopped command doesn't reopen its task.
        n(&mut b, "item/completed", json!({ "threadId": "main", "item": { "type": "commandExecution", "id": "c2", "status": "failed", "exitCode": 143 } }));
        let evs: Vec<Ev> = rx.try_iter().collect();
        assert!(!evs.iter().any(|e| matches!(e, Ev::Task(_))), "{evs:?}");
    }

    #[test]
    fn fast_models_use_the_fast_tier() {
        assert_eq!(split_fast("gpt-6.1-sol-fast"), ("gpt-6.1-sol", true));
        assert_eq!(split_fast("gpt-6.1-sol"), ("gpt-6.1-sol", false));
        assert_eq!(split_fast("-fast"), ("-fast", false));
        let infos = model_infos(&json!({ "model": "gpt-6.1-sol", "displayName": "GPT-6.1 Sol", "description": "d",
            "serviceTiers": [{ "id": "priority", "name": "Fast", "description": "2x speed" }] }));
        assert_eq!(infos.iter().map(|i| i.spec.as_str()).collect::<Vec<_>>(), ["codex/gpt-6.1-sol", "codex/gpt-6.1-sol-fast"]);
        assert_eq!(infos[1].label, "GPT-6.1 Sol (Fast)");
        assert_eq!(model_infos(&json!({ "model": "m", "serviceTiers": [] })).len(), 1);
    }

    #[test]
    fn questions_go_to_the_picker_and_answers_back() {
        let (mut b, rx) = bridge();
        let (me, out) = channel();
        b.proc = LineProcess::spawn("cat", &[], &[], &std::env::temp_dir(), me).unwrap();
        let reply = || loop {
            match out.recv_timeout(Duration::from_secs(5)).unwrap() {
                In::Line(l) => break serde_json::from_str::<Value>(&l).unwrap(),
                _ => continue,
            }
        };
        let args = json!({ "questions": [{ "header": "Lib", "question": "Which library?", "options": [{ "label": "A", "description": "a" }, { "label": "B" }] }] });
        b.on_request("item/tool/call", json!(7), &json!({ "threadId": "main", "turnId": "t0", "callId": "c", "tool": "ask_user", "arguments": args }));
        let Ok(Ev::Approval(a)) = rx.try_recv() else { panic!("no question") };
        assert_eq!(a.questions[0].options, [("A".to_string(), "a".to_string()), ("B".to_string(), String::new())]);
        b.answer(&a.id, Some(vec!["B".into()]));
        let r = reply();
        assert_eq!((r["id"].clone(), r["result"]["success"].clone()), (json!(7), json!(true)));
        assert!(r["result"]["contentItems"][0]["text"].as_str().unwrap().contains("\"Which library?\" → \"B\""));

        // Codex's own request_user_input: answers by question id; dismissing sends none.
        let p = json!({ "threadId": "main", "turnId": "t0", "itemId": "i", "isBlocking": true, "questions": [{ "id": "q1", "header": "H", "question": "Go?", "options": [{ "label": "Yes", "description": "" }] }] });
        b.on_request("item/tool/requestUserInput", json!(8), &p);
        let Ok(Ev::Approval(a)) = rx.try_recv() else { panic!("no question") };
        b.answer(&a.id, Some(vec!["Yes".into()]));
        assert_eq!(reply()["result"], json!({ "answers": { "q1": { "answers": ["Yes"] } } }));
        b.on_request("item/tool/requestUserInput", json!(9), &p);
        let Ok(Ev::Approval(a)) = rx.try_recv() else { panic!("no question") };
        b.answer(&a.id, None);
        assert_eq!(reply()["result"], json!({ "answers": {} }));
        // Other dynamic tools aren't ours.
        b.on_request("item/tool/call", json!(10), &json!({ "tool": "nope", "arguments": {} }));
        assert_eq!(reply()["result"]["success"], json!(false));
    }

    fn bridge() -> (Bridge, Receiver<Ev>) {
        let (events, rx) = channel();
        let (me, _) = channel();
        let proc = LineProcess::spawn("cat", &[], &[], &std::env::temp_dir(), me).unwrap();
        let b = Bridge {
            events,
            proc,
            next: 0,
            pending: HashMap::new(),
            thread: Some("main".into()),
            turn: Some("t0".into()),
            queued: Vec::new(),
            mode: Mode::Ask,
            bg_owner: String::new(),
            model: None,
            effort: None,
            fast: false,
            cwd: String::new(),
            approvals: HashMap::new(),
            asks: HashMap::new(),
            diffs: HashMap::new(),
            children: HashMap::new(),
            usage: Usage::default(),
            turn_tokens: TurnTokens::default(),
            turn_base: 0,
            turn_input_base: 0,
            turn_cached_base: 0,
            skills: HashMap::new(),
            turns: Vec::new(),
            commands: HashMap::new(),
            background: HashSet::new(),
            processes: HashMap::new(),
            trim_at: 0,
            compact_at: 0,
            settled: false,
        };
        (b, rx)
    }

    #[test]
    fn turn_output_counts_from_the_turns_start() {
        let (mut b, rx) = bridge();
        let n = |b: &mut Bridge, method: &str, p: Value| b.on_notification(method, &p);
        let usage = |out: u64| json!({ "threadId": "main", "tokenUsage": { "total": { "outputTokens": out }, "last": {} } });
        n(&mut b, "thread/tokenUsage/updated", usage(500));
        n(&mut b, "turn/started", json!({ "threadId": "main", "turn": { "id": "t1" } }));
        n(&mut b, "item/agentMessage/delta", json!({ "threadId": "main", "itemId": "m", "delta": "x".repeat(200) }));
        n(&mut b, "thread/tokenUsage/updated", usage(560));
        let turns: Vec<(u64, u64)> = rx.try_iter().filter_map(|e| if let Ev::Usage(u) = e { Some((u.turn, u.output)) } else { None }).collect();
        assert_eq!(turns, vec![(500, 500), (50, 500), (60, 560)]);
    }

    #[test]
    fn subagent_threads_become_task_progress() {
        let (mut b, rx) = bridge();
        let n = |b: &mut Bridge, method: &str, p: Value| b.on_notification(method, &p);
        let spawn = json!({ "type": "subAgentActivity", "id": "call_1", "kind": "started", "agentThreadId": "kid", "agentPath": "/root/shell_probe" });
        n(&mut b, "item/started", json!({ "threadId": "main", "item": spawn }));
        n(&mut b, "turn/started", json!({ "threadId": "kid", "turn": { "id": "kt" } }));
        n(&mut b, "item/started", json!({ "threadId": "kid", "item": { "type": "agentMessage", "id": "m" } }));
        n(&mut b, "item/agentMessage/delta", json!({ "threadId": "kid", "itemId": "m", "delta": "child text" }));
        n(&mut b, "item/started", json!({ "threadId": "kid", "item": { "type": "commandExecution", "id": "c", "command": "ls" } }));
        n(&mut b, "thread/tokenUsage/updated", json!({ "threadId": "kid", "tokenUsage": { "total": { "inputTokens": 100, "outputTokens": 5, "cachedInputTokens": 0 } } }));
        n(&mut b, "turn/completed", json!({ "threadId": "kid", "turn": { "id": "kt", "status": "completed" } }));
        let evs: Vec<Ev> = rx.try_iter().collect();
        // The card returns at once; the agent runs on as a task bound to it.
        assert!(matches!(&evs[0], Ev::ToolStart { id, name, title, .. } if id == "call_1" && name == "Agent" && title == "shell probe"));
        assert!(matches!(&evs[1], Ev::ToolEnd { id, .. } if id == "call_1"));
        assert!(matches!(&evs[2], Ev::Task(t) if t.id == "kid" && t.tool_id.as_deref() == Some("call_1") && t.status.as_deref() == Some("running")));
        assert!(evs.iter().any(|e| matches!(e, Ev::Task(t) if t.tools == Some(1) && t.last.as_deref() == Some("Bash"))));
        assert!(matches!(evs.last(), Some(Ev::Task(t)) if t.status.as_deref() == Some("completed")));
        // Nothing of the child's leaks into the main transcript or ends the main turn.
        assert!(!evs.iter().any(|e| matches!(e, Ev::TextDelta { .. } | Ev::TurnDone)));
        assert!(!evs.iter().any(|e| matches!(e, Ev::ToolStart { id, .. } if id == "c")));
        assert!(matches!(evs.iter().find(|e| matches!(e, Ev::Usage(_))), Some(Ev::Usage(u)) if u.input == 100 && u.context == 0));
        assert_eq!(b.turn.as_deref(), Some("t0"));
    }

    #[test]
    fn collab_calls_bind_spawned_threads() {
        let (mut b, rx) = bridge();
        let item = json!({ "type": "collabAgentToolCall", "id": "c1", "tool": "spawnAgent", "status": "completed", "prompt": "Map the repo\nmore", "receiverThreadIds": ["kid"], "agentsStates": { "kid": { "status": "running" } } });
        b.on_notification("item/started", &json!({ "threadId": "main", "item": item }));
        b.on_notification("item/completed", &json!({ "threadId": "main", "item": item }));
        let wait = json!({ "type": "collabAgentToolCall", "id": "c2", "tool": "wait", "status": "completed", "receiverThreadIds": ["kid"], "agentsStates": { "kid": { "status": "completed", "message": "42 files" } } });
        b.on_notification("item/completed", &json!({ "threadId": "main", "item": wait }));
        let evs: Vec<Ev> = rx.try_iter().collect();
        assert!(matches!(&evs[0], Ev::ToolStart { name, title, .. } if name == "Agent" && title == "Map the repo"));
        assert!(matches!(&evs[1], Ev::Task(t) if t.tool_id.as_deref() == Some("c1") && t.status.as_deref() == Some("running")));
        assert!(matches!(&evs[3], Ev::Task(t) if t.id == "kid" && t.tool_id.is_none() && t.status.as_deref() == Some("completed")));
        assert!(matches!(&evs[4], Ev::ToolEnd { id, output, .. } if id == "c2" && output == "42 files"));
    }

    #[test]
    fn undo_reverts_the_last_turns_patches() {
        let (mut b, rx) = bridge();
        let dir = std::env::temp_dir().join(format!("winfer-codex-undo-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        b.cwd = dir.to_string_lossy().into_owned();
        std::fs::write(dir.join("a.txt"), "one\nTWO\nthree\n").unwrap();
        std::fs::write(dir.join("new.txt"), "fresh\n").unwrap();
        let changes = json!([
            { "path": "a.txt", "kind": { "type": "update" }, "diff": "@@ -1,3 +1,3 @@\n one\n-two\n+TWO\n three\n" },
            { "path": "new.txt", "kind": { "type": "add" }, "diff": "fresh\n" },
            { "path": "gone.txt", "kind": { "type": "delete" }, "diff": "old\n" },
        ]);
        let item = json!({ "type": "fileChange", "id": "f", "status": "completed", "changes": changes });
        b.thread = Some("main".into());
        b.turns.push(UserTurn { id: Some("t0".into()), changes: Vec::new() });
        b.on_notification("item/completed", &json!({ "threadId": "main", "item": item }));
        b.on_notification("turn/completed", &json!({ "threadId": "main", "turn": { "id": "t0", "status": "completed" } }));
        b.undo(true);
        let (_, kind) = b.pending.iter().max_by_key(|(id, _)| **id).unwrap();
        assert!(matches!(kind, Pending::Revert { files: true }));
        b.reverted(true);
        assert_eq!(std::fs::read_to_string(dir.join("a.txt")).unwrap(), "one\ntwo\nthree\n");
        assert!(!dir.join("new.txt").exists());
        assert_eq!(std::fs::read_to_string(dir.join("gone.txt")).unwrap(), "old\n");
        assert!(rx.try_iter().any(|e| matches!(e, Ev::Undone(Ok(f)) if f.len() == 3)));
        b.undo(true);
        assert!(matches!(rx.try_iter().last(), Some(Ev::Undone(Err(n))) if n == "Nothing to undo."));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn mcp_servers_become_config_flags() {
        let servers: serde_json::Map<String, Value> =
            serde_json::from_value(json!({ "winfer-x": { "command": "/w", "args": ["mcp-stdio"], "env": { "A": "1" } } })).unwrap();
        let args = mcp_args(&servers);
        assert_eq!(args[1], r#"mcp_servers.winfer_x.command="/w""#);
        assert_eq!(args[3], r#"mcp_servers.winfer_x.args=["mcp-stdio"]"#);
        assert_eq!(args[5], r#"mcp_servers.winfer_x.env={A="1"}"#);
    }
}
