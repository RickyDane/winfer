//! What backends tell the TUI ([`Ev`]) and what the TUI asks of them ([`Cmd`]). Every backend
//! (ChatGPT, the Claude Code bridge, the Codex bridge) speaks only these, so the UI never knows
//! which one it is talking to.

use std::sync::mpsc::Sender;

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone)]
pub enum Ev {
    /// The backend's own session id (resumable with `--resume`).
    Session(String),
    /// The model actually answering (bridges resolve aliases).
    Model(String),
    TextDelta { id: String, text: String },
    /// The full text of a message, when the backend sends it whole (replaces the deltas).
    Text { id: String, text: String },
    ReasoningDelta { id: String, text: String },
    ToolStart { id: String, name: String, title: String, input: Value },
    ToolOutput { id: String, chunk: String },
    /// `diff` is a unified diff for file edits.
    ToolEnd { id: String, ok: bool, output: String, diff: Option<String> },
    Approval(Approval),
    /// Progress of a task the backend runs on its own (Claude Code's subagents, which keep
    /// running in the background after their tool call returns).
    Task(TaskEv),
    Todos(Vec<Todo>),
    /// Skills the user can invoke with `/name` (the backend's own discovery).
    Skills(Vec<Skill>),
    Usage(Usage),
    Notice(String),
    Error(String),
    TurnStart,
    TurnDone,
    /// The backend process ended; the session can't continue.
    Exited(String),
}

/// One update about a background task; unset fields are unchanged.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TaskEv {
    pub id: String,
    /// The tool call that started it (an Agent card), if any.
    pub tool_id: Option<String>,
    pub title: Option<String>,
    /// Set for subagents (their type, e.g. "general-purpose"); unset for background shells.
    pub agent: Option<String>,
    pub tools: Option<u64>,
    pub last: Option<String>,
    /// "running", or how it ended: "completed", "failed", "stopped", "killed".
    pub status: Option<String>,
    /// File receiving a background shell's output.
    pub output_file: Option<String>,
    pub exit_code: Option<i32>,
}

#[derive(Debug, Clone)]
pub struct Approval {
    pub id: String,
    pub tool: String,
    pub title: String,
    pub detail: String,
    pub diff: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Allow,
    /// Allow this kind of call for the rest of the session.
    Always,
    Deny,
}

#[derive(Debug, Clone, Default)]
pub struct Usage {
    /// Tokens in the context right now (last request's input).
    pub context: u64,
    pub window: u64,
    pub input: u64,
    pub output: u64,
    pub cached: u64,
    pub cost: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Todo {
    pub content: String,
    /// "pending" | "in_progress" | "completed"
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelInfo {
    /// `backend/model`, e.g. `claude/opus`.
    pub spec: String,
    pub label: String,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skill {
    pub name: String,
    pub description: String,
}

/// One user turn: text plus attached image files.
#[derive(Debug, Clone, Default)]
pub struct UserMsg {
    pub text: String,
    pub images: Vec<std::path::PathBuf>,
    /// A skill the user invoked explicitly (`/name args`); `text` holds the args.
    pub skill: Option<String>,
}

#[derive(Debug)]
pub enum Cmd {
    Send(UserMsg),
    Interrupt,
    Approve(String, Decision),
    SetModel(String),
    SetEffort(Option<String>),
    SetMode(Mode),
    Compact,
    /// Context fill (%) for trimming and for full compaction; 0 = off.
    SetCompaction { trim: u8, compact: u8 },
    /// Undo the file changes of the last turn (ChatGPT, and Claude Code via its file checkpoints).
    Undo,
    /// Stop one background task (a subagent), by its `TaskEv` id.
    StopTask(String),
    Shutdown,
}

/// Permission mode, cycled with shift+tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Mode {
    /// Ask before edits, commands and network.
    #[default]
    Ask,
    /// File edits go through; commands still ask.
    AutoEdit,
    /// Nothing asks.
    Yolo,
    /// Read-only: plan without changing anything.
    Plan,
}

impl Mode {
    pub fn next(self) -> Self {
        match self {
            Mode::Ask => Mode::AutoEdit,
            Mode::AutoEdit => Mode::Plan,
            Mode::Plan => Mode::Yolo,
            Mode::Yolo => Mode::Ask,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Mode::Ask => "ask",
            Mode::AutoEdit => "auto-edit",
            Mode::Yolo => "yolo",
            Mode::Plan => "plan",
        }
    }
}

pub type Events = Sender<Ev>;
