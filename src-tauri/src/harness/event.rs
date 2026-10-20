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
    /// The backend switched permission mode on its own (e.g. leaving plan mode once the plan is approved).
    Mode(Mode),
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
    /// Answer to [`Cmd::Undo`]: the last user turn is gone from the conversation, with the files
    /// the backend reverted itself (when asked to); or why it couldn't be undone.
    Undone(Result<Vec<String>, String>),
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

#[derive(Debug, Clone, Default)]
pub struct Approval {
    pub id: String,
    pub tool: String,
    pub title: String,
    pub detail: String,
    pub diff: Option<String>,
    /// Questions the agent asks the user (Claude Code's AskUserQuestion); answered with
    /// [`Cmd::Answer`] instead of a [`Decision`] (declining still goes through `Approve`).
    pub questions: Vec<Question>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Question {
    /// Short label, e.g. "Auth method".
    pub header: String,
    pub question: String,
    /// (label, description)
    pub options: Vec<(String, String)>,
    /// Whether more than one option can be picked.
    pub multi: bool,
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
    /// Session totals.
    pub output: u64,
    pub cached: u64,
    pub cost: Option<f64>,
    /// Output tokens of the running turn (or the last one, between turns), partly estimated
    /// while it streams.
    pub turn: u64,
}

/// Counts a turn's output tokens as it runs. Backends report exact counts once per model
/// request; between those, streamed text counts at about 4 characters a token.
#[derive(Debug, Default)]
pub struct TurnTokens {
    exact: u64,
    chars: u64,
    shown: u64,
}

impl TurnTokens {
    pub fn now(&self) -> u64 {
        self.exact + self.chars / 4
    }

    /// The backend's exact count for the turn so far; the estimate it covers is dropped.
    pub fn exact(&mut self, tokens: u64) {
        self.exact = tokens;
        self.chars = 0;
        self.shown = self.now();
    }

    /// Streamed text; true when the count moved enough (~25 tokens) to be worth sending.
    pub fn streamed(&mut self, text: &str) -> bool {
        self.chars += text.len() as u64;
        if self.now() < self.shown + 25 {
            return false;
        }
        self.shown = self.now();
        true
    }
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
    /// Answers to an approval's questions, one per question in order.
    Answer(String, Vec<String>),
    SetModel(String),
    SetEffort(Option<String>),
    SetMode(Mode),
    Compact,
    /// Context fill (%) for trimming and for full compaction; 0 = off.
    SetCompaction { trim: u8, compact: u8 },
    /// Undo the last user turn: drop it (the user's message and everything after) from the
    /// conversation, answered with [`Ev::Undone`]. With `files`, the backend also reverts the
    /// file changes it tracked (ChatGPT its tools' snapshots, Claude Code its file checkpoints,
    /// Codex the patches it reported); without, the TUI restores them from its own checkpoints.
    Undo { files: bool },
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
    /// Plan without asking: still no file edits, but commands and tools run without approval.
    /// Handing over the plan and questions still come to the user.
    YoloPlan,
}

impl Mode {
    pub fn next(self) -> Self {
        match self {
            Mode::Ask => Mode::AutoEdit,
            Mode::AutoEdit => Mode::Plan,
            Mode::Plan => Mode::YoloPlan,
            Mode::YoloPlan => Mode::Yolo,
            Mode::Yolo => Mode::Ask,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Mode::Ask => "ask",
            Mode::AutoEdit => "auto-edit",
            Mode::Yolo => "yolo",
            Mode::Plan => "plan",
            Mode::YoloPlan => "yolo-plan",
        }
    }
    /// Plan mode, asking or not.
    pub fn plans(self) -> bool {
        matches!(self, Mode::Plan | Mode::YoloPlan)
    }
}

pub type Events = Sender<Ev>;
