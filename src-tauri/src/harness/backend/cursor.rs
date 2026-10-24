//! Cursor: winfer code's bridge to `cursor-agent`. Each turn is one `cursor-agent -p` run in
//! stream-json mode, continuing the chat with `--resume <chatId>`; interrupting kills the run.
//! Print mode can't ask for approval: what runs is decided by the flags (`--force`, `--mode
//! plan`) and Cursor's own permission settings.

use std::collections::HashMap;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Mutex, OnceLock};

use serde_json::Value;

use super::{In, LineProcess, Opts};
use crate::harness::diff;
use crate::harness::event::{Cmd, Ev, Events, Mode, ModelInfo, Todo, TurnTokens, Usage, UserMsg};
use crate::harness::tools::short_path;

/// The models `cursor-agent` offers, as `cursor/<model>` specs; empty when it isn't installed.
pub fn models() -> Vec<ModelInfo> {
    if crate::env::find_command("cursor-agent").is_none() {
        return Vec::new();
    }
    let list = listed();
    let ids: Vec<String> = list.iter().map(|(id, _)| id.clone()).collect();
    list.into_iter()
        .map(|(id, label)| ModelInfo { detail: format!("Cursor · {id}"), spec: format!("cursor/{id}"), efforts: efforts(&id, &ids), label })
        .collect()
}

/// `cursor-agent --list-models` as (id, label), asked once (it takes a second or two).
fn listed() -> Vec<(String, String)> {
    static CACHE: OnceLock<Mutex<Vec<(String, String)>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    let mut list = cache.lock().unwrap();
    if list.is_empty() {
        if let Some(out) = run_with_timeout(&["--list-models"], None, std::time::Duration::from_secs(8)) {
            *list = crate::providers::parse_models(&out)
                .into_iter()
                .map(|m| {
                    // Some labels carry zero-width spaces and doubled spaces.
                    let label = m.label.replace(['\u{200b}', '\u{200c}', '\u{200d}', '\u{feff}'], "").replace("current, ", "");
                    (m.id, label.split_whitespace().collect::<Vec<_>>().join(" "))
                })
                .collect();
        }
    }
    list.clone()
}

fn run_with_timeout(args: &[&str], cwd: Option<&std::path::Path>, timeout: std::time::Duration) -> Option<String> {
    use std::io::Read;
    let launcher = crate::env::launcher("cursor-agent")?;
    let mut child = crate::env::no_window(&mut std::process::Command::new(&launcher.program))
        .args(&launcher.args)
        .args(args)
        .current_dir(cwd.unwrap_or(std::path::Path::new(".")))
        .envs(crate::env::login_env())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut out = String::new();
        let _ = stdout.read_to_string(&mut out);
        out
    });
    let deadline = std::time::Instant::now() + timeout;
    while child.try_wait().ok()?.is_none() {
        if std::time::Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    reader.join().ok()
}

/// Effort levels as they appear in Cursor's model ids (`gpt-5.5-low`, `claude-opus-5-5-xhigh-fast`).
const LEVELS: [&str; 8] = ["none", "minimal", "low", "medium", "high", "xhigh", "extra-high", "max"];

/// The variant of `model` with effort `effort`, if Cursor lists one. Cursor bakes effort into
/// the model id (its `model[effort=…]` overrides aren't accepted for these), so the effort
/// setting picks the sibling id.
fn with_effort(model: &str, effort: &str, ids: &[String]) -> Option<String> {
    let (stem, fast) = match model.strip_suffix("-fast") {
        Some(s) => (s, "-fast"),
        None => (model, ""),
    };
    let base = LEVELS.iter().find_map(|l| stem.strip_suffix(&format!("-{l}"))).unwrap_or(stem);
    let mut names = vec![effort.to_string()];
    match effort {
        "xhigh" => names.push("extra-high".into()),
        "minimal" => names.push("none".into()),
        _ => {}
    }
    let mut candidates: Vec<String> = names.iter().map(|n| format!("{base}-{n}{fast}")).collect();
    if effort == "medium" {
        candidates.push(format!("{base}{fast}"));
    }
    candidates.into_iter().find(|c| ids.iter().any(|id| id == c))
}

/// The efforts `model` has variants for (None when it has no sibling with another effort: then
/// Cursor sets none for it).
fn efforts(model: &str, ids: &[String]) -> Option<Vec<String>> {
    let mut seen = Vec::new();
    let mut list = Vec::new();
    for e in ["none", "minimal", "low", "medium", "high", "xhigh", "max"] {
        if let Some(id) = with_effort(model, e, ids).filter(|id| !seen.contains(id)) {
            seen.push(id);
            list.push(e.to_string());
        }
    }
    Some(if list.len() > 1 { list } else { Vec::new() })
}

/// Separates a stdout line from the number of the run it came from; `END` marks a run's end.
const SEP: char = '\u{1f}';
const END: char = '\u{1e}';

struct Bridge {
    events: Events,
    me: Sender<In>,
    cwd: std::path::PathBuf,
    model: String,
    effort: Option<String>,
    mode: Mode,
    /// Prepended to the chat's first message (cursor-agent has no system prompt flag).
    system: Option<String>,
    /// The Cursor chat, once the first run reported it (or the one resumed).
    chat: Option<String>,
    /// The running turn's process; replaced (killing it) by the next run or an interrupt.
    proc: Option<LineProcess>,
    /// Number of the current run: lines of older (killed) runs are ignored.
    run: u64,
    busy: bool,
    /// The current run sent its `result`.
    finished: bool,
    /// Messages that came during a turn, sent as the next ones.
    queue: Vec<UserMsg>,
    /// Ids for streamed text and reasoning blocks.
    seq: u64,
    text: Option<String>,
    reasoning: Option<String>,
    /// Running tool calls: id → (cursor's tool key, display name).
    calls: HashMap<String, (String, String)>,
    /// Stdout that isn't JSON (cursor-agent prints some errors as plain text).
    noise: String,
    explained_ask: bool,
    usage: Usage,
    turn: TurnTokens,
}

pub fn run(opts: Opts, rx: Receiver<In>, me: Sender<In>, events: Events) {
    if crate::env::find_command("cursor-agent").is_none() {
        let _ = events.send(Ev::Exited("`cursor-agent` isn't installed (not on PATH)".into()));
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
        chat: opts.resume.clone(),
        proc: None,
        run: 0,
        busy: false,
        finished: false,
        queue: Vec::new(),
        seq: 0,
        text: None,
        reasoning: None,
        calls: HashMap::new(),
        noise: String::new(),
        explained_ask: false,
        usage: Usage::default(),
        turn: TurnTokens::default(),
    };
    if let Some(chat) = &opts.resume {
        b.emit(Ev::Session(chat.clone()));
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

    /// Ask mode can't ask in print mode; says once what it means instead.
    fn explain_mode(&mut self) {
        if self.mode == Mode::Ask && !self.explained_ask {
            self.explained_ask = true;
            self.emit(Ev::Notice(
                "Cursor can't ask for approval here: in ask mode its file edits go through, and shell commands outside Cursor's allowlist are rejected. Use auto-edit or yolo to let commands run.".into(),
            ));
        }
    }

    fn on_cmd(&mut self, cmd: Cmd) -> bool {
        match cmd {
            Cmd::Send(msg) if self.busy => self.queue.push(msg),
            Cmd::Send(msg) => self.start(msg),
            Cmd::Steer(msg) if self.busy => {
                self.emit(Ev::Notice("Cursor can't take a message mid-turn; it goes in as the next one.".into()));
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
            // Nothing asks (print mode has no approvals), so there's nothing to answer.
            Cmd::Approve(..) | Cmd::Answer(..) => {}
            Cmd::SetModel(model) => {
                self.model = model;
                self.check_effort();
            }
            Cmd::SetEffort(effort) => {
                self.effort = effort;
                self.check_effort();
            }
            Cmd::SetMode(mode) => {
                self.mode = mode;
                self.explain_mode();
            }
            Cmd::Compact => self.emit(Ev::Notice("Cursor manages its chat's context itself; it can't be compacted from here.".into())),
            Cmd::SetCompaction { .. } => {}
            // Its subagents (if any) run on the main model.
            Cmd::SetSubagent { .. } => {}
            Cmd::Undo { .. } => self.emit(Ev::Undone(Err("Cursor can't drop a turn from its chat in print mode.".into()))),
            Cmd::StopTask(id) => {
                if !crate::harness::bg::stop(&id) {
                    self.emit(Ev::Notice("Cursor runs no background tasks winfer can stop.".into()));
                }
            }
            Cmd::Shutdown => return false,
        }
        true
    }

    /// The model to pass, with the effort setting applied when Cursor has that variant.
    fn model_arg(&self) -> Option<String> {
        let model = self.model.trim();
        if model.is_empty() {
            return None;
        }
        let effort = self.effort.as_deref().filter(|e| !e.is_empty());
        let ids: Vec<String> = match effort {
            Some(_) => listed().into_iter().map(|(id, _)| id).collect(),
            None => Vec::new(),
        };
        Some(effort.and_then(|e| with_effort(model, e, &ids)).unwrap_or_else(|| model.to_string()))
    }

    fn check_effort(&self) {
        let Some(effort) = self.effort.as_deref().filter(|e| !e.is_empty()) else { return };
        let ids: Vec<String> = listed().into_iter().map(|(id, _)| id).collect();
        match with_effort(&self.model, effort, &ids) {
            Some(m) if m != self.model => self.emit(Ev::Notice(format!("Effort {effort}: using {m}."))),
            Some(_) => {}
            None => self.emit(Ev::Notice(format!(
                "Cursor sets effort through the model, and {} has no {effort} variant; effort is unchanged.",
                if self.model.is_empty() { "the default model" } else { &self.model }
            ))),
        }
    }

    fn args(&self, prompt: String) -> Vec<String> {
        let mut args: Vec<String> = ["-p", "--output-format", "stream-json", "--stream-partial-output", "--trust"].iter().map(|s| s.to_string()).collect();
        if let Some(model) = self.model_arg() {
            args.extend(["--model".into(), model]);
        }
        if self.mode.plans() {
            args.extend(["--mode".into(), "plan".into()]);
        }
        if matches!(self.mode, Mode::Yolo | Mode::AutoEdit | Mode::YoloPlan) {
            args.extend(["--force".into(), "--approve-mcps".into()]);
        }
        if let Some(chat) = &self.chat {
            args.extend(["--resume".into(), chat.clone()]);
        }
        args.push("--".into());
        args.push(prompt);
        args
    }

    fn prompt(&mut self, msg: &UserMsg) -> String {
        let mut text = match &msg.skill {
            Some(skill) => format!("/{skill} {}", msg.text).trim_end().to_string(),
            None => msg.text.clone(),
        };
        // cursor-agent takes text only; it reads attached images from their files.
        for path in &msg.images {
            text.push_str(&format!("\n\n[Attached image: {}]", path.display()));
        }
        if let Some(system) = self.system.take() {
            text = format!("{system}\n\n{text}");
        }
        text
    }

    /// Starts a turn: one `cursor-agent -p` run.
    fn start(&mut self, msg: UserMsg) {
        self.stop_run();
        self.busy = true;
        self.finished = false;
        self.noise.clear();
        self.turn = TurnTokens::default();
        (self.usage.turn, self.usage.turn_input, self.usage.turn_cached) = (0, 0, 0);
        self.emit(Ev::TurnStart);
        // The chat is made up front, so its id is known even if the first run is interrupted
        // before it reports one (else the next turn would start over).
        if self.chat.is_none() {
            let out = run_with_timeout(&["create-chat"], Some(&self.cwd), std::time::Duration::from_secs(15)).unwrap_or_default();
            if let Some(id) = out.lines().map(str::trim).rfind(|l| l.len() >= 8 && l.chars().all(|c| c.is_ascii_hexdigit() || c == '-')) {
                self.chat = Some(id.to_string());
                self.emit(Ev::Session(id.to_string()));
            }
        }
        let prompt = self.prompt(&msg);
        let args = self.args(prompt);
        let (tx, lines) = channel();
        match LineProcess::spawn("cursor-agent", &args, &[("NO_OPEN_BROWSER", "1")], &self.cwd, tx) {
            Ok(p) => {
                self.proc = Some(p);
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
        self.run += 1;
        self.proc = None;
        for (id, _) in std::mem::take(&mut self.calls) {
            self.emit(Ev::ToolEnd { id, ok: false, output: "Interrupted.".into(), diff: None });
        }
    }

    fn end_turn(&mut self) {
        self.busy = false;
        self.text = None;
        self.reasoning = None;
        self.emit(Ev::TurnDone);
        if !self.queue.is_empty() {
            let msg = self.queue.remove(0);
            self.start(msg);
        }
    }

    fn on_tagged(&mut self, line: &str) {
        let Some(at) = line.find([SEP, END]) else { return };
        if line[..at].parse::<u64>().ok() != Some(self.run) {
            return;
        }
        if line[at..].starts_with(END) {
            return self.on_closed();
        }
        let line = &line[at + SEP.len_utf8()..];
        match serde_json::from_str::<Value>(line) {
            Ok(v) if v.is_object() => self.on_line(&v),
            _ if !line.trim().is_empty() => {
                self.noise.push_str(line.trim());
                self.noise.push('\n');
            }
            _ => {}
        }
    }

    /// The run's output ended: without a `result`, it failed.
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
            self.emit(Ev::ToolEnd { id, ok: false, output: "Cursor stopped.".into(), diff: None });
        }
        self.emit(Ev::Error(format!("Cursor: {why}")));
        self.end_turn();
    }

    fn next_id(&mut self, kind: &str) -> String {
        self.seq += 1;
        format!("cursor-{kind}{}", self.seq)
    }

    fn on_line(&mut self, v: &Value) {
        match v["type"].as_str().unwrap_or("") {
            "system" if v["subtype"] == "init" => {
                if let Some(s) = v["session_id"].as_str() {
                    if self.chat.as_deref() != Some(s) {
                        self.chat = Some(s.to_string());
                        self.emit(Ev::Session(s.to_string()));
                    }
                }
                if let Some(m) = v["model"].as_str() {
                    self.emit(Ev::Model(m.to_string()));
                }
            }
            "thinking" => match v["subtype"].as_str() {
                Some("delta") => {
                    let text = v["text"].as_str().unwrap_or("").to_string();
                    let id = match &self.reasoning {
                        Some(id) => id.clone(),
                        None => {
                            let id = self.next_id("r");
                            self.reasoning = Some(id.clone());
                            id
                        }
                    };
                    if self.turn.streamed(&text) {
                        self.emit_usage();
                    }
                    self.emit(Ev::ReasoningDelta { id, text });
                }
                Some("completed") => self.reasoning = None,
                _ => {}
            },
            "assistant" => {
                let text: String = v["message"]["content"].as_array().into_iter().flatten().filter_map(|b| b["text"].as_str()).collect();
                // With --stream-partial-output the deltas carry a timestamp; the whole text of
                // each message follows them (with a model call id, or neither for the last one).
                let delta = v.get("timestamp_ms").is_some() && v.get("model_call_id").is_none();
                if delta {
                    let id = match &self.text {
                        Some(id) => id.clone(),
                        None => {
                            let id = self.next_id("t");
                            self.text = Some(id.clone());
                            id
                        }
                    };
                    if self.turn.streamed(&text) {
                        self.emit_usage();
                    }
                    self.emit(Ev::TextDelta { id, text });
                } else {
                    let id = self.text.take().unwrap_or_else(|| self.next_id("t"));
                    self.emit(Ev::Text { id, text });
                }
            }
            "tool_call" => self.on_tool(v),
            "result" => {
                self.finished = true;
                let u = &v["usage"];
                let n = |k: &str| u[k].as_u64().unwrap_or(0);
                let input = n("inputTokens") + n("cacheReadTokens") + n("cacheWriteTokens");
                self.usage.input += input;
                self.usage.cached += n("cacheReadTokens");
                self.usage.output += n("outputTokens");
                self.usage.turn = n("outputTokens");
                self.usage.turn_input = input;
                self.usage.turn_cached = n("cacheReadTokens");
                self.usage.turn_streamed = self.turn.streamed_total();
                self.emit(Ev::Usage(self.usage.clone()));
                if v["is_error"].as_bool() == Some(true) {
                    let text = v["result"].as_str().filter(|s| !s.is_empty()).unwrap_or("Cursor reported an error.");
                    self.emit(Ev::Error(text.to_string()));
                }
                for (id, _) in std::mem::take(&mut self.calls) {
                    self.emit(Ev::ToolEnd { id, ok: true, output: String::new(), diff: None });
                }
                self.end_turn();
            }
            _ => {}
        }
    }

    /// Usage while the turn streams: finished turns plus this one's estimate.
    fn emit_usage(&self) {
        let mut u = self.usage.clone();
        u.turn = self.turn.now();
        u.output += u.turn;
        u.turn_streamed = self.turn.streamed_total();
        self.emit(Ev::Usage(u));
    }

    fn on_tool(&mut self, v: &Value) {
        let id = v["call_id"].as_str().unwrap_or("").to_string();
        let Some((key, body)) = v["tool_call"].as_object().and_then(|o| o.iter().find(|(k, b)| k.ends_with("ToolCall") && b.is_object())) else { return };
        let args = &body["args"];
        match v["subtype"].as_str() {
            Some("started") => {
                // A message's text ends where its tools start.
                self.text = None;
                self.reasoning = None;
                let name = display_name(key, args);
                if key == "updateTodosToolCall" {
                    if let Some(todos) = todos(&args["todos"]) {
                        self.emit(Ev::Todos(todos));
                    }
                }
                self.emit(Ev::ToolStart { id: id.clone(), name: name.clone(), title: title(key, args), input: args.clone() });
                self.calls.insert(id, (key.clone(), name));
            }
            Some("completed") => {
                self.calls.remove(&id);
                let (ok, output, diff) = outcome(key, args, &body["result"]);
                if key == "updateTodosToolCall" {
                    if let Some(todos) = todos(&body["result"]["success"]["todos"]) {
                        self.emit(Ev::Todos(todos));
                    }
                }
                self.emit(Ev::ToolEnd { id, ok, output, diff });
            }
            _ => {}
        }
    }
}

/// Cursor's tool names (`shellToolCall`, …) as the names winfer code shows (Claude Code's where
/// they match, so the cards look the same).
fn display_name(key: &str, args: &Value) -> String {
    let s = |k: &str| args[k].as_str().unwrap_or("").to_string();
    match key {
        "shellToolCall" => "Bash".into(),
        "readToolCall" => "Read".into(),
        "editToolCall" => "Edit".into(),
        "deleteToolCall" => "Delete".into(),
        "grepToolCall" => "Grep".into(),
        "globToolCall" => "Glob".into(),
        "lsToolCall" => "LS".into(),
        "updateTodosToolCall" => "TodoWrite".into(),
        "readTodosToolCall" => "TodoRead".into(),
        "webSearchToolCall" => "WebSearch".into(),
        "webFetchToolCall" => "WebFetch".into(),
        "getMcpToolsToolCall" => "MCP tools".into(),
        "mcpToolCall" => {
            let server = [s("providerIdentifier"), s("server"), s("serverName")].into_iter().find(|x| !x.is_empty()).unwrap_or_default();
            let tool = [s("toolName"), s("name")].into_iter().find(|x| !x.is_empty()).unwrap_or_else(|| "tool".into());
            if server.is_empty() { tool } else { format!("{server} · {tool}") }
        }
        _ => {
            let base = key.strip_suffix("ToolCall").unwrap_or(key);
            let mut c = base.chars();
            c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
        }
    }
}

/// A one-line summary of a tool call for its card.
fn title(key: &str, args: &Value) -> String {
    let s = |k: &str| args[k].as_str().unwrap_or("").to_string();
    let t = match key {
        "shellToolCall" => s("command"),
        "readToolCall" | "editToolCall" | "deleteToolCall" | "lsToolCall" => short_path(&s("path")),
        "grepToolCall" => format!("{} {}", s("pattern"), short_path(&s("path"))).trim_end().to_string(),
        "globToolCall" => s("globPattern"),
        "updateTodosToolCall" => "update todos".into(),
        "webSearchToolCall" => [s("searchTerm"), s("query")].into_iter().find(|x| !x.is_empty()).unwrap_or_default(),
        "webFetchToolCall" => s("url"),
        "getMcpToolsToolCall" => format!("{} {}", s("server"), s("toolName")).trim().to_string(),
        _ => args
            .as_object()
            .and_then(|o| o.iter().filter(|(k, _)| *k != "toolCallId").find_map(|(_, v)| v.as_str().map(str::to_string)))
            .unwrap_or_default(),
    };
    t.lines().next().unwrap_or("").to_string()
}

/// Whether a finished call succeeded, its output, and the diff of an edit.
fn outcome(key: &str, args: &Value, result: &Value) -> (bool, String, Option<String>) {
    if let Some(r) = result.get("rejected") {
        let reason = r["reason"].as_str().filter(|s| !s.is_empty()).map(|s| format!(" ({s})")).unwrap_or_default();
        return (false, format!("Cursor rejected this{reason}: it can't ask for approval here. Switch to auto-edit or yolo, or allow it in Cursor's permissions."), None);
    }
    let (ok, r) = match result.get("success") {
        Some(s) => (true, s),
        None => (false, result.as_object().and_then(|o| o.values().next()).unwrap_or(result)),
    };
    let message = || {
        ["message", "error", "errorMessage", "reason"]
            .iter()
            .find_map(|k| r[*k].as_str().filter(|s| !s.is_empty()).map(str::to_string))
            .unwrap_or_else(|| clip(&compact(r)))
    };
    match key {
        "shellToolCall" => {
            let out = r["interleavedOutput"].as_str().map(str::to_string).unwrap_or_else(|| {
                let (o, e) = (r["stdout"].as_str().unwrap_or(""), r["stderr"].as_str().unwrap_or(""));
                format!("{o}{e}")
            });
            let code = r["exitCode"].as_i64().unwrap_or(0);
            let out = if out.is_empty() && !ok { message() } else { out };
            (ok && code == 0, out, None)
        }
        "editToolCall" if ok => {
            let path = r["path"].as_str().or_else(|| args["path"].as_str()).unwrap_or("");
            let diff = r["afterFullFileContent"].as_str().map(|after| diff::unified(&short_path(path), r["beforeFullFileContent"].as_str().unwrap_or(""), after));
            (true, String::new(), diff)
        }
        "readToolCall" if ok => (true, r["content"].as_str().unwrap_or("").to_string(), None),
        "globToolCall" if ok => {
            let files: Vec<&str> = r["files"].as_array().into_iter().flatten().filter_map(|f| f.as_str()).collect();
            (true, files.join("\n"), None)
        }
        "grepToolCall" if ok => (true, grep_output(r), None),
        "updateTodosToolCall" if ok => (true, String::new(), None),
        _ if ok => {
            let out = r["content"].as_str().map(str::to_string).unwrap_or_else(|| clip(&compact(r)));
            (true, out, None)
        }
        _ => (false, message(), None),
    }
}

/// Grep matches as `file:line: text` (or the files, in files mode).
fn grep_output(r: &Value) -> String {
    let mut out = Vec::new();
    for ws in r["workspaceResults"].as_object().into_iter().flat_map(|o| o.values()) {
        for (_, mode) in ws.as_object().into_iter().flatten() {
            for m in mode["matches"].as_array().into_iter().flatten() {
                let file = m["file"].as_str().unwrap_or("");
                for hit in m["matches"].as_array().into_iter().flatten() {
                    out.push(format!("{file}:{}: {}", hit["lineNumber"].as_u64().unwrap_or(0), hit["content"].as_str().unwrap_or("")));
                }
            }
            for f in mode["files"].as_array().into_iter().flatten().filter_map(|f| f.as_str()) {
                out.push(f.to_string());
            }
        }
    }
    if out.is_empty() { clip(&compact(r)) } else { out.join("\n") }
}

/// Cursor's todos (`TODO_STATUS_IN_PROGRESS`, …) as winfer's.
fn todos(list: &Value) -> Option<Vec<Todo>> {
    let list = list.as_array()?;
    Some(
        list.iter()
            .map(|t| {
                let status = match t["status"].as_str().unwrap_or("") {
                    "TODO_STATUS_IN_PROGRESS" => "in_progress",
                    "TODO_STATUS_COMPLETED" | "TODO_STATUS_CANCELLED" => "completed",
                    _ => "pending",
                };
                Todo { content: t["content"].as_str().unwrap_or("").to_string(), status: status.into() }
            })
            .collect(),
    )
}

fn compact(v: &Value) -> String {
    if v.is_null() { String::new() } else { v.to_string() }
}

fn clip(s: &str) -> String {
    const MAX: usize = 4000;
    if s.len() <= MAX {
        return s.to_string();
    }
    let mut out: String = s.chars().take(MAX).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn efforts_come_from_the_listed_variants() {
        let ids: Vec<String> = ["gpt-5.6-sol-none", "gpt-5.6-sol-high", "gpt-5.6-sol-xhigh", "auto", "claude-opus-5-5-low", "claude-opus-5-5-medium", "claude-opus-5-5-max"].map(String::from).into();
        assert_eq!(efforts("gpt-5.6-sol-high", &ids), Some(vec!["none".to_string(), "high".to_string(), "xhigh".to_string()]));
        assert_eq!(efforts("claude-opus-5-5-max", &ids), Some(vec!["low".to_string(), "medium".to_string(), "max".to_string()]));
        assert_eq!(efforts("auto", &ids), Some(vec![]));
    }
    use serde_json::json;

    #[test]
    fn picks_the_effort_variant() {
        let ids: Vec<String> = ["gpt-5.5-low", "gpt-5.5-high", "gpt-5.5-extra-high-fast", "claude-opus-5-5-xhigh", "gpt-5.3-codex", "gpt-5.3-codex-high"].iter().map(|s| s.to_string()).collect();
        assert_eq!(with_effort("gpt-5.5-low", "high", &ids).as_deref(), Some("gpt-5.5-high"));
        assert_eq!(with_effort("gpt-5.5-low-fast", "xhigh", &ids).as_deref(), Some("gpt-5.5-extra-high-fast"));
        assert_eq!(with_effort("claude-opus-5-5-low", "xhigh", &ids).as_deref(), Some("claude-opus-5-5-xhigh"));
        assert_eq!(with_effort("gpt-5.3-codex-high", "medium", &ids).as_deref(), Some("gpt-5.3-codex"));
        assert_eq!(with_effort("auto", "high", &ids), None);
    }

    #[test]
    fn maps_shell_and_edit_results() {
        let ok = outcome("shellToolCall", &json!({}), &json!({ "success": { "exitCode": 0, "interleavedOutput": "hi\n" } }));
        assert_eq!((ok.0, ok.1.as_str()), (true, "hi\n"));
        let rejected = outcome("shellToolCall", &json!({}), &json!({ "rejected": { "command": "rm x", "reason": "" } }));
        assert!(!rejected.0);
        let edit = outcome("editToolCall", &json!({ "path": "/tmp/a.txt" }), &json!({ "success": { "path": "/tmp/a.txt", "beforeFullFileContent": "a\n", "afterFullFileContent": "b\n" } }));
        assert!(edit.2.is_some_and(|d| d.contains("-a") && d.contains("+b")));
        assert_eq!(display_name("shellToolCall", &json!({})), "Bash");
        assert_eq!(display_name("fooBarToolCall", &json!({})), "FooBar");
    }
}
