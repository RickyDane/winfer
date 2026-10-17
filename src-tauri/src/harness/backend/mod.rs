//! The three backends behind `winfer code`. Each runs on its own thread and owns its process
//! or HTTP stream; the TUI talks to it through a [`Handle`].

pub mod chatgpt;
pub mod claude;
pub mod codex;

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
}

impl Kind {
    pub fn id(self) -> &'static str {
        match self {
            Kind::ChatGpt => "chatgpt",
            Kind::Claude => "claude",
            Kind::Codex => "codex",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Kind::ChatGpt => "ChatGPT",
            Kind::Claude => "Claude Code",
            Kind::Codex => "Codex",
        }
    }
    pub fn from_id(id: &str) -> Option<Kind> {
        match id {
            "chatgpt" | "openai" => Some(Kind::ChatGpt),
            "claude" | "anthropic" => Some(Kind::Claude),
            "codex" => Some(Kind::Codex),
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
    /// instead of its full default setup (see [`LEAN_PROMPT`]). Codex stays as it is.
    pub lean: bool,
    /// The skills Claude Code loads in lean mode.
    pub skills: crate::harness::claude_skills::LeanSkills,
}

/// The system prompt for Claude Code in lean mode: the essentials of its long default (which runs
/// to thousands of tokens per request). It's the same text every time, so the prompt cache keeps
/// it warm across sessions; where the agent is comes after it.
pub const LEAN_PROMPT: &str = "You are a coding agent working in the user's repository from their terminal. Work autonomously: explore with search and reads, read a file before editing it, make focused changes that match the surrounding code, and verify with the project's tests or a quick run. Put independent tool calls in one response. Don't stop to ask unless you're blocked or the request is ambiguous. Reply tersely in Markdown and reference code as path:line.";

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

pub fn start(kind: Kind, opts: Opts, events: Events) -> Handle {
    let (tx, rx) = channel();
    let me = tx.clone();
    std::thread::spawn(move || match kind {
        Kind::ChatGpt => chatgpt::run(opts, rx, events),
        Kind::Claude => claude::run(opts, rx, me, events),
        Kind::Codex => codex::run(opts, rx, me, events),
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
    let (tx, rx) = channel::<Vec<ModelInfo>>();
    let a = tx.clone();
    std::thread::spawn(move || {
        let _ = a.send(chatgpt::models());
    });
    let b = tx.clone();
    std::thread::spawn(move || {
        let _ = b.send(codex::models());
    });
    let _ = tx.send(claude::models());
    drop(tx);
    let mut all: Vec<ModelInfo> = rx.iter().flatten().collect();
    let order = |m: &ModelInfo| match m.spec.split('/').next() {
        Some("chatgpt") => 0,
        Some("claude") => 1,
        _ => 2,
    };
    all.sort_by_key(order);
    all
}

/// Waits for the next message, giving up once the receiver closes.
pub fn recv(rx: &Receiver<In>) -> Option<In> {
    rx.recv().ok()
}
