//! Muse bridge: `muse exec --json` per turn (like the Cursor bridge), continuing one
//! session with `--session-id`. The JSONL stream is Muse's session record stream: text
//! arrives as `run.output.delta`, a turn ends with `run.terminal.*`, tool batches are
//! `tool_batch.effect.*` and usage is `run.usage`. Headless exec can't ask for approval
//! mid-turn, so the permission mode maps to Muse's own approval flags (see `args`).

use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::Arc;

use serde_json::Value;

use super::{In, LineProcess, Opts};
use crate::harness::event::{Cmd, Ev, Events, Mode, ModelInfo, Skill, TaskEv, Todo, TurnTokens, Usage, UserMsg};

/// Reasoning efforts Muse takes (`--reasoning-effort`); "none" is its own default tier and
/// stays off the list, as in pi's bridge.
const EFFORTS: [&str; 7] = ["minimal", "low", "medium", "high", "xhigh", "max", "ultra"];

/// The models Muse offers, as `muse/<model>` specs; empty when it isn't installed. Muse has
/// no list-models command (its catalog is served over `muse serve`'s `model/list`), so the
/// known catalog is listed here; any other id still works typed as `muse/<id>`.
pub fn models() -> Vec<ModelInfo> {
    if crate::env::find_command("muse").is_none() {
        return Vec::new();
    }
    [
        ("muse-wonder-1.0", "Muse Wonder 1.0", "most capable"),
        ("muse-spark-1.3", "Muse Spark 1.3", "fast, everyday coding"),
    ]
    .iter()
    .map(|(id, label, detail)| ModelInfo {
        spec: format!("muse/{id}"),
        label: label.to_string(),
        detail: format!("Muse Code · {detail}"),
        efforts: Some(EFFORTS.map(String::from).into()),
    })
    .collect()
}

/// A fresh session id for `--session-id` (a UUID; Muse rejects anything else).
fn new_session_id() -> String {
    let n = rand::random::<u128>();
    let n = (n & !(0xfu128 << 76) | (0x4u128 << 76)) & !(0x3u128 << 62) | (0x2u128 << 62);
    format!(
        "{:08x}-{:04x}-{:04x}-{:04x}-{:012x}",
        (n >> 96) as u32,
        (n >> 80) as u16,
        (n >> 64) as u16,
        (n >> 48) as u16,
        n & 0xffff_ffff_ffff,
    )
}

/// Muse's tool names as the names winfer code shows (Claude Code's where they match).
fn display_name(name: &str) -> String {
    match name {
        "bash" | "shell" | "run_command" => "Bash".into(),
        "read_file" | "read" | "view_file" => "Read".into(),
        "edit_file" | "edit" | "replace_file_content" => "Edit".into(),
        "write_file" | "write" | "write_to_file" => "Write".into(),
        "search" | "grep" | "grep_search" => "Search".into(),
        "list_files" | "glob" | "find_by_name" => "Glob".into(),
        other => {
            let mut c = other.chars();
            c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
        }
    }
}

/// Transcript id of one text block: a new segment after each tool call, so the
/// UI keeps text before and after a call as separate items in order.
fn text_id(run: u64, seg: u64) -> String {
    format!("muse-t{run}-{seg}")
}

/// Streamed answer text of a record, if it carries any.
fn text_delta(v: &Value) -> Option<String> {
    (v["payload_type"] == "run.output.delta")
        .then(|| v["payload"]["text"].as_str().map(str::to_string))
        .flatten()
        .filter(|t| !t.is_empty())
}

/// The turn's end: `Some((ok, final_text))` for `run.terminal.*` records.
fn terminal(v: &Value) -> Option<(bool, String)> {
    let p = &v["payload"];
    match v["payload_type"].as_str()? {
        "run.terminal.completed" => Some((true, p["text"].as_str().unwrap_or("").to_string())),
        "run.terminal.failed" | "run.terminal.cancelled" => {
            let reason = p["reason"].as_str().filter(|r| !r.is_empty()).unwrap_or("Muse stopped the turn.");
            Some((false, reason.to_string()))
        }
        _ => None,
    }
}

/// A tool call starting or ending: `(call_id, display name)` / `(call_id, ok)`.
fn tool_start(v: &Value) -> Option<(String, String, String)> {
    if v["payload_type"] != "tool_batch.effect.started" {
        return None;
    }
    let r = &v["payload"]["record"];
    let id = r["call_id"].as_str().filter(|s| !s.is_empty())?.to_string();
    let name = display_name(r["tool_name"].as_str().unwrap_or("tool"));
    let title = r["parallel_profile"]["subject"].as_str().unwrap_or(&name).to_string();
    Some((id, name, title))
}

fn tool_end(v: &Value) -> Option<(String, bool)> {
    if v["payload_type"] != "tool_batch.effect.terminal" {
        return None;
    }
    let r = &v["payload"]["record"];
    let id = r["call_id"].as_str().filter(|s| !s.is_empty())?.to_string();
    Some((id, r["outcome"]["kind"] == "completed"))
}

/// Calls the model committed to, from the log's `assistant_tool_calls_committed`
/// records: `(call_id, raw tool name, parsed args)`. These precede the matching
/// `tool_batch.effect.started`, so a call's args are known when it starts.
fn committed_calls(v: &Value) -> Vec<(String, String, Value)> {
    if v["payload_type"] != "runtime.session" {
        return Vec::new();
    }
    let ev = &v["payload"]["event"];
    if ev["kind"] != "assistant_tool_calls_committed" {
        return Vec::new();
    }
    let mut out = Vec::new();
    for tc in ev["tool_calls"].as_array().into_iter().flatten() {
        let Some(id) = tc["call_id"].as_str().filter(|s| !s.is_empty()) else { continue };
        let args = tc["args"].as_str().and_then(|a| serde_json::from_str(a).ok()).unwrap_or(Value::Null);
        out.push((id.to_string(), tc["name"].as_str().unwrap_or("tool").to_string(), args));
    }
    out
}

/// Finished calls' result texts, from `tool_result_batch_committed` records.
/// They follow the matching `tool_batch.effect.terminal`, so they complete a
/// call that already ended in the UI.
fn committed_results(v: &Value) -> Vec<(String, String)> {
    if v["payload_type"] != "runtime.session" {
        return Vec::new();
    }
    let ev = &v["payload"]["event"];
    if ev["kind"] != "tool_result_batch_committed" {
        return Vec::new();
    }
    let mut out = Vec::new();
    for r in ev["results"].as_array().into_iter().flatten() {
        let Some(id) = r["tool_call_id"].as_str().filter(|s| !s.is_empty()) else { continue };
        out.push((id.to_string(), r["text"].as_str().unwrap_or("").to_string()));
    }
    out
}

/// The readable part of a result: shell-style tools wrap their output in a JSON
/// envelope (`{"output": "…", "exit_code": …}`); other results are plain text.
fn result_text(text: &str) -> String {
    if let Ok(v) = serde_json::from_str::<Value>(text) {
        if let Some(out) = v["output"].as_str() {
            return out.to_string();
        }
    }
    text.to_string()
}

/// The title other providers show for a call, derived from its args.
fn title_for(name: &str, input: &Value) -> Option<String> {
    let s = |k: &str| input[k].as_str().unwrap_or("");
    let t: String = match name {
        "Bash" => {
            let d = s("description");
            if d.is_empty() { s("command") } else { d }.to_string()
        }
        "Read" | "Edit" | "Write" => crate::harness::tools::short_path(s("path")),
        "Search" => s("pattern").to_string(),
        _ => input.as_object().and_then(|o| o.values().find_map(|v| v.as_str())).unwrap_or("").to_string(),
    };
    let first = t.lines().next().unwrap_or("").trim();
    (!first.is_empty()).then(|| first.to_string())
}

/// A `write_todos` call's list, as the plan sidebar's todos.
fn todos_of(input: &Value) -> Option<Vec<Todo>> {
    let list = input["todos"].as_array()?;
    let todos: Vec<Todo> = list
        .iter()
        .filter_map(|t| {
            let content = t["text"].as_str().or_else(|| t["content"].as_str())?.trim().to_string();
            if content.is_empty() {
                return None;
            }
            let status = match t["status"].as_str() {
                Some(s @ ("in_progress" | "completed")) => s.to_string(),
                _ => "pending".to_string(),
            };
            Some(Todo { content, status })
        })
        .collect();
    (!todos.is_empty()).then_some(todos)
}

/// Skills Muse itself offers (`muse skills list`, TSV), for the `/` picker.
fn muse_skills() -> Vec<Skill> {
    let Some(muse) = crate::env::find_command("muse") else { return Vec::new() };
    let Ok(out) = std::process::Command::new(muse).args(["skills", "list"]).output() else { return Vec::new() };
    let text = String::from_utf8_lossy(&out.stdout);
    let mut skills = Vec::new();
    for line in text.lines().skip(1) {
        let mut cols = line.split('\t');
        let (Some(name), Some(_scope), Some(active)) = (cols.next(), cols.next(), cols.next()) else { continue };
        if active != "on" || name.is_empty() {
            continue;
        }
        skills.push(Skill { name: name.to_string(), description: cols.next().unwrap_or("").to_string() });
    }
    skills
}

/// The diff an edit or write makes, from its args (mirrors the Claude bridge).
fn edit_diff(name: &str, input: &Value) -> Option<String> {
    let short = crate::harness::tools::short_path(input["path"].as_str()?);
    match name {
        "Edit" => Some(crate::harness::diff::unified(&short, input["find"].as_str()?, input["replace"].as_str()?)),
        "Write" => Some(crate::harness::diff::unified(&short, "", input["content"].as_str()?)),
        _ => None,
    }
}

/// One run's usage: (input, cached, output) summed over its model calls.
fn run_usage(v: &Value) -> Option<(u64, u64, u64)> {
    if v["payload_type"] != "run.usage" {
        return None;
    }
    let models = v["payload"]["models"].as_array()?;
    let mut sum = (0u64, 0u64, 0u64);
    for m in models {
        sum.0 += m["input_tokens"].as_u64().unwrap_or(0);
        sum.1 += m["cached_tokens"].as_u64().unwrap_or(0);
        sum.2 += m["output_tokens"].as_u64().unwrap_or(0);
    }
    Some(sum)
}

/// The session a record belongs to (`stream` of the session records).
fn record_session(v: &Value) -> Option<String> {
    (v["stream"]["kind"] == "session")
        .then(|| v["stream"]["id"].as_str().map(str::to_string))
        .flatten()
        .filter(|s| !s.is_empty())
}

fn args(model: &str, effort: Option<&str>, mode: Mode, session: &str, prompt_file: &std::path::Path, images: &[std::path::PathBuf], lean: bool, trim_at: u8, compact_at: u8) -> Vec<String> {
    let mut args: Vec<String> = ["exec", "--json", "--trust-workspace"].iter().map(|s| s.to_string()).collect();
    args.extend(["--session-id".into(), session.to_string()]);
    if !model.is_empty() {
        args.extend(["--model".into(), model.to_string()]);
    }
    if let Some(effort) = effort.filter(|e| !e.is_empty()) {
        args.extend(["--reasoning-effort".into(), effort.to_string()]);
    }
    if lean {
        // The trims Muse has: no other tools' personal context, no web, no reminders.
        args.extend(["--no-foreign-personal-context".into(), "--disable-web-tools".into(), "--disable-reminders".into()]);
    }
    if trim_at > 0 {
        args.extend(["--context-compaction-soft-threshold".into(), format!("{}", f32::from(trim_at) / 100.0)]);
    }
    if compact_at > 0 {
        args.extend(["--context-compaction-hard-threshold".into(), format!("{}", f32::from(compact_at) / 100.0)]);
    }
    match mode {
        Mode::Yolo => args.push("--yolo".into()),
        // Plan changes no files: Muse's non-shell writes off, shell still approval-gated.
        Mode::Plan => args.extend(["--disable-write".into(), "--approval-mode".into(), "on-request".into()]),
        Mode::YoloPlan => args.extend(["--disable-write".into(), "--approval-mode".into(), "never".into()]),
        Mode::Ask | Mode::AutoEdit => args.extend(["--approval-mode".into(), "on-request".into()]),
    }
    for image in images {
        args.extend(["--image".into(), image.to_string_lossy().into_owned()]);
    }
    args.extend(["--prompt-file".into(), prompt_file.to_string_lossy().into_owned()]);
    args
}

/// Separates a stdout line from the number of the run it came from; `END` marks a run's end.
const SEP: char = '\u{1f}';
const END: char = '\u{1e}';
/// Tool effects are persisted in Muse's session log but aren't forwarded by `exec --json`.
const LOG: char = '\u{1d}';

fn session_log(session: &str) -> Option<std::path::PathBuf> {
    let mut roots = Vec::new();
    if let Some(data) = std::env::var_os("XDG_DATA_HOME") {
        roots.push(std::path::PathBuf::from(data).join("muse/sessions"));
    }
    if let Some(home) = std::env::var_os("HOME") {
        let home = std::path::PathBuf::from(home);
        roots.push(home.join(".local/share/muse/sessions"));
        roots.push(home.join("Library/Application Support/muse/sessions"));
    }
    if let Some(data) = dirs::data_dir() {
        roots.push(data.join("muse/sessions"));
    }
    for root in roots {
        let Ok(years) = std::fs::read_dir(root) else { continue };
        for year in years.flatten() {
            let Ok(months) = std::fs::read_dir(year.path()) else { continue };
            for month in months.flatten() {
                let Ok(days) = std::fs::read_dir(month.path()) else { continue };
                for day in days.flatten() {
                    let path = day.path().join(session).join("session.jsonl");
                    if path.is_file() {
                        return Some(path);
                    }
                }
            }
        }
    }
    None
}

/// Shared read position in the session log: kept across runs so a new turn's tail
/// resumes where the last one stopped instead of replaying every earlier tool call.
/// `u64::MAX` means "start at the current end" (a resumed session's history).
fn tail_session_log(me: Sender<In>, run: u64, session: String, live: Arc<AtomicBool>, pos: Arc<std::sync::Mutex<u64>>) {
    std::thread::spawn(move || {
        let mut path = None;
        while live.load(Ordering::Relaxed) && path.is_none() {
            path = session_log(&session);
            if path.is_none() {
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
        }
        let Some(path) = path else { return };
        let Ok(file) = std::fs::File::open(path) else { return };
        let mut reader = BufReader::new(file);
        {
            let mut p = pos.lock().unwrap();
            let len = reader.get_ref().metadata().map(|m| m.len()).unwrap_or(0);
            if *p == u64::MAX || *p > len {
                *p = if *p == u64::MAX { len } else { 0 };
            }
        }
        use std::io::Seek;
        while live.load(Ordering::Relaxed) {
            let cur = *pos.lock().unwrap();
            if reader.seek(std::io::SeekFrom::Start(cur)).is_err() {
                return;
            }
            let mut line = String::new();
            match reader.read_line(&mut line) {
                Ok(0) => std::thread::sleep(std::time::Duration::from_millis(25)),
                // A line without its newline may still be mid-write: retry it whole.
                Ok(_) if !line.ends_with('\n') => std::thread::sleep(std::time::Duration::from_millis(25)),
                Ok(n) => {
                    *pos.lock().unwrap() = cur + n as u64;
                    if me.send(In::Line(format!("{run}{LOG}{line}"))).is_err() { return; }
                }
                Err(_) => return,
            }
        }
    });
}

struct Bridge {
    events: Events,
    me: Sender<In>,
    cwd: std::path::PathBuf,
    model: String,
    effort: Option<String>,
    mode: Mode,
    /// Prepended to the session's first message (Muse exec has no system prompt flag).
    system: Option<String>,
    /// The Muse session, minted on the first turn (or the one resumed).
    session: Option<String>,
    proc: Option<LineProcess>,
    /// Number of the current run: lines of older (killed) runs are ignored.
    run: u64,
    busy: bool,
    finished: bool,
    queue: Vec<UserMsg>,
    /// Text streamed this run, and whether any delta arrived (the terminal text is whole).
    streamed: bool,
    /// Running tool calls: id → display name.
    calls: HashMap<String, String>,
    log_tail: Option<Arc<AtomicBool>>,
    /// Byte offset the session-log tail has consumed (shared with its thread).
    log_pos: Arc<std::sync::Mutex<u64>>,
    /// Text block within the run: bumped at each tool start so text after a tool
    /// becomes a new transcript item instead of merging into the earlier block.
    text_seg: u64,
    /// Committed calls' args by call id (raw name, display name, input).
    inputs: HashMap<String, (String, String, Value)>,
    /// Results that arrived before their call's end record.
    results: HashMap<String, String>,
    /// Ended calls still awaiting their result record: call id → ok.
    ended: HashMap<String, bool>,
    /// Calls shown as tasks (subagents, workflows, monitors): call id → agent label.
    task_calls: HashMap<String, Option<String>>,
    /// Winfer's lean setting, and the compaction thresholds (percent; 0 = off).
    lean: bool,
    trim_at: u8,
    compact_at: u8,
    explained_subagent: bool,
    /// Stdout that isn't JSON (Muse prints warnings as plain text).
    noise: String,
    explained_ask: bool,
    usage: Usage,
    turn: TurnTokens,
}

pub fn run(opts: Opts, rx: Receiver<In>, me: Sender<In>, events: Events) {
    if crate::env::find_command("muse").is_none() {
        let _ = events.send(Ev::Exited("`muse` isn't installed (not on PATH)".into()));
        return;
    }
    let mut b = Bridge {
        events,
        me,
        cwd: opts.cwd,
        model: opts.model,
        effort: opts.effort,
        mode: opts.mode,
        system: if opts.resume.is_some() { None } else { opts.system.filter(|s| !s.trim().is_empty()) },
        session: opts.resume.clone(),
        proc: None,
        run: 0,
        busy: false,
        finished: false,
        queue: Vec::new(),
        streamed: false,
        calls: HashMap::new(),
        log_tail: None,
        // A resumed session's log already holds its earlier turns: skip them.
        log_pos: Arc::new(std::sync::Mutex::new(if opts.resume.is_some() { u64::MAX } else { 0 })),
        text_seg: 0,
        inputs: HashMap::new(),
        results: HashMap::new(),
        ended: HashMap::new(),
        task_calls: HashMap::new(),
        lean: opts.lean,
        trim_at: opts.trim_at,
        compact_at: opts.compact_at,
        explained_subagent: false,
        noise: String::new(),
        explained_ask: false,
        usage: Usage::default(),
        turn: TurnTokens::default(),
    };
    if let Some(session) = &opts.resume {
        b.emit(Ev::Session(session.clone()));
    }
    if !opts.mcp.is_empty() {
        b.emit(Ev::Notice("Muse only loads MCP servers from its own config, so winfer's MCP tools aren't attached.".into()));
    }
    b.explain_mode();
    if b.lean {
        b.emit(Ev::Notice(
            "Lean mode for Muse is partial: personal rules/skills from other tools, web tools and reminders are off. Muse has no short-prompt or trimmed-tool mode.".into(),
        ));
    }
    // Muse's own skill catalog, for the composer's `/` picker.
    let events = b.events.clone();
    std::thread::spawn(move || {
        let skills = muse_skills();
        if !skills.is_empty() {
            let _ = events.send(Ev::Skills(skills));
        }
    });
    while let Some(msg) = super::recv(&rx) {
        match msg {
            In::Line(line) => b.on_tagged(&line),
            In::Closed => {}
            In::Cmd(cmd) => {
                if !b.on_cmd(cmd) {
                    return;
                }
            }
        }
    }
}

impl Bridge {
    fn emit(&self, ev: Ev) {
        let _ = self.events.send(ev);
    }

    /// Ask mode can't ask in headless exec; says once what it means instead.
    fn explain_mode(&mut self) {
        if self.mode == Mode::Ask && !self.explained_ask {
            self.explained_ask = true;
            self.emit(Ev::Notice(
                "Muse can't ask for approval here: in ask mode its approval judge decides, and calls it won't approve are declined. Use yolo to let everything run.".into(),
            ));
        }
    }

    fn on_cmd(&mut self, cmd: Cmd) -> bool {
        match cmd {
            Cmd::Send(msg) if self.busy => self.queue.push(msg),
            Cmd::Send(msg) => self.start(msg),
            Cmd::Steer(msg) if self.busy => {
                self.emit(Ev::Notice("Muse can't take a message mid-turn; it goes in as the next one.".into()));
                self.queue.push(msg);
            }
            Cmd::Steer(msg) => self.start(msg),
            Cmd::Interrupt => {
                if self.busy {
                    self.stop_run();
                    if !self.queue.is_empty() {
                        self.emit(Ev::Notice(format!("Dropped {} queued message(s).", self.queue.len())));
                        self.queue.clear();
                    }
                    self.end_turn();
                }
            }
            // Nothing asks (headless exec has no approval answers), so there's nothing to answer.
            Cmd::Approve(..) | Cmd::Answer(..) => {}
            Cmd::SetModel(model) => self.model = model,
            Cmd::SetEffort(effort) => self.effort = effort,
            Cmd::SetMode(mode) => {
                self.mode = mode;
                self.explain_mode();
            }
            Cmd::Compact => self.emit(Ev::Notice("Muse compacts its context on its own; /compact isn't available.".into())),
            Cmd::SetCompaction { trim, compact } => {
                self.trim_at = trim;
                self.compact_at = compact;
            }
            Cmd::SetSubagent { .. } => {
                if !self.explained_subagent {
                    self.explained_subagent = true;
                    self.emit(Ev::Notice("Muse runs its subagents on the main model in exec mode; a separate subagent model isn't available.".into()));
                }
            }
            Cmd::Undo { .. } => self.emit(Ev::Undone(Err("Muse can't drop a turn from its session in exec mode.".into()))),
            Cmd::StopTask(id) => {
                if !crate::harness::bg::stop(&id) {
                    self.emit(Ev::Notice("Muse runs no background tasks winfer can stop.".into()));
                }
            }
            Cmd::Shutdown => return false,
        }
        true
    }

    fn prompt(&mut self, msg: &UserMsg) -> String {
        let mut text = match &msg.skill {
            Some(skill) => format!("/{skill} {}", msg.text).trim_end().to_string(),
            None => msg.text.clone(),
        };
        if let Some(system) = self.system.take() {
            text = format!("{system}\n\n{text}");
        }
        text
    }

    /// Starts a turn: one `muse exec --json` run on the bridge's session.
    fn start(&mut self, msg: UserMsg) {
        self.stop_run();
        self.busy = true;
        self.finished = false;
        self.streamed = false;
        self.text_seg = 0;
        self.noise.clear();
        self.turn = TurnTokens::default();
        (self.usage.turn, self.usage.turn_input, self.usage.turn_cached) = (0, 0, 0);
        self.emit(Ev::TurnStart);
        let session = match &self.session {
            Some(s) => s.clone(),
            None => {
                let s = new_session_id();
                self.session = Some(s.clone());
                self.emit(Ev::Session(s.clone()));
                s
            }
        };
        if !self.model.is_empty() {
            self.emit(Ev::Model(self.model.clone()));
        }
        // The prompt goes in a file: it's multi-line, which a command-line argument can't
        // carry through Windows' .cmd shims (same reason as llm.rs).
        let dir = crate::harness::config::dir().join("muse");
        let prompt_file = dir.join(format!("prompt-{}-{}.txt", std::process::id(), self.run));
        let text = self.prompt(&msg);
        let args = match std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(&prompt_file, &text)) {
            Ok(()) => args(&self.model, self.effort.as_deref(), self.mode, &session, &prompt_file, &msg.images, self.lean, self.trim_at, self.compact_at),
            Err(_) => {
                let mut a: Vec<String> = ["exec", "--json", "--trust-workspace"].iter().map(|s| s.to_string()).collect();
                a.extend(["--session-id".into(), session.clone()]);
                if !self.model.is_empty() {
                    a.extend(["--model".into(), self.model.clone()]);
                }
                a.push(text);
                a
            }
        };
        let (tx, lines) = channel();
        match LineProcess::spawn("muse", &args, &[], &self.cwd, tx) {
            Ok(p) => {
                self.proc = Some(p);
                let live = Arc::new(AtomicBool::new(true));
                tail_session_log(self.me.clone(), self.run, session, live.clone(), self.log_pos.clone());
                self.log_tail = Some(live);
                // Tags its lines with the run, so a killed run's late output is told apart.
                let (me, run) = (self.me.clone(), self.run);
                std::thread::spawn(move || {
                    for m in lines {
                        let line = match m {
                            In::Line(l) => format!("{run}{SEP}{l}"),
                            In::Closed => format!("{run}{END}"),
                            In::Cmd(_) => continue,
                        };
                        if me.send(In::Line(line)).is_err() {
                            return;
                        }
                    }
                });
            }
            Err(e) => {
                self.emit(Ev::Error(e));
                self.end_turn();
            }
        }
    }

    /// Kills the running process (if any); its remaining output is ignored.
    fn stop_run(&mut self) {
        if let Some(live) = self.log_tail.take() {
            live.store(false, Ordering::Relaxed);
        }
        self.run += 1;
        self.proc = None;
        for (id, _) in std::mem::take(&mut self.calls) {
            self.emit(Ev::ToolEnd { id, ok: false, output: "Interrupted.".into(), diff: None });
        }
    }

    fn end_turn(&mut self) {
        self.busy = false;
        self.emit(Ev::TurnDone);
        // Let the log tail catch effects flushed just before the terminal stdout record, then
        // stop its polling thread while this pane is idle.
        if let Some(live) = self.log_tail.take() {
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(100));
                live.store(false, Ordering::Relaxed);
            });
        }
        if !self.queue.is_empty() {
            let msg = self.queue.remove(0);
            self.start(msg);
        }
    }

    fn emit_usage(&mut self) {
        let l = &mut self.usage;
        l.turn = self.turn.now();
        l.turn_streamed = self.turn.streamed_total();
        let u = l.clone();
        self.emit(Ev::Usage(u));
    }

    fn on_tagged(&mut self, line: &str) {
        let Some(at) = line.find([SEP, END, LOG]) else { return };
        if line[..at].parse::<u64>().ok() != Some(self.run) {
            return;
        }
        if line[at..].starts_with(END) {
            return self.on_closed();
        }
        let from_log = line[at..].starts_with(LOG);
        let line = &line[at + SEP.len_utf8()..];
        match serde_json::from_str::<Value>(line) {
            Ok(v) if v.is_object() && from_log => self.on_log_record(&v),
            Ok(v) if v.is_object() => self.on_record(&v),
            _ if !line.trim().is_empty() => {
                self.noise.push_str(line.trim());
                self.noise.push('\n');
            }
            _ => {}
        }
    }

    /// The run's output ended: without a terminal record, it failed.
    fn on_closed(&mut self) {
        let mut proc = self.proc.take();
        if self.finished || !self.busy {
            return;
        }
        let why = match self.noise.trim() {
            "" => proc.as_mut().map(|p| p.exit_reason()).unwrap_or_else(|| "stopped".into()),
            noise => noise.lines().take(4).collect::<Vec<_>>().join("\n"),
        };
        drop(proc);
        for (id, _) in std::mem::take(&mut self.calls) {
            self.emit(Ev::ToolEnd { id, ok: false, output: "Muse stopped.".into(), diff: None });
        }
        self.emit(Ev::Error(format!("Muse: {why}")));
        self.end_turn();
    }

    fn on_record(&mut self, v: &Value) {
        if let Some(s) = record_session(v) {
            if self.session.as_deref() != Some(&s) {
                self.session = Some(s.clone());
                self.emit(Ev::Session(s));
            }
        }
        if let Some(text) = text_delta(v) {
            self.streamed = true;
            if self.turn.streamed(&text) {
                self.emit_usage();
            }
            self.emit(Ev::TextDelta { id: self.text_id(), text });
            return;
        }
        if let Some((id, name, title)) = tool_start(v) {
            self.start_call(id, name, title);
            return;
        }
        if let Some((id, ok)) = tool_end(v) {
            self.finish_call(&id, ok);
            return;
        }
        if let Some((input, cached, output)) = run_usage(v) {
            let u = &mut self.usage;
            u.input += input;
            u.cached += cached;
            u.output += output;
            u.turn_input += input;
            u.turn_cached += cached;
            u.context = input + output;
            self.turn.exact(output);
            self.emit_usage();
            return;
        }
        if let Some((ok, text)) = terminal(v) {
            self.finished = true;
            for (id, _) in std::mem::take(&mut self.calls) {
                // Their result records may still follow in the log: let them land.
                self.ended.insert(id.clone(), ok);
                let diff = if ok { self.diff_for(&id) } else { None };
                self.emit(Ev::ToolEnd { id, ok, output: String::new(), diff });
            }
            if ok {
                if !self.streamed && !text.is_empty() {
                    self.emit(Ev::Text { id: self.text_id(), text });
                }
            } else {
                self.emit(Ev::Error(format!("Muse: {text}")));
            }
            self.end_turn();
        }
    }

    /// The durable log supplements stdout with tool progress only; stdout remains the authority
    /// for answer text, usage and turn completion.
    fn text_id(&self) -> String {
        text_id(self.run, self.text_seg)
    }

    fn diff_for(&self, id: &str) -> Option<String> {
        self.inputs.get(id).and_then(|(_, name, input)| edit_diff(name, input))
    }

    /// Emits a call's start once (stdout and the log can both report it), with
    /// its committed args as the input the UI opens to.
    fn start_call(&mut self, id: String, name: String, fallback_title: String) {
        if self.calls.contains_key(&id) {
            return;
        }
        // A message's text ends where its tools start: the next delta is a new block.
        self.text_seg += 1;
        let (raw, input) = self
            .inputs
            .get(&id)
            .map(|(raw, _, v)| (raw.clone(), v.clone()))
            .unwrap_or_default();
        let title = title_for(&name, &input).unwrap_or(fallback_title);
        self.emit(Ev::ToolStart { id: id.clone(), name: name.clone(), title: title.clone(), input: input.clone() });
        self.calls.insert(id.clone(), name);
        // Subagents, workflows and monitors also surface as tasks (agents/background UI).
        let agent = ["subagent_type", "role", "agent"].iter().find_map(|k| input[*k].as_str()).map(str::to_string);
        if raw.contains("subagent") || raw == "workflow" || raw == "monitor" || agent.is_some() {
            self.task_calls.insert(id.clone(), agent.clone());
            self.emit(Ev::Task(TaskEv {
                id: id.clone(),
                tool_id: Some(id.clone()),
                title: Some(title),
                description: input["description"].as_str().map(str::to_string),
                agent,
                status: Some("running".into()),
                ..Default::default()
            }));
        }
    }

    /// A call's end record: show it ended, with its diff. Its result record
    /// (which follows in the log) fills in the output via `complete_call`; a
    /// result that somehow arrived first is attached right away.
    fn finish_call(&mut self, id: &str, ok: bool) {
        if self.calls.remove(id).is_none() {
            return;
        }
        if self.task_calls.remove(id).is_some() {
            self.emit(Ev::Task(TaskEv {
                id: id.to_string(),
                status: Some(if ok { "done" } else { "failed" }.into()),
                ..Default::default()
            }));
        }
        let diff = if ok { self.diff_for(id) } else { None };
        if let Some(output) = self.results.remove(id) {
            self.inputs.remove(id);
            self.emit(Ev::ToolEnd { id: id.to_string(), ok, output, diff });
        } else {
            self.ended.insert(id.to_string(), ok);
            self.emit(Ev::ToolEnd { id: id.to_string(), ok, output: String::new(), diff });
        }
    }

    /// A call's result record: attach its output to the already-ended call (the
    /// UI's second `ToolEnd` for the id replaces the empty output).
    fn complete_call(&mut self, id: &str, text: &str) {
        let output = result_text(text);
        if self.calls.contains_key(id) {
            self.results.insert(id.to_string(), output);
        } else if let Some(ok) = self.ended.remove(id) {
            let diff = if ok { self.diff_for(id) } else { None };
            self.inputs.remove(id);
            self.emit(Ev::ToolEnd { id: id.to_string(), ok, output, diff });
        } else {
            self.results.insert(id.to_string(), output);
        }
    }

    fn on_log_record(&mut self, v: &Value) {
        for (id, raw, args) in committed_calls(v) {
            if raw == "write_todos" {
                if let Some(todos) = todos_of(&args) {
                    self.emit(Ev::Todos(todos));
                }
            }
            self.inputs.insert(id, (raw.clone(), display_name(&raw), args));
        }
        for (id, text) in committed_results(v) {
            self.complete_call(&id, &text);
        }
        if let Some((id, name, title)) = tool_start(v) {
            self.start_call(id, name, title);
        } else if let Some((id, ok)) = tool_end(v) {
            self.finish_call(&id, ok);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn session_ids_are_uuids() {
        let id = new_session_id();
        assert_eq!(id.len(), 36);
        assert_eq!(id.matches('-').count(), 4);
        assert_eq!(&id[14..15], "4");
    }

    #[test]
    fn reads_the_exec_stream() {
        let delta = json!({"payload_type": "run.output.delta", "payload": {"kind": "run_output_delta", "text": "hi"}});
        assert_eq!(text_delta(&delta).as_deref(), Some("hi"));
        assert_eq!(terminal(&delta), None);
        let done = json!({"payload_type": "run.terminal.completed", "payload": {"kind": "run_terminal", "terminal": "completed", "text": "hi", "reason": null}});
        assert_eq!(terminal(&done), Some((true, "hi".to_string())));
        let failed = json!({"payload_type": "run.terminal.failed", "payload": {"kind": "run_terminal", "terminal": "failed", "text": "", "reason": "boom"}});
        assert_eq!(terminal(&failed), Some((false, "boom".to_string())));
        let usage = json!({"payload_type": "run.usage", "payload": {"kind": "run_usage", "models": [{"model": "muse-spark-1.3", "input_tokens": 10, "cached_tokens": 3, "output_tokens": 5}]}});
        assert_eq!(run_usage(&usage), Some((10, 3, 5)));
        let rec = json!({"stream": {"kind": "session", "id": "s1"}, "payload_type": "run.output.delta", "payload": {}});
        assert_eq!(record_session(&rec).as_deref(), Some("s1"));
    }

    #[test]
    fn reads_tool_batches() {
        let started = json!({"payload_type": "tool_batch.effect.started", "payload": {"kind": "tool_batch_effect", "record": {"kind": "started", "call_id": "c1", "tool_name": "bash"}}});
        assert_eq!(tool_start(&started), Some(("c1".to_string(), "Bash".to_string(), "Bash".to_string())));
        let done = json!({"payload_type": "tool_batch.effect.terminal", "payload": {"kind": "tool_batch_effect", "record": {"kind": "terminal", "call_id": "c1", "outcome": {"kind": "completed"}}}});
        assert_eq!(tool_end(&done), Some(("c1".to_string(), true)));
        assert_eq!(display_name("read_file"), "Read");
    }

    #[test]
    fn reads_committed_calls_and_results() {
        let calls = json!({"payload_type": "runtime.session", "payload": {"kind": "run", "event": {"kind": "assistant_tool_calls_committed", "tool_calls": [{"call_id": "c1", "name": "bash", "args": "{\"command\": \"ls\", \"description\": \"List files\"}"}]}}});
        let got = committed_calls(&calls);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].0, "c1");
        assert_eq!(title_for("Bash", &got[0].2).as_deref(), Some("List files"));
        let results = json!({"payload_type": "runtime.session", "payload": {"kind": "run", "event": {"kind": "tool_result_batch_committed", "results": [{"tool_call_id": "c1", "text": "{\"output\": \"a.txt\\n\", \"exit_code\": 0}"}]}}});
        assert_eq!(committed_results(&results), vec![("c1".to_string(), "{\"output\": \"a.txt\\n\", \"exit_code\": 0}".to_string())]);
        // Shell envelopes unwrap to their output; plain results pass through.
        assert_eq!(result_text("{\"output\": \"a.txt\\n\", \"exit_code\": 0}"), "a.txt\n");
        assert_eq!(result_text("plain"), "plain");
        // Edits open to a diff of find → replace.
        let edit = json!({"path": "src/a.rs", "find": "old", "replace": "new"});
        assert!(edit_diff("Edit", &edit).is_some_and(|d| d.contains("+new") && d.contains("-old")));
        assert_eq!(title_for("Read", &json!({"path": "src/a.rs"})).as_deref(), Some("src/a.rs"));
    }

    #[test]
    fn todos_come_from_write_todos() {
        let input = json!({"todos": [{"text": "One", "status": "in_progress"}, {"text": "Two", "status": "bogus"}, {"content": "Three", "status": "completed"}]});
        let todos = todos_of(&input).unwrap();
        assert_eq!(todos.iter().map(|t| t.content.as_str()).collect::<Vec<_>>(), ["One", "Two", "Three"]);
        assert_eq!(todos.iter().map(|t| t.status.as_str()).collect::<Vec<_>>(), ["in_progress", "pending", "completed"]);
        assert!(todos_of(&json!({"todos": []})).is_none());
    }

    #[test]
    fn text_blocks_split_at_tool_calls() {
        // Same block while only text streams; a new block once a tool intervened,
        // and a new run never collides with the previous one.
        assert_eq!(text_id(3, 0), text_id(3, 0));
        assert_ne!(text_id(3, 0), text_id(3, 1));
        assert_ne!(text_id(3, 1), text_id(4, 0));
    }

    #[test]
    fn args_carry_model_effort_and_mode() {
        let a = args("muse-spark-1.3", Some("high"), Mode::Yolo, "s", std::path::Path::new("/tmp/p.txt"), &[], false, 0, 0);
        assert!(a.windows(2).any(|w| w == ["--model", "muse-spark-1.3"]));
        assert!(a.windows(2).any(|w| w == ["--reasoning-effort", "high"]));
        assert!(a.contains(&"--yolo".to_string()));
        let plan = args("", None, Mode::Plan, "s", std::path::Path::new("/tmp/p.txt"), &[], false, 0, 0);
        assert!(plan.contains(&"--disable-write".to_string()));
        assert!(!plan.contains(&"--model".to_string()));
        // Lean's partial trims and the compaction thresholds reach the run.
        let lean = args("", None, Mode::Yolo, "s", std::path::Path::new("/tmp/p.txt"), &[], true, 75, 90);
        assert!(lean.contains(&"--no-foreign-personal-context".to_string()));
        assert!(lean.windows(2).any(|w| w == ["--context-compaction-soft-threshold", "0.75"]));
        assert!(lean.windows(2).any(|w| w == ["--context-compaction-hard-threshold", "0.9"]));
    }
}

