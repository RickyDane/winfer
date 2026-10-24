//! Background processes for winfer code's `bg` tool: dev servers, watchers and loops that keep
//! running while the agent works. Each one runs in its own process group with its output in a
//! log file; `read` returns only what's new since the last read and can wait for a marker.
//! The backend `listen`s for what happens to them: they show in winfer code's task list, and
//! the agent gets a note when one exits on its own without a read having shown it.

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde_json::Value;

use super::tools::{Outcome, clip, strip_ansi};

struct Task {
    /// Who started it (the pane of an agent using winfer's MCP server; empty in winfer code):
    /// only they see and control it.
    owner: String,
    cmd: String,
    /// Short name the agent gave it (empty: none); shown instead of the command.
    name: String,
    /// What it's for, in a sentence (empty: none); shown in the task list.
    description: String,
    child: Child,
    log: PathBuf,
    /// Bytes of the log already returned by `read`.
    seen: u64,
    started: Instant,
    exit: Option<i32>,
    /// When the exit was first noticed.
    exited_at: Option<Instant>,
    /// A read showed the agent the exit, so it needs no note about it.
    reported: bool,
    /// The exit went to the listener.
    announced: bool,
}

/// What happens to background tasks, for the backend.
#[derive(Debug, Clone, PartialEq)]
pub enum Update {
    /// `tool_id`: the call that started it. `name`/`description`: as the agent gave them (may be empty).
    Started { id: String, owner: String, tool_id: String, cmd: String, name: String, description: String, log: PathBuf },
    /// Exited on its own; `seen`: a read already showed the agent the exit. `tail`: output the
    /// agent hasn't read.
    Exited { id: String, owner: String, cmd: String, name: String, code: i32, secs: u64, seen: bool, tail: String },
    /// Stopped by the agent, the user (`by_user`) or winfer code exiting.
    Stopped { id: String, owner: String, cmd: String, name: String, by_user: bool },
}

type Listener = Arc<dyn Fn(Update) + Send + Sync>;

fn listener() -> &'static Mutex<Option<Listener>> {
    static L: OnceLock<Mutex<Option<Listener>>> = OnceLock::new();
    L.get_or_init(Mutex::default)
}

/// Sends every later update to `f` (replacing an earlier listener).
pub fn listen(f: impl Fn(Update) + Send + Sync + 'static) {
    *listener().lock().unwrap() = Some(Arc::new(f));
}

fn emit(update: Update) {
    let l = listener().lock().unwrap().clone();
    if let Some(l) = l {
        l(update);
    }
}

/// The `bg` tool's description (winfer code's tool and winfer's MCP server share it).
pub const DESCRIPTION: &str = "Long-running process (server, watch, loop), not bash &: start→id (always give it a short name + description of your own, not the command); read new output (wait ms, until a|b); stop; list. Exits are reported to you.";

/// `bg` as an MCP tool (winfer's MCP server). Claude Code runs an MCP tool alongside others only
/// when it's marked read-only; without the hint, side-by-side starts (each possibly waiting for
/// its marker) run one after another. `read_only` is for winfer code's own `bg` server, which
/// guards what the hint gives away: starts still ask (winfer code answers Claude Code's
/// permission requests by the action, see `claude.rs`) and plan modes refuse them
/// ([`set_mode`]). Agents the app runs directly get no hint, so Claude Code's plan mode keeps
/// asking before a start.
pub fn mcp_tool(read_only: bool) -> Value {
    serde_json::json!({
        "name": "bg",
        "description": DESCRIPTION,
        "annotations": { "readOnlyHint": read_only },
        "inputSchema": { "type": "object", "properties": {
            "action": { "enum": ["start", "read", "stop", "list"] },
            "cmd": { "type": "string", "description": "start: the shell command" },
            "name": { "type": "string", "description": "start: always give one: a 2-5 word name shown to the user, e.g. \"vite dev server\" (not the command)" },
            "description": { "type": "string", "description": "start: always give one: what it's for, in a sentence (not the command)" },
            "id": { "type": "string", "description": "read/stop: the task id start returned" },
            "until": { "type": "string", "description": "read (or start): wait until the output has this text; a|b for either" },
            "wait": { "type": "integer", "description": "read (or start): wait up to this many ms for output (max 290000)" }
        }, "required": ["action"] }
    })
}

impl Update {
    pub fn owner(&self) -> &str {
        match self {
            Update::Started { owner, .. } | Update::Exited { owner, .. } | Update::Stopped { owner, .. } => owner,
        }
    }

    /// The task update for winfer code's task list, and the note to give the agent when the
    /// task ended in a way it hasn't seen.
    pub fn task_event(self) -> (super::event::TaskEv, Option<String>) {
        use super::event::TaskEv;
        match self {
            Update::Started { id, tool_id, cmd, name, description, log, .. } => (
                TaskEv {
                    id,
                    tool_id: Some(tool_id).filter(|t| !t.is_empty()),
                    title: Some(label(&name, &cmd)),
                    description: Some(description).filter(|d| !d.trim().is_empty()),
                    status: Some("running".into()),
                    output_file: Some(log.to_string_lossy().into_owned()),
                    ..Default::default()
                },
                None,
            ),
            Update::Exited { id, cmd, name, code, secs, seen, tail, .. } => {
                let note = (!seen).then(|| {
                    let out = if tail.trim().is_empty() || tail == "(no new output)" { "No new output.".to_string() } else { format!("Its output since your last read:\n{tail}") };
                    format!("Background task {} exited with code {code} after {secs}s. {out}", mention(&id, &name, &cmd))
                });
                let status = if code == 0 { "completed" } else { "failed" };
                (TaskEv { id, status: Some(status.into()), exit_code: Some(code), ..Default::default() }, note)
            }
            Update::Stopped { id, cmd, name, by_user, .. } => {
                let note = by_user.then(|| format!("The user stopped background task {}.", mention(&id, &name, &cmd)));
                (TaskEv { id, status: Some("stopped".into()), ..Default::default() }, note)
            }
        }
    }
}

/// What the user sees a task as: its name, else its command's first line.
pub fn label(name: &str, cmd: &str) -> String {
    let name = name.trim();
    if name.is_empty() { short_command(cmd) } else { name.to_string() }
}

/// A short stand-in name for an unnamed command: its first real step's program and first
/// couple of words (`cd x && cargo test --lib harness | head` → `cargo test --lib`).
pub fn short_command(cmd: &str) -> String {
    let line = cmd.lines().next().unwrap_or("");
    let steps = line.split("&&").flat_map(|s| s.split(';')).map(|s| s.split('|').next().unwrap_or("").trim());
    let step = steps.clone().find(|s| !s.is_empty() && !s.starts_with("cd ") && *s != "cd").or_else(|| steps.clone().find(|s| !s.is_empty())).unwrap_or("");
    let words: Vec<&str> = step.split_whitespace().filter(|w| !w.contains('=') || w.starts_with('-')).take(3).collect();
    words.join(" ")
}

/// A task in a note: `bg1 "dev server" (`npm run dev`)`, or `bg1 (`npm run dev`)` unnamed.
pub fn mention(id: &str, name: &str, cmd: &str) -> String {
    let cmd = cmd.lines().next().unwrap_or("");
    match name.trim() {
        "" => format!("{id} (`{cmd}`)"),
        n => format!("{id} \"{n}\" (`{cmd}`)"),
    }
}

/// Env var that points `winfer mcp-stdio` at winfer code's own `bg` server ("addr token owner"),
/// so the Claude Code and Codex backends' background tasks land in winfer code's task list.
pub const CODE_BG_ENV: &str = "WINFER_CODE_BG";

/// Starts (once) this process's `bg` server for MCP calls and returns `CODE_BG_ENV`'s value for
/// `owner`. Calls are `POST /bg` with `{ "args", "cwd" }` and the token in `X-Winfer-Token`.
pub fn serve(owner: &str) -> Option<String> {
    static SERVER: OnceLock<Option<(String, String)>> = OnceLock::new();
    let (addr, token) = SERVER
        .get_or_init(|| {
            let http = tiny_http::Server::http("127.0.0.1:0").ok()?;
            let addr = http.server_addr().to_ip()?.to_string();
            let token = format!("{:032x}", rand::random::<u128>());
            let secret = token.clone();
            std::thread::spawn(move || {
                for mut req in http.incoming_requests() {
                    let ok = req.headers().iter().any(|h| h.field.equiv("X-Winfer-Token") && h.value.as_str() == secret);
                    if !ok || req.url() != "/bg" {
                        let _ = req.respond(tiny_http::Response::empty(401));
                        continue;
                    }
                    std::thread::spawn(move || {
                        let mut body = String::new();
                        let _ = req.as_reader().take(1024 * 1024).read_to_string(&mut body);
                        let v: Value = serde_json::from_str(&body).unwrap_or_default();
                        let cwd = v["cwd"].as_str().map(PathBuf::from).filter(|p| p.is_dir()).or_else(|| std::env::current_dir().ok()).unwrap_or_default();
                        let out = run(&v["args"], &cwd, &AtomicBool::new(false), "", v["owner"].as_str().unwrap_or(""));
                        let reply = serde_json::json!({ "ok": out.ok, "text": out.output }).to_string();
                        let _ = req.respond(tiny_http::Response::from_string(reply));
                    });
                }
            });
            Some((addr, token))
        })
        .as_ref()?;
    Some(format!("{addr} {token} {owner}"))
}

/// Runs a `bg` call on the server `env` (`CODE_BG_ENV`'s value) names; for `winfer mcp-stdio`.
pub fn call_server(env: &str, args: &Value, cwd: &Path) -> (bool, String) {
    let mut parts = env.splitn(3, ' ');
    let (Some(addr), Some(token), owner) = (parts.next(), parts.next(), parts.next().unwrap_or("")) else {
        return (false, "bad WINFER_CODE_BG".into());
    };
    let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(WAIT_MAX / 1000 + 30))).http_status_as_error(false).build().into();
    let body = serde_json::json!({ "args": args, "cwd": cwd, "owner": owner });
    match agent.post(&format!("http://{addr}/bg")).header("X-Winfer-Token", token).send_json(&body) {
        Ok(mut r) => match r.body_mut().read_json::<Value>() {
            Ok(v) => (v["ok"].as_bool().unwrap_or(false), v["text"].as_str().unwrap_or("").to_string()),
            Err(e) => (false, format!("couldn't read winfer code's reply: {e}")),
        },
        Err(e) => (false, format!("couldn't reach winfer code: {e}")),
    }
}

/// Points winfer's MCP server among `servers` at this process's `bg` server for `owner` (adding
/// the server when winfer code runs outside the app).
pub fn route_mcp(servers: &mut serde_json::Map<String, Value>, owner: &str) {
    let Some(env) = serve(owner) else { return };
    let ours = |s: &Value| s["args"].as_array().is_some_and(|a| a.iter().any(|x| x == "mcp-stdio"));
    if !servers.values().any(ours) {
        let Ok(exe) = std::env::current_exe() else { return };
        servers.insert("winfer".into(), serde_json::json!({ "command": exe, "args": ["mcp-stdio"] }));
    }
    let exe = std::env::current_exe().ok();
    for spec in servers.values_mut().filter(|s| ours(s)) {
        if let Some(o) = spec.as_object_mut() {
            // This binary knows `CODE_BG_ENV` (an app of another version might not).
            if let Some(exe) = &exe {
                o.insert("command".into(), serde_json::json!(exe));
            }
            let e = o.entry("env").or_insert_with(|| serde_json::json!({}));
            if let Some(e) = e.as_object_mut() {
                e.insert(CODE_BG_ENV.into(), Value::String(env.clone()));
            }
        }
    }
}

/// How long an exit waits for a running read to show it before it's announced.
const EXIT_GRACE: Duration = Duration::from_millis(500);

/// Notices tasks that exit on their own (one thread for all, started with the first task).
fn watch() {
    static STARTED: OnceLock<()> = OnceLock::new();
    STARTED.get_or_init(|| {
        std::thread::spawn(|| loop {
            std::thread::sleep(Duration::from_millis(250));
            let mut done = Vec::new();
            for (id, task) in tasks().lock().unwrap().iter_mut() {
                poll(task);
                if let (Some(code), Some(at)) = (task.exit, task.exited_at)
                    && !task.announced
                    && at.elapsed() >= EXIT_GRACE
                {
                    task.announced = true;
                    let tail = if task.reported { String::new() } else { tail(&strip_ansi(&read_from(&task.log, task.seen)), &[], &task.log) };
                    let secs = at.duration_since(task.started).as_secs();
                    done.push(Update::Exited { id: id.clone(), owner: task.owner.clone(), cmd: task.cmd.clone(), name: task.name.clone(), code, secs, seen: task.reported, tail });
                }
            }
            for u in done {
                emit(u);
            }
        });
    });
}

fn tasks() -> &'static Mutex<HashMap<String, Task>> {
    static TASKS: OnceLock<Mutex<HashMap<String, Task>>> = OnceLock::new();
    TASKS.get_or_init(Mutex::default)
}

static NEXT: AtomicUsize = AtomicUsize::new(1);

/// New output a read returns at most (the end of it when there's more).
const READ_MAX: usize = 4_000;
/// New lines a read returns at most (the last ones).
const READ_LINES: usize = 40;
/// Longest a read may wait.
const WAIT_MAX: u64 = 600_000;

/// Runs a `bg` call for `owner`; `call_id` is the tool call's id.
pub fn run(args: &Value, cwd: &Path, stop: &AtomicBool, call_id: &str, owner: &str) -> Outcome {
    let id = args["id"].as_str().unwrap_or("");
    match args["action"].as_str().unwrap_or(if args["cmd"].is_string() { "start" } else { "list" }) {
        "start" => match args["cmd"].as_str() {
            Some(cmd) => start(cmd, cwd, args, stop, call_id, owner),
            None => Outcome::err("cmd is required"),
        },
        _ if !id.is_empty() && !owns(id, owner) => Outcome::err(format!("no background task {id}; bg list shows yours")),
        "read" => read(id, args, stop),
        "stop" => stop_task(id, false),
        "list" => list(owner),
        other => Outcome::err(format!("unknown action {other}; use start, read, stop or list")),
    }
}

fn owns(id: &str, owner: &str) -> bool {
    tasks().lock().unwrap().get(id).is_some_and(|t| t.owner == owner)
}

fn read_only_owners() -> &'static Mutex<std::collections::HashSet<String>> {
    static R: OnceLock<Mutex<std::collections::HashSet<String>>> = OnceLock::new();
    R.get_or_init(Mutex::default)
}

/// Tells `bg` the mode of `owner`'s agent: in plan modes nothing new may start. Claude Code lets
/// read-only tools through in plan mode without asking, and `bg` is marked read-only (see
/// [`mcp_tool`]), so the start is refused here instead.
pub fn set_mode(owner: &str, mode: super::event::Mode) {
    use super::event::Mode;
    let mut all = read_only_owners().lock().unwrap();
    if matches!(mode, Mode::Plan | Mode::YoloPlan) {
        all.insert(owner.to_string());
    } else {
        all.remove(owner);
    }
}

fn start(cmd: &str, cwd: &Path, args: &Value, stop: &AtomicBool, call_id: &str, owner: &str) -> Outcome {
    if read_only_owners().lock().unwrap().contains(owner) {
        return Outcome::err("plan mode is on: bg can't start anything now. Describe the command in the plan instead.");
    }
    let id = format!("bg{}", NEXT.fetch_add(1, Ordering::Relaxed));
    let dir = std::env::temp_dir().join(format!("winfer-bg-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let log = dir.join(format!("{id}.log"));
    let (out, err) = match File::create(&log).and_then(|f| Ok((f.try_clone()?, f))) {
        Ok(f) => f,
        Err(e) => return Outcome::err(format!("couldn't create the log: {e}")),
    };
    let mut command = super::tools::shell_command(cmd, cwd);
    command.stdin(Stdio::null()).stdout(Stdio::from(out)).stderr(Stdio::from(err));
    let child = match command.spawn() {
        Ok(c) => c,
        Err(e) => return Outcome::err(format!("couldn't start: {e}")),
    };
    let text = |k: &str| args[k].as_str().unwrap_or("").trim().to_string();
    let (name, description) = (text("name"), text("description"));
    let task = Task { owner: owner.into(), cmd: cmd.into(), name: name.clone(), description: description.clone(), child, log: log.clone(), seen: 0, started: Instant::now(), exit: None, exited_at: None, reported: false, announced: false };
    tasks().lock().unwrap().insert(id.clone(), task);
    emit(Update::Started { id: id.clone(), owner: owner.into(), tool_id: call_id.into(), cmd: cmd.into(), name, description, log: log.clone() });
    watch();
    // Waiting for a first marker right away saves a separate read.
    if args["until"].is_string() || args["wait"].is_u64() {
        let r = read(&id, args, stop);
        return Outcome { output: format!("started {id} (log {})\n{}", log.display(), r.output), ..r };
    }
    Outcome::ok(format!("started {id} (log {}); bg read {id} for output; you'll be told when it exits", log.display()))
}

fn poll(task: &mut Task) {
    if task.exit.is_none()
        && let Ok(Some(status)) = task.child.try_wait()
    {
        task.exit = Some(status.code().unwrap_or(-1));
        task.exited_at = Some(Instant::now());
    }
}

fn state(task: &Task) -> String {
    let secs = task.started.elapsed().as_secs();
    match task.exit {
        None => format!("running {secs}s"),
        Some(code) => format!("exited {code}"),
    }
}

fn read(id: &str, args: &Value, stop: &AtomicBool) -> Outcome {
    let wait = Duration::from_millis(args["wait"].as_u64().unwrap_or(0).min(WAIT_MAX));
    let until: Vec<&str> = args["until"].as_str().map(|u| u.split('|').filter(|s| !s.is_empty()).collect()).unwrap_or_default();
    // A marker without a wait still waits a while for it.
    let wait = if !until.is_empty() && wait.is_zero() { Duration::from_secs(120) } else { wait };
    let started = Instant::now();
    loop {
        let (log, seen, done) = {
            let mut all = tasks().lock().unwrap();
            let Some(task) = all.get_mut(id) else { return Outcome::err(format!("no background task {id}; bg list shows them")) };
            poll(task);
            (task.log.clone(), task.seen, task.exit.is_some())
        };
        let new = read_from(&log, seen);
        let hit = until.iter().any(|u| new.contains(u));
        let out_of_time = started.elapsed() >= wait || stop.load(Ordering::Relaxed);
        if hit || done || out_of_time || (until.is_empty() && !new.is_empty() && wait.is_zero()) {
            let mut all = tasks().lock().unwrap();
            let Some(task) = all.get_mut(id) else { return Outcome::err(format!("{id} was stopped")) };
            task.seen = seen + new.len() as u64;
            task.reported |= task.exit.is_some();
            let mut head = format!("{id} {}", state(task));
            if !until.is_empty() && !hit {
                head.push_str(&format!(" · \"{}\" not seen", until.join("|")));
            }
            let body = tail(&strip_ansi(&new), &until, &task.log);
            let failed = matches!(task.exit, Some(c) if c != 0);
            return Outcome { ok: !failed, output: format!("{head}\n{body}"), diff: None, todos: None };
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// The last lines of `text` (at most `READ_LINES` and `READ_MAX` bytes), with lines matching
/// `until` that fall before them kept on top, so a marker never scrolls out of view.
fn tail(text: &str, until: &[&str], log: &Path) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let mut start = lines.len().saturating_sub(READ_LINES);
    while start < lines.len() && lines[start..].iter().map(|l| l.len() + 1).sum::<usize>() > READ_MAX {
        start += 1;
    }
    if lines[start..].iter().all(|l| l.trim().is_empty()) && start == 0 {
        return "(no new output)".into();
    }
    let mut out = Vec::new();
    if start > 0 {
        let marks: Vec<&str> = lines[..start].iter().filter(|l| until.iter().any(|u| l.contains(u))).take(5).copied().collect();
        out.push(format!("… [{start} earlier lines in {}]", log.display()));
        out.extend(marks.into_iter().map(|l| clip(l, 500)));
        if out.len() > 1 {
            out.push("…".into());
        }
    }
    out.extend(lines[start..].iter().map(|l| clip(l, 500)));
    out.join("\n")
}

fn read_from(log: &Path, from: u64) -> String {
    let mut buf = Vec::new();
    if let Ok(mut f) = File::open(log)
        && f.seek(SeekFrom::Start(from)).is_ok()
    {
        let _ = f.read_to_end(&mut buf);
    }
    // Don't split a UTF-8 character at the end; the rest comes with the next read.
    let end = match std::str::from_utf8(&buf) {
        Ok(_) => buf.len(),
        Err(e) if e.error_len().is_none() => e.valid_up_to(),
        Err(_) => buf.len(),
    };
    String::from_utf8_lossy(&buf[..end]).into_owned()
}

fn stop_task(id: &str, by_user: bool) -> Outcome {
    let Some(mut task) = tasks().lock().unwrap().remove(id) else { return Outcome::err(format!("no background task {id}")) };
    emit(Update::Stopped { id: id.into(), owner: task.owner.clone(), cmd: task.cmd.clone(), name: task.name.clone(), by_user });
    poll(&mut task);
    if task.exit.is_none() {
        super::tools::kill_tree(&mut task.child);
        let _ = task.child.wait();
    }
    let tail = read_from(&task.log, task.seen);
    let tail = strip_ansi(&tail);
    let tail = if tail.len() > 1_000 { tail[tail.ceil_char_boundary(tail.len() - 1_000)..].to_string() } else { tail };
    let _ = std::fs::remove_file(&task.log);
    let note = if tail.trim().is_empty() { String::new() } else { format!("\nlast output:\n{}", tail.trim_end()) };
    Outcome::ok(format!("stopped {id} ({}){note}", state(&task)))
}

fn list(owner: &str) -> Outcome {
    let mut all = tasks().lock().unwrap();
    let mut ids: Vec<_> = all.iter().filter(|(_, t)| t.owner == owner).map(|(id, _)| id.clone()).collect();
    if ids.is_empty() {
        return Outcome::ok("no background tasks");
    }
    ids.sort_by_key(|id| id.trim_start_matches("bg").parse::<usize>().unwrap_or(0));
    let lines: Vec<String> = ids
        .iter()
        .map(|id| {
            let t = all.get_mut(id).unwrap();
            poll(t);
            let about = if t.description.is_empty() { String::new() } else { format!(" — {}", t.description) };
            format!("{id} {} · {}{about}", state(t), mention("", &t.name, &t.cmd).trim_start())
        })
        .collect();
    Outcome::ok(lines.join("\n"))
}

/// Stops a task for the user (winfer code's task list); false when there's no such task.
pub fn stop(id: &str) -> bool {
    stop_task(id, true).ok
}

/// Stops `owner`'s tasks, e.g. when their pane closes.
pub fn stop_owner(owner: &str) {
    read_only_owners().lock().unwrap().remove(owner);
    let ids: Vec<String> = tasks().lock().unwrap().iter().filter(|(_, t)| t.owner == owner).map(|(id, _)| id.clone()).collect();
    for id in ids {
        let _ = stop_task(&id, false);
    }
}

/// Stops every background task, e.g. when winfer code exits or switches backends.
pub fn stop_all() {
    let ids: Vec<String> = tasks().lock().unwrap().keys().cloned().collect();
    for id in ids {
        let _ = stop_task(&id, false);
    }
    let _ = std::fs::remove_dir(std::env::temp_dir().join(format!("winfer-bg-{}", std::process::id())));
}

#[cfg(test)]
mod tests {
    #[test]
    fn unnamed_commands_get_short_names() {
        assert_eq!(super::short_command("cd src-tauri && cargo test --lib harness 2>&1 | head"), "cargo test --lib");
        assert_eq!(super::short_command("RUST_LOG=debug pnpm dev"), "pnpm dev");
        assert_eq!(super::short_command("gh run watch 123"), "gh run watch");
    }

    use super::*;
    use serde_json::json;

    fn call(args: Value) -> Outcome {
        run(&args, &std::env::temp_dir(), &AtomicBool::new(false), "call", "")
    }

    fn id_of(out: &Outcome) -> String {
        out.output.split_whitespace().nth(1).unwrap().to_string()
    }

    #[test]
    fn plan_modes_refuse_starts() {
        use super::super::event::Mode;
        let start = || run(&json!({ "action": "start", "cmd": "true" }), &std::env::temp_dir(), &AtomicBool::new(false), "c", "code-plan");
        set_mode("code-plan", Mode::Plan);
        let out = start();
        assert!(!out.ok && out.output.contains("plan mode"), "{}", out.output);
        set_mode("code-plan", Mode::YoloPlan);
        assert!(!start().ok);
        // Reading and listing still work, and leaving plan mode allows starts again.
        assert!(run(&json!({ "action": "list" }), &std::env::temp_dir(), &AtomicBool::new(false), "c", "code-plan").ok);
        set_mode("code-plan", Mode::Ask);
        let out = start();
        assert!(out.ok, "{}", out.output);
        stop_owner("code-plan");
    }

    #[cfg(unix)]
    #[test]
    fn start_wait_for_marker_read_new_output_and_stop() {
        let s = call(json!({ "action": "start", "cmd": "echo booting; sleep 0.3; echo ready; while true; do echo tick; sleep 0.1; done", "until": "ready" }));
        assert!(s.ok && s.output.contains("ready") && s.output.contains("running"), "{}", s.output);
        let id = id_of(&s);
        let r = call(json!({ "action": "read", "id": id, "wait": 400 }));
        assert!(r.output.contains("tick") && !r.output.contains("booting"), "{}", r.output);
        assert!(call(json!({ "action": "list" })).output.contains(&id));
        let st = call(json!({ "action": "stop", "id": id }));
        assert!(st.ok && st.output.contains("stopped"), "{}", st.output);
        assert!(!call(json!({ "action": "list" })).output.contains(&format!("{id} ")));
    }

    #[test]
    fn route_mcp_points_winfers_server_at_this_process() {
        let mut servers: serde_json::Map<String, Value> = serde_json::from_value(json!({
            "winfer": { "command": "/w", "args": ["mcp-stdio"], "env": { "WINFER_PANE_ID": "p-1" } },
            "codegraph": { "command": "codegraph" }
        })).unwrap();
        route_mcp(&mut servers, "code-x");
        let env = servers["winfer"]["env"][CODE_BG_ENV].as_str().unwrap().to_string();
        assert!(env.ends_with(" code-x") && env.split(' ').count() == 3, "{env}");
        assert_eq!(servers["winfer"]["env"]["WINFER_PANE_ID"], "p-1");
        assert!(servers["codegraph"].get("env").is_none());
        // Outside the app there's no winfer server yet: one is added for bg.
        let mut none = serde_json::Map::new();
        route_mcp(&mut none, "code-y");
        assert_eq!(none["winfer"]["args"], json!(["mcp-stdio"]));
        // The server runs calls for that owner.
        let (ok, text) = call_server(&env, &json!({ "action": "start", "cmd": "echo hi", "until": "hi" }), &std::env::temp_dir());
        assert!(ok && text.contains("hi"), "{text}");
        let id = text.split_whitespace().nth(1).unwrap().to_string();
        assert!(list("code-x").output.contains(&id) && !list("").output.contains(&id));
        stop_owner("code-x");
    }

    #[test]
    fn tail_keeps_the_marker_line_that_scrolled_out() {
        let text: String = (1..=100).map(|i| if i == 10 { "FAIL a.test\n".to_string() } else { format!("tick {i}\n") }).collect();
        let t = tail(&text, &["FAIL"], Path::new("x.log"));
        assert!(t.starts_with("… [60 earlier lines in x.log]\nFAIL a.test\n…\ntick 61"), "{t}");
        assert!(t.ends_with("tick 100") && t.lines().count() == 43);
        assert_eq!(tail("", &[], Path::new("x")), "(no new output)");
    }

    /// All listener checks in one test: the listener is process-wide.
    #[cfg(unix)]
    #[test]
    fn listener_hears_starts_exits_and_stops() {
        let got: Arc<Mutex<Vec<Update>>> = Arc::default();
        let sink = got.clone();
        listen(move |u| sink.lock().unwrap().push(u));
        let events = |id: &str| -> Vec<Update> {
            got.lock().unwrap().iter().filter(|u| matches!(u, Update::Started { id: i, .. } | Update::Exited { id: i, .. } | Update::Stopped { id: i, .. } if i == id)).cloned().collect()
        };
        // Exits on its own, unread: announced as unseen, with the output.
        let quiet = id_of(&call(json!({ "action": "start", "cmd": "echo oops; exit 4" })));
        // Exits while a read waits on it: announced as seen.
        let read = id_of(&call(json!({ "action": "start", "cmd": "sleep 0.2; echo done" })));
        let r = call(json!({ "action": "read", "id": read, "until": "never", "wait": 3000 }));
        assert!(r.output.contains("exited 0"), "{}", r.output);
        // Stopped from winfer code's task list.
        let user = id_of(&call(json!({ "action": "start", "cmd": "sleep 30" })));
        assert!(stop(&user) && !stop(&user));
        std::thread::sleep(Duration::from_millis(1200));
        let q = events(&quiet);
        assert!(matches!(&q[0], Update::Started { tool_id, .. } if tool_id == "call"), "{q:?}");
        assert!(matches!(&q[1], Update::Exited { code: 4, seen: false, tail, .. } if tail.contains("oops")), "{q:?}");
        assert!(matches!(&events(&read)[1], Update::Exited { code: 0, seen: true, .. }), "{:?}", events(&read));
        assert!(matches!(&events(&user)[1], Update::Stopped { by_user: true, .. }), "{:?}", events(&user));
        call(json!({ "action": "stop", "id": quiet }));
        call(json!({ "action": "stop", "id": read }));
    }

    #[test]
    fn names_and_descriptions_label_tasks() {
        assert_eq!(label("  ", "npm run dev\nmore"), "npm run dev");
        assert_eq!(label("vite", "npm run dev"), "vite");
        assert_eq!(mention("bg1", "", "make"), "bg1 (`make`)");
        assert_eq!(mention("bg1", "build", "make"), "bg1 \"build\" (`make`)");
        let (ev, _) = Update::Started { id: "bg1".into(), owner: String::new(), tool_id: String::new(), cmd: "npm run dev".into(), name: "vite".into(), description: "Serves the UI".into(), log: "/tmp/l".into() }.task_event();
        assert_eq!((ev.title.as_deref(), ev.description.as_deref()), (Some("vite"), Some("Serves the UI")));
        assert_eq!(super::super::tools::title("bg", &json!({ "action": "start", "cmd": "npm run dev", "name": "vite" })), "start vite");
        assert_eq!(super::super::tools::title("bg", &json!({ "action": "start", "cmd": "npm run dev" })), "start npm run dev");
    }

    #[cfg(unix)]
    #[test]
    fn reports_exit_code_and_missing_marker() {
        let s = call(json!({ "action": "start", "cmd": "echo building; exit 3" }));
        let id = id_of(&s);
        let r = call(json!({ "action": "read", "id": id, "until": "done", "wait": 2000 }));
        assert!(!r.ok && r.output.contains("exited 3") && r.output.contains("not seen") && r.output.contains("building"), "{}", r.output);
        call(json!({ "action": "stop", "id": id }));
    }

    #[cfg(unix)]
    #[test]
    fn stop_kills_the_whole_process_group() {
        let pidfile = std::env::temp_dir().join(format!("winfer-bg-test-{}", std::process::id()));
        let s = call(json!({ "action": "start", "cmd": format!("sh -c 'sleep 30 & echo $! > {}; wait'", pidfile.display()) }));
        let id = id_of(&s);
        std::thread::sleep(Duration::from_millis(300));
        let pid: i32 = std::fs::read_to_string(&pidfile).unwrap().trim().parse().unwrap();
        call(json!({ "action": "stop", "id": id }));
        std::thread::sleep(Duration::from_millis(100));
        assert_ne!(unsafe { libc::kill(pid, 0) }, 0, "grandchild {pid} survived");
        let _ = std::fs::remove_file(pidfile);
    }
}
