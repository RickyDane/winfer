//! The JSON lines `winfer code --serve` speaks (`harness::event::{Ev, Cmd}`), as the web UI saw them.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Mode {
    #[default]
    #[serde(rename = "ask")]
    Ask,
    #[serde(rename = "auto-edit")]
    AutoEdit,
    #[serde(rename = "yolo")]
    Yolo,
    #[serde(rename = "plan")]
    Plan,
    #[serde(rename = "yolo-plan")]
    YoloPlan,
}

impl Mode {
    pub const ALL: [Mode; 5] = [Mode::Ask, Mode::AutoEdit, Mode::Plan, Mode::YoloPlan, Mode::Yolo];

    pub fn id(self) -> &'static str {
        match self {
            Mode::Ask => "ask",
            Mode::AutoEdit => "auto-edit",
            Mode::Yolo => "yolo",
            Mode::Plan => "plan",
            Mode::YoloPlan => "yolo-plan",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Mode::Ask => "Ask",
            Mode::AutoEdit => "Auto-edit",
            Mode::Yolo => "Yolo",
            Mode::Plan => "Plan",
            Mode::YoloPlan => "Yolo plan",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            Mode::Ask => "Ask before edits, commands and network",
            Mode::AutoEdit => "File edits go through; commands still ask",
            Mode::Plan => "Read-only: plan without changing anything",
            Mode::YoloPlan => "Plan; tools run without approval",
            Mode::Yolo => "Nothing asks",
        }
    }

    pub fn parse(id: &str) -> Option<Mode> {
        Mode::ALL.into_iter().find(|m| m.id() == id)
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct QuestionOption {
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub description: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct Question {
    #[serde(default)]
    pub header: String,
    #[serde(default)]
    pub question: String,
    #[serde(default)]
    pub options: Vec<QuestionOption>,
    #[serde(default)]
    pub multi: bool,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct Approval {
    pub id: String,
    #[serde(default)]
    pub tool: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub detail: String,
    #[serde(default)]
    pub diff: Option<String>,
    #[serde(default)]
    pub questions: Vec<Question>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Todo {
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub status: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct TaskEv {
    pub id: String,
    #[serde(default)]
    pub tool_id: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub agent: Option<String>,
    #[serde(default)]
    pub tools: Option<u64>,
    #[serde(default)]
    pub last: Option<String>,
    /// running | done | failed | stopped
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub output_file: Option<String>,
    #[serde(default)]
    pub exit_code: Option<i64>,
    #[serde(default)]
    pub model: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct Usage {
    #[serde(default)]
    pub context: u64,
    #[serde(default)]
    pub window: u64,
    #[serde(default)]
    pub input: u64,
    #[serde(default)]
    pub cached: u64,
    #[serde(default)]
    pub output: u64,
    #[serde(default)]
    pub cost: Option<f64>,
    #[serde(default)]
    pub turn: Option<u64>,
    #[serde(default)]
    pub turn_input: Option<u64>,
    #[serde(default)]
    pub turn_cached: Option<u64>,
    #[serde(default)]
    pub turn_streamed: Option<u64>,
    #[serde(default)]
    pub turn_active_ms: Option<u64>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct SessionInfo {
    pub id: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub updated: f64,
    #[serde(default)]
    pub messages: u64,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct ModelInfo {
    pub spec: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub detail: String,
    #[serde(default)]
    pub efforts: Option<Vec<String>>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct Source {
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub tokens: u64,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct ContextPart {
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub tokens: u64,
    #[serde(default)]
    pub sources: Vec<Source>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct Restored {
    #[serde(default)]
    pub role: String,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub state: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Ev {
    Session { id: String },
    Model { model: String },
    Mode { mode: Mode },
    TextDelta { id: String, text: String },
    Text { id: String, text: String },
    ReasoningDelta { id: String, text: String },
    ToolStart { id: String, name: String, #[serde(default)] title: String, #[serde(default)] input: Value },
    ToolOutput { id: String, chunk: String },
    ToolEnd { id: String, #[serde(default)] ok: bool, #[serde(default)] output: String, #[serde(default)] diff: Option<String> },
    Approval(Approval),
    Task(TaskEv),
    Todos { todos: Vec<Todo> },
    Skills { skills: Vec<String> },
    Usage(Usage),
    Notice { text: String },
    Error { text: String },
    TurnStart,
    TurnDone,
    Exited { text: String },
    Undone { #[serde(default)] files: Option<Vec<String>>, #[serde(default)] error: Option<String> },
    BgNote { text: String },
    Ready {
        session: String,
        #[serde(default)]
        backend_session: Option<String>,
        #[serde(default)]
        title: Option<String>,
        model: String,
        mode: Mode,
        #[serde(default)]
        effort: Option<String>,
        #[serde(default)]
        subagent_model: Option<String>,
        #[serde(default)]
        subagent_effort: Option<String>,
        #[serde(default)]
        cwd: String,
        #[serde(default)]
        branch: Option<String>,
        #[serde(default)]
        trim_at: Option<u64>,
        #[serde(default)]
        compact_at: Option<u64>,
        #[serde(default)]
        mcp: Option<Vec<String>>,
    },
    Sessions { sessions: Vec<SessionInfo> },
    ContextBreakdown { tokens: u64, #[serde(default)] parts: Option<Vec<ContextPart>> },
    Btw { id: String, #[serde(default)] answer: Option<String>, #[serde(default)] error: Option<String> },
    TaskLog { id: String, #[serde(default)] text: Option<String>, #[serde(default)] file: Option<String> },
    Models { models: Vec<ModelInfo> },
    Files { files: Vec<String> },
    Restore { transcript: Vec<Restored> },
    User { text: String },
    Cleared,
    Answered { id: String },
    Account {},
    #[serde(other)]
    Unknown,
}

/// A pasted or dropped image: base64 without the `data:` prefix.
#[derive(Clone, Debug)]
pub struct ImageData {
    pub mime: String,
    pub data: String,
}

#[derive(Clone, Debug)]
pub struct FileData {
    pub name: String,
    pub data: Option<String>,
    pub path: Option<String>,
}

#[derive(Clone, Debug)]
pub enum Decision {
    Allow,
    Always,
    Deny,
}

/// Commands to the session, as the JSON line the process reads.
#[derive(Clone, Debug)]
pub enum Cmd {
    Send { text: String, images: Vec<ImageData>, files: Vec<FileData>, skill: Option<String>, steer: bool },
    Interrupt,
    Approve { id: String, decision: Decision },
    Answer { id: String, answers: Vec<String> },
    SetModel(String),
    SetEffort(Option<String>),
    SetSubagent { model: Option<String>, effort: Option<String> },
    SetMode(Mode),
    Compact,
    Undo { files: bool },
    StopTask(String),
    New,
    SetCompaction { trim: Option<u64>, compact: Option<u64> },
    Diff,
    Btw { id: String, question: String },
    Export(String),
    Sessions,
    ContextBreakdown { tokens: u64 },
    TaskLog(String),
    Resume(String),
    Files,
}

impl Cmd {
    pub fn json(&self) -> Value {
        match self {
            Cmd::Send { text, images, files, skill, steer } => {
                let mut v = json!({ "type": if *steer { "steer" } else { "send" }, "text": text });
                if let Some(s) = skill {
                    v["skill"] = json!(s);
                }
                if !images.is_empty() {
                    v["images"] = images.iter().map(|i| json!({ "mime": i.mime, "data": i.data })).collect();
                }
                if !files.is_empty() {
                    v["files"] = files.iter().map(|f| match (&f.path, &f.data) {
                        (Some(p), _) => json!({ "name": f.name, "path": p }),
                        (None, d) => json!({ "name": f.name, "data": d }),
                    }).collect();
                }
                v
            }
            Cmd::Interrupt => json!({ "type": "interrupt" }),
            Cmd::Approve { id, decision } => json!({ "type": "approve", "id": id, "decision": match decision { Decision::Allow => "allow", Decision::Always => "always", Decision::Deny => "deny" } }),
            Cmd::Answer { id, answers } => json!({ "type": "answer", "id": id, "answers": answers }),
            Cmd::SetModel(m) => json!({ "type": "set_model", "model": m }),
            Cmd::SetEffort(e) => json!({ "type": "set_effort", "effort": e }),
            Cmd::SetSubagent { model, effort } => json!({ "type": "set_subagent", "model": model, "effort": effort }),
            Cmd::SetMode(m) => json!({ "type": "set_mode", "mode": m.id() }),
            Cmd::Compact => json!({ "type": "compact" }),
            Cmd::Undo { files } => json!({ "type": "undo", "files": files }),
            Cmd::StopTask(id) => json!({ "type": "stop_task", "id": id }),
            Cmd::New => json!({ "type": "new" }),
            Cmd::SetCompaction { trim, compact } => {
                let mut v = json!({ "type": "set_compaction" });
                if let Some(t) = trim {
                    v["trim"] = json!(t);
                }
                if let Some(c) = compact {
                    v["compact"] = json!(c);
                }
                v
            }
            Cmd::Diff => json!({ "type": "diff" }),
            Cmd::Btw { id, question } => json!({ "type": "btw", "id": id, "question": question }),
            Cmd::Export(md) => json!({ "type": "export", "markdown": md }),
            Cmd::Sessions => json!({ "type": "sessions" }),
            Cmd::ContextBreakdown { tokens } => json!({ "type": "context_breakdown", "tokens": tokens }),
            Cmd::TaskLog(id) => json!({ "type": "task_log", "id": id }),
            Cmd::Resume(id) => json!({ "type": "resume", "id": id }),
            Cmd::Files => json!({ "type": "files" }),
        }
    }

    /// The line written to the process.
    pub fn line(&self) -> String {
        format!("{}\n", self.json())
    }
}

/// Mirrors `tui::app::COMMANDS`.
pub const COMMANDS: [(&str, &str); 23] = [
    ("/model", "Switch model (ChatGPT, Claude, Codex, Muse)"),
    ("/subagent-model", "Model subagents run on while the main model orchestrates"),
    ("/effort", "Set reasoning effort"),
    ("/mode", "Permission mode: ask, auto-edit, plan, yolo-plan, yolo"),
    ("/new", "Start a new conversation"),
    ("/resume", "Resume a past session"),
    ("/btw", "Ask a side question about this session without interrupting the turn"),
    ("/compact", "Summarize the conversation to free context"),
    ("/compaction", "When to trim and auto-compact (% of context)"),
    ("/undo", "Undo the last turn: its messages and file changes"),
    ("/init", "Create an AGENTS.md for this project"),
    ("/review", "Review uncommitted changes"),
    ("/diff", "Show uncommitted git changes"),
    ("/cost", "Token usage, context breakdown and cost"),
    ("/copy", "Copy the last reply"),
    ("/export", "Save the conversation as Markdown"),
    ("/agents", "Subagents and background tasks (ctrl+g)"),
    ("/tasks", "Background shells and their output (ctrl+b)"),
    ("/mcp", "MCP servers in use"),
    ("/status", "Session, model and directory"),
    ("/sidebar", "Plan in a sidebar on wide panes (on/off)"),
    ("/login", "How to sign in to each backend"),
    ("/help", "Keys and commands"),
];

pub const INIT_PROMPT: &str = "Analyze this codebase and create an AGENTS.md file at the repository root with concise guidance for coding agents: what the project is, how it's laid out, how to build, run, test and lint it, code style and conventions, and anything non-obvious. If one exists, improve it instead.";
pub const REVIEW_PROMPT: &str = "Review the uncommitted changes in this repository (git diff, including staged). Look for bugs, regressions, missing edge cases and unclear code. Report findings ranked by severity with file:line references. Don't change any files.";

/// One line of the process's output, as an event; anything else is the process talking past the
/// protocol (a crash, a warning) and shows as a notice.
pub fn parse_line(line: &str) -> Option<Ev> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }
    if line.starts_with('{') {
        if let Ok(ev) = serde_json::from_str::<Ev>(line) {
            return Some(ev);
        }
    }
    Some(Ev::Notice { text: line.to_string() })
}

/// Splits a byte stream into lines, keeping the partial tail for the next chunk.
#[derive(Default)]
pub struct LineBuffer {
    buf: Vec<u8>,
}

impl LineBuffer {
    pub fn push(&mut self, bytes: &[u8]) -> Vec<String> {
        self.buf.extend_from_slice(bytes);
        let mut lines = Vec::new();
        while let Some(i) = self.buf.iter().position(|b| *b == b'\n' || *b == b'\r') {
            let line: Vec<u8> = self.buf.drain(..=i).collect();
            lines.push(String::from_utf8_lossy(&line[..line.len() - 1]).into_owned());
        }
        lines
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_events() {
        let ev = parse_line(r#"{"type":"text_delta","id":"a","text":"hi"}"#).unwrap();
        assert!(matches!(ev, Ev::TextDelta { ref text, .. } if text == "hi"));
        let ev = parse_line(r#"{"type":"approval","id":"x","tool":"Bash","title":"ls","detail":"","questions":[]}"#).unwrap();
        assert!(matches!(ev, Ev::Approval(a) if a.id == "x"));
        assert!(matches!(parse_line(r#"{"type":"from_the_future"}"#), Some(Ev::Unknown)));
        assert!(matches!(parse_line("panic: oh no"), Some(Ev::Notice { .. })));
        assert!(parse_line("  ").is_none());
    }

    #[test]
    fn ready_carries_the_session() {
        let ev = parse_line(r#"{"type":"ready","session":"s1","model":"claude/opus","mode":"auto-edit","cwd":"/x","branch":"main"}"#).unwrap();
        let Ev::Ready { mode, branch, .. } = ev else { panic!() };
        assert_eq!(mode, Mode::AutoEdit);
        assert_eq!(branch.as_deref(), Some("main"));
    }

    #[test]
    fn commands_are_lines() {
        let c = Cmd::Send { text: "hi".into(), images: vec![], files: vec![], skill: None, steer: true };
        assert_eq!(c.json()["type"], "steer");
        assert!(c.line().ends_with('\n'));
        assert_eq!(Cmd::SetMode(Mode::YoloPlan).json()["mode"], "yolo-plan");
        assert_eq!(Cmd::SetEffort(None).json()["effort"], Value::Null);
    }

    #[test]
    fn line_buffer_keeps_partial_lines() {
        let mut b = LineBuffer::default();
        assert_eq!(b.push(b"{\"a\":1}\n{\"b\""), ["{\"a\":1}"]);
        assert_eq!(b.push(b":2}\r\n"), ["{\"b\":2}", ""]);
    }
}
