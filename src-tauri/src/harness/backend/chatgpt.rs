//! ChatGPT backend: the Responses API with the account from Settings → AI (Sign in with
//! ChatGPT), and winfer's own tools. History is kept locally (`store: false`) and saved per
//! session so it can be resumed.

use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};

use super::{In, Opts};
use crate::harness::event::{Approval, Cmd, Decision, Ev, Events, Mode, ModelInfo, TaskEv, TurnTokens, Usage, UserMsg};
use crate::harness::mcp_client;
use crate::harness::bg;
use crate::harness::tools::{self, Risk};

pub const DEFAULT_MODEL: &str = "gpt-6.1-sol";

pub fn models() -> Vec<ModelInfo> {
    if !crate::chatgpt::signed_in() {
        return Vec::new();
    }
    let mut list = crate::chatgpt::models().unwrap_or_default();
    if list.is_empty() {
        list.push(DEFAULT_MODEL.to_string());
    }
    list.into_iter()
        .map(|m| ModelInfo { spec: format!("chatgpt/{m}"), label: m.clone(), detail: "ChatGPT account · winfer tools".into() })
        .collect()
}

const SYSTEM: &str = "You are winfer code, a terminal coding agent. Explore with grep/glob/read; read before edit; verify with bash. Put every tool call you can already make into one response (e.g. read all relevant files at once); chain shell steps in one bash call. Use todo only for long multi-step work, never just to tick off the last item. Hand broad searches and independent subtasks to agent (several in one response run in parallel; background: true keeps you working meanwhile). If a call is declined, ask. Reply tersely in Markdown; cite path:line.";

/// Steers long-running processes to the `bg` tool when it's on.
const BG_HINT: &str = " Run dev servers, watchers and loops with bg (not bash with &/nohup); wait on them with bg read until (not sleep loops) and stop them when done.";

/// A subagent's instructions, in place of `SYSTEM`.
const SUB_SYSTEM: &str = "You are a winfer code subagent: another agent handed you one task. Work on it alone with your tools (explore with grep/glob/read; read before edit; verify with bash; batch independent calls in one response). You can't ask questions: if something is ambiguous, pick the sensible reading and say so. End with a compact report of what you found or did (paths as path:line, what changed, anything left open); that report is all the other agent sees.";

/// Tool steps a subagent gets before it has to report.
const SUB_STEPS: usize = 100;

/// winfer's tools plus `agent`, which subagents don't get.
pub fn tool_definitions() -> Vec<Value> {
    let mut tools = tools::definitions();
    tools.push(json!({
        "type": "function",
        "name": "agent",
        "description": "Hand a self-contained task to a subagent with a fresh context and the same tools; returns its final report. background: true returns at once and the report arrives in a later message (don't poll).",
        "parameters": {
            "type": "object",
            "properties": {
                "description": { "type": "string", "description": "3-5 word title" },
                "prompt": { "type": "string", "description": "The whole task: goal, context, paths, what to report" },
                "background": { "type": "boolean" },
            },
            "required": ["description", "prompt"],
        },
    }));
    tools
}

/// What a turn shares with the backend thread.
struct State {
    items: Vec<Value>,
    model: String,
    effort: Option<String>,
    mode: Mode,
    allowed: HashSet<String>,
    usage: Usage,
    /// The running turn's output tokens, and the session's output total when it started.
    turn: TurnTokens,
    turn_base: u64,
    /// The session's input total when the running turn started.
    turn_input_base: u64,
    turn_cached_base: u64,
    trim_at: u8,
    compact_at: u8,
    /// Whether the account's route caches prompts (None until seen): without a cache, resending
    /// big old tool output costs full price every request, so it's trimmed as the turn goes on.
    cache: Option<bool>,
    cache_misses: u8,
    /// The user turns, newest last, for /undo.
    undo: Vec<UndoTurn>,
}

/// Where a user turn starts in the conversation, and the files its tools changed.
#[derive(Default)]
struct UndoTurn {
    /// User items before the turn's message (trimming drops reasoning items, never user ones).
    users: usize,
    /// Original contents (None: didn't exist).
    files: HashMap<PathBuf, Option<String>>,
}

struct Shared {
    state: Mutex<State>,
    events: Events,
    stop: Arc<AtomicBool>,
    cwd: PathBuf,
    instructions: String,
    session: String,
    mcp: Mutex<Vec<mcp_client::Server>>,
    /// Starts the MCP servers; joined before the first request so the tool list never changes
    /// mid-session (that would break the prompt cache).
    mcp_start: Mutex<Option<std::thread::JoinHandle<()>>>,
    reads: tools::ReadCache,
    /// Answers to approval requests, by request id, until the asker takes them.
    decisions: Mutex<HashMap<String, Decision>>,
    /// Approval requests still waiting for an answer.
    asking: Mutex<HashSet<String>>,
    /// Stop flags of running subagents, by task id.
    agents: Mutex<HashMap<String, Arc<AtomicBool>>>,
    next_agent: AtomicU64,
    /// Reports of background subagents not yet seen by the main conversation.
    notes: Mutex<Vec<String>>,
    /// Owner of this backend's `bg` tasks (a backend started after it has its own).
    bg_owner: String,
}

/// A subagent's own conversation.
struct Sub {
    id: String,
    title: String,
    items: Mutex<Vec<Value>>,
    stop: Arc<AtomicBool>,
    /// Background agents outlive the turn; foreground ones stop with it.
    background: bool,
    reads: tools::ReadCache,
    instructions: String,
    tools: AtomicU64,
}

/// Whether the work of `sub` (or the main turn) should stop.
fn stopped(shared: &Shared, sub: Option<&Sub>) -> bool {
    match sub {
        Some(s) => s.stop.load(Ordering::Relaxed) || (!s.background && shared.stop.load(Ordering::Relaxed)),
        None => shared.stop.load(Ordering::Relaxed),
    }
}

/// Adds items to the conversation of `sub`, or the main one.
fn push_items(shared: &Shared, sub: Option<&Sub>, items: impl IntoIterator<Item = Value>) {
    match sub {
        Some(s) => s.items.lock().unwrap().extend(items),
        None => shared.state.lock().unwrap().items.extend(items),
    }
}

pub fn run(opts: Opts, rx: Receiver<In>, events: Events) {
    if !crate::chatgpt::signed_in() {
        let _ = events.send(Ev::Exited(
            "Not signed in with ChatGPT. Open winfer → Settings → AI → Sign in with ChatGPT, or pick a Codex/Claude model with /model.".into(),
        ));
        return;
    }
    let session = opts.resume.clone().unwrap_or_else(new_id);
    let items = opts.resume.as_ref().map(|id| load_items(id)).unwrap_or_default();
    let shared = Arc::new(Shared {
        state: Mutex::new(State {
            items,
            model: if opts.model.is_empty() { DEFAULT_MODEL.to_string() } else { opts.model.clone() },
            effort: opts.effort.clone(),
            mode: opts.mode,
            allowed: opts.allowed_commands.iter().map(|c| format!("bash:{c}")).collect(),
            usage: Usage { window: 272_000, ..Default::default() },
            trim_at: opts.trim_at,
            compact_at: opts.compact_at,
            cache: None,
            cache_misses: 0,
            turn: TurnTokens::default(),
            turn_base: 0,
            turn_input_base: 0,
            turn_cached_base: 0,
            undo: Vec::new(),
        }),
        events: events.clone(),
        stop: Arc::new(AtomicBool::new(false)),
        cwd: opts.cwd.clone(),
        instructions: instructions(&opts),
        session: session.clone(),
        mcp: Mutex::new(Vec::new()),
        mcp_start: Mutex::new(None),
        reads: Default::default(),
        decisions: Mutex::new(HashMap::new()),
        asking: Mutex::new(HashSet::new()),
        agents: Mutex::new(HashMap::new()),
        next_agent: AtomicU64::new(0),
        notes: Mutex::new(Vec::new()),
        bg_owner: format!("code-{}", new_id()),
    });
    let _ = events.send(Ev::Session(session));
    let _ = events.send(Ev::Model(shared.state.lock().unwrap().model.clone()));
    {
        let me = Arc::downgrade(&shared);
        bg::listen(move |u| {
            if let Some(shared) = me.upgrade() {
                on_bg(&shared, u);
            }
        });
    }

    // MCP servers start in the background while the user types.
    {
        let me = shared.clone();
        let servers = opts.mcp.clone();
        *shared.mcp_start.lock().unwrap() = Some(std::thread::spawn(move || {
            let shared = me;
            for (name, mut spec) in servers {
                if tools::bg_enabled() {
                    hide_mcp_bg(&mut spec);
                }
                match mcp_client::Server::start(&name, &spec, &shared.cwd) {
                    Ok(s) => shared.mcp.lock().unwrap().push(s),
                    Err(e) => {
                        let _ = shared.events.send(Ev::Notice(format!("MCP: {e}")));
                    }
                }
            }
        }));
    }

    let mut worker: Option<std::thread::JoinHandle<()>> = None;
    let mut queue: Vec<Job> = Vec::new();
    let (done_tx, done_rx) = channel::<()>();

    loop {
        // Start the next queued job when idle.
        if worker.as_ref().is_none_or(|w| w.is_finished()) {
            if let Some(w) = worker.take() {
                let _ = w.join();
            }
            // A background subagent finished while nothing ran: let the model see its report.
            if queue.is_empty() && !shared.notes.lock().unwrap().is_empty() {
                queue.push(Job::Notes);
            }
            if !queue.is_empty() {
                let job = queue.remove(0);
                shared.stop.store(false, Ordering::Relaxed);
                let shared = shared.clone();
                let done = done_tx.clone();
                worker = Some(std::thread::spawn(move || {
                    match job {
                        Job::Turn(msg) => turn(&shared, Some(msg)),
                        Job::Notes => turn(&shared, None),
                        Job::Compact => compact(&shared),
                    }
                    let _ = done.send(());
                }));
            }
        }
        let msg = match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(m) => m,
            Err(RecvTimeoutError::Timeout) => {
                while done_rx.try_recv().is_ok() {}
                continue;
            }
            Err(RecvTimeoutError::Disconnected) => break,
        };
        let In::Cmd(cmd) = msg else { continue };
        match cmd {
            Cmd::Send(m) => queue.push(Job::Turn(m)),
            Cmd::Compact => queue.push(Job::Compact),
            Cmd::Interrupt => {
                queue.clear();
                shared.stop.store(true, Ordering::Relaxed);
                // The UI drops open approvals on interrupt, background subagents' too.
                let mut decisions = shared.decisions.lock().unwrap();
                for id in shared.asking.lock().unwrap().iter() {
                    decisions.insert(id.clone(), Decision::Deny);
                }
            }
            Cmd::Approve(id, d) => {
                shared.decisions.lock().unwrap().insert(id, d);
            }
            Cmd::Answer(..) => {}
            Cmd::SetModel(m) => shared.state.lock().unwrap().model = m,
            Cmd::SetEffort(e) => shared.state.lock().unwrap().effort = e,
            Cmd::SetMode(m) => shared.state.lock().unwrap().mode = m,
            Cmd::SetCompaction { trim, compact } => {
                let mut st = shared.state.lock().unwrap();
                st.trim_at = trim;
                st.compact_at = compact;
            }
            Cmd::Undo { files } => {
                if worker.as_ref().is_some_and(|w| !w.is_finished()) || !queue.is_empty() {
                    let _ = shared.events.send(Ev::Undone(Err("Wait for the turn to finish (or press Esc) before /undo.".into())));
                } else {
                    undo(&shared, files);
                }
            }
            Cmd::StopTask(id) => match shared.agents.lock().unwrap().get(&id) {
                Some(stop) => stop.store(true, Ordering::Relaxed),
                None if bg::stop(&id) => {}
                None => {
                    let _ = shared.events.send(Ev::Notice("That task isn't running.".into()));
                }
            },
            Cmd::Shutdown => {
                shared.stop.store(true, Ordering::Relaxed);
                for stop in shared.agents.lock().unwrap().values() {
                    stop.store(true, Ordering::Relaxed);
                }
                break;
            }
        }
    }
    // Background processes end with the backend (winfer code exiting, /model, /new).
    bg::stop_owner(&shared.bg_owner);
}

/// winfer's MCP server also offers `bg`; this backend has its own (with the task list), so the
/// agent gets only that one.
fn hide_mcp_bg(spec: &mut Value) {
    if !spec["args"].as_array().is_some_and(|a| a.iter().any(|x| x == "mcp-stdio")) {
        return;
    }
    let env = spec.as_object_mut().unwrap().entry("env").or_insert_with(|| json!({}));
    let Some(env) = env.as_object_mut() else { return };
    let hidden = env.get(crate::mcp::HIDE_ENV).and_then(Value::as_str).unwrap_or("");
    let list = if hidden.is_empty() { "bg".to_string() } else { format!("{hidden},bg") };
    env.insert(crate::mcp::HIDE_ENV.into(), json!(list));
}

/// A background process started, exited or was stopped: update its task in winfer code's list,
/// and tell the agent when it ended in a way it hasn't seen.
fn on_bg(shared: &Shared, u: bg::Update) {
    if u.owner() != shared.bg_owner {
        return;
    }
    let (task, note) = u.task_event();
    let _ = shared.events.send(Ev::Task(task));
    if let Some(note) = note {
        shared.notes.lock().unwrap().push(note);
    }
}

enum Job {
    Turn(UserMsg),
    /// A turn on background subagents' reports alone.
    Notes,
    Compact,
}

fn new_id() -> String {
    format!("{:016x}", rand::random::<u64>())
}

/// The conversation's items (Responses API input), saved after each turn.
pub(crate) fn session_path(id: &str) -> PathBuf {
    crate::harness::config::dir().join("chatgpt").join(format!("{id}.json"))
}

fn load_items(id: &str) -> Vec<Value> {
    std::fs::read_to_string(session_path(id)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

/// Beside the items: how big the instructions and tools sent with them are, for winfer's
/// context breakdown (`{ system, tools, mcp }` in characters).
pub(crate) fn setup_path(id: &str) -> PathBuf {
    session_path(id).with_extension("setup.json")
}

fn save_setup(id: &str, instructions: &str, tools: &[Value], mcp: Option<&Value>) {
    let size = |v: &Value| v.to_string().len();
    let setup = json!({ "system": instructions.len(), "tools": tools.iter().map(size).sum::<usize>(), "mcp": mcp.map_or(0, size) });
    let path = setup_path(id);
    let _ = std::fs::create_dir_all(path.parent().unwrap());
    let _ = std::fs::write(path, setup.to_string());
}

fn save_items(id: &str, items: &[Value]) {
    let path = session_path(id);
    let _ = std::fs::create_dir_all(path.parent().unwrap());
    if let Ok(text) = serde_json::to_string(items) {
        let _ = std::fs::write(path, text);
    }
}

fn instructions(opts: &Opts) -> String {
    let cwd = opts.cwd.display();
    let branch = std::process::Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(&opts.cwd)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());
    let bg = if tools::bg_enabled() { BG_HINT } else { "" };
    let mut s = format!(
        "{SYSTEM}{bg}\n\ncwd {cwd} · {} · shell {} · {}{}",
        std::env::consts::OS,
        crate::env::shell_label(),
        today(),
        branch.map(|b| format!(" · git {b}")).unwrap_or_default(),
    );
    let project = crate::harness::config::project_instructions(&opts.cwd);
    if !project.is_empty() {
        s.push_str(&format!("\n\n{project}"));
    }
    if let Some(extra) = &opts.system {
        s.push_str(&format!("\n\n{extra}"));
    }
    s
}

pub(crate) fn today() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) as i64;
    let days = secs / 86_400;
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    format!("{y:04}-{m:02}-{d:02}")
}

fn user_item(msg: &UserMsg) -> Value {
    let mut content = vec![json!({ "type": "input_text", "text": msg.text })];
    for path in &msg.images {
        if let Some((media, data)) = tools::image_base64(path) {
            content.push(json!({ "type": "input_image", "image_url": format!("data:{media};base64,{data}") }));
        }
    }
    json!({ "role": "user", "content": content })
}

/// Output items are sent back without server ids (nothing is stored server-side).
fn replayable(mut item: Value) -> Value {
    if let Some(o) = item.as_object_mut() {
        o.remove("id");
        o.remove("status");
    }
    item
}

enum Stream {
    Event(Value),
    Failed(String),
    End,
}

/// One model request for the main conversation, or a subagent's (`sub`): streams the main
/// one's deltas to the UI; returns the output items, or None if stopped.
fn request(shared: &Shared, tools_on: bool, sub: Option<&Sub>) -> Result<Option<Vec<Value>>, String> {
    if let Some(start) = shared.mcp_start.lock().unwrap().take() {
        let _ = start.join();
    }
    let (body, model) = {
        let st = shared.state.lock().unwrap();
        let mut tools = match (tools_on, sub) {
            (false, _) => Vec::new(),
            (true, Some(_)) => tools::definitions(),
            (true, None) => tool_definitions(),
        };
        let meta = if tools_on { mcp_client::meta_tool(&shared.mcp.lock().unwrap()) } else { None };
        if sub.is_none() {
            save_setup(&shared.session, &shared.instructions, &tools, meta.as_ref());
        }
        tools.extend(meta);
        let (instructions, input, cache_key) = match sub {
            // Subagents share one cache key: their instructions and tools are the same.
            Some(s) => (s.instructions.as_str(), json!(*s.items.lock().unwrap()), format!("{}-agents", shared.session)),
            None => (shared.instructions.as_str(), json!(st.items), shared.session.clone()),
        };
        let mut body = json!({
            "model": st.model,
            "instructions": instructions,
            "input": input,
            "store": false,
            "stream": true,
            "include": ["reasoning.encrypted_content"],
            "reasoning": { "summary": "auto" },
            "prompt_cache_key": cache_key,
        });
        if !tools.is_empty() {
            body["tools"] = json!(tools);
            body["tool_choice"] = json!("auto");
            body["parallel_tool_calls"] = json!(true);
        }
        if let Some(e) = &st.effort {
            body["reasoning"]["effort"] = json!(e);
        }
        (body, st.model.clone())
    };
    let _ = model;
    let started = std::time::Instant::now();
    trace(&json!({ "request": { "bytes": body.to_string().len(), "items": body["input"].as_array().map(Vec::len), "prefix_hash": prefix_hash(&body) } }));
    let reader = crate::chatgpt::open_stream(&body)?;
    trace(&json!({ "headers_ms": started.elapsed().as_millis() as u64 }));
    let (tx, rx) = channel::<Stream>();
    std::thread::spawn(move || {
        for line in BufReader::new(reader).lines() {
            let Ok(line) = line else {
                let _ = tx.send(Stream::Failed("the connection to OpenAI dropped".into()));
                return;
            };
            let Some(data) = line.strip_prefix("data:") else { continue };
            if let Ok(v) = serde_json::from_str::<Value>(data.trim()) {
                if tx.send(Stream::Event(v)).is_err() {
                    return;
                }
            }
        }
        let _ = tx.send(Stream::End);
    });
    let mut out = Vec::new();
    let mut partial: HashMap<String, String> = HashMap::new();
    loop {
        if stopped(shared, sub) {
            // Keep what was said so far.
            for (_, text) in partial {
                if !text.is_empty() {
                    out.push(json!({ "role": "assistant", "content": [{ "type": "output_text", "text": text }] }));
                }
            }
            push_items(shared, sub, out);
            return Ok(None);
        }
        let ev = match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(Stream::Event(v)) => v,
            Ok(Stream::Failed(e)) => return Err(e),
            Ok(Stream::End) | Err(RecvTimeoutError::Disconnected) => return Err("OpenAI's answer stopped before it finished.".into()),
            Err(RecvTimeoutError::Timeout) => continue,
        };
        let emit = |e: Ev| {
            let _ = shared.events.send(e);
        };
        // A subagent's text and reasoning stay out of the transcript.
        let stream = |e: Ev| {
            if sub.is_none() {
                emit(e);
            }
        };
        // The turn's output so far, estimated from the text until the response reports its count
        // (a subagent's counts too: its tokens add to the totals).
        let streamed = |d: &str| {
            let mut st = shared.state.lock().unwrap();
            if st.turn.streamed(d) {
                st.usage.turn = st.turn.now();
                emit(Ev::Usage(st.usage.clone()));
            }
        };
        let id = ev["item_id"].as_str().unwrap_or("").to_string();
        match ev["type"].as_str().unwrap_or("") {
            "response.output_text.delta" => {
                let d = ev["delta"].as_str().unwrap_or("").to_string();
                partial.entry(id.clone()).or_default().push_str(&d);
                streamed(&d);
                stream(Ev::TextDelta { id, text: d });
            }
            "response.reasoning_summary_text.delta" => {
                let d = ev["delta"].as_str().unwrap_or("");
                streamed(d);
                stream(Ev::ReasoningDelta { id, text: d.into() });
            }
            "response.reasoning_summary_part.added" if ev["summary_index"].as_u64().unwrap_or(0) > 0 => {
                stream(Ev::ReasoningDelta { id, text: "\n\n".into() })
            }
            "response.output_item.done" => {
                let item = &ev["item"];
                if item["type"] == "message" {
                    partial.remove(item["id"].as_str().unwrap_or(""));
                }
                out.push(replayable(item.clone()));
            }
            "response.completed" | "response.incomplete" => {
                let r = &ev["response"];
                let u = &r["usage"];
                trace(&json!({ "done_ms": started.elapsed().as_millis() as u64, "usage": u }));
                let input = u["input_tokens"].as_u64().unwrap_or(0);
                let output = u["output_tokens"].as_u64().unwrap_or(0);
                let usage = {
                    let mut st = shared.state.lock().unwrap();
                    // A subagent's tokens count toward the totals, not the main context.
                    if sub.is_none() {
                        st.usage.context = input + output;
                    }
                    st.usage.input += input;
                    st.usage.output += output;
                    let cached = u["input_tokens_details"]["cached_tokens"].as_u64().unwrap_or(0);
                    st.usage.cached += cached;
                    let turn = st.usage.output.saturating_sub(st.turn_base);
                    st.turn.exact(turn);
                    st.usage.turn = turn;
                    st.usage.turn_input = st.usage.input.saturating_sub(st.turn_input_base);
                    st.usage.turn_cached = st.usage.cached.saturating_sub(st.turn_cached_base);
                    if sub.is_none() {
                        if cached > 0 {
                            st.cache = Some(true);
                        } else if input >= 2048 && st.cache.is_none() {
                            st.cache_misses += 1;
                            if st.cache_misses >= 2 {
                                st.cache = Some(false);
                            }
                        }
                    }
                    st.usage.clone()
                };
                emit(Ev::Usage(usage));
                if ev["type"] == "response.incomplete" && sub.is_none() {
                    let why = r["incomplete_details"]["reason"].as_str().unwrap_or("unknown");
                    emit(Ev::Notice(format!("The answer was cut short ({why}).")));
                }
                return Ok(Some(out));
            }
            "response.failed" | "error" => {
                let error = if ev["response"]["error"].is_object() { &ev["response"]["error"] } else { &ev["error"] };
                return Err(crate::chatgpt::api_error(200, &json!({ "error": error })).1);
            }
            _ => {}
        }
    }
}

fn turn(shared: &Arc<Shared>, msg: Option<UserMsg>) {
    let emit = |e: Ev| {
        let _ = shared.events.send(e);
    };
    emit(Ev::TurnStart);
    {
        let mut st = shared.state.lock().unwrap();
        st.turn = TurnTokens::default();
        st.turn_base = st.usage.output;
        st.turn_input_base = st.usage.input;
        st.turn_cached_base = st.usage.cached;
    }
    // Compact first when the context is nearly full.
    let full = {
        let st = shared.state.lock().unwrap();
        over(&st.usage, st.compact_at)
    };
    if full {
        emit(Ev::Notice("Context nearly full; compacting…".into()));
        summarize(shared);
    } else {
        let mut st = shared.state.lock().unwrap();
        if over(&st.usage, st.trim_at) {
            let saved = trim_old(&mut st.items, 2);
            if saved > 0 {
                st.usage.context = st.usage.context.saturating_sub(saved as u64 / 4);
                // Trimmed file contents are gone from the model's view: let it read them again.
                shared.reads.lock().unwrap().clear();
            }
        }
    }
    {
        let mut st = shared.state.lock().unwrap();
        if let Some(msg) = &msg {
            let users = st.items.iter().filter(|i| i["role"] == "user").count();
            st.undo.push(UndoTurn { users, files: HashMap::new() });
            st.items.push(user_item(msg));
        }
    }
    let mut steps = 0;
    loop {
        steps += 1;
        // Reports of background subagents that finished meanwhile.
        let notes = std::mem::take(&mut *shared.notes.lock().unwrap());
        if !notes.is_empty() {
            shared.state.lock().unwrap().items.push(json!({ "role": "user", "content": [{ "type": "input_text", "text": notes.join("\n\n") }] }));
        } else if msg.is_none() && steps == 1 {
            break;
        }
        if steps > 1 {
            let mut st = shared.state.lock().unwrap();
            if st.cache == Some(false) && trim_stale(&mut st.items, KEEP_RECENT_OUTPUTS) > 0 {
                shared.reads.lock().unwrap().clear();
            }
        }
        let out = match request(shared, true, None) {
            Ok(Some(out)) => out,
            Ok(None) => break,
            Err(e) => {
                emit(Ev::Error(e));
                break;
            }
        };
        let calls: Vec<Value> = out.iter().filter(|i| i["type"] == "function_call").cloned().collect();
        shared.state.lock().unwrap().items.extend(out);
        if calls.is_empty() || steps > 200 {
            break;
        }
        run_calls(shared, &calls, None);
        if shared.stop.load(Ordering::Relaxed) {
            break;
        }
    }
    {
        let st = shared.state.lock().unwrap();
        save_items(&shared.session, &st.items);
    }
    emit(Ev::TurnDone);
}

/// Runs a response's tool calls for the main conversation or `sub`, and adds their outputs to
/// it. Read-only calls and subagents next to each other run in parallel; the rest one at a time.
fn run_calls(shared: &Arc<Shared>, calls: &[Value], sub: Option<&Sub>) {
    let mut i = 0;
    while i < calls.len() {
        let mut j = i + 1;
        if parallel(&calls[i]) {
            while j < calls.len() && parallel(&calls[j]) {
                j += 1;
            }
        }
        let outputs: Vec<String> = if j - i > 1 {
            std::thread::scope(|scope| {
                let handles: Vec<_> = calls[i..j].iter().map(|c| scope.spawn(|| run_call(shared, c, sub))).collect();
                handles.into_iter().map(|h| h.join().unwrap_or_else(|_| "tool crashed".into())).collect()
            })
        } else if stopped(shared, sub) {
            vec!["Interrupted by the user before this ran.".to_string()]
        } else {
            vec![run_call(shared, &calls[i], sub)]
        };
        let items = calls[i..j].iter().zip(outputs).map(|(call, output)| json!({ "type": "function_call_output", "call_id": call["call_id"], "output": output }));
        push_items(shared, sub, items.collect::<Vec<_>>());
        i = j;
    }
}

/// Runs one tool call (asking first when needed) for the main conversation or `sub`; returns
/// the output the model sees. A subagent's calls show as progress on its task, not as cards.
fn run_call(shared: &Arc<Shared>, call: &Value, sub: Option<&Sub>) -> String {
    let emit = |e: Ev| {
        let _ = shared.events.send(e);
    };
    let card = |e: Ev| {
        if sub.is_none() {
            emit(e);
        }
    };
    let name = call["name"].as_str().unwrap_or("");
    let id = call["call_id"].as_str().unwrap_or("").to_string();
    let args: Value = serde_json::from_str(call["arguments"].as_str().unwrap_or("{}")).unwrap_or(json!({}));
    if name == "agent" && sub.is_none() {
        return run_agent(shared, &id, &args);
    }
    let mcp = (name == "mcp").then(|| (args["server"].as_str().unwrap_or("").to_string(), args["tool"].as_str().unwrap_or("").to_string()));
    let (display, title) = match &mcp {
        Some((server, tool)) if tool.is_empty() => (format!("{server} · tools"), String::new()),
        Some((server, tool)) => (format!("{server} · {tool}"), tools::title("", &args["args"])),
        None => (display_name(name).to_string(), tools::title(name, &args)),
    };
    match sub {
        Some(s) => {
            let n = s.tools.fetch_add(1, Ordering::Relaxed) + 1;
            emit(Ev::Task(TaskEv { id: s.id.clone(), tools: Some(n), last: Some(display.clone()), ..Default::default() }));
        }
        None => emit(Ev::ToolStart { id: id.clone(), name: display.clone(), title: title.clone(), input: args.clone() }),
    }

    let risk = match &mcp {
        Some((_, tool)) if tool.is_empty() => Risk::Read,
        Some(_) => Risk::Exec,
        None => tools::risk(name, &args),
    };
    let (mode, key) = {
        let st = shared.state.lock().unwrap();
        let key = match name {
            "bash" => format!("bash:{}", args["cmd"].as_str().or(args["command"].as_str()).unwrap_or("").split_whitespace().next().unwrap_or("")),
            // Allowing one program in the background doesn't allow every other one.
            "bg" => format!("bg:{}", args["cmd"].as_str().unwrap_or("").split_whitespace().next().unwrap_or("")),
            "mcp" => format!("mcp:{}:{}", args["server"].as_str().unwrap_or(""), args["tool"].as_str().unwrap_or("")),
            _ => name.to_string(),
        };
        // Plan mode refuses file edits even when they were always-allowed; commands and MCP tools
        // (the browser, panes) ask as usual, or just run in yolo-plan.
        let allowed = st.allowed.contains(&key) && !(st.mode.plans() && risk == Risk::Edit);
        (if allowed { Mode::Yolo } else { st.mode }, key)
    };
    let ask = match (risk, mode) {
        (Risk::Read, _) | (_, Mode::Yolo) => false,
        (Risk::Edit, m) if m.plans() => {
            let msg = "Plan mode is on: files can't be edited. Describe the change instead of making it.".to_string();
            card(Ev::ToolEnd { id, ok: false, output: msg.clone(), diff: None });
            return msg;
        }
        (Risk::Edit, Mode::AutoEdit) | (_, Mode::YoloPlan) => false,
        _ => true,
    };
    if ask {
        let approval_id = format!("a-{id}");
        emit(Ev::Approval(Approval {
            id: approval_id.clone(),
            tool: display,
            title,
            detail: sub.map(|s| format!("Asked by subagent “{}”.", s.title)).unwrap_or_default(),
            diff: tools::preview(&shared.cwd, name, &args),
            ..Default::default()
        }));
        shared.asking.lock().unwrap().insert(approval_id.clone());
        let decision = loop {
            if stopped(shared, sub) {
                break Decision::Deny;
            }
            if let Some(d) = shared.decisions.lock().unwrap().remove(&approval_id) {
                break d;
            }
            std::thread::sleep(Duration::from_millis(50));
        };
        shared.asking.lock().unwrap().remove(&approval_id);
        match decision {
            Decision::Deny => {
                let msg = "The user declined this tool call. Don't retry it; ask how to proceed.".to_string();
                card(Ev::ToolEnd { id, ok: false, output: "Declined".into(), diff: None });
                return msg;
            }
            Decision::Always => {
                shared.state.lock().unwrap().allowed.insert(key);
            }
            Decision::Allow => {}
        }
    }

    if let Some((server, tool)) = mcp {
        let mut servers = shared.mcp.lock().unwrap();
        let (ok, text) = match servers.iter_mut().find(|s| s.name == server) {
            Some(s) if tool.is_empty() => (true, s.schemas()),
            Some(s) => s.call_tool(&tool, args["args"].clone()),
            None => (false, format!("no MCP server {server}")),
        };
        let text = tools::clip(&text, 10_000);
        card(Ev::ToolEnd { id, ok, output: text.clone(), diff: None });
        return text;
    }

    let events = shared.events.clone();
    let out_id = id.clone();
    let live = sub.is_none();
    let output = move |chunk: &str| {
        if live {
            let _ = events.send(Ev::ToolOutput { id: out_id.clone(), chunk: chunk.to_string() });
        }
    };
    let mut snapshots = HashMap::new();
    let outcome = {
        let stop = sub.map_or_else(|| shared.stop.clone(), |s| s.stop.clone());
        let reads = sub.map_or(&shared.reads, |s| &s.reads);
        let mut ctx = tools::Ctx { cwd: &shared.cwd, stop, output: &output, call_id: &id, bg_owner: &shared.bg_owner, snapshots: &mut snapshots, reads };
        tools::run(name, &args, &mut ctx)
    };
    if !snapshots.is_empty() {
        let mut st = shared.state.lock().unwrap();
        if let Some(turn) = st.undo.last_mut() {
            for (k, v) in snapshots {
                turn.files.entry(k).or_insert(v);
            }
        }
    }
    // A subagent's task list is its own business.
    if let Some(todos) = outcome.todos.clone().filter(|_| sub.is_none()) {
        emit(Ev::Todos(todos));
    }
    card(Ev::ToolEnd { id, ok: outcome.ok, output: outcome.output.clone(), diff: outcome.diff.clone() });
    outcome.output
}

/// The `agent` tool: a subagent with its own conversation works on the task. In the foreground
/// the call returns its report; in the background the call returns at once and the report
/// reaches the main conversation when it's done (waking it if idle).
fn run_agent(shared: &Arc<Shared>, call_id: &str, args: &Value) -> String {
    let emit = |e: Ev| {
        let _ = shared.events.send(e);
    };
    let title = args["description"].as_str().map(str::trim).filter(|t| !t.is_empty()).unwrap_or("subagent").to_string();
    let prompt = args["prompt"].as_str().unwrap_or("").trim().to_string();
    let background = args["background"].as_bool() == Some(true);
    emit(Ev::ToolStart { id: call_id.to_string(), name: "Agent".into(), title: title.clone(), input: args.clone() });
    if prompt.is_empty() {
        let msg = "agent needs a prompt: the whole task for the subagent.".to_string();
        emit(Ev::ToolEnd { id: call_id.to_string(), ok: false, output: msg.clone(), diff: None });
        return msg;
    }
    let id = format!("agent-{}", shared.next_agent.fetch_add(1, Ordering::Relaxed) + 1);
    let stop = Arc::new(AtomicBool::new(false));
    shared.agents.lock().unwrap().insert(id.clone(), stop.clone());
    let sub = Sub {
        id: id.clone(),
        title: title.clone(),
        items: Mutex::new(vec![json!({ "role": "user", "content": [{ "type": "input_text", "text": prompt }] })]),
        stop,
        background,
        reads: Default::default(),
        instructions: format!("{SUB_SYSTEM}{}", shared.instructions.strip_prefix(SYSTEM).unwrap_or("")),
        tools: AtomicU64::new(0),
    };
    emit(Ev::Task(TaskEv {
        id: id.clone(),
        tool_id: Some(call_id.to_string()),
        title: Some(title.clone()),
        agent: Some("general".into()),
        status: Some("running".into()),
        ..Default::default()
    }));
    if background {
        let shared = shared.clone();
        std::thread::spawn(move || {
            let note = match agent_run(&shared, &sub) {
                Ok(report) => format!("Background subagent {} (“{}”) finished. Its report:\n\n{report}", sub.id, sub.title),
                Err(None) => format!("Background subagent {} (“{}”) was stopped by the user before it finished.", sub.id, sub.title),
                Err(Some(e)) => format!("Background subagent {} (“{}”) failed: {e}", sub.id, sub.title),
            };
            shared.notes.lock().unwrap().push(note);
        });
        emit(Ev::ToolEnd { id: call_id.to_string(), ok: true, output: format!("Running in the background as {id}."), diff: None });
        return format!("Started subagent {id} in the background. Its report arrives in a later message when it's done; don't wait or poll for it.");
    }
    let (ok, report) = match agent_run(shared, &sub) {
        Ok(report) => (true, report),
        Err(None) => (false, "The user stopped this subagent before it finished.".to_string()),
        Err(Some(e)) => (false, format!("The subagent failed: {e}")),
    };
    emit(Ev::ToolEnd { id: call_id.to_string(), ok, output: report.clone(), diff: None });
    report
}

/// Runs a subagent to its report; `Err(None)` when it was stopped. Ends its task either way.
fn agent_run(shared: &Arc<Shared>, sub: &Sub) -> Result<String, Option<String>> {
    let result = agent_loop(shared, sub);
    shared.agents.lock().unwrap().remove(&sub.id);
    let status = match &result {
        Ok(_) => "completed",
        Err(None) => "stopped",
        Err(Some(_)) => "failed",
    };
    let _ = shared.events.send(Ev::Task(TaskEv { id: sub.id.clone(), status: Some(status.into()), ..Default::default() }));
    result
}

fn agent_loop(shared: &Arc<Shared>, sub: &Sub) -> Result<String, Option<String>> {
    for step in 0..=SUB_STEPS {
        if step == SUB_STEPS {
            sub.items.lock().unwrap().push(json!({ "role": "user", "content": [{ "type": "input_text", "text": "You're out of tool steps. Report now: what you found or did, and what's left." }] }));
        } else if step > 0 && shared.state.lock().unwrap().cache == Some(false) && trim_stale(&mut sub.items.lock().unwrap(), KEEP_RECENT_OUTPUTS) > 0 {
            sub.reads.lock().unwrap().clear();
        }
        let out = match request(shared, step < SUB_STEPS, Some(sub)) {
            Ok(Some(out)) => out,
            Ok(None) => return Err(None),
            Err(e) => return Err(Some(e)),
        };
        let calls: Vec<Value> = out.iter().filter(|i| i["type"] == "function_call").cloned().collect();
        let text = output_text(&out);
        push_items(shared, Some(sub), out);
        if calls.is_empty() {
            return Ok(if text.trim().is_empty() { "(The subagent finished without a report.)".into() } else { text });
        }
        run_calls(shared, &calls, Some(sub));
        if stopped(shared, Some(sub)) {
            return Err(None);
        }
    }
    Ok("(The subagent ran out of steps before reporting.)".into())
}

/// The text of a response's messages.
fn output_text(items: &[Value]) -> String {
    items
        .iter()
        .filter(|i| i["type"] == "message")
        .flat_map(|i| i["content"].as_array().cloned().unwrap_or_default())
        .filter_map(|c| c["text"].as_str().map(str::to_string))
        .collect::<Vec<_>>()
        .join("\n")
}

fn display_name(name: &str) -> &str {
    match name {
        "read" | "ls" => "Read",
        "write" => "Write",
        "edit" => "Edit",
        "bash" => tools::shell_display(),
        "bg" => "Background",
        "grep" => "Grep",
        "glob" => "Glob",
        "fetch" | "web_fetch" => "Fetch",
        "todo" | "todo_write" => "Todos",
        "agent" => "Agent",
        other => other,
    }
}

/// With `WINFER_CODE_TRACE=<file>`, appends request/usage diagnostics (sizes, timings, the
/// raw usage) as JSON lines, for checking prompt caching and latency.
fn trace(v: &Value) {
    let Ok(path) = std::env::var("WINFER_CODE_TRACE") else { return };
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        use std::io::Write;
        let _ = writeln!(f, "{v}");
    }
}

/// Hash of everything but the newest input item: equal between consecutive requests when the
/// prefix (and so the prompt cache) is stable.
fn prefix_hash(body: &Value) -> String {
    use sha2::{Digest, Sha256};
    let mut b = body.clone();
    if let Some(items) = b["input"].as_array_mut() {
        items.pop();
    }
    let digest = Sha256::digest(b.to_string().as_bytes());
    hex::encode(&digest[..6])
}

/// Whether the context is past `percent` of the window (0 = never).
fn over(usage: &Usage, percent: u8) -> bool {
    percent > 0 && usage.window > 0 && usage.context * 100 > usage.window * percent as u64
}

/// Calls that can run alongside others: read-only tools, subagents and `bg` (its tasks are
/// separate processes, and a start waiting for its marker shouldn't hold up the next one).
fn parallel(call: &Value) -> bool {
    match call["name"].as_str().unwrap_or("") {
        "mcp" => false,
        "agent" | "bg" => true,
        name => {
            let args: Value = serde_json::from_str(call["arguments"].as_str().unwrap_or("{}")).unwrap_or(Value::Null);
            tools::risk(name, &args) == Risk::Read
        }
    }
}

/// Shrinks history older than the last `keep` user turns: long tool outputs and file contents
/// in tool arguments are cut, reasoning is dropped. Returns the bytes saved.
fn trim_old(items: &mut Vec<Value>, keep: usize) -> usize {
    let users: Vec<usize> = items.iter().enumerate().filter(|(_, i)| i["role"] == "user").map(|(n, _)| n).collect();
    let Some(&cut) = users.len().checked_sub(keep).and_then(|k| users.get(k)) else { return 0 };
    trim_before(items, cut, 300)
}

/// Tool outputs kept whole when the route has no prompt cache.
const KEEP_RECENT_OUTPUTS: usize = 6;

/// Without a prompt cache: cuts big tool outputs (over 2 KB) older than the last `keep` ones,
/// so they aren't paid for again on every request. The model can re-run a tool if it needs one.
fn trim_stale(items: &mut Vec<Value>, keep: usize) -> usize {
    let outputs: Vec<usize> = items.iter().enumerate().filter(|(_, i)| i["type"] == "function_call_output").map(|(n, _)| n).collect();
    let Some(&cut) = outputs.len().checked_sub(keep).and_then(|k| outputs.get(k)) else { return 0 };
    trim_before(items, cut, 2000)
}

/// Shrinks items before `cut`: tool outputs and file contents in tool arguments longer than
/// `min` bytes are cut, reasoning is dropped. Returns the bytes saved.
fn trim_before(items: &mut Vec<Value>, cut: usize, min: usize) -> usize {
    let before = serde_json::to_string(&items[..cut]).map(|s| s.len()).unwrap_or(0);
    let mut kept = Vec::with_capacity(items.len());
    for (n, mut item) in std::mem::take(items).into_iter().enumerate() {
        if n < cut {
            match item["type"].as_str() {
                Some("reasoning") => continue,
                Some("function_call_output") => {
                    if let Some(out) = item["output"].as_str().filter(|o| o.len() > min) {
                        let head = &out[..out.floor_char_boundary(200)];
                        item["output"] = json!(format!("{head} …[trimmed; run the tool again if needed]"));
                    }
                }
                Some("function_call") => {
                    if let Ok(mut args) = serde_json::from_str::<Value>(item["arguments"].as_str().unwrap_or("")) {
                        let mut changed = false;
                        for k in ["content", "old", "new", "old_string", "new_string"] {
                            if args[k].as_str().is_some_and(|v| v.len() > min) {
                                args[k] = json!("[trimmed]");
                                changed = true;
                            }
                        }
                        if changed {
                            item["arguments"] = json!(args.to_string());
                        }
                    }
                }
                _ => {}
            }
        }
        kept.push(item);
    }
    *items = kept;
    let after = serde_json::to_string(&items[..items.len().min(cut)]).map(|s| s.len()).unwrap_or(0);
    before.saturating_sub(after)
}

/// Replaces the history with a summary of it.
fn summarize(shared: &Shared) -> bool {
    let saved = {
        let mut st = shared.state.lock().unwrap();
        if st.items.is_empty() {
            return false;
        }
        let saved = st.items.clone();
        st.items.push(json!({ "role": "user", "content": [{ "type": "input_text", "text": "Summarize this conversation so far so that work can continue from the summary alone: the user's goals and requests, decisions made, files changed (with paths), commands that matter, current state, and what's left to do. Be thorough but compact. Reply with the summary only." }] }));
        saved
    };
    let summary = match request(shared, false, None) {
        Ok(Some(out)) => output_text(&out),
        Ok(None) => String::new(),
        Err(e) => {
            let _ = shared.events.send(Ev::Error(format!("Couldn't compact: {e}")));
            String::new()
        }
    };
    let mut st = shared.state.lock().unwrap();
    if summary.trim().is_empty() {
        st.items = saved;
        return false;
    }
    st.items = vec![json!({ "role": "user", "content": [{ "type": "input_text", "text": format!("(Summary of the conversation so far, compacted to save context.)\n\n{summary}") }] })];
    // The turns before are only in the summary now.
    st.undo.clear();
    shared.reads.lock().unwrap().clear();
    st.usage.context = 0;
    true
}

fn compact(shared: &Shared) {
    let _ = shared.events.send(Ev::TurnStart);
    if summarize(shared) {
        let st = shared.state.lock().unwrap();
        save_items(&shared.session, &st.items);
        let _ = shared.events.send(Ev::Usage(st.usage.clone()));
        let _ = shared.events.send(Ev::Notice("Context compacted.".into()));
    }
    let _ = shared.events.send(Ev::TurnDone);
}

/// Drops the last user turn from the conversation, and with `files` puts back the files its
/// tools changed.
fn undo(shared: &Shared, files: bool) {
    let mut st = shared.state.lock().unwrap();
    let Some(turn) = st.undo.pop() else {
        let _ = shared.events.send(Ev::Undone(Err("Nothing to undo (compacted turns can't be undone).".into())));
        return;
    };
    let start = st.items.iter().enumerate().filter(|(_, i)| i["role"] == "user").nth(turn.users).map(|(n, _)| n);
    if let Some(start) = start {
        st.items.truncate(start);
    }
    let mut names = Vec::new();
    if files {
        for (path, content) in &turn.files {
            let _ = match content {
                Some(text) => std::fs::write(path, text),
                None => std::fs::remove_file(path),
            };
            names.push(tools::short_path(&path.to_string_lossy()));
        }
        names.sort();
    }
    // What the model read in that turn is gone from its view.
    shared.reads.lock().unwrap().clear();
    save_items(&shared.session, &st.items);
    let _ = shared.events.send(Ev::Undone(Ok(names)));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn undo_drops_the_last_user_turn_and_its_files() {
        let (sh, rx, dir) = shared(Mode::Yolo);
        let user = |t: &str| json!({ "role": "user", "content": [{ "type": "input_text", "text": t }] });
        {
            let mut st = sh.state.lock().unwrap();
            st.undo = vec![UndoTurn { users: 0, files: HashMap::new() }];
            st.items = vec![user("one"), json!({ "type": "message", "role": "assistant", "content": "ok" })];
            let files = HashMap::from([(dir.join("a.txt"), Some("hello\n".to_string())), (dir.join("new.txt"), None)]);
            st.undo.push(UndoTurn { users: 1, files });
            st.items.extend([
                user("two"),
                json!({ "type": "function_call", "call_id": "1", "name": "write", "arguments": "{}" }),
                json!({ "type": "function_call_output", "call_id": "1", "output": "ok" }),
                user("(a background agent's report)"),
                json!({ "type": "message", "role": "assistant", "content": "done" }),
            ]);
        }
        std::fs::write(dir.join("a.txt"), "changed\n").unwrap();
        std::fs::write(dir.join("new.txt"), "x\n").unwrap();
        undo(&sh, true);
        assert_eq!(sh.state.lock().unwrap().items.len(), 2);
        assert_eq!(std::fs::read_to_string(dir.join("a.txt")).unwrap(), "hello\n");
        assert!(!dir.join("new.txt").exists());
        assert!(matches!(rx.try_recv(), Ok(Ev::Undone(Ok(f))) if f.len() == 2));
        // Files are left alone when the TUI restores them itself.
        std::fs::write(dir.join("a.txt"), "again\n").unwrap();
        undo(&sh, false);
        assert!(sh.state.lock().unwrap().items.is_empty());
        assert_eq!(std::fs::read_to_string(dir.join("a.txt")).unwrap(), "again\n");
        assert!(matches!(rx.try_recv(), Ok(Ev::Undone(Ok(f))) if f.is_empty()));
        undo(&sh, false);
        assert!(matches!(rx.try_recv(), Ok(Ev::Undone(Err(_)))));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn items_replay_without_ids() {
        let v = replayable(json!({ "type": "function_call", "id": "fc_1", "status": "completed", "call_id": "c", "name": "ls", "arguments": "{}" }));
        assert!(v.get("id").is_none() && v.get("status").is_none());
        assert_eq!(v["call_id"], "c");
    }

    #[test]
    fn trims_only_old_turns() {
        let big = "x".repeat(1000);
        let mut items = vec![
            json!({ "role": "user", "content": "one" }),
            json!({ "type": "reasoning", "encrypted_content": "e" }),
            json!({ "type": "function_call", "call_id": "1", "name": "write", "arguments": json!({ "path": "a", "content": big }).to_string() }),
            json!({ "type": "function_call_output", "call_id": "1", "output": big }),
            json!({ "role": "user", "content": "two" }),
            json!({ "role": "user", "content": "three" }),
            json!({ "type": "function_call_output", "call_id": "2", "output": big }),
        ];
        let saved = trim_old(&mut items, 2);
        assert!(saved > 1500);
        assert_eq!(items.len(), 6);
        assert!(items[2]["output"].as_str().unwrap().contains("[trimmed"));
        assert!(items[1]["arguments"].as_str().unwrap().contains("[trimmed]"));
        assert_eq!(items[5]["output"].as_str().unwrap().len(), 1000);
    }

    #[test]
    fn stale_outputs_trim_only_big_old_ones() {
        let big = "y".repeat(5000);
        let mut items = vec![json!({ "role": "user", "content": "go" })];
        for n in 0..8 {
            items.push(json!({ "type": "function_call", "call_id": n.to_string(), "name": "read", "arguments": "{}" }));
            let out = if n % 2 == 0 { big.clone() } else { "small".to_string() };
            items.push(json!({ "type": "function_call_output", "call_id": n.to_string(), "output": out }));
        }
        assert!(trim_stale(&mut items, 6) > 4500);
        let outs: Vec<usize> = items.iter().filter(|i| i["type"] == "function_call_output").map(|i| i["output"].as_str().unwrap().len()).collect();
        assert!(outs[0] < 300 && outs[1] == 5);
        assert_eq!(outs[2], 5000);
        assert_eq!(trim_stale(&mut items, 6), 0);
    }

    fn shared(mode: Mode) -> (Arc<Shared>, Receiver<Ev>, PathBuf) {
        let dir = std::env::temp_dir().join(format!("winfer-agent-test-{}", new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), "hello\n").unwrap();
        let (events, rx) = channel();
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                items: Vec::new(),
                model: DEFAULT_MODEL.into(),
                effort: None,
                mode,
                allowed: HashSet::new(),
                usage: Usage::default(),
                trim_at: 0,
                compact_at: 0,
                cache: None,
                cache_misses: 0,
                turn: TurnTokens::default(),
                turn_base: 0,
            turn_input_base: 0,
            turn_cached_base: 0,
                undo: vec![UndoTurn::default()],
            }),
            events,
            stop: Arc::new(AtomicBool::new(false)),
            cwd: dir.clone(),
            instructions: format!("{SYSTEM}\n\ncwd x"),
            session: "s".into(),
            mcp: Mutex::new(Vec::new()),
            mcp_start: Mutex::new(None),
            reads: Default::default(),
            decisions: Mutex::new(HashMap::new()),
            asking: Mutex::new(HashSet::new()),
            agents: Mutex::new(HashMap::new()),
            next_agent: AtomicU64::new(0),
            notes: Mutex::new(Vec::new()),
            bg_owner: String::new(),
        });
        (shared, rx, dir)
    }

    fn sub(background: bool) -> Sub {
        Sub {
            id: "agent-1".into(),
            title: "Probe".into(),
            items: Mutex::new(Vec::new()),
            stop: Arc::new(AtomicBool::new(false)),
            background,
            reads: Default::default(),
            instructions: SUB_SYSTEM.into(),
            tools: AtomicU64::new(0),
        }
    }

    fn call(id: &str, name: &str, args: Value) -> Value {
        json!({ "type": "function_call", "call_id": id, "name": name, "arguments": args.to_string() })
    }

    #[test]
    fn only_the_main_agent_gets_the_agent_tool() {
        let names = |tools: Vec<Value>| tools.iter().map(|t| t["name"].as_str().unwrap().to_string()).collect::<Vec<_>>();
        assert!(names(tool_definitions()).contains(&"agent".to_string()));
        assert!(!names(tools::definitions()).contains(&"agent".to_string()));
        assert!(parallel(&call("1", "agent", json!({}))) && parallel(&call("2", "read", json!({}))));
        assert!(!parallel(&call("3", "bash", json!({}))) && !parallel(&call("4", "mcp", json!({}))));
    }

    #[test]
    fn background_tasks_become_tasks_and_unseen_exits_notes() {
        let (shared, rx, dir) = shared(Mode::Ask);
        on_bg(&shared, bg::Update::Started { id: "bg7".into(), owner: String::new(), tool_id: "c1".into(), cmd: "npm run dev\nmore".into(), name: String::new(), description: String::new(), log: "/tmp/bg7.log".into() });
        on_bg(&shared, bg::Update::Exited { id: "bg7".into(), owner: String::new(), cmd: "npm run dev".into(), name: String::new(), code: 1, secs: 3, seen: false, tail: "Error: port in use".into() });
        on_bg(&shared, bg::Update::Exited { id: "bg8".into(), owner: String::new(), cmd: "make".into(), name: String::new(), code: 0, secs: 1, seen: true, tail: String::new() });
        on_bg(&shared, bg::Update::Stopped { id: "bg9".into(), owner: String::new(), cmd: "watch".into(), name: "watcher".into(), by_user: true });
        on_bg(&shared, bg::Update::Stopped { id: "bg10".into(), owner: String::new(), cmd: "watch".into(), name: String::new(), by_user: false });
        let tasks: Vec<TaskEv> = rx.try_iter().filter_map(|e| if let Ev::Task(t) = e { Some(t) } else { None }).collect();
        assert_eq!(tasks[0].tool_id.as_deref(), Some("c1"));
        assert_eq!((tasks[0].title.as_deref(), tasks[0].output_file.as_deref(), tasks[0].status.as_deref()), (Some("npm run dev"), Some("/tmp/bg7.log"), Some("running")));
        assert_eq!((tasks[1].status.as_deref(), tasks[1].exit_code), (Some("failed"), Some(1)));
        assert_eq!(tasks[2].status.as_deref(), Some("completed"));
        assert_eq!(tasks[3].status.as_deref(), Some("stopped"));
        let notes = shared.notes.lock().unwrap().clone();
        assert_eq!(notes.len(), 2, "{notes:?}");
        assert!(notes[0].contains("bg7") && notes[0].contains("code 1") && notes[0].contains("port in use"));
        assert!(notes[1].contains("bg9 \"watcher\" (`watch`)"), "{notes:?}");
        assert!(notes[1].contains("user stopped") && notes[1].contains("bg9"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn winfers_mcp_bg_is_hidden_behind_the_own_one() {
        let mut winfer = json!({ "command": "/w", "args": ["mcp-stdio"], "env": { "WINFER_MCP_HIDE": "kb_save" } });
        hide_mcp_bg(&mut winfer);
        assert_eq!(winfer["env"]["WINFER_MCP_HIDE"], "kb_save,bg");
        let mut bare = json!({ "command": "/w", "args": ["mcp-stdio"] });
        hide_mcp_bg(&mut bare);
        assert_eq!(bare["env"]["WINFER_MCP_HIDE"], "bg");
        let mut other = json!({ "command": "codegraph" });
        hide_mcp_bg(&mut other);
        assert!(other.get("env").is_none());
    }

    #[test]
    fn bg_asks_only_to_start_something() {
        let (shared, rx, dir) = shared(Mode::Ask);
        assert!(parallel(&call("1", "bg", json!({ "action": "read", "id": "bg1" }))) && parallel(&call("2", "bg", json!({ "action": "start", "cmd": "x" }))));
        let out = run_call(&shared, &call("r1", "bg", json!({ "action": "list" })), None);
        assert!(!rx.try_iter().any(|e| matches!(e, Ev::Approval(_))), "{out}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn subagent_calls_are_task_progress_not_cards() {
        let (shared, rx, dir) = shared(Mode::Ask);
        let s = sub(false);
        run_calls(&shared, &[call("c1", "read", json!({ "path": "a.txt" })), call("c2", "glob", json!({ "pattern": "*.txt" }))], Some(&s));
        let evs: Vec<Ev> = rx.try_iter().collect();
        assert!(!evs.iter().any(|e| matches!(e, Ev::ToolStart { .. } | Ev::ToolEnd { .. } | Ev::ToolOutput { .. })), "{evs:?}");
        assert!(evs.iter().any(|e| matches!(e, Ev::Task(t) if t.id == "agent-1" && t.tools == Some(2))));
        // The outputs land in the subagent's conversation, not the main one.
        let items = s.items.lock().unwrap();
        assert_eq!(items.len(), 2);
        assert!(items[0]["output"].as_str().unwrap().contains("hello"));
        assert!(shared.state.lock().unwrap().items.is_empty());
        // It reads with its own cache: the main conversation hasn't seen the file.
        assert!(shared.reads.lock().unwrap().is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn subagent_approvals_wait_for_their_own_answer() {
        let (shared, rx, dir) = shared(Mode::Ask);
        let s = sub(true);
        let answer = {
            let shared = shared.clone();
            std::thread::spawn(move || {
                let approval = loop {
                    if let Ok(Ev::Approval(a)) = rx.recv() {
                        break a;
                    }
                };
                assert!(approval.detail.contains("Probe"));
                // Another request's answer doesn't wake it.
                shared.decisions.lock().unwrap().insert("a-other".into(), Decision::Allow);
                shared.decisions.lock().unwrap().insert(approval.id, Decision::Allow);
            })
        };
        let out = run_call(&shared, &call("w1", "write", json!({ "path": "b.txt", "content": "new" })), Some(&s));
        answer.join().unwrap();
        assert!(!out.contains("declined"), "{out}");
        assert_eq!(std::fs::read_to_string(dir.join("b.txt")).unwrap(), "new");
        assert!(shared.decisions.lock().unwrap().contains_key("a-other"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn plan_mode_refuses_edits_but_asks_for_commands() {
        let (shared, rx, dir) = shared(Mode::Plan);
        shared.state.lock().unwrap().allowed.insert("write".into());
        let out = run_call(&shared, &call("w1", "write", json!({ "path": "b.txt", "content": "new" })), None);
        assert!(out.contains("Plan mode") && !dir.join("b.txt").exists(), "{out}");
        let answer = std::thread::spawn({
            let shared = shared.clone();
            move || loop {
                if let Ok(Ev::Approval(a)) = rx.recv() {
                    shared.decisions.lock().unwrap().insert(a.id, Decision::Allow);
                    break;
                }
            }
        });
        let out = run_call(&shared, &call("b1", "bash", json!({ "command": "echo hi" })), None);
        answer.join().unwrap();
        assert!(out.contains("hi") && !out.contains("Plan mode"), "{out}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn yolo_plan_refuses_edits_and_runs_commands_without_asking() {
        let (shared, rx, dir) = shared(Mode::YoloPlan);
        let out = run_call(&shared, &call("w1", "write", json!({ "path": "b.txt", "content": "new" })), None);
        assert!(out.contains("Plan mode") && !dir.join("b.txt").exists(), "{out}");
        let out = run_call(&shared, &call("b1", "bash", json!({ "command": "echo hi" })), None);
        assert!(out.contains("hi"), "{out}");
        assert!(!rx.try_iter().any(|e| matches!(e, Ev::Approval(_))));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn interrupting_stops_foreground_subagents_only() {
        let (shared, _rx, dir) = shared(Mode::Ask);
        shared.stop.store(true, Ordering::Relaxed);
        assert!(stopped(&shared, Some(&sub(false))));
        assert!(!stopped(&shared, Some(&sub(true))));
        let s = sub(true);
        s.stop.store(true, Ordering::Relaxed);
        assert!(stopped(&shared, Some(&s)));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn agent_without_a_prompt_fails_its_card() {
        let (shared, rx, dir) = shared(Mode::Ask);
        let out = run_call(&shared, &call("g1", "agent", json!({ "description": "Look around" })), None);
        assert!(out.contains("needs a prompt"));
        let evs: Vec<Ev> = rx.try_iter().collect();
        assert!(matches!(&evs[0], Ev::ToolStart { name, title, .. } if name == "Agent" && title == "Look around"));
        assert!(matches!(&evs[1], Ev::ToolEnd { ok: false, .. }));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn today_is_a_date() {
        let t = today();
        assert_eq!(t.len(), 10);
        assert!(t.starts_with("20"));
    }
}
