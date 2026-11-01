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
use crate::harness::event::{Cmd, Ev, Events, Mode, ModelInfo, TurnTokens, Usage, UserMsg};

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

fn args(model: &str, effort: Option<&str>, mode: Mode, session: &str, prompt_file: &std::path::Path, images: &[std::path::PathBuf]) -> Vec<String> {
    let mut args: Vec<String> = ["exec", "--json", "--trust-workspace"].iter().map(|s| s.to_string()).collect();
    args.extend(["--session-id".into(), session.to_string()]);
    if !model.is_empty() {
        args.extend(["--model".into(), model.to_string()]);
    }
    if let Some(effort) = effort.filter(|e| !e.is_empty()) {
        args.extend(["--reasoning-effort".into(), effort.to_string()]);
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

fn tail_session_log(me: Sender<In>, run: u64, session: String, live: Arc<AtomicBool>) {
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
        let mut lines = BufReader::new(file).lines();
        while live.load(Ordering::Relaxed) {
            match lines.next() {
                Some(Ok(line)) => {
                    if me.send(In::Line(format!("{run}{LOG}{line}"))).is_err() { return; }
                }
                Some(Err(_)) => return,
                None => std::thread::sleep(std::time::Duration::from_millis(25)),
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
            Cmd::SetCompaction { .. } => {}
            // Its subagents (if any) run on the main model.
            Cmd::SetSubagent { .. } => {}
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
            Ok(()) => args(&self.model, self.effort.as_deref(), self.mode, &session, &prompt_file, &msg.images),
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
                tail_session_log(self.me.clone(), self.run, session, live.clone());
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
            self.emit(Ev::TextDelta { id: format!("muse-t{}", self.run), text });
            return;
        }
        if let Some((id, name, title)) = tool_start(v) {
            // A message's text ends where its tools start: the next delta is a new block.
            self.emit(Ev::ToolStart { id: id.clone(), name: name.clone(), title, input: Value::Null });
            self.calls.insert(id, name);
            return;
        }
        if let Some((id, ok)) = tool_end(v) {
            if self.calls.remove(&id).is_some() {
                self.emit(Ev::ToolEnd { id, ok, output: String::new(), diff: None });
            }
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
                self.emit(Ev::ToolEnd { id, ok, output: String::new(), diff: None });
            }
            if ok {
                if !self.streamed && !text.is_empty() {
                    self.emit(Ev::Text { id: format!("muse-t{}", self.run), text });
                }
            } else {
                self.emit(Ev::Error(format!("Muse: {text}")));
            }
            self.end_turn();
        }
    }

    /// The durable log supplements stdout with tool progress only; stdout remains the authority
    /// for answer text, usage and turn completion.
    fn on_log_record(&mut self, v: &Value) {
        if let Some((id, name, title)) = tool_start(v) {
            if !self.calls.contains_key(&id) {
                self.emit(Ev::ToolStart { id: id.clone(), name: name.clone(), title, input: Value::Null });
                self.calls.insert(id, name);
            }
        } else if let Some((id, ok)) = tool_end(v) {
            if self.calls.remove(&id).is_some() {
                self.emit(Ev::ToolEnd { id, ok, output: String::new(), diff: None });
            }
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
    fn args_carry_model_effort_and_mode() {
        let a = args("muse-spark-1.3", Some("high"), Mode::Yolo, "s", std::path::Path::new("/tmp/p.txt"), &[]);
        assert!(a.windows(2).any(|w| w == ["--model", "muse-spark-1.3"]));
        assert!(a.windows(2).any(|w| w == ["--reasoning-effort", "high"]));
        assert!(a.contains(&"--yolo".to_string()));
        let plan = args("", None, Mode::Plan, "s", std::path::Path::new("/tmp/p.txt"), &[]);
        assert!(plan.contains(&"--disable-write".to_string()));
        assert!(!plan.contains(&"--model".to_string()));
    }
}

