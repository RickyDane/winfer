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
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender};

use serde_json::{Value, json};

use super::{In, LineProcess, Opts};
use crate::harness::diff;
use crate::harness::event::{Cmd, Ev, Events, Mode, ModelInfo, TurnTokens, Usage, UserMsg};

/// Effort suffixes `agy models` attaches to per-level model variants, lowest first.
const EFFORT_SUFFIXES: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];

/// Splits an effort-suffixed Antigravity model id (`gemini-4-neon-xhigh` → `("gemini-4-neon", Some("xhigh"))`).
fn split_effort(id: &str) -> (&str, Option<&str>) {
    for level in EFFORT_SUFFIXES {
        if let Some(base) = id.strip_suffix(&format!("-{level}")) {
            if !base.is_empty() {
                return (base, Some(level));
            }
        }
    }
    (id, None)
}

/// Drops a trailing effort tag from `agy models`' label (`Gemini 4 Neon (XHigh)` → `Gemini 4 Neon`),
/// keeping non-effort parentheticals like `Orchestrator (Neon)` intact.
fn clean_label(label: &str) -> String {
    let trimmed = label.trim();
    if let Some(open) = trimmed.rfind('(') {
        if let Some(inside) = trimmed[open + 1..].strip_suffix(')') {
            let tag = inside.trim().to_ascii_lowercase();
            if matches!(tag.as_str(), "low" | "medium" | "high" | "xhigh" | "extra" | "extra high" | "extra-high" | "max") {
                return trimmed[..open].trim_end().to_string();
            }
        }
    }
    trimmed.to_string()
}

/// Consolidates `agy models`' per-effort variants by base slug so each model appears once and its
/// reasoning level is controlled solely through the effort picker.
fn consolidate(parsed: &[crate::providers::ModelInfo]) -> Vec<ModelInfo> {
    let mut order: Vec<String> = Vec::new();
    let mut groups: HashMap<String, (String, Vec<&'static str>)> = HashMap::new();
    for m in parsed {
        let (base, level) = split_effort(&m.id);
        let entry = groups.entry(base.to_string()).or_insert_with(|| {
            order.push(base.to_string());
            (clean_label(&m.label), Vec::new())
        });
        if let Some(l) = level {
            if let Some(&canonical) = EFFORT_SUFFIXES.iter().find(|&&s| s == l) {
                if !entry.1.contains(&canonical) {
                    entry.1.push(canonical);
                }
            }
        }
    }
    order
        .into_iter()
        .map(|base| {
            let (label, seen) = groups.remove(&base).unwrap();
            let levels: Vec<String> = EFFORT_SUFFIXES.iter().filter(|l| seen.contains(l)).map(|s| (*s).to_string()).collect();
            let efforts = if levels.len() > 1 {
                Some(levels)
            } else if levels.len() == 1 || base.contains("orchestrator") {
                // Single fixed level (e.g. gpt-oss-120b-medium) or Orchestrator: no adjustable effort.
                Some(Vec::new())
            } else if base.starts_with("gemini-3.1-pro") {
                Some(["low", "high"].map(String::from).into())
            } else {
                Some(["low", "medium", "high"].map(String::from).into())
            };
            ModelInfo { detail: format!("Antigravity · {}", vendor(&base)), spec: format!("agy/{base}"), efforts, label }
        })
        .collect()
}

/// The models `agy` offers, consolidated by base slug as `agy/<model>` specs; empty when it isn't installed.
pub fn models() -> Vec<ModelInfo> {
    static CACHE: std::sync::Mutex<Option<Vec<ModelInfo>>> = std::sync::Mutex::new(None);
    if crate::env::find_command("agy").is_none() {
        return Vec::new();
    }
    if let Some(hit) = CACHE.lock().unwrap().clone() {
        return hit;
    }
    let Some(out) = list_models() else { return Vec::new() };
    let list = consolidate(&crate::providers::parse_models(&out));
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

/// Maps a requested effort to the closest level supported by `base` (None when the model has no
/// adjustable effort, e.g. Orchestrator or a single-level model, so `agy` never rejects `--effort`).
fn fit_effort(base: &str, effort: Option<&str>, catalog: &[ModelInfo]) -> Option<String> {
    let raw = effort?.trim();
    if raw.is_empty() || raw == "default" {
        return None;
    }
    let req = if raw == "minimal" { "low" } else { raw };
    let supported: Vec<&str> = match catalog.iter().find(|m| m.spec.strip_prefix("agy/") == Some(base)).and_then(|m| m.efforts.as_deref()) {
        Some(list) => list.iter().map(String::as_str).collect(),
        None if base.contains("orchestrator") => Vec::new(),
        None if base.starts_with("gemini-3.1-pro") => vec!["low", "high"],
        None => vec!["low", "medium", "high", "xhigh"],
    };
    if supported.is_empty() {
        return None;
    }
    if supported.contains(&req) {
        return Some(req.to_string());
    }
    let rank = |l: &str| EFFORT_SUFFIXES.iter().position(|&s| s == l);
    let target = rank(req)?;
    // Prefer clamping downward to the highest supported level <= requested, else the closest above.
    supported
        .iter()
        .filter_map(|&s| rank(s).map(|r| (r, s)))
        .filter(|(r, _)| *r <= target)
        .max_by_key(|(r, _)| *r)
        .or_else(|| supported.iter().filter_map(|&s| rank(s).map(|r| (r, s))).min_by_key(|(r, _)| r.abs_diff(target)))
        .map(|(_, s)| s.to_string())
}

/// Resolves a (possibly legacy effort-suffixed) model and optional effort into the non-conflicting
/// `(--model, --effort)` pair for `agy`.
fn resolve_model_and_effort(model: &str, effort: Option<&str>, catalog: &[ModelInfo]) -> (String, Option<String>) {
    if model.is_empty() {
        return (String::new(), effort.map(|e| if e == "minimal" { "low" } else { e }.to_string()));
    }
    let (base, _) = split_effort(model);
    (base.to_string(), fit_effort(base, effort, catalog))
}

/// Temporary workspace directory holding `.agents/mcp_config.json` for `agy --add-dir`; removed
/// when the bridge exits so sessions stay isolated and leave nothing behind.
struct McpDir(PathBuf);

impl McpDir {
    fn create(servers: &serde_json::Map<String, Value>) -> Result<Self, String> {
        static SEQ: AtomicU64 = AtomicU64::new(1);
        let key = format!("code-{}-{}", std::process::id(), SEQ.fetch_add(1, Ordering::Relaxed));
        crate::mcp::write_agy_mcp_dir(&key, servers)
            .map(Self)
            .map_err(|e| format!("couldn't write Antigravity MCP config: {e}"))
    }
}

impl Drop for McpDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Session-only `--settings` JSON allowing MCP tool calls for the servers winfer attaches (print
/// mode has no approval protocol and denies unallowed MCP calls). Existing `permissions.allow`
/// entries from `~/.gemini/antigravity-cli/settings.json` are preserved so `--settings` doesn't
/// drop the user's own allow rules.
fn mcp_settings_arg(servers: &serde_json::Map<String, Value>, existing: Option<&Value>) -> Option<String> {
    if servers.is_empty() {
        return None;
    }
    let mut allow: Vec<String> = existing
        .and_then(|v| v["permissions"]["allow"].as_array())
        .into_iter()
        .flatten()
        .filter_map(|v| v.as_str().map(String::from))
        .collect();
    for name in servers.keys() {
        let rule = format!("mcp({name}/*)");
        if !allow.iter().any(|r| r == &rule) {
            allow.push(rule);
        }
    }
    let mut perms = existing
        .and_then(|v| v["permissions"].as_object())
        .cloned()
        .unwrap_or_default();
    perms.insert("allow".into(), json!(allow));
    Some(json!({ "permissions": perms }).to_string())
}

fn user_settings() -> Option<Value> {
    let path = dirs::home_dir()?.join(".gemini/antigravity-cli/settings.json");
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

fn args(opts: &Opts, mcp_dir: Option<&Path>, settings: Option<&str>, catalog: &[ModelInfo]) -> Vec<String> {
    // `--print=`: a bare `--print` would take the next flag as its prompt.
    let mut args: Vec<String> = ["--print=", "--input-format", "stream-json", "--output-format", "stream-json"].iter().map(|s| s.to_string()).collect();
    let (model, effort) = resolve_model_and_effort(&opts.model, opts.effort.as_deref(), catalog);
    if !model.is_empty() {
        args.extend(["--model".into(), model]);
    }
    if let Some(e) = effort {
        args.extend(["--effort".into(), e]);
    }
    match opts.mode {
        Mode::Ask => {}
        Mode::AutoEdit => args.extend(["--mode".into(), "accept-edits".into()]),
        Mode::Plan | Mode::YoloPlan => args.extend(["--mode".into(), "plan".into()]),
        Mode::Yolo => args.push("--dangerously-skip-permissions".into()),
    }
    if let Some(dir) = mcp_dir {
        args.extend(["--add-dir".into(), dir.to_string_lossy().into_owned()]);
    }
    if let Some(s) = settings.filter(|_| opts.mode != Mode::Yolo) {
        args.extend(["--settings".into(), s.to_string()]);
    }
    if let Some(session) = &opts.resume {
        args.extend(["--conversation".into(), session.clone()]);
    }
    args
}

/// Claude Code's names for Antigravity's tools, so their cards look the same.
fn display_name(name: &str, input: &Value) -> String {
    if name == "call_mcp_tool" {
        let server = input["ServerName"].as_str().unwrap_or("");
        let tool = input["ToolName"].as_str().unwrap_or("");
        if !server.is_empty() && !tool.is_empty() {
            return format!("{server} · {tool}");
        }
    }
    if let Some(rest) = name.strip_prefix("mcp_") {
        if let Some((server, tool)) = rest.split_once('_') {
            if !server.is_empty() && !tool.is_empty() {
                return format!("{server} · {tool}");
            }
        }
    }
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
    if name == "call_mcp_tool" {
        let args = &input["Arguments"];
        if input["ServerName"].as_str() == Some("winfer") && input["ToolName"].as_str() == Some("bg") {
            return crate::harness::tools::title("bg", args);
        }
        return super::claude::title(input["ToolName"].as_str().unwrap_or(name), args);
    }
    if name == "mcp_winfer_bg" {
        return crate::harness::tools::title("bg", input);
    }
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
                opts.model = split_effort(&m).0.to_string();
                return self.settings_changed();
            }
            Cmd::SetEffort(e) => {
                opts.model = split_effort(&opts.model).0.to_string();
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
            // Its subagents (if any) run on the main model.
            Cmd::SetSubagent { .. } => {}
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
                    let base = split_effort(m).0;
                    self.model = base.to_string();
                    self.usage.window = window(base);
                    self.emit(Ev::Model(base.to_string()));
                }
            }
            "step_update" => self.on_step(&v["step_update"]),
            "result" => {
                let r = &v["result"];
                if !r["usage"].is_null() {
                    self.on_usage(&r["usage"]);
                }
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
                    if self.turn.streamed(text) {
                        self.emit_usage();
                    }
                    self.emit(Ev::TextDelta { id: format!("t{index}"), text: text.to_string() });
                }
            }
            "tool" => {
                let id = format!("s{index}");
                let info = &s["tool_info"];
                let name = info["name"].as_str().or_else(|| s["tool_name"].as_str()).unwrap_or("tool").to_string();
                let input = info["parameters"].clone();
                if !self.calls.contains_key(&id) {
                    let shown = display_name(&name, &input);
                    let edits = shown == "Write" || shown == "Edit";
                    let before = path_of(&input).filter(|_| edits).map(|p| (p.to_string(), std::fs::read_to_string(p).unwrap_or_default()));
                    self.emit(Ev::ToolStart { id: id.clone(), name: shown, title: title(&name, &input), input: input.clone() });
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

    /// Streaming usage snapshot while text deltas arrive.
    fn emit_usage(&mut self) {
        let mut u = self.usage.clone();
        u.turn = self.turn.now();
        u.turn_streamed = self.turn.streamed_total();
        u.turn_active_ms = self.turn.active_ms();
        if u.window == 0 {
            u.window = window(&self.model);
        }
        self.emit(Ev::Usage(u));
    }

    /// One model request's usage (a step's).
    /// `agy` reports `input_tokens` excluding `cache_read_tokens` (and any `cache_creation_tokens`),
    /// so total prompt input is their sum (matching Claude/Cursor/Pi).
    fn on_usage(&mut self, u: &Value) {
        let n = |a: &str, b: &str| u[a].as_u64().or_else(|| u[b].as_u64()).unwrap_or(0);
        let raw_input = n("input_tokens", "inputTokens");
        let output = n("output_tokens", "outputTokens");
        let cached = n("cache_read_tokens", "cacheReadTokens");
        let cache_write = n("cache_creation_tokens", "cacheCreationTokens").max(n("cache_write_tokens", "cacheWriteTokens"));
        let input = raw_input + cached + cache_write;
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
        us.turn_active_ms = self.turn.active_ms();
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

fn spawn(opts: &Opts, mcp_dir: Option<&Path>, settings: Option<&str>, catalog: &[ModelInfo], me: &Sender<In>) -> Result<LineProcess, String> {
    LineProcess::spawn("agy", &args(opts, mcp_dir, settings, catalog), &[], &opts.cwd, me.clone())
}

pub fn run(opts: Opts, rx: Receiver<In>, me: Sender<In>, events: Events) {
    let mut opts = opts;
    opts.model = split_effort(&opts.model).0.to_string();
    let catalog = models();
    let mut mcp_err = None;
    let mcp_dir = if opts.mcp.is_empty() {
        None
    } else {
        match McpDir::create(&opts.mcp) {
            Ok(dir) => Some(dir),
            Err(e) => {
                mcp_err = Some(e);
                None
            }
        }
    };
    let settings = mcp_dir.as_ref().and_then(|_| mcp_settings_arg(&opts.mcp, user_settings().as_ref()));
    let mut proc = match spawn(&opts, mcp_dir.as_ref().map(|d| d.0.as_path()), settings.as_deref(), &catalog, &me) {
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
    if let Some(err) = mcp_err {
        b.emit(Ev::Notice(format!("Couldn't attach winfer's MCP tools to Antigravity: {err}")));
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
                proc = match spawn(&opts, mcp_dir.as_ref().map(|d| d.0.as_path()), settings.as_deref(), &catalog, &me) {
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
    fn consolidates_models_and_includes_xhigh() {
        let raw = |id: &str, label: &str| crate::providers::ModelInfo { id: id.into(), label: label.into() };
        let parsed = vec![
            raw("gemini-4-neon-high", "Gemini 4 Neon (High)"),
            raw("gemini-4-neon-xhigh", "Gemini 4 Neon (XHigh)"),
            raw("gemini-4-neon-medium", "Gemini 4 Neon (Medium)"),
            raw("gemini-4-neon-low", "Gemini 4 Neon (Low)"),
            raw("gemini-4-argon-medium", "Gemini 4 Argon (Medium)"),
            raw("gemini-4-argon-high", "Gemini 4 Argon (High)"),
            raw("gemini-4-argon-low", "Gemini 4 Argon (Low)"),
            raw("gemini-pro-orchestrator-neon", "Orchestrator (Neon)"),
            raw("gpt-oss-120b-medium", "GPT-OSS 120B (Medium)"),
        ];
        let out = consolidate(&parsed);
        let specs: Vec<&str> = out.iter().map(|m| m.spec.as_str()).collect();
        assert_eq!(specs, ["agy/gemini-4-neon", "agy/gemini-4-argon", "agy/gemini-pro-orchestrator-neon", "agy/gpt-oss-120b"]);
        assert_eq!(out[0].label, "Gemini 4 Neon");
        assert_eq!(out[0].efforts, Some(vec!["low".into(), "medium".into(), "high".into(), "xhigh".into()]));
        assert_eq!(out[1].efforts, Some(vec!["low".into(), "medium".into(), "high".into()]));
        assert_eq!(out[2].label, "Orchestrator (Neon)");
        assert_eq!(out[2].efforts, Some(vec![]));
        assert_eq!(out[3].label, "GPT-OSS 120B");
        assert_eq!(out[3].efforts, Some(vec![]));

        // Legacy suffixed specs resolve cleanly without conflicting `--effort`, and unsupported
        // levels clamp to the highest supported level (or drop on fixed models).
        assert_eq!(resolve_model_and_effort("gemini-4-neon-xhigh", Some("high"), &out), ("gemini-4-neon".into(), Some("high".into())));
        assert_eq!(resolve_model_and_effort("gemini-4-neon", Some("xhigh"), &out), ("gemini-4-neon".into(), Some("xhigh".into())));
        assert_eq!(resolve_model_and_effort("gemini-4-argon", Some("xhigh"), &out), ("gemini-4-argon".into(), Some("high".into())));
        assert_eq!(resolve_model_and_effort("gemini-pro-orchestrator-neon", Some("high"), &out), ("gemini-pro-orchestrator-neon".into(), None));
        assert_eq!(resolve_model_and_effort("gpt-oss-120b", Some("high"), &out), ("gpt-oss-120b".into(), None));
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

    #[test]
    fn mcp_tools_attach_via_add_dir_and_settings() {
        let mut servers = serde_json::Map::new();
        servers.insert("winfer".into(), json!({ "command": "/app/winfer", "args": ["mcp-stdio"], "env": { "WINFER_PANE_ID": "p-9" } }));
        let dir = McpDir::create(&servers).unwrap();
        let cfg_path = dir.0.join(".agents/mcp_config.json");
        let cfg: Value = serde_json::from_str(&std::fs::read_to_string(&cfg_path).unwrap()).unwrap();
        assert_eq!(cfg["mcpServers"]["winfer"]["command"], "/app/winfer");
        assert_eq!(cfg["mcpServers"]["winfer"]["timeoutSeconds"], 3660);

        let existing = json!({ "permissions": { "allow": ["command(git)"], "deny": ["command(rm)"] } });
        let settings = mcp_settings_arg(&servers, Some(&existing)).unwrap();
        let parsed: Value = serde_json::from_str(&settings).unwrap();
        assert_eq!(parsed["permissions"]["allow"], json!(["command(git)", "mcp(winfer/*)"]));
        assert_eq!(parsed["permissions"]["deny"], json!(["command(rm)"]));

        let path = dir.0.clone();
        drop(dir);
        assert!(!path.exists());
    }

    #[test]
    fn mcp_tool_cards_format_cleanly() {
        let lazy = json!({ "ServerName": "winfer", "ToolName": "bg", "Arguments": { "action": "list" } });
        assert_eq!(display_name("call_mcp_tool", &lazy), "winfer · bg");
        assert_eq!(title("call_mcp_tool", &lazy), "list");
        assert_eq!(display_name("mcp_winfer_open_url", &json!({ "url": "http://localhost:1420" })), "winfer · open_url");
    }

    #[test]
    fn usage_counts_cached_tokens_in_input() {
        let (tx, rx) = std::sync::mpsc::channel();
        let mut b = Bridge {
            events: tx, session: None, model: "gemini-4-neon".into(), system: None, busy: true, queued: 0, calls: HashMap::new(),
            restart: false, usage: Usage::default(), turn: TurnTokens::default(), mode: Mode::Ask, bg_owner: String::new(),
        };
        b.on_usage(&json!({ "input_tokens": 22_100, "cache_read_tokens": 38_900, "output_tokens": 972 }));
        let Some(Ev::Usage(u)) = rx.try_iter().last() else { panic!("expected Usage event") };
        assert_eq!(u.input, 61_000);
        assert_eq!(u.turn_input, 61_000);
        assert_eq!(u.cached, 38_900);
        assert_eq!(u.turn_cached, 38_900);
        assert_eq!(u.output, 972);
        assert_eq!(u.context, 61_972);
        assert!(u.cached <= u.input);
    }
}
