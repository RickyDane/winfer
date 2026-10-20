//! winfer's own tools, for the ChatGPT backend (the bridges use their CLI's tools).

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use base64::Engine;
use serde_json::{Value, json};

use super::diff;
use super::event::Todo;

/// How much a tool may do without asking.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Risk {
    Read,
    Edit,
    Exec,
}

/// `bg` only runs something new on `start`.
pub fn risk(name: &str, args: &Value) -> Risk {
    match canonical(name) {
        "bg" if args["action"].as_str().map_or(!args["cmd"].is_string(), |a| a != "start") => Risk::Read,
        "read" | "grep" | "glob" | "todo" => Risk::Read,
        "write" | "edit" => Risk::Edit,
        _ => Risk::Exec,
    }
}

/// Current tool names for older ones (saved sessions replay with them).
fn canonical(name: &str) -> &str {
    match name {
        "ls" => "read",
        "web_fetch" => "fetch",
        "todo_write" => "todo",
        other => other,
    }
}

/// Arguments with older parameter names renamed to the current ones.
fn normalize(args: &Value) -> Value {
    let mut args = args.clone();
    if let Some(o) = args.as_object_mut() {
        for (old, new) in [("old_string", "old"), ("new_string", "new"), ("replace_all", "all"), ("command", "cmd"), ("timeout_ms", "timeout"), ("ignore_case", "i"), ("todos", "items")] {
            if let Some(v) = o.remove(old) {
                o.entry(new).or_insert(v);
            }
        }
    }
    args
}

/// Function tool definitions for the Responses API: deliberately terse (they're sent with
/// every request).
pub fn definitions() -> Vec<Value> {
    let f = |name: &str, description: &str, properties: Value, required: &[&str]| {
        json!({
            "type": "function",
            "name": name,
            "description": description,
            "parameters": { "type": "object", "properties": properties, "required": required },
        })
    };
    let s = json!({ "type": "string" });
    let n = json!({ "type": "integer" });
    let b = json!({ "type": "boolean" });
    let mut tools = vec![
        f("read", "Read file (numbered lines) or list dir.", json!({ "path": s, "offset": n, "limit": n }), &["path"]),
        f("write", "Create/overwrite file.", json!({ "path": s, "content": s }), &["path", "content"]),
        f("edit", "Replace exact unique text in file (all: every match).", json!({ "path": s, "old": s, "new": s, "all": b }), &["path", "old", "new"]),
        f("bash", shell_description(), json!({ "cmd": s, "timeout": n }), &["cmd"]),
        f(
            "bg",
            super::bg::DESCRIPTION,
            json!({ "action": { "enum": ["start", "read", "stop", "list"] }, "cmd": s, "id": s, "until": s, "wait": n }),
            &["action"],
        ),
        f("grep", "Regex search file contents (i: ignore case).", json!({ "pattern": s, "path": s, "glob": s, "i": b }), &["pattern"]),
        f("glob", "Find files by glob, newest first.", json!({ "pattern": s, "path": s }), &["pattern"]),
        f("fetch", "Fetch URL as text.", json!({ "url": s }), &["url"]),
        f(
            "todo",
            "Set task list for multi-step work.",
            json!({ "items": { "type": "array", "items": { "type": "object", "properties": { "content": s, "status": { "enum": ["pending", "in_progress", "completed"] } } } } }),
            &["items"],
        ),
    ];
    if !bg_enabled() {
        tools.retain(|t| t["name"] != "bg");
    }
    tools
}

/// Whether agents get the `bg` tool (`WINFER_CODE_NO_BG=1` turns it off, e.g. to compare).
pub fn bg_enabled() -> bool {
    std::env::var_os("WINFER_CODE_NO_BG").is_none_or(|v| v.is_empty() || v == "0")
}

pub struct Ctx<'a> {
    pub cwd: &'a Path,
    pub stop: Arc<AtomicBool>,
    /// Live output (bash).
    pub output: &'a dyn Fn(&str),
    /// The tool call's id (links a `bg` task to its card).
    pub call_id: &'a str,
    /// Whose `bg` tasks these are (one backend's).
    pub bg_owner: &'a str,
    /// Original contents of files this turn changed (None: didn't exist), for /undo.
    pub snapshots: &'a mut HashMap<PathBuf, Option<String>>,
    /// What this session has read, so an unchanged re-read costs a line instead of the file.
    pub reads: &'a ReadCache,
}

/// (path, offset, limit) → (modified, size) at the last read.
pub type ReadCache = std::sync::Mutex<HashMap<(PathBuf, usize, usize), (std::time::SystemTime, u64)>>;

pub struct Outcome {
    pub ok: bool,
    pub output: String,
    pub diff: Option<String>,
    pub todos: Option<Vec<Todo>>,
}

impl Outcome {
    pub(super) fn ok(output: impl Into<String>) -> Self {
        Outcome { ok: true, output: output.into(), diff: None, todos: None }
    }
    pub(super) fn err(output: impl Into<String>) -> Self {
        Outcome { ok: false, output: output.into(), diff: None, todos: None }
    }
}

pub fn resolve(cwd: &Path, path: &str) -> PathBuf {
    let path = shellexpand_home(path);
    if path.is_absolute() { path } else { cwd.join(path) }
}

fn shellexpand_home(path: &str) -> PathBuf {
    match (path.strip_prefix("~/"), dirs::home_dir()) {
        (Some(rest), Some(home)) => home.join(rest),
        _ => PathBuf::from(path),
    }
}

/// `path` relative to the working directory (or with `~`), for display.
pub fn short_path(path: &str) -> String {
    let p = Path::new(path);
    if let Ok(cwd) = std::env::current_dir() {
        if let Some(rel) = p.strip_prefix(&cwd).ok().filter(|r| !r.as_os_str().is_empty()) {
            return rel.to_string_lossy().into_owned();
        }
    }
    if let Some(home) = dirs::home_dir() {
        if let Ok(rel) = p.strip_prefix(&home) {
            return format!("~/{}", rel.to_string_lossy());
        }
    }
    path.to_string()
}

/// The card title for a call.
pub fn title(name: &str, args: &Value) -> String {
    let args = &normalize(args);
    let s = |k: &str| args[k].as_str().unwrap_or("").to_string();
    let t = match canonical(name) {
        "read" | "write" | "edit" => short_path(&s("path")),
        "bash" => s("cmd"),
        "bg" => format!("{} {}", s("action"), if args["cmd"].is_string() { s("cmd") } else { s("id") }).trim().to_string(),
        "grep" => format!("{}{}", s("pattern"), args["path"].as_str().map(|p| format!(" in {p}")).unwrap_or_default()),
        "glob" => s("pattern"),
        "fetch" => s("url"),
        "todo" => "update todos".into(),
        _ => args.as_object().and_then(|o| o.values().find_map(|v| v.as_str().map(str::to_string))).unwrap_or_default(),
    };
    t.lines().next().unwrap_or("").to_string()
}

/// The diff an edit would make, for its approval prompt.
pub fn preview(cwd: &Path, name: &str, args: &Value) -> Option<String> {
    let args = &normalize(args);
    let path = resolve(cwd, args["path"].as_str()?);
    let short = short_path(&path.to_string_lossy());
    let old = std::fs::read_to_string(&path).unwrap_or_default();
    match canonical(name) {
        "write" => Some(diff::unified(&short, &old, args["content"].as_str()?)),
        "edit" => {
            let new = apply_edit(&old, args).ok()?;
            Some(diff::unified(&short, &old, &new))
        }
        _ => None,
    }
}

fn apply_edit(old: &str, args: &Value) -> Result<String, String> {
    let from = args["old"].as_str().ok_or("old is required")?;
    let to = args["new"].as_str().ok_or("new is required")?;
    if from.is_empty() {
        return Err("old is empty; use write to create files".into());
    }
    let count = old.matches(from).count();
    match (count, args["all"].as_bool().unwrap_or(false)) {
        (0, _) => Err("old not found; copy the text exactly from read.".into()),
        (1, _) | (_, true) => Ok(old.replace(from, to)),
        (n, false) => Err(format!("old matches {n} times; add context or set all")),
    }
}

pub fn run(name: &str, args: &Value, ctx: &mut Ctx) -> Outcome {
    let args = &normalize(args);
    match canonical(name) {
        "read" => read(ctx, args),
        n @ ("write" | "edit") => write(n, args, ctx),
        "bash" => bash(args, ctx),
        "bg" => super::bg::run(args, ctx.cwd, &ctx.stop, ctx.call_id, ctx.bg_owner),
        "grep" => grep(ctx.cwd, args),
        "glob" => glob(ctx.cwd, args),
        "fetch" => web_fetch(args),
        "todo" => match serde_json::from_value::<Vec<Todo>>(args["items"].clone()) {
            Ok(todos) => Outcome { ok: true, output: "ok".into(), diff: None, todos: Some(todos) },
            Err(e) => Outcome::err(format!("invalid todos: {e}")),
        },
        _ => Outcome::err(format!("unknown tool {name}")),
    }
}

const READ_LINES: usize = 500;
const LINE_CHARS: usize = 500;

fn read(ctx: &Ctx, args: &Value) -> Outcome {
    let path = args["path"].as_str().unwrap_or(".");
    let path = resolve(ctx.cwd, path);
    if path.is_dir() {
        return ls(&path);
    }
    let offset = args["offset"].as_u64().unwrap_or(1).max(1) as usize;
    let limit = args["limit"].as_u64().unwrap_or(READ_LINES as u64) as usize;
    let stamp = std::fs::metadata(&path).ok().map(|m| (m.modified().unwrap_or(std::time::UNIX_EPOCH), m.len()));
    let key = (path.clone(), offset, limit);
    if let Some(stamp) = stamp {
        let mut reads = ctx.reads.lock().unwrap();
        if reads.get(&key) == Some(&stamp) {
            return Outcome::ok("(unchanged since your last read)");
        }
        reads.insert(key, stamp);
    }
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) => return Outcome::err(format!("couldn't read {}: {e}", path.display())),
    };
    if bytes.iter().take(8000).any(|&b| b == 0) {
        return Outcome::err(format!("{} is a binary file ({} bytes)", path.display(), bytes.len()));
    }
    let text = String::from_utf8_lossy(&bytes);
    let total = text.lines().count();
    let mut out = String::new();
    for (i, line) in text.lines().enumerate().skip(offset - 1).take(limit) {
        let line = if line.len() > LINE_CHARS { &line[..line.floor_char_boundary(LINE_CHARS)] } else { line };
        out.push_str(&format!("{}\t{line}\n", i + 1));
    }
    if offset - 1 + limit < total {
        out.push_str(&format!("… {} more lines (offset={})\n", total - (offset - 1 + limit), offset + limit));
    }
    if out.is_empty() {
        out = "(empty file)".into();
    }
    Outcome::ok(out)
}

fn write(name: &str, args: &Value, ctx: &mut Ctx) -> Outcome {
    let Some(p) = args["path"].as_str() else { return Outcome::err("path is required") };
    let path = resolve(ctx.cwd, p);
    let old = std::fs::read_to_string(&path).ok();
    let new = if name == "write" {
        match args["content"].as_str() {
            Some(c) => c.to_string(),
            None => return Outcome::err("content is required"),
        }
    } else {
        let Some(old) = &old else { return Outcome::err(format!("{} doesn't exist", path.display())) };
        match apply_edit(old, args) {
            Ok(new) => new,
            Err(e) => return Outcome::err(e),
        }
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    ctx.snapshots.entry(path.clone()).or_insert_with(|| old.clone());
    ctx.reads.lock().unwrap().retain(|(p, _, _), _| *p != path);
    if let Err(e) = std::fs::write(&path, &new) {
        return Outcome::err(format!("couldn't write {}: {e}", path.display()));
    }
    let short = short_path(&path.to_string_lossy());
    let d = diff::unified(&short, old.as_deref().unwrap_or(""), &new);
    let (add, del) = diff::stat(&d);
    let verb = if old.is_none() { "Created" } else { "Updated" };
    Outcome { ok: true, output: format!("{verb} {short} (+{add} -{del})"), diff: Some(d), todos: None }
}

const MAX_OUTPUT: usize = 10_000;

/// Keeps the start and end of long output.
pub fn clip(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    let head = text.floor_char_boundary(max / 2);
    let tail = text.ceil_char_boundary(text.len() - max / 2);
    format!("{}\n… [{} bytes omitted] …\n{}", &text[..head], tail - head, &text[tail..])
}

/// The program and arguments that run a command line: PowerShell on Windows (pwsh when
/// installed), else the user's bash/zsh, else /bin/sh.
fn shell() -> (String, Vec<&'static str>) {
    let sh = crate::env::user_shell();
    let name = crate::env::shell_name(&sh);
    if cfg!(windows) {
        return (sh, vec!["-NoLogo", "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass"]);
    }
    if matches!(name.as_str(), "bash" | "zsh") { (sh, vec!["-c"]) } else { ("/bin/sh".into(), vec!["-c"]) }
}

/// The shell the `bash` tool and `!` commands run, as a tool card name: `PowerShell` on
/// Windows, else `Bash`/`Zsh`/`Sh` (what `shell()` picks).
pub fn shell_display() -> &'static str {
    if cfg!(windows) {
        return "PowerShell";
    }
    match crate::env::shell_name(&shell().0).as_str() {
        "bash" => "Bash",
        "zsh" => "Zsh",
        _ => "Sh",
    }
}

/// The `bash` tool's description, naming the shell commands run in.
fn shell_description() -> &'static str {
    if cfg!(windows) {
        "Run PowerShell (not bash, not for reading files; timeout ms, default 120000)."
    } else {
        "Run shell command (timeout ms, default 120000)."
    }
}

/// PowerShell: UTF-8 output, no progress bars, and the exit code of the last native command
/// (or 1 when a cmdlet failed) instead of always 0/1. `Get-Content` reads UTF-8 (Windows
/// PowerShell 5.1 reads BOM-less files as ANSI), and `head`/`tail` work when no such program is
/// installed, since agents pipe into them out of bash habit.
fn powershell_script(command: &str) -> String {
    format!(
        "$ProgressPreference='SilentlyContinue'; [Console]::OutputEncoding=[System.Text.Encoding]::UTF8; $OutputEncoding=[System.Text.Encoding]::UTF8; $PSDefaultParameterValues['Get-Content:Encoding']='UTF8'\n\
         if (-not (Get-Command head -ErrorAction SilentlyContinue)) {{ function head {{ param([int]$n = 10, [string]$Path) $c = [Math]::Abs($n); if ($Path) {{ Get-Content -LiteralPath $Path -TotalCount $c }} else {{ $input | Select-Object -First $c }} }} }}\n\
         if (-not (Get-Command tail -ErrorAction SilentlyContinue)) {{ function tail {{ param([int]$n = 10, [string]$Path) $c = [Math]::Abs($n); if ($Path) {{ Get-Content -LiteralPath $Path -Tail $c }} else {{ $input | Select-Object -Last $c }} }} }}\n\
         $global:LASTEXITCODE=0\n{command}\nif (-not $?) {{ if ($LASTEXITCODE) {{ exit $LASTEXITCODE }} else {{ exit 1 }} }} else {{ exit $LASTEXITCODE }}"
    )
}

/// The arguments that hand PowerShell `script`: `-EncodedCommand` (base64 UTF-16LE), which
/// reaches it intact, where `-Command` loses embedded quotes on Windows PowerShell 5.1. Scripts
/// too long for a command line (32767 chars) fall back to `-Command`.
fn powershell_args(script: &str) -> [String; 2] {
    let bytes: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
    let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
    if encoded.len() < 30_000 { ["-EncodedCommand".into(), encoded] } else { ["-Command".into(), script.into()] }
}

fn bash(args: &Value, ctx: &mut Ctx) -> Outcome {
    let Some(command) = args["cmd"].as_str() else { return Outcome::err("cmd is required") };
    let timeout = Duration::from_millis(args["timeout"].as_u64().unwrap_or(120_000).min(600_000));
    run_shell(command, ctx.cwd, timeout, &ctx.stop, ctx.output)
}

/// Runs `command` through the user's shell, streaming output; stops on timeout or `stop`.
pub fn run_shell(command: &str, cwd: &Path, timeout: Duration, stop: &AtomicBool, output: &dyn Fn(&str)) -> Outcome {
    let mut cmd = shell_command(command, cwd);
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return Outcome::err(format!("couldn't run the command: {e}")),
    };
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    for stream in [child.stdout.take().map(|s| Box::new(s) as Box<dyn Read + Send>), child.stderr.take().map(|s| Box::new(s) as Box<dyn Read + Send>)]
        .into_iter()
        .flatten()
    {
        let tx = tx.clone();
        std::thread::spawn(move || {
            let mut stream = stream;
            let mut buf = [0u8; 4096];
            while let Ok(n) = stream.read(&mut buf) {
                if n == 0 || tx.send(String::from_utf8_lossy(&buf[..n]).into_owned()).is_err() {
                    break;
                }
            }
        });
    }
    drop(tx);
    let started = Instant::now();
    let mut text = String::new();
    let mut ended = None;
    loop {
        match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(chunk) => {
                output(&chunk);
                if text.len() < MAX_OUTPUT * 4 {
                    text.push_str(&chunk);
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            Err(_) => {}
        }
        if stop.load(Ordering::Relaxed) {
            ended = Some("interrupted by the user");
            break;
        }
        if started.elapsed() > timeout {
            ended = Some("timed out");
            break;
        }
    }
    if ended.is_some() {
        kill_tree(&mut child);
    }
    let status = child.wait().ok();
    let mut out = clip(&strip_ansi(&text), MAX_OUTPUT);
    if let Some(why) = ended {
        out.push_str(&format!("\n[command {why} after {:.0}s]", started.elapsed().as_secs_f32()));
        return Outcome::err(out);
    }
    let code = status.and_then(|s| s.code()).unwrap_or(-1);
    if out.trim().is_empty() {
        out = "(no output)".into();
    }
    if code != 0 {
        out.push_str(&format!("\n[exit code {code}]"));
    }
    Outcome { ok: code == 0, output: out, diff: None, todos: None }
}

/// `command` for the user's shell in its own process group, so `kill_tree` gets all of it.
pub fn shell_command(command: &str, cwd: &Path) -> Command {
    let (sh, flags) = shell();
    let mut cmd = Command::new(sh);
    cmd.args(flags).current_dir(cwd);
    if cfg!(windows) { cmd.args(powershell_args(&powershell_script(command))) } else { cmd.arg(command) };
    cmd.env("PATH", crate::env::path_var()).env("TERM", "dumb").env("NO_COLOR", "1").env("GIT_PAGER", "cat").env("PAGER", "cat");
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    crate::env::no_window(&mut cmd);
    cmd
}

pub fn kill_tree(child: &mut std::process::Child) {
    #[cfg(unix)]
    unsafe {
        libc::kill(-(child.id() as i32), libc::SIGKILL);
    }
    // PowerShell's children (node, cargo…) outlive it otherwise.
    #[cfg(windows)]
    {
        let mut kill = Command::new("taskkill");
        kill.args(["/T", "/F", "/PID", &child.id().to_string()]).stdout(Stdio::null()).stderr(Stdio::null());
        let _ = crate::env::no_window(&mut kill).status();
    }
    let _ = child.kill();
}

pub fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            if chars.peek() == Some(&'[') {
                chars.next();
                for c in chars.by_ref() {
                    if ('@'..='~').contains(&c) {
                        break;
                    }
                }
            } else if chars.peek() == Some(&']') {
                for c in chars.by_ref() {
                    if c == '\x07' {
                        break;
                    }
                }
            } else {
                chars.next();
            }
        } else if c == '\r' {
            continue;
        } else {
            out.push(c);
        }
    }
    out
}

fn grep(cwd: &Path, args: &Value) -> Outcome {
    let Some(pattern) = args["pattern"].as_str() else { return Outcome::err("pattern is required") };
    let path = args["path"].as_str().map(|p| resolve(cwd, p)).unwrap_or_else(|| cwd.to_path_buf());
    let mut cmd;
    if crate::env::find_command("rg").is_some() {
        cmd = Command::new("rg");
        cmd.args(["-n", "--no-heading", "--color", "never", "-M", "200", "--max-count", "30"]);
        if args["i"].as_bool() == Some(true) {
            cmd.arg("-i");
        }
        if let Some(g) = args["glob"].as_str() {
            cmd.args(["-g", g]);
        }
        cmd.arg("-e").arg(pattern).arg(&path);
    } else {
        cmd = Command::new("grep");
        cmd.args(["-rnE", "--exclude-dir=.git", "--exclude-dir=node_modules", "--exclude-dir=target"]);
        if args["i"].as_bool() == Some(true) {
            cmd.arg("-i");
        }
        if let Some(g) = args["glob"].as_str() {
            cmd.arg(format!("--include={g}"));
        }
        cmd.arg("-e").arg(pattern).arg(&path);
    }
    cmd.current_dir(cwd).env("PATH", crate::env::path_var());
    crate::env::no_window(&mut cmd);
    match cmd.output() {
        Ok(out) => {
            let text = String::from_utf8_lossy(&out.stdout);
            let base = format!("{}/", cwd.display());
            let lines: Vec<String> = text.lines().take(GREP_MAX).map(|l| l.strip_prefix(&base).or_else(|| l.strip_prefix("./")).unwrap_or(l).to_string()).collect();
            if lines.is_empty() {
                let err = String::from_utf8_lossy(&out.stderr);
                if !err.trim().is_empty() && !out.status.success() && out.status.code() != Some(1) {
                    return Outcome::err(err.trim().to_string());
                }
                return Outcome::ok("No matches.");
            }
            let more = text.lines().count().saturating_sub(GREP_MAX);
            let mut s = lines.join("\n");
            if more > 0 {
                s.push_str(&format!("\n… {more} more matches"));
            }
            Outcome::ok(s)
        }
        Err(e) => Outcome::err(format!("search failed: {e}")),
    }
}

/// Files under `root` (respecting .gitignore), as paths relative to it.
pub fn walk(root: &Path, limit: usize) -> Vec<(PathBuf, std::time::SystemTime)> {
    let mut out = Vec::new();
    for entry in ignore::WalkBuilder::new(root).hidden(true).build().flatten() {
        if entry.file_type().is_some_and(|t| t.is_file()) {
            let modified = entry.metadata().ok().and_then(|m| m.modified().ok()).unwrap_or(std::time::UNIX_EPOCH);
            out.push((entry.path().strip_prefix(root).unwrap_or(entry.path()).to_path_buf(), modified));
            if out.len() >= limit {
                break;
            }
        }
    }
    out
}

fn glob(cwd: &Path, args: &Value) -> Outcome {
    let Some(pattern) = args["pattern"].as_str() else { return Outcome::err("pattern is required") };
    let root = args["path"].as_str().map(|p| resolve(cwd, p)).unwrap_or_else(|| cwd.to_path_buf());
    let matcher = match globset::GlobBuilder::new(pattern).literal_separator(false).build() {
        Ok(g) => g.compile_matcher(),
        Err(e) => return Outcome::err(format!("bad pattern: {e}")),
    };
    let mut hits: Vec<_> = walk(&root, 200_000).into_iter().filter(|(p, _)| matcher.is_match(p)).collect();
    hits.sort_by(|a, b| b.1.cmp(&a.1));
    let total = hits.len();
    let mut s: Vec<String> = hits.iter().take(GLOB_MAX).map(|(p, _)| p.to_string_lossy().replace('\\', "/")).collect();
    if total > GLOB_MAX {
        s.push(format!("… {} more", total - GLOB_MAX));
    }
    Outcome::ok(if s.is_empty() { "No files matched.".into() } else { s.join("\n") })
}

const GREP_MAX: usize = 100;
const GLOB_MAX: usize = 100;

fn ls(path: &Path) -> Outcome {
    let entries = match std::fs::read_dir(&path) {
        Ok(e) => e,
        Err(e) => return Outcome::err(format!("couldn't list {}: {e}", path.display())),
    };
    let mut names: Vec<String> = entries
        .flatten()
        .map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            if e.file_type().is_ok_and(|t| t.is_dir()) { format!("{name}/") } else { name }
        })
        .collect();
    names.sort();
    Outcome::ok(if names.is_empty() { "(empty)".into() } else { names.join("\n") })
}

fn web_fetch(args: &Value) -> Outcome {
    let Some(url) = args["url"].as_str() else { return Outcome::err("url is required") };
    let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(30))).http_status_as_error(false).build().into();
    let mut res = match agent.get(url).header("User-Agent", "winfer-code").call() {
        Ok(r) => r,
        Err(e) => return Outcome::err(format!("couldn't fetch {url}: {e}")),
    };
    let status = res.status().as_u16();
    let html = res.headers().get("content-type").and_then(|v| v.to_str().ok()).is_some_and(|t| t.contains("html"));
    let body = res.body_mut().with_config().limit(5 * 1024 * 1024).read_to_string().unwrap_or_default();
    let text = if html { html_text(&body) } else { body };
    let out = clip(&text, 15_000);
    if (200..300).contains(&status) { Outcome::ok(out) } else { Outcome::err(format!("HTTP {status}\n{out}")) }
}

/// Rough HTML → text: drops scripts/styles/tags, keeps line structure.
fn html_text(html: &str) -> String {
    let mut out = String::new();
    let mut rest = html;
    while let Some(start) = rest.find('<') {
        out.push_str(&rest[..start]);
        rest = &rest[start..];
        let lower: String = rest.chars().take(10).collect::<String>().to_ascii_lowercase();
        let skip_to = if lower.starts_with("<script") {
            Some("</script>")
        } else if lower.starts_with("<style") {
            Some("</style>")
        } else {
            None
        };
        if let Some(end) = skip_to {
            match rest.to_ascii_lowercase().find(end) {
                Some(i) => rest = &rest[i + end.len()..],
                None => rest = "",
            }
            continue;
        }
        let tag_end = rest.find('>').map(|i| i + 1).unwrap_or(rest.len());
        let tag = rest[..tag_end].to_ascii_lowercase();
        if ["<p", "<br", "<div", "<li", "<h1", "<h2", "<h3", "<h4", "<tr", "</p", "</div", "</h"].iter().any(|t| tag.starts_with(t)) {
            out.push('\n');
        }
        rest = &rest[tag_end..];
    }
    out.push_str(rest);
    let out = out.replace("&nbsp;", " ").replace("&amp;", "&").replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&#39;", "'");
    let mut lines: Vec<&str> = Vec::new();
    for line in out.lines().map(str::trim) {
        if line.is_empty() && lines.last().is_some_and(|l| l.is_empty()) {
            continue;
        }
        lines.push(line);
    }
    lines.join("\n").trim().to_string()
}

/// An image file as (media type, base64), for model input.
pub fn image_base64(path: &Path) -> Option<(&'static str, String)> {
    let media = match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        _ => return None,
    };
    let bytes = std::fs::read(path).ok()?;
    Some((media, base64::engine::general_purpose::STANDARD.encode(bytes)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn powershell_gets_the_script_base64_encoded() {
        let script = "Get-Content -LiteralPath 'C:\\a b\\[x].txt' | head -n 2; \"é\"";
        let [flag, arg] = powershell_args(script);
        assert_eq!(flag, "-EncodedCommand");
        let bytes = base64::engine::general_purpose::STANDARD.decode(arg).unwrap();
        let units: Vec<u16> = bytes.chunks(2).map(|b| u16::from_le_bytes([b[0], b[1]])).collect();
        assert_eq!(String::from_utf16(&units).unwrap(), script);
        assert_eq!(powershell_args(&"x".repeat(20_000))[0], "-Command");
    }

    #[cfg(windows)]
    #[test]
    fn powershell_reads_awkward_paths_and_accepts_head() {
        let d = std::env::temp_dir().join(format!("winfer-pwsh-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("a 'b' [c].txt"), "één\ntwo\nthree\n").unwrap();
        let o = run_shell(
            "Get-Content -LiteralPath \"a 'b' [c].txt\" | head -n 2",
            &d,
            Duration::from_secs(60),
            &AtomicBool::new(false),
            &|_| {},
        );
        let _ = std::fs::remove_dir_all(&d);
        assert!(o.ok, "{}", o.output);
        assert!(o.output.contains("één") && o.output.contains("two") && !o.output.contains("three"), "{}", o.output);
    }

    fn ctx_run(name: &str, args: Value, dir: &Path) -> (Outcome, HashMap<PathBuf, Option<String>>) {
        ctx_run_with(name, args, dir, &ReadCache::default())
    }

    fn ctx_run_with(name: &str, args: Value, dir: &Path, reads: &ReadCache) -> (Outcome, HashMap<PathBuf, Option<String>>) {
        let mut snaps = HashMap::new();
        let out = {
            let mut ctx = Ctx { cwd: dir, stop: Arc::new(AtomicBool::new(false)), output: &|_| {}, call_id: "call", bg_owner: "", snapshots: &mut snaps, reads };
            run(name, &args, &mut ctx)
        };
        (out, snaps)
    }

    #[test]
    fn tool_definitions_stay_lean() {
        let size = serde_json::to_string(&definitions()).unwrap().len();
        // ~2.27 KB with bg (~1.87 KB without; was ~3.5 KB), ~2.31 KB with Windows' longer shell
        // description; most of it is JSON schema boilerplate.
        assert!(size < 2350, "tool definitions grew to {size} bytes");
    }

    #[test]
    fn unchanged_rereads_are_stubbed_until_an_edit() {
        let d = tempdir("dedupe");
        let reads = ReadCache::default();
        std::fs::write(d.join("a.txt"), "x\n").unwrap();
        let (o, _) = ctx_run_with("read", json!({ "path": "a.txt" }), &d, &reads);
        assert_eq!(o.output, "1\tx\n");
        let (o, _) = ctx_run_with("read", json!({ "path": "a.txt" }), &d, &reads);
        assert!(o.output.contains("unchanged"));
        let (o, _) = ctx_run_with("edit", json!({ "path": "a.txt", "old": "x", "new": "y" }), &d, &reads);
        assert!(o.ok);
        let (o, _) = ctx_run_with("read", json!({ "path": "a.txt" }), &d, &reads);
        assert_eq!(o.output, "1\ty\n");
    }

    #[test]
    fn old_tool_and_argument_names_still_work() {
        let d = tempdir("alias");
        std::fs::write(d.join("a.txt"), "x").unwrap();
        let (o, _) = ctx_run("ls", json!({ "path": "." }), &d);
        assert_eq!(o.output, "a.txt");
        assert_eq!(risk("todo_write", &Value::Null), Risk::Read);
        assert_eq!(risk("bg", &json!({ "action": "start", "cmd": "npm run dev" })), Risk::Exec);
        assert_eq!(risk("bg", &json!({ "action": "read", "id": "bg1" })), Risk::Read);
        assert_eq!(risk("bg", &json!({ "cmd": "npm run dev" })), Risk::Exec);
        assert_eq!(title("bash", &json!({ "command": "ls -la" })), "ls -la");
    }

    fn tempdir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("winfer-tools-{name}-{}", rand::random::<u32>()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn write_edit_read_roundtrip() {
        let d = tempdir("rw");
        let (o, snaps) = ctx_run("write", json!({ "path": "a.txt", "content": "one\ntwo\n" }), &d);
        assert!(o.ok && o.output.starts_with("Created"));
        assert_eq!(snaps.get(&d.join("a.txt")), Some(&None));
        let (o, _) = ctx_run("edit", json!({ "path": "a.txt", "old_string": "two", "new_string": "2" }), &d);
        assert!(o.ok, "{}", o.output);
        assert!(o.diff.unwrap().contains("+2"));
        let (o, _) = ctx_run("edit", json!({ "path": "a.txt", "old_string": "nope", "new_string": "x" }), &d);
        assert!(!o.ok);
        let (o, _) = ctx_run("read", json!({ "path": "a.txt" }), &d);
        assert_eq!(o.output, "1\tone\n2\t2\n");
    }

    #[test]
    fn edit_needs_a_unique_match() {
        let d = tempdir("uniq");
        std::fs::write(d.join("b.txt"), "x x").unwrap();
        let (o, _) = ctx_run("edit", json!({ "path": "b.txt", "old_string": "x", "new_string": "y" }), &d);
        assert!(!o.ok && o.output.contains("2 times"));
        let (o, _) = ctx_run("edit", json!({ "path": "b.txt", "old_string": "x", "new_string": "y", "replace_all": true }), &d);
        assert!(o.ok);
        assert_eq!(std::fs::read_to_string(d.join("b.txt")).unwrap(), "y y");
    }

    #[cfg(unix)]
    #[test]
    fn bash_reports_exit_codes() {
        let d = tempdir("sh");
        let (o, _) = ctx_run("bash", json!({ "cmd": "echo hi; exit 3" }), &d);
        assert!(!o.ok);
        assert!(o.output.contains("hi") && o.output.contains("exit code 3"));
    }

    #[test]
    fn glob_finds_files() {
        let d = tempdir("glob");
        std::fs::create_dir_all(d.join("src")).unwrap();
        std::fs::write(d.join("src/x.rs"), "").unwrap();
        std::fs::write(d.join("y.ts"), "").unwrap();
        let (o, _) = ctx_run("glob", json!({ "pattern": "**/*.rs" }), &d);
        assert_eq!(o.output, "src/x.rs");
    }

    #[test]
    fn clips_long_output_in_the_middle() {
        let s = "a".repeat(100) + &"b".repeat(100);
        let c = clip(&s, 50);
        assert!(c.starts_with("aaaa") && c.ends_with("bbbb") && c.contains("omitted"));
    }

    #[test]
    fn html_to_text() {
        assert_eq!(html_text("<html><style>x{}</style><p>Hi &amp; bye</p><script>1</script><div>end</div>"), "Hi & bye\n\nend");
    }
}
