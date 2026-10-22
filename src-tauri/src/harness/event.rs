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
    /// Something the agent should hear about its `bg` tasks (one exited unseen, the user stopped
    /// one): the TUI sends it as the next message (backends with their own queue don't emit it).
    BgNote(String),
}

/// One update about a background task; unset fields are unchanged.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TaskEv {
    pub id: String,
    /// The tool call that started it (an Agent card), if any.
    pub tool_id: Option<String>,
    pub title: Option<String>,
    /// What a background task is for (winfer's `bg` tasks the agent described).
    pub description: Option<String>,
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

impl Question {
    /// Questions in the shape Claude Code's AskUserQuestion and winfer code's `ask_user` take:
    /// `{ questions: [{ header, question, options: [{ label, description }], multiSelect }] }`.
    pub fn parse_all(input: &Value) -> Vec<Question> {
        let text = |v: &Value| v.as_str().unwrap_or("").trim().to_string();
        input["questions"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|q| Question {
                header: text(&q["header"]),
                question: text(&q["question"]),
                options: q["options"].as_array().into_iter().flatten().map(|o| (text(&o["label"]), text(&o["description"]))).filter(|(l, _)| !l.is_empty()).collect(),
                multi: q["multiSelect"].as_bool().unwrap_or(false),
            })
            .filter(|q| !q.question.is_empty())
            .collect()
    }
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
    /// Input tokens of the running turn (or the last one), summed over its requests.
    pub turn_input: u64,
    /// The part of `turn_input` read from the prompt cache.
    pub turn_cached: u64,
    /// Estimated tokens of the main agent's text streamed so far this turn. Unlike `turn` it
    /// only grows as text arrives, never by an exact count landing (which also covers hidden
    /// reasoning and subagents), so the tok/s rate is taken from it.
    pub turn_streamed: u64,
}

/// Counts a turn's output tokens as it runs. Backends report exact counts once per model
/// request; between those, streamed text counts at about 4 characters a token.
#[derive(Debug, Default)]
pub struct TurnTokens {
    exact: u64,
    chars: u64,
    shown: u64,
    /// Every streamed character of the main agent this turn, kept across exact counts.
    paced: u64,
}

impl TurnTokens {
    pub fn now(&self) -> u64 {
        self.exact + self.chars / 4
    }

    /// The main agent's streamed text so far, in estimated tokens (`Usage::turn_streamed`).
    pub fn streamed_total(&self) -> u64 {
        self.paced / 4
    }

    /// Streamed text of a subagent: adds to the count but not to the pace, since subagents
    /// stream alongside the main agent and each other.
    pub fn streamed_sub(&mut self, text: &str) -> bool {
        self.count(text)
    }

    /// The backend's exact count for the turn so far; the estimate it covers is dropped.
    pub fn exact(&mut self, tokens: u64) {
        self.exact = tokens;
        self.chars = 0;
        self.shown = self.now();
    }

    /// Streamed text; true when the count moved enough (~25 tokens) to be worth sending.
    pub fn streamed(&mut self, text: &str) -> bool {
        self.paced += text.len() as u64;
        self.count(text)
    }

    fn count(&mut self, text: &str) -> bool {
        self.chars += text.len() as u64;
        if self.now() < self.shown + 25 {
            return false;
        }
        self.shown = self.now();
        true
    }
}

/// A turn's output speed in tokens per second, over the time tokens actually streamed in:
/// gaps between counts longer than `IDLE` (tool runs, waiting on the next request's first
/// token) don't count, so the rate is the model's, not the turn's. It's fed the streamed
/// count (`Usage::turn_streamed`), not `Usage::turn`: an exact count landing at a request's
/// end adds hidden reasoning (made during a silent gap) in an instant and would spike it.
#[derive(Debug, Default)]
pub struct Tps {
    last: Option<(std::time::Instant, u64)>,
    tokens: u64,
    active: std::time::Duration,
}

impl Tps {
    const IDLE: std::time::Duration = std::time::Duration::from_secs(3);

    /// The turn's streamed output count (`Usage::turn_streamed`) at `at`.
    pub fn sample(&mut self, at: std::time::Instant, turn: u64) {
        if let Some((t, n)) = self.last {
            let gap = at.saturating_duration_since(t);
            if turn > n && gap <= Self::IDLE {
                self.tokens += turn - n;
                self.active += gap;
            }
        }
        self.last = Some((at, turn));
    }

    /// Tokens per second, once there's enough streaming to tell.
    pub fn rate(&self) -> Option<f64> {
        (self.tokens > 0 && self.active >= std::time::Duration::from_millis(500)).then(|| self.tokens as f64 / self.active.as_secs_f64())
    }
}

#[cfg(test)]
mod tps_tests {
    use super::Tps;
    use std::time::{Duration, Instant};

    #[test]
    fn counts_streaming_time_only() {
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let mut r = Tps::default();
        r.sample(at(0), 0);
        assert_eq!(r.rate(), None);
        r.sample(at(500), 25);
        r.sample(at(1000), 50);
        assert_eq!(r.rate(), Some(50.0));
        // A 10s tool run, then the next request streams: the gap isn't counted.
        r.sample(at(11_000), 75);
        r.sample(at(11_500), 100);
        assert_eq!(r.rate(), Some(75.0 / 1.5));
        // An exact count lower than the estimate just rebases.
        r.sample(at(12_000), 90);
        r.sample(at(12_500), 115);
        assert_eq!(r.rate(), Some(100.0 / 2.0));
    }

    #[test]
    fn exact_counts_dont_move_the_pace() {
        use super::TurnTokens;
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let (mut turn, mut r) = (TurnTokens::default(), Tps::default());
        // 10s of hidden reasoning, then 100 chars (25 tokens) every 250ms: 100 tok/s.
        r.sample(at(0), turn.streamed_total());
        for i in 1..=8 {
            turn.streamed(&"x".repeat(100));
            r.sample(at(10_000 + i * 250), turn.streamed_total());
        }
        // The request ends: its exact count includes 1500 reasoning tokens.
        turn.exact(1700);
        r.sample(at(12_100), turn.streamed_total());
        assert_eq!(turn.now(), 1700);
        assert_eq!(r.rate(), Some(175.0 / 1.75));
        // A subagent's text adds to the count, not to the pace.
        turn.streamed_sub(&"x".repeat(4000));
        assert_eq!((turn.now(), turn.streamed_total()), (2700, 200));
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
    /// A message for the running turn: the model gets it at its next step (after the tool calls
    /// in flight) instead of after the turn. Without a running turn it's a [`Cmd::Send`].
    Steer(UserMsg),
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
