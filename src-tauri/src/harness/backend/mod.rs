//! The backends behind `winfer code`. Each runs on its own thread and owns its process
//! or HTTP stream; the TUI talks to it through a [`Handle`].

pub mod agy;
pub mod chatgpt;
pub mod claude;
pub mod codex;
pub mod cursor;
pub mod microcompact;
pub mod opencode;
pub mod pi;

use std::io::{BufRead, BufReader, Read};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};

use serde_json::Value;

use super::event::{Cmd, Events, Mode, ModelInfo};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    ChatGpt,
    Claude,
    Codex,
    /// Google Antigravity's CLI (`agy`).
    Agy,
    /// Cursor's CLI (`cursor-agent`).
    Cursor,
    OpenCode,
    /// The pi coding agent (`pi`).
    Pi,
}

impl Kind {
    pub fn id(self) -> &'static str {
        match self {
            Kind::ChatGpt => "chatgpt",
            Kind::Claude => "claude",
            Kind::Codex => "codex",
            Kind::Agy => "agy",
            Kind::Cursor => "cursor",
            Kind::OpenCode => "opencode",
            Kind::Pi => "pi",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Kind::ChatGpt => "ChatGPT",
            Kind::Claude => "Claude Code",
            Kind::Codex => "Codex",
            Kind::Agy => "Antigravity",
            Kind::Cursor => "Cursor",
            Kind::OpenCode => "OpenCode",
            Kind::Pi => "Pi",
        }
    }
    pub fn from_id(id: &str) -> Option<Kind> {
        match id {
            "chatgpt" | "openai" => Some(Kind::ChatGpt),
            "claude" | "anthropic" => Some(Kind::Claude),
            "codex" => Some(Kind::Codex),
            "agy" | "antigravity" => Some(Kind::Agy),
            "cursor" | "cursor-agent" => Some(Kind::Cursor),
            "opencode" => Some(Kind::OpenCode),
            "pi" => Some(Kind::Pi),
            _ => None,
        }
    }
}

/// Splits `backend/model` (or a bare model, guessed from its name) into its parts.
pub fn parse_spec(spec: &str) -> (Kind, String) {
    if let Some((backend, model)) = spec.split_once('/') {
        if let Some(kind) = Kind::from_id(backend) {
            return (kind, model.to_string());
        }
    }
    let lower = spec.to_ascii_lowercase();
    let claude = ["claude", "opus", "sonnet", "haiku", "fable"].iter().any(|m| lower.contains(m));
    if claude {
        (Kind::Claude, spec.to_string())
    } else if spec.is_empty() {
        (default_kind(), String::new())
    } else if crate::chatgpt::signed_in() {
        (Kind::ChatGpt, spec.to_string())
    } else {
        (Kind::Codex, spec.to_string())
    }
}

/// The backend to use when nothing is configured: the ChatGPT account, else whichever CLI exists.
pub fn default_kind() -> Kind {
    if crate::chatgpt::signed_in() {
        Kind::ChatGpt
    } else if crate::env::find_command("claude").is_some() {
        Kind::Claude
    } else {
        Kind::Codex
    }
}

pub struct Opts {
    pub cwd: PathBuf,
    pub model: String,
    pub effort: Option<String>,
    pub mode: Mode,
    /// The backend's session to continue.
    pub resume: Option<String>,
    /// Extra system prompt (winfer's `systemPromptArgs`).
    pub system: Option<String>,
    pub mcp: serde_json::Map<String, Value>,
    pub allowed_commands: Vec<String>,
    /// Context fill (%) that triggers trimming / full compaction; 0 = off.
    pub trim_at: u8,
    pub compact_at: u8,
    /// Start Claude Code lean: a short system prompt and only the tools a coding task needs,
    /// instead of its full default setup (see [`LEAN_PROMPT`]). Codex goes lean with its own
    /// setting (`Config::lean_codex`, see codex.rs).
    pub lean: bool,
    /// The skills Claude Code loads in lean mode.
    pub skills: crate::harness::claude_skills::LeanSkills,
    /// Who the CLI's `bg` tasks belong to (set by [`start`]); the backend tells `bg` its mode.
    pub bg_owner: String,
}

/// The system prompt for Claude Code in lean mode: the essentials of its long default (which runs
/// to thousands of tokens per request). It's the same text every time, so the prompt cache keeps
/// it warm across sessions; where the agent is comes after it.
pub const LEAN_PROMPT: &str = "You are a coding agent working in the user's repository from their terminal. Work autonomously: explore with search and reads, read a file before editing it, make focused changes that match the surrounding code, and verify with the project's tests or a quick run. Put independent tool calls in one response. Don't stop to ask unless you're blocked or the request is ambiguous. Reply tersely in Markdown and reference code as path:line.";

/// [`LEAN_PROMPT`] for a project indexed with codegraph (its MCP server attached, see
/// `mcp::codegraph_server`): explore through the graph instead of "search and reads", which
/// otherwise contradicts codegraph's own instructions and has the agent do both.
pub const LEAN_CODEGRAPH_PROMPT: &str = "You are a coding agent working in the user's repository from their terminal. Work autonomously. To find or understand code, call codegraph_explore first, naming the 2–4 symbols or files at the heart of the question (for a flow, its two ends and the hop you're unsure of), not every hop: too many names thin out what it returns. Treat the source it returns as already read and don't re-check it with grep. Use grep and reads only for what it didn't cover: hops dispatched by a string (command ids, events, IPC channels), configs, docs, and exact lists of every use of a name. Read a file before editing it, make focused changes that match the surrounding code, and verify with the project's tests or a quick run. Put independent tool calls in one response. Don't stop to ask unless you're blocked or the request is ambiguous. Reply tersely in Markdown and reference code as path:line.";

/// Lean Codex's short system prompt (Settings → winfer code → Short Codex prompt), in place of
/// Codex's built-in one; its tools describe themselves.
pub const CODEX_LEAN_PROMPT: &str = "You are a coding agent working in the user's repository from their terminal. Work autonomously: explore with search and reads, read a file before editing it, make focused changes that match the surrounding code, and verify with the project's tests or a quick run. Edit files with apply_patch. Put independent tool calls in one response. Don't stop to ask unless you're blocked or the request is ambiguous. Reply tersely in Markdown and reference code as path:line.";

/// Where the agent is (Claude Code drops it with its default prompt); goes after [`LEAN_PROMPT`]
/// so the cached prefix stays the same.
pub fn environment(cwd: &std::path::Path) -> String {
    let branch = std::process::Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(cwd)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| format!(" · git branch {}", String::from_utf8_lossy(&o.stdout).trim()))
        .unwrap_or_default();
    format!(
        "Environment: cwd {} · {} · {}{branch}",
        cwd.display(),
        std::env::consts::OS,
        chatgpt::today(),
    )
}

/// What a backend thread receives: the TUI's commands and its own process's output.
pub enum In {
    Cmd(Cmd),
    Line(String),
    Closed,
}

#[derive(Clone)]
pub struct Handle {
    tx: Sender<In>,
}

impl Handle {
    pub fn send(&self, cmd: Cmd) {
        let _ = self.tx.send(In::Cmd(cmd));
    }
}

pub fn start(kind: Kind, mut opts: Opts, events: Events) -> Handle {
    let (tx, rx) = channel();
    let me = tx.clone();
    std::thread::spawn(move || match kind {
        Kind::ChatGpt => chatgpt::run(opts, rx, events),
        _ => {
            // The CLIs reach `bg` through winfer's MCP server; send those calls here, so the
            // tasks show in winfer code's task list and end with this backend.
            let owner = format!("code-{:016x}", rand::random::<u64>());
            if crate::harness::tools::bg_enabled() {
                crate::harness::bg::route_mcp(&mut opts.mcp, &owner);
                opts.bg_owner = owner.clone();
                let sink = events.clone();
                let mine = owner.clone();
                crate::harness::bg::listen(move |u| {
                    if u.owner() != mine {
                        return;
                    }
                    let (task, note) = u.task_event();
                    let _ = sink.send(super::event::Ev::Task(task));
                    if let Some(note) = note {
                        let _ = sink.send(super::event::Ev::BgNote(note));
                    }
                });
            }
            match kind {
                Kind::Claude => claude::run(opts, rx, me, events),
                Kind::Agy => agy::run(opts, rx, me, events),
                Kind::Cursor => cursor::run(opts, rx, me, events),
                Kind::OpenCode => opencode::run(opts, rx, me, events),
                Kind::Pi => pi::run(opts, rx, me, events),
                _ => codex::run(opts, rx, me, events),
            }
            crate::harness::bg::stop_owner(&owner);
        }
    });
    Handle { tx }
}

/// A child process speaking JSON lines: stdout lines arrive on the backend's channel as
/// [`In::Line`], stderr is kept for error messages.
pub struct LineProcess {
    pub child: Child,
    pub stdin: ChildStdin,
    pub stderr: Arc<Mutex<String>>,
}

impl LineProcess {
    pub fn spawn(program: &str, args: &[String], envs: &[(&str, &str)], cwd: &std::path::Path, tx: Sender<In>) -> Result<Self, String> {
        let launcher = crate::env::launcher(program).ok_or_else(|| format!("`{program}` isn't installed (not on PATH)"))?;
        let mut cmd = Command::new(&launcher.program);
        cmd.args(&launcher.args).args(args).envs(envs.iter().copied()).current_dir(cwd);
        cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        crate::env::no_window(&mut cmd);
        let mut child = cmd.spawn().map_err(|e| format!("couldn't start {program}: {e}"))?;
        let stdin = child.stdin.take().ok_or("no stdin")?;
        let stdout = child.stdout.take().ok_or("no stdout")?;
        let mut err = child.stderr.take().ok_or("no stderr")?;
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if tx.send(In::Line(line)).is_err() {
                    return;
                }
            }
            let _ = tx.send(In::Closed);
        });
        let stderr = Arc::new(Mutex::new(String::new()));
        let sink = stderr.clone();
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            while let Ok(n) = err.read(&mut buf) {
                if n == 0 {
                    break;
                }
                let mut s = sink.lock().unwrap();
                s.push_str(&String::from_utf8_lossy(&buf[..n]));
                let extra = s.len().saturating_sub(8000);
                if extra > 0 {
                    let cut = s.ceil_char_boundary(extra);
                    s.drain(..cut);
                }
            }
        });
        Ok(LineProcess { child, stdin, stderr })
    }

    pub fn write(&mut self, value: &Value) -> Result<(), String> {
        use std::io::Write;
        let mut line = value.to_string();
        line.push('\n');
        self.stdin.write_all(line.as_bytes()).and_then(|_| self.stdin.flush()).map_err(|e| e.to_string())
    }

    /// Why the process ended, from the tail of its stderr.
    pub fn exit_reason(&mut self) -> String {
        let status = self.child.try_wait().ok().flatten();
        let err = self.stderr.lock().unwrap();
        let tail: Vec<&str> = err.lines().filter(|l| !l.trim().is_empty()).rev().take(6).collect();
        let tail: Vec<&str> = tail.into_iter().rev().collect();
        match (status, tail.is_empty()) {
            (_, false) => tail.join("\n"),
            (Some(s), true) => format!("exited ({s})"),
            (None, true) => "stopped responding".into(),
        }
    }
}

impl Drop for LineProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Every model the picker offers, from all backends that are available (slow: run off-thread).
pub fn all_models() -> Vec<ModelInfo> {
    // Each backend lists its own (asking its CLI, so some take seconds); one that isn't
    // installed lists nothing, so it doesn't show in the picker.
    let listers: [fn() -> Vec<ModelInfo>; 7] = [chatgpt::models, claude::models, codex::models, agy::models, cursor::models, opencode::models, pi::models];
    let (tx, rx) = channel::<Vec<ModelInfo>>();
    for list in listers {
        let tx = tx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(list());
        });
    }
    drop(tx);
    let mut all: Vec<ModelInfo> = rx.iter().flatten().collect();
    let order = |m: &ModelInfo| match m.spec.split('/').next() {
        Some("chatgpt") => 0,
        Some("claude") => 1,
        Some("codex") => 2,
        Some("agy") => 3,
        Some("cursor") => 4,
        Some("opencode") => 5,
        _ => 6,
    };
    all.sort_by_key(order);
    all
}

/// After killing a CLI for a restart: waits (briefly) for its output to end, holding on to the
/// commands that come meanwhile. Returns false if it didn't end in time, so its late
/// [`In::Closed`] is to be ignored.
pub fn drain_closed(rx: &Receiver<In>, held: &mut Vec<Cmd>) -> bool {
    let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        match rx.recv_timeout(until.saturating_duration_since(std::time::Instant::now())) {
            Ok(In::Closed) => return true,
            Ok(In::Cmd(cmd)) => held.push(cmd),
            Ok(In::Line(_)) => {}
            Err(_) => return false,
        }
    }
}

/// Waits for the next message, giving up once the receiver closes.
pub fn recv(rx: &Receiver<In>) -> Option<In> {
    rx.recv().ok()
}
