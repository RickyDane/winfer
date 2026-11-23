//! Google Antigravity: winfer code's bridge to `agy`.
//!
//! `agy --print= --input-format stream-json --output-format stream-json` is one long-lived
//! process that runs a turn for each `{"event":"user",...}` line on stdin. It reports an
//! `init` (conversation id, model), then `step_update`s (user input, agent responses with
//! `text_delta`s and per-request usage, tool steps going ACTIVE → DONE) and a `result` per turn.
//! Its stdin takes nothing but user messages (text only): no interrupts, approvals or settings
//! changes, so those restart the process on the same conversation (`--conversation`). Print
//! mode runs every tool without asking, whatever the mode.

use std::collections::{HashMap, HashSet, VecDeque};
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};

use super::{In, LineProcess, Opts};
use crate::harness::diff;
use crate::harness::event::{Cmd, Ev, Events, Mode, ModelInfo, TaskEv, TurnTokens, Usage, UserMsg};

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

/// Read-only commands allowed without prompting in `Mode::Plan`.
///
/// `agy --print=` has no interactive approval protocol: any command that evaluates to Ask is
/// immediately denied and aborts the turn (and permanently halts any subagent that hits it).
/// Because Antigravity evaluates every segment of `&&`, `||`, and `|` pipelines against allow
/// rules, common composition helpers (`true`, `sort`, `head`, etc.) are included alongside
/// filesystem/search/git inspection tools.
const PLAN_READONLY_COMMANDS: &[&str] = &[
    "ls", "ll", "find", "fd", "tree", "stat", "file", "wc", "du", "df", "pwd", "realpath", "readlink",
    "dirname", "basename", "which", "type", "cat", "head", "tail", "nl", "rg", "grep", "egrep",
    "fgrep", "ag", "ack", "sort", "uniq", "cut", "tr", "diff", "comm", "cmp", "jq", "yq", "strings",
    "od", "hexdump", "xxd", "shasum", "sha256sum", "md5", "md5sum", "echo", "printf", "true",
    "false", "test", "[", "cd", "uname", "date", "env", "printenv", "id", "whoami", "hostname",
    "ps", "lsof", "sw_vers", "sysctl", "git status", "git log", "git diff", "git show", "git branch",
    "git rev-parse", "git ls-files", "git ls-tree", "git blame", "git grep", "git tag",
    "git describe", "git remote", "git shortlog", "git config --get", "git config --list",
    "gh pr view", "gh pr list", "gh pr diff", "gh issue view", "gh issue list", "gh repo view",
    "cargo metadata", "rustc --version", "node --version", "pnpm --version", "npm --version",
    "python3 --version",
];

fn push_unique(list: &mut Vec<String>, rule: String) {
    if !list.iter().any(|r| r == &rule) {
        list.push(rule);
    }
}

fn normalized_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

/// Workspace `write_file(...)` deny rules for plan modes (includes the canonical path when `cwd`
/// crosses a symlink like `/tmp` → `/private/tmp`), keeping Antigravity's own artifact dir outside
/// the workspace writable.
fn workspace_write_denies(cwd: &Path) -> Vec<String> {
    let mut rules = Vec::new();
    let raw = normalized_path(cwd);
    if !raw.is_empty() {
        push_unique(&mut rules, format!("write_file({raw})"));
    }
    if let Ok(canon) = std::fs::canonicalize(cwd) {
        let canon = normalized_path(&canon);
        if !canon.is_empty() {
            push_unique(&mut rules, format!("write_file({canon})"));
        }
    }
    rules
}

/// Session-only `--settings` JSON for Antigravity:
/// - allows MCP tool calls for the servers winfer attaches (`mcp(<name>/*)`),
/// - in `Mode::Plan`, allows read-only exploration commands + `read_file(*)` + user-configured
///   `allowed_commands`, and denies workspace `write_file(...)` so planning stays read-only
///   without print mode aborting on every shell inspection command,
/// - in `Mode::YoloPlan`, allows `command(*)`, `read_file(*)`, `read_url(*)`, and denies
///   workspace `write_file(...)`.
///
/// Existing `permissions` entries from `~/.gemini/antigravity-cli/settings.json` are preserved so
/// `--settings` doesn't drop the user's own rules.
fn settings_arg(
    mode: Mode,
    cwd: &Path,
    servers: &serde_json::Map<String, Value>,
    allowed_commands: &[String],
    existing: Option<&Value>,
) -> Option<String> {
    if mode == Mode::Yolo {
        return None;
    }
    if !mode.plans() && servers.is_empty() {
        return None;
    }
    let read_list = |key: &str| -> Vec<String> {
        existing
            .and_then(|v| v["permissions"][key].as_array())
            .into_iter()
            .flatten()
            .filter_map(|v| v.as_str().map(String::from))
            .collect()
    };
    let mut allow = read_list("allow");
    let mut deny = read_list("deny");
    for name in servers.keys() {
        push_unique(&mut allow, format!("mcp({name}/*)"));
    }
    match mode {
        Mode::Plan => {
            push_unique(&mut allow, "read_file(*)".into());
            for cmd in PLAN_READONLY_COMMANDS {
                push_unique(&mut allow, format!("command({cmd})"));
            }
            for cmd in allowed_commands {
                let cmd = cmd.trim();
                if !cmd.is_empty() {
                    push_unique(&mut allow, format!("command({cmd})"));
                }
            }
            for rule in workspace_write_denies(cwd) {
                push_unique(&mut deny, rule);
            }
        }
        Mode::YoloPlan => {
            push_unique(&mut allow, "command(*)".into());
            push_unique(&mut allow, "read_file(*)".into());
            push_unique(&mut allow, "read_url(*)".into());
            for rule in workspace_write_denies(cwd) {
                push_unique(&mut deny, rule);
            }
        }
        Mode::Ask | Mode::AutoEdit | Mode::Yolo => {}
    }
    let mut perms = existing
        .and_then(|v| v["permissions"].as_object())
        .cloned()
        .unwrap_or_default();
    perms.insert("allow".into(), json!(allow));
    if !deny.is_empty() {
        perms.insert("deny".into(), json!(deny));
    }
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
        Mode::Ask => args.extend(["--mode".into(), "auto".into()]),
        Mode::AutoEdit => args.extend(["--mode".into(), "accept-edits".into()]),
        Mode::Plan | Mode::YoloPlan => args.extend(["--mode".into(), "plan".into()]),
        Mode::Yolo => args.extend(["--mode".into(), "auto".into(), "--dangerously-skip-permissions".into()]),
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

/// A spawned Antigravity subagent tracked via its `transcript.jsonl` on disk.
struct Subagent {
    agent: String,
    log_path: Option<PathBuf>,
    /// Byte offset already parsed in `log_path`.
    offset: u64,
    tools: u64,
    model: Option<String>,
    last: Option<String>,
    /// True once the subagent sent a message back to the parent via `send_message`.
    sent_reply: bool,
    /// True when the latest observed step was an error.
    saw_error: bool,
    /// True once a terminal `Ev::Task` status has been emitted.
    done: bool,
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(hi), Some(lo)) = ((bytes[i + 1] as char).to_digit(16), (bytes[i + 2] as char).to_digit(16)) {
                out.push(((hi << 4) | lo) as u8);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn file_uri_to_path(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    let decoded = percent_decode(rest);
    if decoded.is_empty() {
        return None;
    }
    #[cfg(windows)]
    let decoded = {
        let b = decoded.as_bytes();
        if b.len() >= 3 && b[0] == b'/' && b[1].is_ascii_alphabetic() && b[2] == b':' {
            decoded[1..].to_string()
        } else {
            decoded
        }
    };
    Some(PathBuf::from(decoded))
}

fn default_subagent_log(conv_id: &str) -> Option<PathBuf> {
    Some(dirs::home_dir()?.join(".gemini/antigravity-cli/brain").join(conv_id).join(".system_generated/logs/transcript.jsonl"))
}

/// Decodes `args` from an Antigravity `transcript.jsonl` tool call, where values are often
/// JSON-encoded strings (e.g. `"CommandLine": "\"ls -la\""`).
fn decode_transcript_args(raw: &Value) -> Value {
    let Some(map) = raw.as_object() else {
        return raw.clone();
    };
    let mut out = serde_json::Map::with_capacity(map.len());
    for (k, v) in map {
        let decoded = match v.as_str() {
            Some(s) => serde_json::from_str::<Value>(s).unwrap_or_else(|_| Value::String(s.to_string())),
            None => v.clone(),
        };
        out.insert(k.clone(), decoded);
    }
    Value::Object(out)
}

fn truncate_line(s: &str, max: usize) -> String {
    let first = s.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim();
    if first.chars().count() <= max {
        first.to_string()
    } else {
        let mut out: String = first.chars().take(max.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

fn format_subagent_tool(name: &str, args: &Value) -> String {
    let shown = display_name(name, args);
    let t = title(name, args);
    let line = if t.is_empty() || t == shown { shown } else { format!("{shown} {t}") };
    truncate_line(&line, 72)
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
    /// Spawned subagents keyed by their Antigravity `conversation_id`.
    subagents: HashMap<String, Subagent>,
    /// A settings change waiting for the turn to end, to restart with it.
    restart: bool,
    usage: Usage,
    /// The running turn's steps reported their usage, so its `result` usage (their sum) is already counted.
    step_usage: bool,
    turn: TurnTokens,
    mode: Mode,
    bg_owner: String,
    asks: Arc<Asks>,
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
            Cmd::Interrupt => {
                self.asks.decline_all();
                if self.busy {
                    return Next::Restart;
                }
            }
            // Declining a question goes through `Approve`.
            Cmd::Approve(id, _) => {
                self.asks.declined.lock().unwrap().insert(id);
            }
            Cmd::Answer(id, answers) => {
                self.asks.answers.lock().unwrap().insert(id, answers);
            }
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
            Cmd::SetSubagent { .. } | Cmd::SetUltracode(_) => {}
            Cmd::Undo { .. } => self.emit(Ev::Undone(Err("Antigravity's CLI can't rewind a conversation.".into()))),
            Cmd::StopTask(id) => {
                if !crate::harness::bg::stop(&id) {
                    self.emit(Ev::Notice("Antigravity can't stop that task from here.".into()));
                }
            }
            Cmd::Shutdown => {
                self.asks.decline_all();
                return Next::Stop;
            }
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

    fn has_running_subagents(&self) -> bool {
        self.subagents.values().any(|s| !s.done)
    }

    /// Reads newly appended lines from each running subagent's `transcript.jsonl` and emits
    /// `Ev::Task` progress or completion updates.
    fn poll_subagents(&mut self) {
        let mut updates: Vec<TaskEv> = Vec::new();
        for (id, sub) in &mut self.subagents {
            if sub.done {
                continue;
            }
            let Some(path) = sub.log_path.as_deref() else { continue };
            let Ok(mut file) = std::fs::File::open(path) else { continue };
            let Ok(len) = file.metadata().map(|m| m.len()) else { continue };
            if len <= sub.offset {
                continue;
            }
            if file.seek(SeekFrom::Start(sub.offset)).is_err() {
                continue;
            }
            let mut reader = BufReader::new(file);
            let mut line = String::new();
            let mut changed = false;
            let mut model_changed = false;
            let mut terminal: Option<&'static str> = None;
            loop {
                line.clear();
                let Ok(n) = reader.read_line(&mut line) else { break };
                if n == 0 || !line.ends_with('\n') {
                    // Wait for the writer to finish the line before advancing `offset`.
                    break;
                }
                sub.offset += n as u64;
                let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
                if let Some(m) = v["model"].as_str().filter(|m| !m.is_empty()) {
                    let base = split_effort(m).0.to_string();
                    if sub.model.as_deref() != Some(base.as_str()) {
                        sub.model = Some(base);
                        model_changed = true;
                        changed = true;
                    }
                }
                let step_type = v["type"].as_str().unwrap_or("");
                let status = v["status"].as_str().unwrap_or("");
                match step_type {
                    "PLANNER_RESPONSE" => {
                        let calls = v["tool_calls"].as_array().map(Vec::as_slice).unwrap_or(&[]);
                        if !calls.is_empty() {
                            sub.saw_error = false;
                            for tc in calls {
                                let name = tc["name"].as_str().unwrap_or("");
                                if name == "send_message" {
                                    sub.sent_reply = true;
                                    sub.last = Some("Reporting results".into());
                                    changed = true;
                                    continue;
                                }
                                if !name.is_empty() {
                                    let args = decode_transcript_args(&tc["args"]);
                                    sub.tools += 1;
                                    sub.last = Some(format_subagent_tool(name, &args));
                                    changed = true;
                                }
                            }
                        } else if status == "DONE" {
                            terminal = Some("completed");
                            changed = true;
                        }
                    }
                    "GENERIC" => {
                        if status == "ERROR" {
                            sub.saw_error = true;
                            let err = v["error"].as_str().unwrap_or("");
                            if err.contains("permission check failed") || err.contains("user denied permission") {
                                sub.last = Some("Permission denied".into());
                                terminal = Some("failed");
                                changed = true;
                            }
                        } else if status == "DONE" {
                            sub.saw_error = false;
                        }
                    }
                    _ => {}
                }
            }
            if changed {
                if terminal.is_some() {
                    sub.done = true;
                }
                updates.push(TaskEv {
                    id: id.clone(),
                    agent: Some(sub.agent.clone()),
                    tools: Some(sub.tools),
                    last: sub.last.clone(),
                    status: terminal.map(str::to_string),
                    model: model_changed.then(|| sub.model.clone()).flatten(),
                    ..Default::default()
                });
            }
        }
        for t in updates {
            self.emit(Ev::Task(t));
        }
    }

    /// Reconciles running subagents once a parent turn finishes with a `result` event.
    fn settle_subagents_on_result(&mut self, ok: bool) {
        self.poll_subagents();
        let mut updates = Vec::new();
        for (id, sub) in &mut self.subagents {
            if sub.done {
                continue;
            }
            let status = if sub.saw_error || !ok {
                Some("failed")
            } else if sub.sent_reply {
                Some("completed")
            } else {
                None
            };
            if let Some(status) = status {
                sub.done = true;
                updates.push(TaskEv {
                    id: id.clone(),
                    agent: Some(sub.agent.clone()),
                    tools: Some(sub.tools),
                    last: sub.last.clone(),
                    status: Some(status.into()),
                    ..Default::default()
                });
            }
        }
        for t in updates {
            self.emit(Ev::Task(t));
        }
    }

    /// Marks every still-running subagent with `status` (e.g. `"stopped"` on interrupt or exit).
    fn finish_subagents(&mut self, status: &str) {
        let mut updates = Vec::new();
        for (id, sub) in &mut self.subagents {
            if sub.done {
                continue;
            }
            sub.done = true;
            updates.push(TaskEv {
                id: id.clone(),
                agent: Some(sub.agent.clone()),
                tools: Some(sub.tools),
                last: sub.last.clone(),
                status: Some(status.into()),
                ..Default::default()
            });
        }
        for t in updates {
            self.emit(Ev::Task(t));
        }
    }

    fn on_subagent_step(&mut self, s: &Value, index: u64, state: &str, running: bool) {
        let empty = Vec::new();
        let list = s["subagent_info"]["subagents"].as_array().unwrap_or(&empty);
        if list.is_empty() {
            let id = format!("s{index}");
            if !self.calls.contains_key(&id) {
                self.emit(Ev::ToolStart { id: id.clone(), name: "Agent".into(), title: "subagent".into(), input: s["subagent_info"].clone() });
                self.calls.insert(id.clone(), Call { before: None });
            }
            if !running {
                self.calls.remove(&id);
                let ok = state == "DONE" && s["error"].is_null();
                let output = s["error"].as_str().unwrap_or("").to_string();
                self.emit(Ev::ToolEnd { id, ok, output, diff: None });
            }
            return;
        }
        let multi = list.len() > 1;
        let mut spawned = false;
        for (i, sub) in list.iter().enumerate() {
            let card_id = if multi { format!("s{index}-{i}") } else { format!("s{index}") };
            let agent = sub["type_name"].as_str().filter(|v| !v.is_empty()).unwrap_or("subagent").to_string();
            let role = sub["role"].as_str().filter(|v| !v.is_empty());
            let prompt = sub["initial_prompt"].as_str().filter(|v| !v.is_empty());
            let card_title = role
                .map(str::to_string)
                .or_else(|| prompt.map(|p| truncate_line(p, 80)).filter(|t| !t.is_empty()))
                .unwrap_or_else(|| agent.clone());
            let description = prompt.map(|p| truncate_line(p, 160)).filter(|d| !d.is_empty() && d != &card_title);
            if !self.calls.contains_key(&card_id) {
                let input = json!({
                    "subagent_type": agent,
                    "role": role,
                    "prompt": prompt,
                });
                self.emit(Ev::ToolStart { id: card_id.clone(), name: "Agent".into(), title: card_title.clone(), input });
                self.calls.insert(card_id.clone(), Call { before: None });
            }
            if !running {
                let existed = self.calls.remove(&card_id).is_some();
                let ok = state == "DONE" && s["error"].is_null();
                let conv_id = sub["conversation_id"].as_str().filter(|v| !v.is_empty()).map(String::from);
                if ok {
                    if let Some(conv_id) = conv_id {
                        if !self.subagents.contains_key(&conv_id) {
                            let log_path = sub["log_uri"].as_str().and_then(file_uri_to_path).or_else(|| default_subagent_log(&conv_id));
                            let model = (!self.model.is_empty()).then(|| self.model.clone());
                            self.subagents.insert(
                                conv_id.clone(),
                                Subagent {
                                    agent: agent.clone(),
                                    log_path,
                                    offset: 0,
                                    tools: 0,
                                    model: model.clone(),
                                    last: None,
                                    sent_reply: false,
                                    saw_error: false,
                                    done: false,
                                },
                            );
                            self.emit(Ev::Task(TaskEv {
                                id: conv_id,
                                tool_id: Some(card_id.clone()),
                                title: Some(card_title.clone()),
                                description,
                                agent: Some(agent.clone()),
                                tools: Some(0),
                                last: None,
                                status: Some("running".into()),
                                output_file: None,
                                exit_code: None,
                                model,
                            }));
                            spawned = true;
                        }
                    }
                    if existed {
                        self.emit(Ev::ToolEnd { id: card_id, ok: true, output: format!("Started {agent} subagent ({card_title})."), diff: None });
                    }
                } else if existed {
                    let output = s["error"].as_str().unwrap_or("Subagent failed to start.").to_string();
                    self.emit(Ev::ToolEnd { id: card_id, ok: false, output, diff: None });
                }
            }
        }
        if spawned {
            self.poll_subagents();
        }
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
                let ok = r["status"] == "SUCCESS";
                self.settle_subagents_on_result(ok);
                // The result's usage is the whole turn's: the sum of its model requests. Counting it
                // again would double the totals and make the context the sum of every request's prompt.
                if !r["usage"].is_null() && !std::mem::take(&mut self.step_usage) {
                    self.on_usage(&r["usage"]);
                }
                if !ok {
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
            "subagent" => self.on_subagent_step(s, index, state, running),
            "system_message" => self.poll_subagents(),
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
            self.step_usage = true;
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

/// What the model is told when winfer's `ask_user` is attached.
const ASK_USER_NOTE: &str = "To ask the user a question (a choice, a decision, a missing detail), call the `ask_user` tool of the winfer MCP server. Never use the built-in `ask_question` tool: print mode skips it and returns an empty answer.";

/// Antigravity's own `ask_question` has no way to reach the user in print mode (it comes back
/// skipped), so winfer's `ask_user` MCP tool stands in: it's attached even though winfer code
/// otherwise hides it (its other backends show their own picker). False if there's no winfer server.
fn expose_ask_user(servers: &mut serde_json::Map<String, Value>) -> bool {
    let Some(env) = servers.get_mut("winfer").and_then(|s| s["env"].as_object_mut()) else {
        return false;
    };
    if let Some(hidden) = env.get(crate::mcp::HIDE_ENV).and_then(Value::as_str).map(String::from) {
        let kept: Vec<&str> = hidden.split(',').map(str::trim).filter(|t| !t.is_empty() && *t != crate::harness::tools::ASK_USER).collect();
        if kept.is_empty() {
            env.remove(crate::mcp::HIDE_ENV);
        } else {
            env.insert(crate::mcp::HIDE_ENV.into(), json!(kept.join(",")));
        }
    }
    true
}

/// Sends winfer's `ask_user` to winfer code's own picker: the MCP server asks winfer code's server
/// (`bg::serve`) rather than the app. False when winfer code runs no such server.
fn route_ask_user(servers: &mut serde_json::Map<String, Value>) -> bool {
    let Some(env) = servers.get_mut("winfer").and_then(|s| s["env"].as_object_mut()) else {
        return false;
    };
    if !env.contains_key(crate::harness::bg::CODE_BG_ENV) {
        return false;
    }
    env.insert(crate::harness::bg::CODE_ASK_ENV.into(), json!("1"));
    true
}

/// Questions from Antigravity's `ask_user` calls (answered through the TUI's picker), by id.
#[derive(Default)]
struct Asks {
    answers: Mutex<HashMap<String, Vec<String>>>,
    /// Questions the user dismissed.
    declined: Mutex<HashSet<String>>,
    /// Questions waiting for an answer.
    open: Mutex<HashSet<String>>,
}

impl Asks {
    fn decline_all(&self) {
        let open = self.open.lock().unwrap().clone();
        self.declined.lock().unwrap().extend(open);
    }
}

static ASK_SERIAL: AtomicU64 = AtomicU64::new(1);

/// An `ask_user` call: shows the questions as the TUI's picker and waits for the answers.
fn ask_user(events: &Events, asks: &Asks, args: &Value) -> (bool, String) {
    use crate::harness::event::{Approval, Question};
    use crate::harness::tools;
    let questions = Question::parse_all(args);
    if questions.is_empty() {
        return (false, "ask_user needs at least one question with its options.".into());
    }
    let id = format!("q-agy-{}", ASK_SERIAL.fetch_add(1, Ordering::Relaxed));
    asks.open.lock().unwrap().insert(id.clone());
    let approval = Approval { id: id.clone(), tool: "Question".into(), title: tools::title(tools::ASK_USER, args), questions: questions.clone(), ..Default::default() };
    let started = std::time::Instant::now();
    let answers = if events.send(Ev::Approval(approval)).is_err() {
        None
    } else {
        loop {
            if let Some(a) = asks.answers.lock().unwrap().remove(&id) {
                break Some(a);
            }
            if asks.declined.lock().unwrap().remove(&id) || started.elapsed() > Duration::from_secs(3600) {
                break None;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    };
    asks.open.lock().unwrap().remove(&id);
    match answers {
        Some(a) => (true, tools::ask_user_result(&questions, &a)),
        None => (true, tools::ASK_USER_DECLINED.to_string()),
    }
}

pub fn run(opts: Opts, rx: Receiver<In>, me: Sender<In>, events: Events) {
    let mut opts = opts;
    opts.model = split_effort(&opts.model).0.to_string();
    let asks = Arc::new(Asks::default());
    if expose_ask_user(&mut opts.mcp) {
        if !opts.bg_owner.is_empty() && route_ask_user(&mut opts.mcp) {
            let (sink, asks) = (events.clone(), asks.clone());
            crate::harness::bg::set_asker(&opts.bg_owner, Arc::new(move |args| ask_user(&sink, &asks, args)));
        }
        let system = opts.system.take().filter(|s| !s.trim().is_empty());
        opts.system = Some(match system {
            Some(s) => format!("{s}\n\n{ASK_USER_NOTE}"),
            None => ASK_USER_NOTE.to_string(),
        });
    }
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
    let settings_for = |o: &Opts, has_mcp: bool| {
        let empty = serde_json::Map::new();
        let servers = if has_mcp { &o.mcp } else { &empty };
        settings_arg(o.mode, &o.cwd, servers, &o.allowed_commands, user_settings().as_ref())
    };
    let settings = settings_for(&opts, mcp_dir.is_some());
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
        subagents: HashMap::new(),
        restart: false,
        usage: Usage { window: window(&opts.model), ..Default::default() },
        step_usage: false,
        turn: TurnTokens::default(),
        mode: opts.mode,
        bg_owner: opts.bg_owner.clone(),
        asks,
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
            None if b.has_running_subagents() => match rx.recv_timeout(Duration::from_millis(250)) {
                Ok(m) => m,
                Err(RecvTimeoutError::Timeout) => {
                    b.poll_subagents();
                    continue;
                }
                Err(RecvTimeoutError::Disconnected) => {
                    b.finish_subagents("stopped");
                    kill_tree(&mut proc);
                    return;
                }
            },
            None => match super::recv(&rx) {
                Some(m) => m,
                None => {
                    b.finish_subagents("stopped");
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
                b.finish_subagents("stopped");
                let reason = proc.exit_reason();
                let _ = b.events.send(Ev::Exited(format!("Antigravity stopped: {reason}")));
                return;
            }
            In::Cmd(cmd) => b.on_cmd(cmd, &mut proc, &mut opts),
        };
        match next {
            Next::Go => {}
            Next::Stop => {
                b.finish_subagents("stopped");
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
                b.finish_subagents("stopped");
                if b.busy {
                    b.end_turn("Interrupted.");
                    b.emit(Ev::Notice("Interrupted.".into()));
                }
                b.restart = false;
                opts.resume = b.session.clone();
                let settings = settings_for(&opts, mcp_dir.is_some());
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

    fn test_bridge(tx: Sender<Ev>, model: &str) -> Bridge {
        Bridge {
            events: tx, session: None, model: model.into(), system: None, busy: true, queued: 0,
            calls: HashMap::new(), subagents: HashMap::new(), restart: false, usage: Usage::default(), step_usage: false,
            turn: TurnTokens::default(), mode: Mode::Ask, bg_owner: String::new(), asks: Arc::default(),
        }
    }

    #[test]
    fn tool_steps_become_cards() {
        let (tx, rx) = std::sync::mpsc::channel();
        let mut b = test_bridge(tx, "");
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
    fn ask_user_is_unhidden_for_antigravity() {
        let mut servers = serde_json::Map::new();
        servers.insert("winfer".into(), json!({ "env": { "WINFER_MCP_HIDE": "ask_user,kb_save" } }));
        assert!(expose_ask_user(&mut servers));
        assert_eq!(servers["winfer"]["env"]["WINFER_MCP_HIDE"], "kb_save");
        servers.insert("winfer".into(), json!({ "env": { "WINFER_MCP_HIDE": "ask_user" } }));
        assert!(expose_ask_user(&mut servers));
        assert!(servers["winfer"]["env"].get("WINFER_MCP_HIDE").is_none());
        assert!(!expose_ask_user(&mut serde_json::Map::new()));
    }

    #[test]
    fn ask_user_goes_to_the_picker() {
        let mut servers = serde_json::Map::new();
        servers.insert("winfer".into(), json!({ "env": {} }));
        assert!(!route_ask_user(&mut servers));
        servers.insert("winfer".into(), json!({ "env": { "WINFER_CODE_BG": "127.0.0.1:1 t o" } }));
        assert!(route_ask_user(&mut servers));
        assert_eq!(servers["winfer"]["env"]["WINFER_CODE_ASK"], "1");

        let (tx, rx) = std::sync::mpsc::channel();
        let asks = Arc::new(Asks::default());
        let answerer = {
            let asks = asks.clone();
            std::thread::spawn(move || {
                let Ev::Approval(a) = rx.recv().unwrap() else { panic!("expected an approval") };
                assert_eq!(a.tool, "Question");
                asks.answers.lock().unwrap().insert(a.id, vec!["Blue".into()]);
            })
        };
        let args = json!({ "questions": [{ "question": "Color?", "header": "Color", "options": [{ "label": "Red", "description": "" }, { "label": "Blue", "description": "" }], "multiSelect": false }] });
        let (ok, text) = ask_user(&tx, &asks, &args);
        answerer.join().unwrap();
        assert!(ok && text.contains("Blue"), "{text}");
        assert!(asks.open.lock().unwrap().is_empty());
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
        let settings = settings_arg(Mode::Ask, Path::new("/repo"), &servers, &[], Some(&existing)).unwrap();
        let parsed: Value = serde_json::from_str(&settings).unwrap();
        assert_eq!(parsed["permissions"]["allow"], json!(["command(git)", "mcp(winfer/*)"]));
        assert_eq!(parsed["permissions"]["deny"], json!(["command(rm)"]));

        let path = dir.0.clone();
        drop(dir);
        assert!(!path.exists());
    }

    #[test]
    fn plan_and_yolo_plan_settings_allow_exploration_and_deny_workspace_writes() {
        let empty = serde_json::Map::new();
        let existing = json!({ "permissions": { "allow": ["command(custom)"], "deny": ["command(rm -rf)"] } });
        let cwd = Path::new("/tmp/winfer-plan-unit-nonexistent");

        let plan: Value = serde_json::from_str(
            &settings_arg(Mode::Plan, cwd, &empty, &["cargo test".into()], Some(&existing)).unwrap(),
        ).unwrap();
        let allow: Vec<&str> = plan["permissions"]["allow"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
        let deny: Vec<&str> = plan["permissions"]["deny"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
        assert!(allow.contains(&"command(custom)"));
        assert!(allow.contains(&"read_file(*)"));
        assert!(allow.contains(&"command(ls)"));
        assert!(allow.contains(&"command(find)"));
        assert!(allow.contains(&"command(rg)"));
        assert!(allow.contains(&"command(git status)"));
        assert!(allow.contains(&"command(cargo test)"));
        assert!(!allow.contains(&"command(*)"));
        assert!(deny.contains(&"command(rm -rf)"));
        assert!(deny.contains(&"write_file(/tmp/winfer-plan-unit-nonexistent)"));

        let yolo_plan: Value = serde_json::from_str(
            &settings_arg(Mode::YoloPlan, cwd, &empty, &[], None).unwrap(),
        ).unwrap();
        let yp_allow: Vec<&str> = yolo_plan["permissions"]["allow"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
        let yp_deny: Vec<&str> = yolo_plan["permissions"]["deny"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
        assert!(yp_allow.contains(&"command(*)"));
        assert!(yp_allow.contains(&"read_file(*)"));
        assert!(yp_allow.contains(&"read_url(*)"));
        assert!(yp_deny.contains(&"write_file(/tmp/winfer-plan-unit-nonexistent)"));

        assert!(settings_arg(Mode::Yolo, cwd, &empty, &[], None).is_none());
        assert!(settings_arg(Mode::Ask, cwd, &empty, &[], None).is_none());
    }

    #[test]
    fn subagent_steps_emit_agent_cards_and_track_transcript_progress() {
        let dir = std::env::temp_dir().join(format!("winfer-agy-sub-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("transcript.jsonl");
        let log_uri = format!("file://{}", log.to_string_lossy().replace(' ', "%20"));

        let (tx, rx) = std::sync::mpsc::channel();
        let mut b = test_bridge(tx, "gemini-4-neon");
        let active = json!({
            "event": "step_update",
            "step_update": {
                "step_index": 12,
                "state": "ACTIVE",
                "step_type": "subagent",
                "tool_name": "invoke_subagent",
                "subagent_info": {
                    "subagents": [{
                        "type_name": "research",
                        "role": "Frontend Explorer",
                        "initial_prompt": "Explore src/ for markdown rendering."
                    }]
                }
            }
        });
        let done = json!({
            "event": "step_update",
            "step_update": {
                "step_index": 12,
                "state": "DONE",
                "step_type": "subagent",
                "tool_name": "invoke_subagent",
                "subagent_info": {
                    "subagents": [{
                        "type_name": "research",
                        "role": "Frontend Explorer",
                        "initial_prompt": "Explore src/ for markdown rendering.",
                        "conversation_id": "sub-123",
                        "log_uri": log_uri
                    }]
                }
            }
        });
        b.on_line(&active);
        b.on_line(&done);
        assert!(b.has_running_subagents());

        let initial: Vec<Ev> = rx.try_iter().collect();
        assert!(matches!(&initial[0], Ev::ToolStart { id, name, title, .. } if id == "s12" && name == "Agent" && title == "Frontend Explorer"));
        assert!(matches!(&initial[1], Ev::Task(t) if t.id == "sub-123" && t.tool_id.as_deref() == Some("s12") && t.agent.as_deref() == Some("research") && t.status.as_deref() == Some("running")));
        assert!(matches!(&initial[2], Ev::ToolEnd { id, ok: true, .. } if id == "s12"));

        // Subagent runs a tool call and writes to its transcript.
        std::fs::write(
            &log,
            concat!(
                r#"{"step_index":1,"type":"PLANNER_RESPONSE","status":"DONE","model":"gemini-4-argon-low","tool_calls":[{"name":"read_file","args":{"Path":"\"/repo/package.json\""}}]}"#,
                "\n",
            ),
        ).unwrap();
        b.poll_subagents();
        let prog: Vec<Ev> = rx.try_iter().collect();
        assert!(matches!(&prog[0], Ev::Task(t) if t.id == "sub-123" && t.tools == Some(1) && t.last.as_deref() == Some("Read /repo/package.json") && t.model.as_deref() == Some("gemini-4-argon") && t.status.is_none()), "{prog:?}");

        // Subagent reports back and finishes its turn.
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new().append(true).open(&log).unwrap();
        writeln!(f, r#"{{"step_index":2,"type":"PLANNER_RESPONSE","status":"DONE","tool_calls":[{{"name":"send_message","args":{{"Message":"\"done\""}}}}]}}"#).unwrap();
        writeln!(f, r#"{{"step_index":3,"type":"PLANNER_RESPONSE","status":"DONE","content":"Reported back."}}"#).unwrap();
        drop(f);
        b.poll_subagents();
        assert!(!b.has_running_subagents());
        let done_evs: Vec<Ev> = rx.try_iter().collect();
        assert!(matches!(&done_evs[0], Ev::Task(t) if t.id == "sub-123" && t.status.as_deref() == Some("completed")), "{done_evs:?}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn subagent_permission_denial_marks_task_failed() {
        let dir = std::env::temp_dir().join(format!("winfer-agy-sub-err-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("transcript.jsonl");
        let log_uri = format!("file://{}", log.to_string_lossy());

        let (tx, rx) = std::sync::mpsc::channel();
        let mut b = test_bridge(tx, "gemini-4-neon");
        b.on_line(&json!({
            "event": "step_update",
            "step_update": {
                "step_index": 5,
                "state": "DONE",
                "step_type": "subagent",
                "subagent_info": {
                    "subagents": [{
                        "type_name": "research",
                        "role": "Backend Explorer",
                        "conversation_id": "sub-err",
                        "log_uri": log_uri
                    }]
                }
            }
        }));
        let _ = rx.try_iter().count();

        std::fs::write(
            &log,
            r#"{"step_index":2,"type":"GENERIC","status":"ERROR","error":"permission check failed for unsandboxed \"ls\": user denied permission to run command"}"#.to_string() + "\n",
        ).unwrap();
        b.poll_subagents();
        assert!(!b.has_running_subagents());
        let evs: Vec<Ev> = rx.try_iter().collect();
        assert!(matches!(&evs[0], Ev::Task(t) if t.id == "sub-err" && t.status.as_deref() == Some("failed") && t.last.as_deref() == Some("Permission denied")), "{evs:?}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn file_uri_decodes_spaces() {
        assert_eq!(
            file_uri_to_path("file:///Volumes/(DGD)%20Dane%20Game%20Drive/transcript.jsonl"),
            Some(PathBuf::from("/Volumes/(DGD) Dane Game Drive/transcript.jsonl")),
        );
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
        let mut b = test_bridge(tx, "gemini-4-neon");
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

    /// agy's `result` usage is the sum of the turn's steps (seen from agy 2026-11): not counted twice.
    #[test]
    fn result_usage_does_not_recount_steps() {
        let (tx, rx) = std::sync::mpsc::channel();
        let mut b = test_bridge(tx, "gemini-4-neon");
        let step = |i: u64, input: u64, cached: u64, output: u64| json!({ "event": "step_update", "step_update": {
            "step_index": i, "step_type": "agent_response", "state": "DONE",
            "usage": { "input_tokens": input, "cache_read_tokens": cached, "output_tokens": output } } });
        b.on_line(&step(1, 10_248, 0, 284));
        b.on_line(&step(4, 2_672, 8_191, 67));
        b.on_line(&json!({ "event": "result", "result": { "status": "SUCCESS",
            "usage": { "input_tokens": 12_920, "cache_read_tokens": 8_191, "output_tokens": 351 } } }));
        let Some(Ev::Usage(u)) = rx.try_iter().filter(|e| matches!(e, Ev::Usage(_))).last() else { panic!("expected Usage event") };
        assert_eq!(u.context, 2_672 + 8_191 + 67);
        assert_eq!(u.input, 10_248 + 2_672 + 8_191);
        assert_eq!(u.cached, 8_191);
        assert_eq!(u.output, 351);
        // A turn whose steps reported nothing still counts its result.
        b.on_line(&json!({ "event": "result", "result": { "status": "SUCCESS", "usage": { "input_tokens": 100, "output_tokens": 5 } } }));
        let Some(Ev::Usage(u)) = rx.try_iter().filter(|e| matches!(e, Ev::Usage(_))).last() else { panic!("expected Usage event") };
        assert_eq!(u.context, 105);
    }
}
