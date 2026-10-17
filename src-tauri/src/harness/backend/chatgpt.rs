//! ChatGPT backend: the Responses API with the account from Settings → AI (Sign in with
//! ChatGPT), and winfer's own tools. History is kept locally (`store: false`) and saved per
//! session so it can be resumed.

use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};

use super::{In, Opts};
use crate::harness::event::{Approval, Cmd, Decision, Ev, Events, Mode, ModelInfo, Usage, UserMsg};
use crate::harness::mcp_client;
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

const SYSTEM: &str = "You are winfer code, a terminal coding agent. Explore with grep/glob/read; read before edit; verify with bash. Put every tool call you can already make into one response (e.g. read all relevant files at once); chain shell steps in one bash call. Use todo only for long multi-step work, never just to tick off the last item. If a call is declined, ask. Reply tersely in Markdown; cite path:line.";

/// What a turn shares with the backend thread.
struct State {
    items: Vec<Value>,
    model: String,
    effort: Option<String>,
    mode: Mode,
    allowed: HashSet<String>,
    usage: Usage,
    trim_at: u8,
    compact_at: u8,
    /// Whether the account's route caches prompts (None until seen): without a cache, resending
    /// big old tool output costs full price every request, so it's trimmed as the turn goes on.
    cache: Option<bool>,
    cache_misses: u8,
    /// File snapshots of each turn, newest last, for /undo.
    undo: Vec<HashMap<PathBuf, Option<String>>>,
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
    });
    let _ = events.send(Ev::Session(session));
    let _ = events.send(Ev::Model(shared.state.lock().unwrap().model.clone()));

    // MCP servers start in the background while the user types.
    {
        let me = shared.clone();
        let servers = opts.mcp.clone();
        *shared.mcp_start.lock().unwrap() = Some(std::thread::spawn(move || {
            let shared = me;
            for (name, spec) in servers {
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
    let (approve_tx, approve_rx) = channel::<(String, Decision)>();
    let approve_rx = Arc::new(Mutex::new(approve_rx));
    let (done_tx, done_rx) = channel::<()>();

    loop {
        // Start the next queued job when idle.
        if worker.as_ref().is_none_or(|w| w.is_finished()) {
            if let Some(w) = worker.take() {
                let _ = w.join();
            }
            if !queue.is_empty() {
                let job = queue.remove(0);
                shared.stop.store(false, Ordering::Relaxed);
                let shared = shared.clone();
                let approvals = approve_rx.clone();
                let done = done_tx.clone();
                worker = Some(std::thread::spawn(move || {
                    match job {
                        Job::Turn(msg) => turn(&shared, msg, &approvals),
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
            }
            Cmd::Approve(id, d) => {
                let _ = approve_tx.send((id, d));
            }
            Cmd::SetModel(m) => shared.state.lock().unwrap().model = m,
            Cmd::SetEffort(e) => shared.state.lock().unwrap().effort = e,
            Cmd::SetMode(m) => shared.state.lock().unwrap().mode = m,
            Cmd::SetCompaction { trim, compact } => {
                let mut st = shared.state.lock().unwrap();
                st.trim_at = trim;
                st.compact_at = compact;
            }
            Cmd::Undo => undo(&shared),
            Cmd::Shutdown => {
                shared.stop.store(true, Ordering::Relaxed);
                break;
            }
        }
    }
}

enum Job {
    Turn(UserMsg),
    Compact,
}

fn new_id() -> String {
    format!("{:016x}", rand::random::<u64>())
}

fn session_path(id: &str) -> PathBuf {
    crate::harness::config::dir().join("chatgpt").join(format!("{id}.json"))
}

fn load_items(id: &str) -> Vec<Value> {
    std::fs::read_to_string(session_path(id)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
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
    let mut s = format!(
        "{SYSTEM}\n\ncwd {cwd} · {} · {}{}",
        std::env::consts::OS,
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

/// One model request: streams deltas to the UI; returns the output items, or None if stopped.
fn request(shared: &Shared, tools_on: bool) -> Result<Option<Vec<Value>>, String> {
    if let Some(start) = shared.mcp_start.lock().unwrap().take() {
        let _ = start.join();
    }
    let (body, model) = {
        let st = shared.state.lock().unwrap();
        let mut tools = if tools_on { tools::definitions() } else { Vec::new() };
        if tools_on {
            if let Some(meta) = mcp_client::meta_tool(&shared.mcp.lock().unwrap()) {
                tools.push(meta);
            }
        }
        let mut body = json!({
            "model": st.model,
            "instructions": shared.instructions,
            "input": st.items,
            "store": false,
            "stream": true,
            "include": ["reasoning.encrypted_content"],
            "reasoning": { "summary": "auto" },
            "prompt_cache_key": shared.session,
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
        if shared.stop.load(Ordering::Relaxed) {
            // Keep what was said so far.
            for (_, text) in partial {
                if !text.is_empty() {
                    out.push(json!({ "role": "assistant", "content": [{ "type": "output_text", "text": text }] }));
                }
            }
            shared.state.lock().unwrap().items.extend(out);
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
        let id = ev["item_id"].as_str().unwrap_or("").to_string();
        match ev["type"].as_str().unwrap_or("") {
            "response.output_text.delta" => {
                let d = ev["delta"].as_str().unwrap_or("").to_string();
                partial.entry(id.clone()).or_default().push_str(&d);
                emit(Ev::TextDelta { id, text: d });
            }
            "response.reasoning_summary_text.delta" => emit(Ev::ReasoningDelta { id, text: ev["delta"].as_str().unwrap_or("").into() }),
            "response.reasoning_summary_part.added" if ev["summary_index"].as_u64().unwrap_or(0) > 0 => {
                emit(Ev::ReasoningDelta { id, text: "\n\n".into() })
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
                    st.usage.context = input + output;
                    st.usage.input += input;
                    st.usage.output += output;
                    let cached = u["input_tokens_details"]["cached_tokens"].as_u64().unwrap_or(0);
                    st.usage.cached += cached;
                    if cached > 0 {
                        st.cache = Some(true);
                    } else if input >= 2048 && st.cache.is_none() {
                        st.cache_misses += 1;
                        if st.cache_misses >= 2 {
                            st.cache = Some(false);
                        }
                    }
                    st.usage.clone()
                };
                emit(Ev::Usage(usage));
                if ev["type"] == "response.incomplete" {
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

fn turn(shared: &Shared, msg: UserMsg, approvals: &Mutex<Receiver<(String, Decision)>>) {
    let emit = |e: Ev| {
        let _ = shared.events.send(e);
    };
    emit(Ev::TurnStart);
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
        st.items.push(user_item(&msg));
        st.undo.push(HashMap::new());
    }
    let mut steps = 0;
    loop {
        steps += 1;
        if steps > 1 {
            let mut st = shared.state.lock().unwrap();
            if st.cache == Some(false) && trim_stale(&mut st.items, KEEP_RECENT_OUTPUTS) > 0 {
                shared.reads.lock().unwrap().clear();
            }
        }
        let out = match request(shared, true) {
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
        // Read-only calls next to each other run in parallel; the rest one at a time.
        let mut i = 0;
        while i < calls.len() {
            let mut j = i + 1;
            if read_only(&calls[i]) {
                while j < calls.len() && read_only(&calls[j]) {
                    j += 1;
                }
            }
            let outputs: Vec<String> = if j - i > 1 {
                std::thread::scope(|scope| {
                    let handles: Vec<_> = calls[i..j].iter().map(|c| scope.spawn(|| run_call(shared, c, approvals))).collect();
                    handles.into_iter().map(|h| h.join().unwrap_or_else(|_| "tool crashed".into())).collect()
                })
            } else if shared.stop.load(Ordering::Relaxed) {
                vec!["Interrupted by the user before this ran.".to_string()]
            } else {
                vec![run_call(shared, &calls[i], approvals)]
            };
            let mut st = shared.state.lock().unwrap();
            for (call, output) in calls[i..j].iter().zip(outputs) {
                st.items.push(json!({ "type": "function_call_output", "call_id": call["call_id"], "output": output }));
            }
            drop(st);
            i = j;
        }
        if shared.stop.load(Ordering::Relaxed) {
            break;
        }
    }
    {
        let mut st = shared.state.lock().unwrap();
        if st.undo.last().is_some_and(HashMap::is_empty) {
            st.undo.pop();
        }
        save_items(&shared.session, &st.items);
    }
    emit(Ev::TurnDone);
}

/// Runs one tool call (asking first when needed); returns the output the model sees.
fn run_call(shared: &Shared, call: &Value, approvals: &Mutex<Receiver<(String, Decision)>>) -> String {
    let emit = |e: Ev| {
        let _ = shared.events.send(e);
    };
    let name = call["name"].as_str().unwrap_or("");
    let id = call["call_id"].as_str().unwrap_or("").to_string();
    let args: Value = serde_json::from_str(call["arguments"].as_str().unwrap_or("{}")).unwrap_or(json!({}));
    let mcp = (name == "mcp").then(|| (args["server"].as_str().unwrap_or("").to_string(), args["tool"].as_str().unwrap_or("").to_string()));
    let (display, title) = match &mcp {
        Some((server, tool)) if tool.is_empty() => (format!("{server} · tools"), String::new()),
        Some((server, tool)) => (format!("{server} · {tool}"), tools::title("", &args["args"])),
        None => (display_name(name).to_string(), tools::title(name, &args)),
    };
    emit(Ev::ToolStart { id: id.clone(), name: display.clone(), title: title.clone(), input: args.clone() });

    let risk = match &mcp {
        Some((_, tool)) if tool.is_empty() => Risk::Read,
        Some(_) => Risk::Exec,
        None => tools::risk(name),
    };
    let (mode, key) = {
        let st = shared.state.lock().unwrap();
        let key = match name {
            "bash" => format!("bash:{}", args["cmd"].as_str().or(args["command"].as_str()).unwrap_or("").split_whitespace().next().unwrap_or("")),
            "mcp" => format!("mcp:{}:{}", args["server"].as_str().unwrap_or(""), args["tool"].as_str().unwrap_or("")),
            _ => name.to_string(),
        };
        let allowed = st.allowed.contains(&key);
        (if allowed { Mode::Yolo } else { st.mode }, key)
    };
    let ask = match (risk, mode) {
        (Risk::Read, _) | (_, Mode::Yolo) => false,
        (_, Mode::Plan) => {
            let msg = "Plan mode is on: this is read-only. Describe the change instead of making it.".to_string();
            emit(Ev::ToolEnd { id, ok: false, output: msg.clone(), diff: None });
            return msg;
        }
        (Risk::Edit, Mode::AutoEdit) => false,
        _ => true,
    };
    if ask {
        let approval_id = format!("a-{id}");
        emit(Ev::Approval(Approval {
            id: approval_id.clone(),
            tool: display,
            title,
            detail: String::new(),
            diff: tools::preview(&shared.cwd, name, &args),
        }));
        let decision = loop {
            if shared.stop.load(Ordering::Relaxed) {
                break Decision::Deny;
            }
            match approvals.lock().unwrap().recv_timeout(Duration::from_millis(100)) {
                Ok((got, d)) if got == approval_id => break d,
                Ok(_) | Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break Decision::Deny,
            }
        };
        match decision {
            Decision::Deny => {
                let msg = "The user declined this tool call. Don't retry it; ask how to proceed.".to_string();
                emit(Ev::ToolEnd { id, ok: false, output: "Declined".into(), diff: None });
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
        emit(Ev::ToolEnd { id, ok, output: text.clone(), diff: None });
        return text;
    }

    let events = shared.events.clone();
    let out_id = id.clone();
    let output = move |chunk: &str| {
        let _ = events.send(Ev::ToolOutput { id: out_id.clone(), chunk: chunk.to_string() });
    };
    let mut snapshots = HashMap::new();
    let outcome = {
        let mut ctx = tools::Ctx { cwd: &shared.cwd, stop: shared.stop.clone(), output: &output, snapshots: &mut snapshots, reads: &shared.reads };
        tools::run(name, &args, &mut ctx)
    };
    if !snapshots.is_empty() {
        let mut st = shared.state.lock().unwrap();
        if let Some(turn) = st.undo.last_mut() {
            for (k, v) in snapshots {
                turn.entry(k).or_insert(v);
            }
        }
    }
    if let Some(todos) = outcome.todos.clone() {
        emit(Ev::Todos(todos));
    }
    emit(Ev::ToolEnd { id, ok: outcome.ok, output: outcome.output.clone(), diff: outcome.diff.clone() });
    outcome.output
}

fn display_name(name: &str) -> &str {
    match name {
        "read" | "ls" => "Read",
        "write" => "Write",
        "edit" => "Edit",
        "bash" => "Bash",
        "grep" => "Grep",
        "glob" => "Glob",
        "fetch" | "web_fetch" => "Fetch",
        "todo" | "todo_write" => "Todos",
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

/// Calls that can run alongside others: read-only tools, and MCP schema lookups.
fn read_only(call: &Value) -> bool {
    let name = call["name"].as_str().unwrap_or("");
    if name == "mcp" {
        return false;
    }
    tools::risk(name) == Risk::Read
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
    let summary = match request(shared, false) {
        Ok(Some(out)) => out
            .iter()
            .filter(|i| i["type"] == "message")
            .flat_map(|i| i["content"].as_array().cloned().unwrap_or_default())
            .filter_map(|c| c["text"].as_str().map(str::to_string))
            .collect::<Vec<_>>()
            .join("\n"),
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

fn undo(shared: &Shared) {
    let mut st = shared.state.lock().unwrap();
    let Some(turn) = st.undo.pop() else {
        let _ = shared.events.send(Ev::Notice("Nothing to undo.".into()));
        return;
    };
    let mut names = Vec::new();
    for (path, content) in &turn {
        let _ = match content {
            Some(text) => std::fs::write(path, text),
            None => std::fs::remove_file(path),
        };
        names.push(tools::short_path(&path.to_string_lossy()));
    }
    names.sort();
    st.items.push(json!({ "role": "user", "content": [{ "type": "input_text", "text": format!("(The user reverted your file changes from the last turn: {}.)", names.join(", ")) }] }));
    let _ = shared.events.send(Ev::Notice(format!("Reverted {}", names.join(", "))));
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn today_is_a_date() {
        let t = today();
        assert_eq!(t.len(), 10);
        assert!(t.starts_with("20"));
    }
}
