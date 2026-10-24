//! Google Antigravity: winfer code's bridge to `agy`.
//!
//! `agy --print= --input-format stream-json --output-format stream-json` is one long-lived
//! process that runs a turn for each `{"event":"user",...}` line on stdin. It reports an
//! `init` (conversation id, model), then `step_update`s (user input, agent responses with
//! `text_delta`s and per-request usage, tool steps going ACTIVE → DONE) and a `result` per turn.
//! Its stdin takes nothing but user messages (text only): no interrupts, approvals or settings
//! changes, so those restart the process on the same conversation (`--conversation`). Print
//! mode runs every tool without asking, whatever the mode.

use std::collections::{HashMap, VecDeque};
use std::sync::mpsc::{Receiver, Sender};

use serde_json::{Value, json};

use super::{In, LineProcess, Opts};
use crate::harness::diff;
use crate::harness::event::{Cmd, Ev, Events, Mode, ModelInfo, TurnTokens, Usage, UserMsg};

/// The models `agy` offers, as `agy/<model>` specs; empty when it isn't installed.
pub fn models() -> Vec<ModelInfo> {
    static CACHE: std::sync::Mutex<Option<Vec<ModelInfo>>> = std::sync::Mutex::new(None);
    if crate::env::find_command("agy").is_none() {
        return Vec::new();
    }
    if let Some(hit) = CACHE.lock().unwrap().clone() {
        return hit;
    }
    let Some(out) = list_models() else { return Vec::new() };
    let parsed = crate::providers::parse_models(&out);
    let ids: Vec<&str> = parsed.iter().map(|m| m.id.as_str()).collect();
    let list: Vec<ModelInfo> = parsed
        .iter()
        .map(|m| ModelInfo { detail: format!("Antigravity · {}", vendor(&m.id)), spec: format!("agy/{}", m.id), efforts: efforts(&m.id, &ids), label: m.label.clone() })
        .collect();
    if !list.is_empty() {
        *CACHE.lock().unwrap() = Some(list.clone());
    }
    list
}

/// `agy models`' output, or None if it fails or takes longer than 8 seconds.
fn list_models() -> Option<String> {
    let (tx, rx) = std::sync::mpsc::channel();
    let proc = LineProcess::spawn("agy", &["models".to_string()], &[], &std::env::temp_dir(), tx).ok()?;
    let until = std::time::Instant::now() + std::time::Duration::from_secs(8);
    let mut out = String::new();
    loop {
        match rx.recv_timeout(until.saturating_duration_since(std::time::Instant::now())) {
            Ok(In::Line(line)) => {
                out.push_str(&line);
                out.push('\n');
            }
            Ok(In::Closed) => break,
            Ok(In::Cmd(_)) => {}
            Err(_) => return None,
        }
    }
    drop(proc);
    Some(out)
}

fn vendor(id: &str) -> &'static str {
    if id.starts_with("gemini") {
        "Google"
    } else if id.starts_with("claude") {
        "Anthropic"
    } else if id.starts_with("gpt") {
        "OpenAI"
    } else {
        "model"
    }
}

/// The context window of an Antigravity model (it doesn't say).
fn window(model: &str) -> u64 {
    if model.starts_with("claude") {
        200_000
    } else if model.starts_with("gpt-oss") {
        131_072
    } else {
        1_048_576
    }
}

/// Models named with a thinking level (`gemini-3.8-flash-high`) refuse `--effort`: the model
/// name already says it, so they start without one.
fn effort_flag(model: &str, effort: Option<&str>) -> Option<String> {
    let suffixed = ["-low", "-medium", "-high"].iter().any(|s| model.ends_with(s));
    effort.filter(|_| !suffixed).map(|e| if e == "minimal" { "low" } else { e }.to_string())
}

/// The efforts a model takes: a model named with a thinking level switches between its listed
/// siblings; the others take `--effort` low/medium/high (antigravity.google/docs/models; Gemini
/// 3.1 Pro has just low and high, and xhigh/max only on models that list such variants).
fn efforts(model: &str, ids: &[&str]) -> Option<Vec<String>> {
    let list: Vec<&str> = match ["-low", "-medium", "-high"].iter().find_map(|s| model.strip_suffix(s)) {
        Some(base) => ["low", "medium", "high"].into_iter().filter(|l| ids.contains(&format!("{base}-{l}").as_str())).collect(),
        None if model.starts_with("gemini-3.1-pro") => vec!["low", "high"],
        None => vec!["low", "medium", "high"],
    };
    Some(list.into_iter().map(String::from).collect())
}

/// For a new effort picked on a model named with a thinking level: its sibling with that level.
fn sibling(model: &str, effort: Option<&str>) -> Option<String> {
    let base = ["-low", "-medium", "-high"].iter().find_map(|s| model.strip_suffix(s))?;
    let level = match effort? {
        "minimal" | "low" => "low",
        "medium" => "medium",
        "high" | "xhigh" | "max" => "high",
        _ => return None,
    };
    Some(format!("{base}-{level}"))
}

fn args(opts: &Opts) -> Vec<String> {
    // `--print=`: a bare `--print` would take the next flag as its prompt.
    let mut args: Vec<String> = ["--print=", "--input-format", "stream-json", "--output-format", "stream-json"].iter().map(|s| s.to_string()).collect();
    if !opts.model.is_empty() {
        args.extend(["--model".into(), opts.model.clone()]);
    }
    if let Some(e) = effort_flag(&opts.model, opts.effort.as_deref()) {
        args.extend(["--effort".into(), e]);
    }
    match opts.mode {
        Mode::Ask => {}
        Mode::AutoEdit => args.extend(["--mode".into(), "accept-edits".into()]),
        Mode::Plan | Mode::YoloPlan => args.extend(["--mode".into(), "plan".into()]),
        Mode::Yolo => args.push("--dangerously-skip-permissions".into()),
    }
    if let Some(session) = &opts.resume {
        args.extend(["--conversation".into(), session.clone()]);
    }
    args
}

/// Claude Code's names for Antigravity's tools, so their cards look the same.
fn display_name(name: &str) -> String {
    match name {
        "run_command" => "Bash",
        "view_file" | "read_file" => "Read",
        "write_file" | "write_to_file" => "Write",
        "edit_file" | "replace_file_content" | "multi_replace_file_content" | "sed_file" | "notebook_edit" => "Edit",
        "grep_search" => "Grep",
        "find_by_name" => "Glob",
        "list_dir" => "List",
        "search_web" => "WebSearch",
        "read_url_content" => "WebFetch",
        "invoke_subagent" | "browser_subagent" => "Agent",
        other => other,
    }
    .to_string()
}

/// A file a tool works on, from its parameters.
fn path_of(input: &Value) -> Option<&str> {
    ["Path", "TargetFile", "AbsolutePath", "FilePath", "File", "path", "file_path"].iter().find_map(|k| input[k].as_str()).filter(|p| !p.is_empty())
}

fn title(name: &str, input: &Value) -> String {
    let first = |keys: &[&str]| keys.iter().find_map(|k| input[k].as_str()).map(str::to_string);
    let t = match name {
        "run_command" => first(&["CommandLine", "Command", "command"]),
        "list_dir" => first(&["DirectoryPath", "Path"]).map(|p| crate::harness::tools::short_path(&p)),
        "grep_search" => first(&["Query", "Pattern"]).map(|q| match first(&["SearchPath", "SearchDirectory", "Path"]) {
            Some(p) => format!("{q} {}", crate::harness::tools::short_path(&p)),
            None => q,
        }),
        "find_by_name" => first(&["Pattern", "SearchDirectory"]),
        "search_web" => first(&["query", "Query"]),
        "read_url_content" => first(&["Url", "URL", "url"]),
        _ => path_of(input).map(crate::harness::tools::short_path),
    };
    // Anything else: its first string parameter.
    t.map(|t| t.lines().next().unwrap_or("").to_string()).unwrap_or_else(|| super::claude::title(name, input))
}

fn user_message(msg: &UserMsg, system: Option<&str>) -> Value {
    let mut text = match &msg.skill {
        Some(skill) => format!("/{skill} {}", msg.text).trim_end().to_string(),
        None => msg.text.clone(),
    };
    // Images only go in as text: point the agent at the files, which it can view.
    for path in &msg.images {
        text.push_str(&format!("\n\n[Attached image: {}]", path.display()));
    }
    if let Some(system) = system {
        text = format!("<system>\n{system}\n</system>\n\n{text}");
    }
    json!({ "event": "user", "message": { "role": "user", "content": [{ "type": "text", "text": text }] } })
}

struct Call {
    /// An edited file and its content before the edit, for the diff.
    before: Option<(String, String)>,
}

/// What a command asks of the process loop.
enum Next {
    Go,
    Restart,
    Stop,
}

struct Bridge {
    events: Events,
    session: Option<String>,
    model: String,
    /// The system prompt still to go in front of the first message (agy has no flag for it).
    system: Option<String>,
    busy: bool,
    /// Messages written while a turn ran: agy runs each as a turn of its own.
    queued: usize,
    calls: HashMap<String, Call>,
    /// A settings change waiting for the turn to end, to restart with it.
    restart: bool,
    usage: Usage,
    turn: TurnTokens,
    mode: Mode,
    bg_owner: String,
}

impl Bridge {
    fn emit(&self, ev: Ev) {
        let _ = self.events.send(ev);
    }

    fn send(&mut self, proc: &mut LineProcess, msg: &UserMsg) {
        let line = user_message(msg, self.system.take().as_deref());
        if let Err(e) = proc.write(&line) {
            self.emit(Ev::Error(format!("couldn't reach Antigravity: {e}")));
        }
    }

    fn on_cmd(&mut self, cmd: Cmd, proc: &mut LineProcess, opts: &mut Opts) -> Next {
        match cmd {
            Cmd::Send(msg) => {
                if self.busy {
                    self.queued += 1;
                } else {
                    self.busy = true;
                    self.turn = TurnTokens::default();
                    self.usage.turn_input = 0;
                    self.usage.turn_cached = 0;
                    self.emit(Ev::TurnStart);
                }
                self.send(proc, &msg);
            }
            // agy doesn't take a message into the running turn: it runs it as the next turn.
            Cmd::Steer(msg) => return self.on_cmd(Cmd::Send(msg), proc, opts),
            Cmd::Interrupt if self.busy => return Next::Restart,
            Cmd::Interrupt => {}
            Cmd::Approve(..) | Cmd::Answer(..) => {}
            Cmd::SetModel(m) => {
                opts.model = m;
                return self.settings_changed();
            }
            Cmd::SetEffort(e) => {
                if let Some(m) = sibling(&opts.model, e.as_deref()) {
                    opts.model = m;
                }
                opts.effort = e;
                return self.settings_changed();
            }
            Cmd::SetMode(mode) => {
                self.mode = mode;
                opts.mode = mode;
                crate::harness::bg::set_mode(&self.bg_owner, mode);
                return self.settings_changed();
            }
            Cmd::Compact => self.emit(Ev::Notice("Antigravity compacts its context on its own; /compact isn't available.".into())),
            Cmd::SetCompaction { .. } => {}
            Cmd::Undo { .. } => self.emit(Ev::Undone(Err("Antigravity's CLI can't rewind a conversation.".into()))),
            Cmd::StopTask(id) => {
                if !crate::harness::bg::stop(&id) {
                    self.emit(Ev::Notice("Antigravity can't stop that task from here.".into()));
                }
            }
            Cmd::Shutdown => return Next::Stop,
        }
        Next::Go
    }

    /// Settings only take effect in a new process: now when idle, else once the turn ends.
    fn settings_changed(&mut self) -> Next {
        if self.busy {
            self.restart = true;
            Next::Go
        } else {
            Next::Restart
        }
    }

    /// The turn is over (or cut short): closes its open tool cards.
    fn end_turn(&mut self, why: &str) {
        for (id, _) in std::mem::take(&mut self.calls) {
            self.emit(Ev::ToolEnd { id, ok: false, output: why.into(), diff: None });
        }
        self.busy = false;
        self.queued = 0;
        self.emit(Ev::TurnDone);
    }

    fn on_line(&mut self, v: &Value) {
        match v["event"].as_str().unwrap_or("") {
            "init" => {
                if let Some(s) = v["conversation_id"].as_str().filter(|s| !s.is_empty()) {
                    self.session = Some(s.to_string());
                    self.emit(Ev::Session(s.to_string()));
                }
                if let Some(m) = v["init"]["model"].as_str().filter(|m| !m.is_empty()) {
                    self.model = m.to_string();
                    self.usage.window = window(m);
                    self.emit(Ev::Model(m.to_string()));
                }
            }
            "step_update" => self.on_step(&v["step_update"]),
            "result" => {
                let r = &v["result"];
                if r["status"] != "SUCCESS" {
                    let error = r["error"].as_str().filter(|e| !e.is_empty()).unwrap_or("Antigravity reported an error.");
                    self.emit(Ev::Error(error.to_string()));
                }
                if self.queued > 0 {
                    self.queued -= 1;
                } else if self.busy {
                    self.end_turn("Ended with the turn.");
                }
            }
            "error" => {
                let e = v["error"].as_str().or_else(|| v["error"]["message"].as_str()).or_else(|| v["message"].as_str()).unwrap_or("Antigravity reported an error.");
                self.emit(Ev::Error(e.to_string()));
            }
            _ => {}
        }
    }

    fn on_step(&mut self, s: &Value) {
        let index = s["step_index"].as_u64().unwrap_or(0);
        let state = s["state"].as_str().unwrap_or("");
        let running = matches!(state, "ACTIVE" | "PENDING" | "RUNNING" | "WAITING" | "GENERATING");
        // agy starts a turn on its own sometimes (a finished background command); show it working.
        if !self.busy && s["step_type"] != "user_input" && running {
            self.busy = true;
            self.emit(Ev::TurnStart);
        }
        match s["step_type"].as_str().unwrap_or("") {
            "agent_response" => {
                if let Some(text) = s["text_delta"].as_str().filter(|t| !t.is_empty()) {
                    self.turn.streamed(text);
                    self.emit(Ev::TextDelta { id: format!("t{index}"), text: text.to_string() });
                }
            }
            "tool" => {
                let id = format!("s{index}");
                let info = &s["tool_info"];
                let name = info["name"].as_str().or_else(|| s["tool_name"].as_str()).unwrap_or("tool").to_string();
                let input = info["parameters"].clone();
                if !self.calls.contains_key(&id) {
                    let edits = display_name(&name) == "Write" || display_name(&name) == "Edit";
                    let before = path_of(&input).filter(|_| edits).map(|p| (p.to_string(), std::fs::read_to_string(p).unwrap_or_default()));
                    self.emit(Ev::ToolStart { id: id.clone(), name: display_name(&name), title: title(&name, &input), input: input.clone() });
                    self.calls.insert(id.clone(), Call { before });
                }
                if !running {
                    let call = self.calls.remove(&id);
                    let ok = state == "DONE" && s["error"].is_null();
                    let output = match &info["output"] {
                        Value::String(o) => o.clone(),
                        Value::Null => s["error"].as_str().unwrap_or("").to_string(),
                        other => other.to_string(),
                    };
                    let diff = call.and_then(|c| c.before).filter(|_| ok).and_then(|(path, old)| {
                        let new = std::fs::read_to_string(&path).ok()?;
                        (new != old).then(|| diff::unified(&crate::harness::tools::short_path(&path), &old, &new))
                    });
                    self.emit(Ev::ToolEnd { id, ok, output, diff });
                }
            }
            _ => {
                if let Some(e) = s["error"].as_str().filter(|e| !e.is_empty()) {
                    self.emit(Ev::Error(e.to_string()));
                }
            }
        }
        if !s["usage"].is_null() {
            self.on_usage(&s["usage"]);
        }
    }

    /// One model request's usage (a step's).
    fn on_usage(&mut self, u: &Value) {
        let n = |k: &str| u[k].as_u64().unwrap_or(0);
        let (input, output, cached) = (n("input_tokens"), n("output_tokens"), n("cache_read_tokens"));
        let total = self.turn.now();
        self.turn.exact(total + output);
        let us = &mut self.usage;
        us.context = input + output;
        us.input += input;
        us.output += output;
        us.cached += cached;
        us.turn = self.turn.now();
        us.turn_input += input;
        us.turn_cached += cached;
        us.turn_streamed = self.turn.streamed_total();
        if us.window == 0 {
            us.window = window(&self.model);
        }
        self.emit(Ev::Usage(self.usage.clone()));
    }
}

/// Kills agy with the commands it runs: they live in shells of their own that outlast it.
fn kill_tree(proc: &mut LineProcess) {
    let root = proc.child.id();
    #[cfg(unix)]
    {
        let table = std::process::Command::new("ps").args(["-A", "-o", "pid=,ppid="]).output().map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default();
        let pairs: Vec<(u32, u32)> = table
            .lines()
            .filter_map(|l| {
                let mut it = l.split_whitespace().map(|n| n.parse::<u32>().ok());
                Some((it.next()??, it.next()??))
            })
            .collect();
        let mut tree = vec![root];
        let mut i = 0;
        while i < tree.len() {
            let parent = tree[i];
            tree.extend(pairs.iter().filter(|(_, pp)| *pp == parent).map(|(p, _)| *p));
            i += 1;
        }
        for pid in tree.into_iter().skip(1) {
            // SAFETY: plain kill(2) on a pid we just listed.
            unsafe { libc::kill(pid as i32, libc::SIGKILL) };
        }
    }
    #[cfg(windows)]
    {
        let mut cmd = std::process::Command::new("taskkill");
        cmd.args(["/T", "/F", "/PID", &root.to_string()]);
        let _ = crate::env::no_window(&mut cmd).output();
    }
    let _ = proc.child.kill();
}

fn spawn(opts: &Opts, me: &Sender<In>) -> Result<LineProcess, String> {
    LineProcess::spawn("agy", &args(opts), &[], &opts.cwd, me.clone())
}

pub fn run(opts: Opts, rx: Receiver<In>, me: Sender<In>, events: Events) {
    let mut opts = opts;
    let mut proc = match spawn(&opts, &me) {
        Ok(p) => p,
        Err(e) => {
            let _ = events.send(Ev::Exited(e));
            return;
        }
    };
    let mut b = Bridge {
        events,
        session: opts.resume.clone(),
        model: opts.model.clone(),
        system: opts.system.clone().filter(|s| !s.trim().is_empty()),
        busy: false,
        queued: 0,
        calls: HashMap::new(),
        restart: false,
        usage: Usage { window: window(&opts.model), ..Default::default() },
        turn: TurnTokens::default(),
        mode: opts.mode,
        bg_owner: opts.bg_owner.clone(),
    };
    crate::harness::bg::set_mode(&b.bg_owner, b.mode);
    if !opts.mcp.is_empty() {
        b.emit(Ev::Notice("Antigravity only loads MCP servers from its own config (`agy mcp`), so winfer's MCP tools aren't attached.".into()));
    }
    if opts.mode == Mode::Ask {
        b.emit(Ev::Notice("Antigravity's CLI runs tools without asking (it has no approval protocol).".into()));
    }
    // Commands that came in during a restart, and exits of replaced processes still to come.
    let mut held: VecDeque<Cmd> = VecDeque::new();
    let mut stale_exits = 0;
    loop {
        let msg = match held.pop_front() {
            Some(cmd) => In::Cmd(cmd),
            None => match super::recv(&rx) {
                Some(m) => m,
                None => {
                    kill_tree(&mut proc);
                    return;
                }
            },
        };
        let next = match msg {
            In::Line(line) => {
                if let Ok(v) = serde_json::from_str::<Value>(&line) {
                    b.on_line(&v);
                }
                if b.restart && !b.busy { Next::Restart } else { Next::Go }
            }
            In::Closed if stale_exits > 0 => {
                stale_exits -= 1;
                Next::Go
            }
            In::Closed => {
                let reason = proc.exit_reason();
                let _ = b.events.send(Ev::Exited(format!("Antigravity stopped: {reason}")));
                return;
            }
            In::Cmd(cmd) => b.on_cmd(cmd, &mut proc, &mut opts),
        };
        match next {
            Next::Go => {}
            Next::Stop => {
                kill_tree(&mut proc);
                return;
            }
            Next::Restart => {
                // Stop it and carry on with the same conversation in a new process.
                kill_tree(&mut proc);
                let _ = proc.child.wait();
                let mut cmds = Vec::new();
                stale_exits += usize::from(!super::drain_closed(&rx, &mut cmds));
                held.extend(cmds);
                if b.busy {
                    b.end_turn("Interrupted.");
                    b.emit(Ev::Notice("Interrupted.".into()));
                }
                b.restart = false;
                opts.resume = b.session.clone();
                proc = match spawn(&opts, &me) {
                    Ok(p) => p,
                    Err(e) => {
                        let _ = b.events.send(Ev::Exited(format!("Antigravity didn't restart: {e}")));
                        return;
                    }
                };
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn efforts_fit_the_model() {
        let ids = ["gemini-3.8-flash-low", "gemini-3.8-flash-high", "gemini-3.1-pro", "gemini-4-argon"];
        assert_eq!(efforts("gemini-3.8-flash-low", &ids), Some(vec!["low".to_string(), "high".to_string()]));
        assert_eq!(efforts("gemini-3.1-pro", &ids), Some(vec!["low".to_string(), "high".to_string()]));
        assert_eq!(efforts("gemini-4-argon", &ids), Some(vec!["low".to_string(), "medium".to_string(), "high".to_string()]));
    }

    #[test]
    fn effort_fits_the_model() {
        assert_eq!(effort_flag("gemini-3.8-flash-low", Some("medium")), None);
        assert_eq!(effort_flag("gemini-3.8-flash", Some("minimal")), Some("low".into()));
        assert_eq!(sibling("gemini-3.8-flash-high", Some("low")), Some("gemini-3.8-flash-low".into()));
        assert_eq!(sibling("gemini-3.8-flash-low", Some("max")), Some("gemini-3.8-flash-high".into()));
        assert_eq!(sibling("claude-opus-4-6-thinking", Some("low")), None);
    }

    #[test]
    fn tool_steps_become_cards() {
        let (tx, rx) = std::sync::mpsc::channel();
        let mut b = Bridge {
            events: tx, session: None, model: String::new(), system: None, busy: true, queued: 0, calls: HashMap::new(),
            restart: false, usage: Usage::default(), turn: TurnTokens::default(), mode: Mode::Ask, bg_owner: String::new(),
        };
        let step = |state: &str, output: Option<&str>| json!({ "event": "step_update", "step_update": {
            "step_index": 2, "state": state, "step_type": "tool", "tool_name": "run_command",
            "tool_info": { "name": "run_command", "parameters": { "CommandLine": "ls" }, "output": output } } });
        b.on_line(&step("ACTIVE", None));
        b.on_line(&step("DONE", Some("a.txt")));
        b.on_line(&json!({ "event": "result", "result": { "status": "SUCCESS" } }));
        let evs: Vec<Ev> = rx.try_iter().collect();
        assert!(matches!(&evs[0], Ev::ToolStart { name, title, .. } if name == "Bash" && title == "ls"));
        assert!(matches!(&evs[1], Ev::ToolEnd { ok: true, output, .. } if output == "a.txt"));
        assert!(matches!(&evs[2], Ev::TurnDone));
    }
}
