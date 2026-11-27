//! The backends behind `winfer code`. Each runs on its own thread and owns its process
//! or HTTP stream; the TUI talks to it through a [`Handle`].

pub mod agy;
pub mod chatgpt;
pub mod claude;
pub mod codex;
pub mod cursor;
pub mod local;
pub mod microcompact;
pub mod muse;
pub mod opencode;
pub(super) mod partial_json;
pub mod pi;

use std::io::{BufRead, BufReader, Read};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};

use serde_json::Value;

use super::claude_skills::{self, LeanSkills};
use super::event::{Cmd, Ev, Events, Mode, ModelInfo, Skill, UserMsg};

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
    /// Meta's Muse (`muse`).
    Muse,
    /// A local OpenAI-compatible server, run with winfer's own tools (local.rs, on the ChatGPT backend's loop).
    Local,
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
            Kind::Muse => "muse",
            Kind::Local => "local",
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
            Kind::Muse => "Muse Code",
            Kind::Local => "Local model",
        }
    }
    /// Runs winfer's own agent loop and tools (not a CLI bridge).
    pub fn native(self) -> bool {
        matches!(self, Kind::ChatGpt | Kind::Local)
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
            "muse" => Some(Kind::Muse),
            "local" => Some(Kind::Local),
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
    // `local` alone: the server's first model.
    if lower == "local" {
        return (Kind::Local, String::new());
    }
    if lower.starts_with("muse-") || lower == "muse" {
        return (Kind::Muse, spec.to_string());
    }
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
    /// Ultracode multi-agent mode (Claude Code's `ultracode` flag, Codex's multi-agent + ultra effort, Antigravity's `/swarm`).
    pub ultracode: bool,
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
    /// The model subagents run on (None: the main one) and their effort.
    pub sub_model: Option<String>,
    pub sub_effort: Option<String>,
    /// The Claude Code / Codex account to run on (see accounts.rs); None = the default sign-in.
    pub account: Option<String>,
}

impl Opts {
    /// The environment that runs the CLI on [`Opts::account`].
    pub fn account_env(&self) -> Vec<(String, String)> {
        crate::accounts::env_for(self.account.as_deref())
    }
}

/// The account a new Claude Code / Codex bridge runs on: the session's own while it has usage,
/// else the first with usage left (None: the default sign-in, or another kind of backend).
pub fn account_for(kind: Kind, saved: Option<&str>) -> Option<String> {
    let provider = match kind {
        Kind::Claude => "claude",
        Kind::Codex => "codex",
        _ => return None,
    };
    if let Some(saved) = saved.filter(|s| crate::accounts::provider_of(s) == provider) {
        if crate::accounts::usable(saved) {
            return Some(saved.to_string());
        }
    }
    crate::accounts::pick(provider, &[]).or_else(|| saved.map(String::from))
}

/// Borrowed pairs of an owned environment, for [`LineProcess::spawn`].
pub fn env_pairs(env: &[(String, String)]) -> Vec<(&str, &str)> {
    env.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect()
}

// Accounts --------------------------------------------------------------------------------------

/// What a bridge sends to carry on after moving to another account. The failed request is
/// already in the session, so it asks to continue rather than repeating it.
pub const CONTINUE_PROMPT: &str = "[winfer] The previous request stopped because the account ran out of usage. winfer switched to another account: continue where you left off.";

/// Short note injected into the conversation when the user changes permission mode, so the
/// model doesn't keep acting on an earlier read-only/plan refusal or stale sandbox state.
pub fn mode_change_note(from: Mode, to: Mode) -> String {
    let summary = match to {
        Mode::Yolo => "Full write access is now enabled; file edits and commands run without asking.",
        Mode::AutoEdit => "Workspace file edits are now enabled without asking; commands still ask for approval.",
        Mode::Ask => "File edits and commands are now allowed with user approval.",
        Mode::Plan => "Read-only planning mode is active: do not edit files.",
        Mode::YoloPlan => "Read-only planning mode is active: do not edit files; read-only tools and commands run without asking.",
    };
    format!("[winfer] Session mode changed from {} to {}. {summary}", from.label(), to.label())
}

/// Whether a CLI's error is its plan's usage limit, with the reset time (unix ms) if it says.
/// Claude Code: "Claude AI usage limit reached|<unix seconds>", "You've hit your limit · resets
/// 3pm", "You've hit your 5-hour limit · resets 1:30am", "…weekly limit…"; Codex: "You've hit
/// your usage limit…".
pub fn usage_limit(text: &str) -> Option<Option<i64>> {
    let lower = text.to_lowercase();
    // "hit your <a few words> limit": 5-hour, weekly, Opus, session…
    let hit_your = lower.split("hit your ").skip(1).any(|rest| rest.split_whitespace().take(4).any(|w| w.trim_matches(|c: char| !c.is_alphanumeric()) == "limit"));
    let hit = hit_your
        || ["usage limit", "limit reached", "out of extra usage", "usagelimitexceeded", "usage_limit_reached"].iter().any(|p| lower.contains(p));
    let hit = hit && !lower.contains("context");
    hit.then(|| {
        text.rsplit_once('|')
            .and_then(|(_, t)| t.trim().parse::<i64>().ok())
            .map(|t| if t < 100_000_000_000 { t * 1000 } else { t })
    })
}

/// Usage left this low (%) on the account counts as out, so the next turn starts on another.
pub const LOW_LEFT: f64 = 2.0;

/// Moves an agent off its account (out of usage until `reset_at`): the next one to use, or None
/// when there's no other account with usage left.
pub fn next_account(provider: &str, current: Option<&str>, reset_at: Option<i64>) -> Option<String> {
    let current = current.map_or_else(|| crate::accounts::default_id(provider), String::from);
    crate::accounts::mark_exhausted(&current, reset_at);
    crate::accounts::pick(provider, &[current.as_str()])
}

/// What a bridge sends to carry on after moving off an account that wasn't signed in, when the
/// session already had turns (a first message is sent again instead).
pub const SIGNED_IN_PROMPT: &str = "[winfer] The previous request stopped because the account wasn't signed in. winfer switched to another account: continue where you left off.";

/// Whether a CLI's error says its account isn't signed in. Claude Code: "Not logged in · Please
/// run /login", "Invalid API key · Please run /login", "OAuth token revoked · Please run /login".
pub fn signed_out(text: &str) -> bool {
    let lower = text.to_lowercase();
    ["not logged in", "please run /login"].iter().any(|p| lower.contains(p))
}

/// Moves an agent off an account that isn't signed in: the next signed-in one, or None.
pub fn next_signed_in(provider: &str, current: Option<&str>) -> Option<String> {
    let current = current.map_or_else(|| crate::accounts::default_id(provider), String::from);
    crate::accounts::mark_signed_out(&current);
    crate::accounts::pick(provider, &[current.as_str()])
}

/// The notice for a move off an account that isn't signed in.
pub fn signed_out_notice(from: Option<&str>, to: &str, provider: &str) -> String {
    let from = crate::accounts::label(&from.map_or_else(|| crate::accounts::default_id(provider), String::from));
    format!("Account \"{from}\" isn't signed in here; continuing on \"{}\".", crate::accounts::label(to))
}

/// The notice for a move between accounts.
pub fn switched_notice(from: Option<&str>, to: &str, reset_at: Option<i64>, provider: &str) -> String {
    let from = crate::accounts::label(&from.map_or_else(|| crate::accounts::default_id(provider), String::from));
    let until = reset_at.map(|t| format!(" for {}", in_time(t))).unwrap_or_default();
    format!("Account \"{from}\" is out of usage{until}; continuing on \"{}\".", crate::accounts::label(to))
}

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// How long until `at` (unix ms): "2h 15m", "40m".
fn in_time(at: i64) -> String {
    let minutes = ((at - now_ms()).max(0) + 59_999) / 60_000;
    match (minutes / 60, minutes % 60) {
        (0, m) => format!("{m}m"),
        (h, 0) => format!("{h}h"),
        (h, m) => format!("{h}h {m}m"),
    }
}

// Waiting out a usage limit ---------------------------------------------------------------------

/// What a bridge sends once the usage limit it waited out has reset. The failed request is
/// already in the session, so it asks to continue rather than repeating it.
pub const RESUME_PROMPT: &str = "[winfer] The previous request stopped because the account ran out of usage. The usage limit has reset now: continue where you left off.";

/// After the reset time, this much longer before continuing (the provider's clock and ours differ).
const RESUME_MARGIN_MS: i64 = 60_000;
/// When the CLI doesn't say when its limit resets: try again after this long.
const UNKNOWN_RESET_WAIT_MS: i64 = 30 * 60_000;

/// Every account is out of usage (Settings → WinferCode → Resume after usage limits): the session
/// waits for the limit to reset, holding the messages sent meanwhile, then continues and sends
/// them one at a time, each once the agent is done with the one before.
#[derive(Default)]
pub struct LimitWait {
    /// When to continue (unix ms), while waiting.
    until: Option<i64>,
    /// Messages to send, oldest first.
    held: std::collections::VecDeque<UserMsg>,
}

impl LimitWait {
    /// Whether the setting is on (read each time, so a change applies to running sessions).
    pub fn enabled() -> bool {
        super::config::Config::load().auto_resume
    }

    pub fn waiting(&self) -> bool {
        self.until.is_some()
    }

    /// Starts waiting for the limit to reset at `reset` (unix ms), if the CLI said when.
    pub fn start(&mut self, reset: Option<i64>, events: &Events) {
        let now = now_ms();
        let known = reset.filter(|&r| r > now);
        let until = known.map_or(now + UNKNOWN_RESET_WAIT_MS, |r| r + RESUME_MARGIN_MS);
        self.until = Some(until);
        let when = match known {
            Some(_) => format!("the limit resets in {}", in_time(until)),
            None => format!("the limit's reset time isn't known, so winfer tries again in {}", in_time(until)),
        };
        let _ = events.send(Ev::Notice(format!("Out of usage. Paused: {when}, then this session continues. Messages sent meanwhile are queued; Esc stops waiting.")));
        self.emit(events);
    }

    fn emit(&self, events: &Events) {
        let _ = events.send(Ev::LimitWait { until: self.until, queued: self.held.len() });
    }

    /// How long the bridge may wait for its next message before it's time to continue.
    pub fn timeout(&self) -> Option<std::time::Duration> {
        self.until.map(|u| std::time::Duration::from_millis((u - now_ms()).max(0) as u64))
    }

    /// Whether it's time to continue.
    pub fn due(&self) -> bool {
        self.until.is_some_and(|u| u <= now_ms())
    }

    /// Takes the commands that have to wait: messages while paused (or sent behind queued ones).
    /// Esc stops waiting and drops the queue. Everything else comes back to run.
    pub fn intercept(&mut self, cmd: Cmd, events: &Events) -> Option<Cmd> {
        match cmd {
            Cmd::Send(msg) | Cmd::Steer(msg) if self.waiting() => {
                self.held.push_back(msg);
                let _ = events.send(Ev::Notice(format!("Queued until the usage limit resets ({} waiting).", self.held.len())));
                self.emit(events);
                None
            }
            Cmd::Send(msg) if !self.held.is_empty() => {
                self.held.push_back(msg);
                let _ = events.send(Ev::Notice(format!("Queued behind the messages sent while out of usage ({} waiting).", self.held.len())));
                self.emit(events);
                None
            }
            Cmd::Interrupt if self.waiting() || !self.held.is_empty() => {
                let was_waiting = self.until.take().is_some();
                let dropped = std::mem::take(&mut self.held).len();
                let dropped_text = match dropped {
                    0 => String::new(),
                    1 => " Dropped 1 queued message.".to_string(),
                    n => format!(" Dropped {n} queued messages."),
                };
                let text = if was_waiting { format!("Stopped waiting for the usage limit to reset.{dropped_text}") } else { dropped_text.trim().to_string() };
                let _ = events.send(Ev::Notice(text));
                self.emit(events);
                (!was_waiting).then_some(Cmd::Interrupt)
            }
            cmd => Some(cmd),
        }
    }

    /// Stops waiting: the message that continues the session.
    pub fn resume(&mut self, events: &Events) -> UserMsg {
        self.until = None;
        let _ = events.send(Ev::Notice("The usage limit has reset: continuing.".into()));
        self.emit(events);
        UserMsg { text: RESUME_PROMPT.into(), ..Default::default() }
    }

    /// The next queued message to send (none while paused).
    pub fn next(&mut self, events: &Events) -> Option<UserMsg> {
        if self.waiting() {
            return None;
        }
        let msg = self.held.pop_front()?;
        self.emit(events);
        Some(msg)
    }
}

/// The account to continue on after waiting out a limit, when it isn't the current one (which
/// stays while it has usage): the first other one with usage left.
pub fn resume_account(provider: &str, current: Option<&str>) -> Option<String> {
    let current = current.map_or_else(|| crate::accounts::default_id(provider), String::from);
    if !crate::accounts::has_alternatives(provider) || crate::accounts::usable(&current) {
        return None;
    }
    crate::accounts::pick(provider, &[current.as_str()])
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

/// Added to the system prompt when subagents run on a model of their own: the main model
/// orchestrates and checks their work.
pub const ORCHESTRATE_PROMPT: &str = "Subagents run on a smaller, faster model than yours. Delegate codebase exploration, web lookups and small, well-specified fixes to them (give each the full context it needs); keep the hard reasoning and the decisions yourself. Treat a subagent's report as unverified: before relying on it or calling the work done, read the files it changed and rerun the relevant checks.";

/// The extra system prompt for a session: the app's own (`Opts::system`) plus
/// [`ORCHESTRATE_PROMPT`] when subagents have a model of their own.
pub fn extra_system(opts: &Opts) -> Option<String> {
    let parts: Vec<&str> = opts.system.as_deref().into_iter().chain(opts.sub_model.as_ref().map(|_| ORCHESTRATE_PROMPT)).collect();
    (!parts.is_empty()).then(|| parts.join("\n\n"))
}

/// Where the agent is (Claude Code drops it with its default prompt); goes after [`LEAN_PROMPT`]
/// so the cached prefix stays the same.
pub fn environment(cwd: &std::path::Path) -> String {
    let branch = crate::env::no_window(
        std::process::Command::new("git")
            .args(["rev-parse", "--abbrev-ref", "HEAD"])
            .current_dir(cwd),
    )
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

/// Skills `/name` can run, read from disk so they're offered from the moment a session
/// opens — not only once the backend's own session init reports them. The backend still
/// sends its authoritative list when it starts, which replaces this one.
fn preloaded_skills(kind: Kind, home: &std::path::Path, cwd: &std::path::Path, lean: &LeanSkills) -> Vec<Skill> {
    let mut out: Vec<Skill> = Vec::new();
    let mut push = |name: String, description: String| {
        if !name.is_empty() && !out.iter().any(|s: &Skill| s.name == name) {
            out.push(Skill { name, description });
        }
    };
    match kind {
        Kind::ChatGpt | Kind::Local => {}
        Kind::Claude => {
            let available = claude_skills::available_in(home);
            let picked = match lean {
                LeanSkills::All => available,
                LeanSkills::Only(ids) => available.into_iter().filter(|s| ids.contains(&s.id)).collect(),
            };
            for s in picked {
                push(s.name, s.description);
            }
            for s in claude_skills::in_dir(&cwd.join(".claude/skills"), "project") {
                push(s.name, s.description);
            }
        }
        Kind::Codex => {
            for s in claude_skills::codex_in(home) {
                push(s.name, s.description);
            }
            for dir in [".codex/skills", ".agents/skills"] {
                for s in claude_skills::in_dir(&cwd.join(dir), "project") {
                    push(s.name, s.description);
                }
            }
        }
        Kind::Muse => {
            for s in claude_skills::pickable_in(home) {
                push(s.name, s.description);
            }
            for dir in [".claude/skills", ".agents/skills"] {
                for s in claude_skills::in_dir(&cwd.join(dir), "project") {
                    push(s.name, s.description);
                }
            }
        }
        Kind::Agy | Kind::Cursor | Kind::OpenCode | Kind::Pi => {
            let targets: &[&str] = match kind {
                Kind::Agy => &["gemini", "agents"],
                Kind::Cursor => &["cursor", "agents"],
                Kind::OpenCode => &["opencode", "agents"],
                _ => &["pi", "agents"],
            };
            for s in crate::skills::list_installed(home, &[cwd.to_path_buf()]) {
                if s.copies.iter().any(|c| c.kind == crate::skills::Kind::Skill && targets.contains(&c.target.as_str())) {
                    push(s.name, s.description);
                }
            }
        }
    }
    out
}

pub fn start(kind: Kind, mut opts: Opts, events: Events) -> Handle {
    let (tx, rx) = channel();
    let me = tx.clone();
    std::thread::spawn(move || {
        // Offer the disk's skills at once; the backend's own list follows on session init.
        if let Some(home) = dirs::home_dir() {
            let preloaded = preloaded_skills(kind, &home, &opts.cwd, &opts.skills);
            if !preloaded.is_empty() {
                let _ = events.send(super::event::Ev::Skills(preloaded));
            }
        }
        match kind {
        Kind::ChatGpt => chatgpt::run(opts, rx, events),
        Kind::Local => local::run(opts, rx, events),
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
                Kind::Muse => muse::run(opts, rx, me, events),
                _ => codex::run(opts, rx, me, events),
            }
            crate::harness::bg::stop_owner(&owner);
            crate::harness::bg::remove_asker(&owner);
        }
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
    let listers: [fn() -> Vec<ModelInfo>; 9] = [chatgpt::models, local::models, claude::models, codex::models, agy::models, cursor::models, opencode::models, pi::models, muse::models];
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
        Some("muse") => 6,
        Some("local") => 7,
        _ => 8,
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

/// [`recv`] for a bridge that may be waiting out a usage limit: `Some(None)` when it's time to
/// continue.
pub fn recv_or_wake(rx: &Receiver<In>, wait: &LimitWait) -> Option<Option<In>> {
    match wait.timeout() {
        None => rx.recv().ok().map(Some),
        Some(t) => match rx.recv_timeout(t) {
            Ok(msg) => Some(Some(msg)),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => Some(None),
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => None,
        },
    }
}

#[cfg(test)]
mod limit_wait_tests {
    use super::{Cmd, Ev, LimitWait, UserMsg, now_ms};

    fn msg(text: &str) -> UserMsg {
        UserMsg { text: text.into(), ..Default::default() }
    }

    fn states(rx: &std::sync::mpsc::Receiver<Ev>) -> Vec<(Option<i64>, usize)> {
        rx.try_iter().filter_map(|e| match e { Ev::LimitWait { until, queued } => Some((until, queued)), _ => None }).collect()
    }

    #[test]
    fn holds_messages_until_the_reset_then_sends_them_in_order() {
        let (tx, rx) = std::sync::mpsc::channel();
        let mut w = LimitWait::default();
        assert!(w.timeout().is_none());
        let reset = now_ms() + 3_600_000;
        w.start(Some(reset), &tx);
        assert!(w.waiting() && !w.due());
        assert_eq!(states(&rx), vec![(Some(reset + 60_000), 0)]);
        // Messages and steers wait; other commands go through.
        assert!(w.intercept(Cmd::Send(msg("a")), &tx).is_none());
        assert!(w.intercept(Cmd::Steer(msg("b")), &tx).is_none());
        assert!(matches!(w.intercept(Cmd::Compact, &tx), Some(Cmd::Compact)));
        assert_eq!(states(&rx).last(), Some(&(Some(reset + 60_000), 2)));
        assert!(w.next(&tx).is_none(), "nothing goes while waiting");
        // Due: continue, then the queue one at a time; a new message goes behind it.
        w.until = Some(now_ms() - 1);
        assert!(w.due());
        assert_eq!(w.resume(&tx).text, super::RESUME_PROMPT);
        assert!(w.timeout().is_none());
        assert!(w.intercept(Cmd::Send(msg("c")), &tx).is_none());
        let sent: Vec<String> = std::iter::from_fn(|| w.next(&tx)).map(|m| m.text).collect();
        assert_eq!(sent, ["a", "b", "c"]);
        assert_eq!(states(&rx).last(), Some(&(None, 0)));
        assert!(matches!(w.intercept(Cmd::Send(msg("d")), &tx), Some(Cmd::Send(_))));
    }

    #[test]
    fn esc_stops_waiting_and_drops_the_queue() {
        let (tx, rx) = std::sync::mpsc::channel();
        let mut w = LimitWait::default();
        w.start(None, &tx);
        // An unknown reset tries again in a while.
        assert!(w.timeout().is_some_and(|t| t.as_secs() > 25 * 60));
        let _ = w.intercept(Cmd::Send(msg("a")), &tx);
        assert!(w.intercept(Cmd::Interrupt, &tx).is_none(), "nothing runs to interrupt");
        assert!(!w.waiting() && w.next(&tx).is_none());
        assert_eq!(states(&rx).last(), Some(&(None, 0)));
        // Not waiting: Esc is for the running turn.
        assert!(matches!(w.intercept(Cmd::Interrupt, &tx), Some(Cmd::Interrupt)));
    }
}

#[cfg(test)]
mod preload_tests {
    use super::{preloaded_skills, Kind, LeanSkills};
    use std::path::Path;

    fn skill(dir: &Path, desc: &str) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join("SKILL.md"), format!("---\nname: x\ndescription: {desc}\n---\nbody")).unwrap();
    }

    #[test]
    fn skills_are_available_before_the_session_starts() {
        let root = std::env::temp_dir().join(format!("winfer-preload-skills-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (home, cwd) = (root.join("home"), root.join("proj"));
        skill(&home.join(".claude/skills/graphify"), "Graphs");
        skill(&home.join(".codex/skills/wrangler"), "Workers");
        skill(&cwd.join(".claude/skills/projskill"), "Project");
        let names = |v: Vec<super::Skill>| v.iter().map(|s| s.name.clone()).collect::<Vec<_>>();
        // Claude sees its user + project skills, Codex its own, ChatGPT none.
        assert_eq!(names(preloaded_skills(Kind::Claude, &home, &cwd, &LeanSkills::All)), ["graphify", "projskill"]);
        assert_eq!(names(preloaded_skills(Kind::Codex, &home, &cwd, &LeanSkills::All)), ["wrangler"]);
        assert!(preloaded_skills(Kind::ChatGpt, &home, &cwd, &LeanSkills::All).is_empty());
        // Lean mode keeps only the picked Claude skills (project skills still load).
        assert_eq!(names(preloaded_skills(Kind::Claude, &home, &cwd, &LeanSkills::Only(vec![]))), ["projskill"]);
        let _ = std::fs::remove_dir_all(&root);
    }
}

#[cfg(test)]
mod account_tests {
    use super::{signed_out, usage_limit};

    #[test]
    fn recognizes_usage_limits() {
        assert_eq!(usage_limit("Claude AI usage limit reached|1730000000"), Some(Some(1_730_000_000_000)));
        assert_eq!(usage_limit("You've hit your limit · resets 3pm (Europe/Berlin)"), Some(None));
        assert_eq!(usage_limit("You've hit your usage limit. Upgrade to Pro or try again later."), Some(None));
        assert_eq!(usage_limit("You've hit your 5-hour limit · resets 1:30am (Europe/Berlin)"), Some(None));
        assert_eq!(usage_limit("You've hit your weekly limit · resets Mon 9am"), Some(None));
        assert_eq!(usage_limit("You've hit your Opus limit."), Some(None));
        assert_eq!(usage_limit("You hit your stride, no limits here"), None);
        assert_eq!(usage_limit("Prompt is too long"), None);
        assert_eq!(usage_limit("context limit reached"), None);
    }

    #[test]
    fn recognizes_signed_out_accounts() {
        assert!(signed_out("Not logged in · Please run /login"));
        assert!(signed_out("Invalid API key · Please run /login"));
        assert!(signed_out("OAuth token revoked · Please run /login"));
        assert!(!signed_out("Claude AI usage limit reached|1730000000"));
        assert!(!signed_out("API Error: 500 Internal server error"));
        assert_eq!(usage_limit("Not logged in · Please run /login"), None);
    }
}
