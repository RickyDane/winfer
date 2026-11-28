//! Home: one orchestrator (the conductor) above every workspace and paired machine, and a
//! sub-orchestrator per forked task. See docs/plans/home-orchestrator.md.
//!
//! Both run like winfer code chat panes under made-up pane ids no workspace lists
//! (`crate::voice::{CONDUCTOR, task_pane}`). Their MCP calls come in with those ids
//! (`agent_api.rs` → `home_tool`), where they also get Home's task tools and winfer's UI actions.
//!
//! Every coding job is a task (`HomeTask`), shown as a card on Home:
//! - light: the conductor started one agent and follows it itself;
//! - fork: a task orchestrator here plans, runs and checks it in its workspace;
//! - remote: a task orchestrator on the paired machine the workspace is on.

use super::connect_ui::{Mirror, RemotePane};
use super::voice_tools;
use super::*;
use crate::code::protocol::{Cmd, Decision};
use crate::code::session::{CodeEvent, CodeModel};
use crate::code::mentions::{Mention, slug};
use crate::code::state::Item;
use crate::model::{Pane, Workspace};
use crate::voice::{CONDUCTOR, HomePane, home_pane, task_pane};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashSet;
use std::time::{Duration, Instant};

pub(super) const CONDUCTOR_ROLE: &str = include_str!("../../assets/conductor-role.txt");
pub(super) const TASK_ROLE: &str = include_str!("../../assets/task-role.txt");
/// How Home's orchestrators choose models for tasks (in their system prompt, not shown to the user).
pub(super) const ROUTING: &str = include_str!("../../assets/model-routing.md");

/// The model routing guide: `<data>/home/model-routing.md` when the user wrote their own, else
/// the built-in one.
pub(super) fn routing_guide() -> String {
    let own = std::path::Path::new(&home_dir()).join("model-routing.md");
    std::fs::read_to_string(own).ok().filter(|t| !t.trim().is_empty()).unwrap_or_else(|| ROUTING.to_string())
}
/// What an orchestrator replies when there's nothing to tell.
const SILENT: &str = "[silent]";
/// Seconds (ticks) a pane's new status must hold before an orchestrator hears of it: hooks fire
/// per turn and idle detection can flap.
const SETTLE_TICKS: u8 = 2;
const MAX_SPOKEN: usize = 1500;
const MAX_TOAST: usize = 280;
const NO_PICKER: &str = "Nobody sees questions asked here. Put the question in your reply instead.";
/// The opening brief at most this often.
const BRIEF_EVERY: Duration = Duration::from_secs(2 * 60 * 60);

// ---------------------------------------------------------------------------------------------
// Tasks

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TaskMode {
    #[default]
    Light,
    Fork,
    Remote,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Stage {
    #[default]
    Planning,
    Running,
    Checking,
    NeedsYou,
    Ready,
    Merged,
    Failed,
    Cancelled,
}

impl Stage {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s.trim().to_lowercase().replace([' ', '_'], "-").as_str() {
            "planning" => Self::Planning,
            "running" | "working" => Self::Running,
            "checking" | "reviewing" => Self::Checking,
            "needs-you" | "needs-input" | "waiting" => Self::NeedsYou,
            "ready" | "done" => Self::Ready,
            "merged" => Self::Merged,
            "failed" | "error" => Self::Failed,
            "cancelled" | "canceled" => Self::Cancelled,
            _ => return None,
        })
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Planning => "Planning",
            Self::Running => "Running",
            Self::Checking => "Checking",
            Self::NeedsYou => "Needs you",
            Self::Ready => "Ready",
            Self::Merged => "Merged",
            Self::Failed => "Failed",
            Self::Cancelled => "Cancelled",
        }
    }

    /// Still being worked on or waiting for the user.
    pub fn open(self) -> bool {
        !matches!(self, Self::Merged | Self::Cancelled)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct HomeTask {
    pub id: String,
    pub number: u32,
    pub goal: String,
    /// The workspace as the user knows it: "api", or "mac-mini/web" on another machine.
    pub workspace: String,
    /// This machine's workspace id (light and fork tasks).
    pub workspace_id: Option<String>,
    /// The paired machine (peer id) a remote task runs on.
    pub machine: Option<String>,
    pub mode: TaskMode,
    pub stage: Stage,
    /// Agent panes working on it here.
    pub panes: Vec<String>,
    /// The worktree folders its agents here had (kept after their panes close).
    pub worktrees: Vec<String>,
    /// Those of them merged back.
    pub merged: Vec<String>,
    /// A remote task's orchestrator: its pane id on that machine.
    pub remote_pane: Option<String>,
    /// Panes there already titled like it when it was asked for: an earlier task of its number.
    pub earlier_panes: Vec<String>,
    pub summary: String,
    /// A fork's winfer code session, to resume it after a restart.
    pub session: Option<String>,
    /// The agent (provider id) its agents run as; None: its orchestrator picks.
    pub agent: Option<String>,
    /// The model its agents run on (a WinferCode spec, e.g. "codex/gpt-6.1-sol"); None: default.
    pub model: Option<String>,
    pub effort: Option<String>,
    /// Why its model was last chosen or switched (shown on the card).
    pub model_note: String,
    pub created: u64,
    pub updated: u64,
}

impl HomeTask {
    pub fn title(&self) -> String {
        format!("Task {}", self.number)
    }

    /// Whether a pane on a remote task's machine is its orchestrator.
    pub fn orchestrated_by(&self, pane: &RemotePane) -> bool {
        match &self.remote_pane {
            Some(id) => pane.id == *id,
            // Not seen yet: found by its title.
            None => pane.title.starts_with(&format!("{} ·", self.title())) && !self.earlier_panes.contains(&pane.id),
        }
    }
}

pub(super) fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// Remembers the worktrees of the task's agent panes; true when one is new. A tile of another
/// machine's pane is left out: its folder isn't here to check.
fn note_worktrees(t: &mut HomeTask, s: &AppState) -> bool {
    let mut new = false;
    for pane in t.panes.iter().filter_map(|p| s.pane(p)).filter(|p| p.remote.is_none()) {
        if let Some(w) = &pane.worktree
            && !t.worktrees.contains(&w.path)
        {
            t.worktrees.push(w.path.clone());
            new = true;
        }
    }
    new
}

/// All of the task's work is in: something was merged, and every other worktree is merged or gone.
pub fn all_merged(t: &HomeTask, exists: impl Fn(&str) -> bool) -> bool {
    !t.merged.is_empty() && t.worktrees.iter().all(|w| t.merged.contains(w) || !exists(w))
}

/// A done task with nothing left to clean up: no agent pane and no worktree. `remote_open`: its
/// orchestrator on another machine may still be there.
pub fn finished(t: &HomeTask, pane_open: impl Fn(&str) -> bool, exists: impl Fn(&str) -> bool, remote_open: bool) -> bool {
    !t.stage.open() && !remote_open && !t.panes.iter().any(|p| pane_open(p)) && !t.worktrees.iter().any(|w| exists(w))
}

/// Whether a remote task's orchestrator may still be open there. `mirror`: what its machine says
/// now (None: offline or not heard from, so unknown); `starting`: it was only just asked for.
pub fn remote_pane_open(t: &HomeTask, mirror: Option<&Mirror>, starting: bool) -> bool {
    if t.mode != TaskMode::Remote {
        return false;
    }
    let Some(mirror) = mirror else { return true };
    (starting && t.remote_pane.is_none()) || mirror.panes.iter().any(|p| t.orchestrated_by(p))
}

/// The tasks Home's panel lists (and the phones): open ones first (newest first), then the done
/// ones that still have something to clean up.
pub fn listed_tasks(tasks: &[HomeTask]) -> Vec<&HomeTask> {
    let mut list: Vec<&HomeTask> = tasks.iter().collect();
    list.sort_by_key(|t| (!t.stage.open(), std::cmp::Reverse(t.updated)));
    list.truncate(12);
    list
}

/// The lowest number no open task has.
pub fn free_number(tasks: &[HomeTask]) -> u32 {
    let used: HashSet<u32> = tasks.iter().filter(|t| t.stage.open()).map(|t| t.number).collect();
    let mut n = 1;
    while used.contains(&n) {
        n += 1;
    }
    n
}

/// Numbers are reused, so a task's id carries more than its number.
fn new_task_id(number: u32) -> String {
    crate::model::new_id(&format!("t{number}"))
}

/// A done task about to be taken up again: what was merged before doesn't finish it, and it gets
/// a free number if an open task has taken its own.
fn reopen(tasks: &mut [HomeTask], id: &str) {
    let free = free_number(tasks);
    let Some(i) = tasks.iter().position(|t| t.id == id && !t.stage.open()) else { return };
    let number = tasks[i].number;
    if tasks.iter().any(|t| t.stage.open() && t.number == number) {
        tasks[i].number = free;
    }
    tasks[i].merged.clear();
}

/// Finds a task by "3", "Task 3", "t3" or its id. By number, the open one wins over done ones
/// that had it before (else the latest of those).
pub fn find_task<'a>(tasks: &'a [HomeTask], key: &Value) -> Option<&'a HomeTask> {
    let text = key.as_u64().map(|n| n.to_string()).or_else(|| key.as_str().map(str::to_string))?;
    let t = text.trim().to_lowercase();
    let digits = t.trim_start_matches("task").trim_start_matches('t').trim();
    match digits.parse::<u32>() {
        Ok(n) => tasks.iter().filter(|x| x.number == n).max_by_key(|x| (x.stage.open(), x.updated)),
        Err(_) => tasks.iter().find(|x| x.id == t),
    }
}

// ---------------------------------------------------------------------------------------------
// Orchestrator sessions

#[derive(Default)]
pub(super) struct Home {
    /// list_models calls waiting for WinferCode's first model list: (call, since).
    waiting_models: Vec<(String, String, Instant)>,
    /// The models WinferCode last reported (kept across restarts in `home.models`).
    models: Vec<crate::code::protocol::ModelInfo>,
    conductor: Option<Orch>,
    /// Forked tasks' orchestrators on this machine, by task id.
    forks: HashMap<String, Orch>,
    pub tasks: Vec<HomeTask>,
    pub(super) loaded: bool,
    /// The thread the Home chat shows: None for the conductor, else a forked task's id.
    pub thread: Option<String>,
    pub views: HashMap<String, Entity<CodeView>>,
    /// The conductor's latest request came by voice: its answer is spoken.
    voice_turn: bool,
    /// Remote tasks' orchestrators, by task id.
    remote_watch: HashMap<String, Watch>,
    last_brief: Option<Instant>,
    /// When the user last looked at Home (for the opening brief).
    pub left_at: Option<Instant>,
    /// Home is on screen.
    pub shown: bool,
    /// When the conductor last changed (ms), for the phones' list.
    pub touched_ms: u64,
    /// A task whose orchestrator was asked to start on a paired machine, waiting to see it there.
    pending_remote: HashMap<String, Instant>,
    /// Tool calls sent to paired machines for remote tasks: request id → task id.
    remote_calls: HashMap<String, String>,
    /// Panes on paired machines the conductor started or messaged.
    pub follows: super::home_follow::Follows,
}

struct Orch {
    model: Entity<CodeModel>,
    /// Events waiting for the session to be free.
    queue: Vec<String>,
    /// Agent panes it follows, and what it last heard of their status.
    owned: HashMap<String, Watch>,
    /// Approvals and questions already answered.
    handled: HashSet<String>,
    /// The tool it's running, for the voice HUD and the task card.
    step: Option<String>,
    _subscriptions: Vec<Subscription>,
}

/// Follows one pane's status for its orchestrator.
#[derive(Clone, Copy, Debug)]
pub(super) struct Watch {
    seen: AgentStatus,
    /// A different status, and for how many ticks it has held.
    next: Option<(AgentStatus, u8)>,
    /// It worked since the orchestrator last heard of it.
    worked: bool,
}

impl Watch {
    pub fn new(status: AgentStatus) -> Self {
        Self { seen: status, next: None, worked: status == AgentStatus::Working }
    }

    /// For a pane just started, first seen at `status`: whatever it did since then counts.
    pub fn started(status: AgentStatus) -> Self {
        Self { seen: AgentStatus::Idle, next: None, worked: matches!(status, AgentStatus::Working | AgentStatus::Done) }
    }

    /// One tick with the pane at `now`: what to tell the orchestrator, once a change has settled.
    pub fn tick(&mut self, now: AgentStatus) -> Option<&'static str> {
        if now == AgentStatus::Working {
            (self.seen, self.next, self.worked) = (now, None, true);
            return None;
        }
        if now == self.seen {
            self.next = None;
            return None;
        }
        let held = match self.next {
            Some((s, n)) if s == now => n + 1,
            _ => 1,
        };
        if held < SETTLE_TICKS {
            self.next = Some((now, held));
            return None;
        }
        (self.seen, self.next) = (now, None);
        let event = match now {
            AgentStatus::NeedsInput => Some("needs input"),
            AgentStatus::Error => Some("hit an error"),
            AgentStatus::Done | AgentStatus::Idle if self.worked => Some("finished"),
            _ => None,
        };
        if event.is_some() {
            self.worked = false;
        }
        event
    }
}

/// What an orchestrator said at the end of its turn (its text after its last tool call), or
/// `None` when it stays silent.
pub fn answer_of(items: &[Item]) -> Option<String> {
    let start = items.iter().rposition(|i| matches!(i, Item::User { .. })).map_or(0, |n| n + 1);
    let turn = &items[start.min(items.len())..];
    let texts = |from: usize| -> Vec<&str> {
        turn[from..].iter().filter_map(|i| if let Item::Text { text, .. } = i { Some(text.trim()) } else { None }).filter(|t| !t.is_empty()).collect()
    };
    let after_tools = turn.iter().rposition(|i| matches!(i, Item::Tool { .. })).map_or(0, |n| n + 1);
    let mut said = texts(after_tools);
    if said.is_empty() {
        said = texts(0).into_iter().last().into_iter().collect();
    }
    let text = said.join(" ");
    let bare = text.trim().trim_end_matches('.').trim();
    (!bare.is_empty() && !bare.eq_ignore_ascii_case(SILENT)).then(|| text.chars().take(MAX_SPOKEN).collect())
}

/// The tool an orchestrator is running.
pub fn current_step(items: &[Item]) -> Option<String> {
    items.iter().rev().find_map(|i| match i {
        Item::Tool { name, title, ended: None, .. } => Some(if title.trim().is_empty() { name.clone() } else { title.trim().chars().take(60).collect() }),
        _ => None,
    })
}

/// An event about an agent an orchestrator follows: `place` is "api/2" (or "pane 2" in a fork's
/// own workspace), `task` the task it works on, if the event goes to the conductor.
pub fn pane_event(task: Option<&HomeTask>, place: &str, agent: &str, branch: Option<&str>, what: &str, message: Option<&str>) -> String {
    let branch = branch.map(|b| format!(", branch {b}")).unwrap_or_default();
    let message = message.map(str::trim).filter(|m| !m.is_empty()).map(|m| format!(": {}", m.chars().take(200).collect::<String>())).unwrap_or_default();
    let task = task.map(|t| format!("{} · ", t.title())).unwrap_or_default();
    let place = if place.starts_with("pane ") { format!("Pane {}", &place[5..]) } else { place.to_string() };
    let text = format!("[winfer] {task}{place} ({agent}{branch}) {what}{message}");
    if text.ends_with(['.', '?', '!']) { text } else { format!("{text}.") }
}

/// winfer's UI actions as MCP tools, for orchestrators.
pub fn ui_tools(providers: &[(String, String)]) -> Vec<Value> {
    voice_tools::registry_tools(providers).into_iter().map(|t| json!({ "name": t["name"], "description": t["description"], "inputSchema": t["parameters"] })).collect()
}

/// Home's own tools: the conductor's and a task orchestrator's.
/// `agent`: the agent the user set for tasks (Settings → Home), if any.
pub fn home_tools(conductor: bool, agent: Option<&str>) -> Vec<Value> {
    let provider = match agent {
        Some(a) => format!("The agent to start. Leave it out: tasks run as {a} (the user's setting). Only give one when the user asked for a specific agent in this request."),
        None => "Light: the agent to start (installed ids, e.g. claude, codex, winfer). Fork: the agent its orchestrator starts (optional)".to_string(),
    };
    let task = json!({ "type": "string", "description": "Task number (3, \"Task 3\") or id, from list_tasks" });
    let stage = json!({ "type": "string", "enum": ["planning", "running", "checking", "needs-you", "ready", "failed", "merged", "cancelled"] });
    let model = json!({ "type": "string", "description": "A model spec from list_models (e.g. claude/opus, codex/gpt-…, agy/gemini-…), chosen with your model guide" });
    let effort = json!({ "type": "string", "description": "Reasoning effort the model offers (list_models): low, medium, high, …; default: the model's" });
    let mut tools = vec![json!({ "name": "list_models", "description": "List the models task agents can run on (WinferCode runs Claude, Codex/GPT and Gemini models): spec, name, effort levels, and each provider's usage left.", "inputSchema": { "type": "object", "properties": {} } })];
    tools.push(json!({ "name": "list_tasks", "description": "List Home's tasks: number, stage, workspace, how it runs (light, fork, remote), its model, its agents and summary. Done ones (merged, cancelled) only with all: true.", "inputSchema": { "type": "object", "properties": { "all": { "type": "boolean" } } } }));
    if conductor {
        tools.extend([
            json!({ "name": "start_task", "description": "Start a coding task in a workspace and get a card for it on Home (not for chores that change no project code, like running commands or updating an app). Light (default): one agent with your prompt, which you follow yourself; give a complete standalone prompt. fork: true: a task orchestrator plans, runs and checks it there and reports to you; give the goal and what you know. Workspaces on other machines always fork, on that machine.", "inputSchema": { "type": "object", "properties": {
                "workspace": { "type": "string", "description": "Workspace name from list_workspaces; \"machine/name\" for another machine's" },
                "goal": { "type": "string", "description": "One line: what the task achieves (the card's title)" },
                "prompt": { "type": "string", "description": "Light: the agent's complete prompt. Fork: context for the task orchestrator (what you found, constraints)" },
                "provider": { "type": "string", "description": provider },
                "fork": { "type": "boolean" },
                "worktree": { "type": "boolean", "description": "Light: its own git worktree (default true)" },
                "model": model.clone(),
                "effort": effort.clone(),
                "reason": { "type": "string", "description": "One line: why this model (shown on the task)" }
            }, "required": ["workspace", "goal"] } }),
            json!({ "name": "set_task_model", "description": "Switch a task's agents to another model (any provider's, for WinferCode agents) mid-run: from their next turn, WinferCode hands their conversation over, so nothing is lost. For a remote task, its orchestrator there switches them.", "inputSchema": { "type": "object", "properties": {
                "task": task.clone(), "model": model.clone(), "effort": effort.clone(),
                "pane": { "type": "string", "description": "Only this agent of the task (\"workspace/N\"); default: all of them" },
                "reason": { "type": "string", "description": "One line: why (shown on the task)" }
            }, "required": ["task", "reason"] } }),
            json!({ "name": "fork_task", "description": "Promote a light task to a forked one: a task orchestrator takes over its agent(s) and history. Use when it grows: the agent failed twice, asks design questions, or the user added scope.", "inputSchema": { "type": "object", "properties": { "task": task, "context": { "type": "string", "description": "What happened so far and what's next" } }, "required": ["task", "context"] } }),
            json!({ "name": "task_say", "description": "Send a message to a forked or remote task's orchestrator (instructions, answers, the user's wishes).", "inputSchema": { "type": "object", "properties": { "task": task, "text": { "type": "string" } }, "required": ["task", "text"] } }),
            json!({ "name": "update_task", "description": "Set a task's stage and/or one-line summary on its card.", "inputSchema": { "type": "object", "properties": { "task": task, "stage": stage, "summary": { "type": "string" } }, "required": ["task"] } }),
            json!({ "name": "close_task", "description": "Close a task's card: merged when its work landed, cancelled when dropped. Its orchestrator (if any) stops; its agent panes stay.", "inputSchema": { "type": "object", "properties": { "task": task, "stage": { "type": "string", "enum": ["merged", "cancelled"] } }, "required": ["task"] } }),
        ]);
    } else {
        tools.push(json!({ "name": "update_task", "description": "Set your task's stage and/or one-line summary on its card on Home.", "inputSchema": { "type": "object", "properties": { "stage": stage, "summary": { "type": "string" } } } }));
        tools.push(json!({ "name": "set_task_model", "description": "Switch your task's agents to another model mid-run (any provider's, for WinferCode agents): from their next turn, WinferCode hands their conversation over. New agents: pass model to spawn_agent.", "inputSchema": { "type": "object", "properties": {
            "model": model, "effort": effort,
            "pane": { "type": "string", "description": "Only this agent (its number in list_panes); default: all of the task's" },
            "reason": { "type": "string", "description": "One line: why (shown on the task)" }
        }, "required": ["reason"] } }));
    }
    tools
}

/// Pane ids in the order agents number them: tiled, then archived.
pub(super) fn numbered(ws: &Workspace) -> Vec<String> {
    let mut ids = ws.layout.as_ref().map(leaves).unwrap_or_default();
    ids.extend(ws.panes.iter().filter(|p| p.archived).map(|p| p.id.clone()));
    ids
}

/// "api/2": a pane's global address on this machine.
pub(super) fn address(ws: &Workspace, pane_id: &str) -> String {
    let n = numbered(ws).iter().position(|p| p == pane_id).map_or(0, |n| n + 1);
    format!("{}/{n}", ws.name)
}

/// Settings → Home.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct HomeSettings {
    pub conductor_model: Option<String>,
    pub task_model: Option<String>,
    /// The agent tasks start (provider id); None: the orchestrators pick.
    pub task_agent: Option<String>,
    pub open_on_launch: bool,
    pub brief: bool,
}

pub(super) fn home_settings(s: &AppState) -> HomeSettings {
    let home = s.rest.get("home");
    let text = |v: Option<&Value>| v.and_then(Value::as_str).map(str::trim).filter(|m| !m.is_empty() && *m != "default").map(str::to_string);
    HomeSettings {
        // Before Home, voice's orchestrator had its own model setting.
        conductor_model: text(home.map(|h| &h["conductorModel"])).or_else(|| text(s.rest.get("voice").map(|v| &v["orchestratorModel"]))),
        task_model: text(home.map(|h| &h["taskModel"])),
        // Default: WinferCode (when it's installed); "auto": the orchestrators pick.
        task_agent: match text(home.map(|h| &h["taskAgent"])) {
            Some(a) if a == "auto" => None,
            Some(a) => Some(a),
            None => s.providers.iter().any(|p| p.id == "winfer" && p.installed).then(|| "winfer".to_string()),
        },
        open_on_launch: home.and_then(|h| h["openOnLaunch"].as_bool()).unwrap_or(true),
        brief: home.and_then(|h| h["brief"].as_bool()).unwrap_or(true),
    }
}

impl Root {
    // ---- tasks: load, save ----

    /// Reads the tasks from the saved state, once it's loaded.
    pub(super) fn load_home(&mut self, cx: &mut Context<Self>) {
        if self.home.loaded || !self.state.read(cx).hydrated {
            return;
        }
        self.home.loaded = true;
        let s = self.state.read(cx);
        self.home.tasks = s.rest.get("home").and_then(|h| serde_json::from_value(h["tasks"].clone()).ok()).unwrap_or_default();
        self.home.models = s.rest.get("home").and_then(|h| serde_json::from_value(h["models"].clone()).ok()).unwrap_or_default();
        // Panes that closed while winfer was off.
        let live: HashSet<String> = s.workspaces.iter().flat_map(|w| w.panes.iter().map(|p| p.id.clone())).collect();
        for t in &mut self.home.tasks {
            t.panes.retain(|p| live.contains(p));
        }
        // The light tasks' agents: the conductor hears of them again (once it runs).
        if s.view() == View::Home || home_settings(s).open_on_launch {
            self.home.left_at = None;
        }
        self.settle_home_tasks(cx);
    }

    // ---- tasks: finishing ----

    /// Remembers the tasks' worktrees, marks tasks Merged once all their work is in, and removes
    /// done tasks that have no agent pane and no worktree left.
    pub(super) fn settle_home_tasks(&mut self, cx: &mut Context<Self>) {
        if !self.home.loaded {
            return;
        }
        let s = self.state.read(cx);
        let exists = |w: &str| std::path::Path::new(w).exists();
        let mut changed = false;
        let mut gone = Vec::new();
        for t in &mut self.home.tasks {
            changed |= note_worktrees(t, s);
            // An agent still waiting for its worktree has work that isn't merged.
            let unborn = t.panes.iter().filter_map(|p| s.pane(p)).any(|p| p.wants_worktree && p.worktree.is_none());
            if t.stage != Stage::Merged && !unborn && all_merged(t, exists) {
                t.stage = Stage::Merged;
                t.updated = now_secs();
                changed = true;
            }
            // Its orchestrator here finishes its turn (and reports) first.
            if self.home.forks.get(&t.id).is_some_and(|o| o.model.read(cx).state.busy) {
                continue;
            }
            let mirror = t.machine.as_ref().filter(|p| crate::connect::get().is_some_and(|c| c.is_up(p))).and_then(|p| self.connect.mirrors.get(p)).filter(|m| m.loaded);
            let starting = self.home.pending_remote.get(&t.id).is_some_and(|at| at.elapsed() <= Duration::from_secs(120));
            if finished(t, |p| s.pane(p).is_some(), exists, remote_pane_open(t, mirror, starting)) {
                gone.push(t.id.clone());
            }
        }
        for id in &gone {
            self.home.tasks.retain(|t| t.id != *id);
            self.home.remote_watch.remove(id);
            self.home.pending_remote.remove(id);
            self.home.remote_calls.retain(|_, task| task != id);
            self.stop_fork(id, cx);
        }
        if changed || !gone.is_empty() {
            self.save_home(cx);
            cx.notify();
        }
    }

    /// A pane's worktree was merged back: its task counts it (and is Merged once all are in).
    pub(super) fn home_worktree_merged(&mut self, pane: &Pane, cx: &mut Context<Self>) {
        let Some(path) = pane.worktree.as_ref().map(|w| w.path.clone()) else { return };
        self.load_home(cx);
        let mut changed = false;
        for t in &mut self.home.tasks {
            if !t.panes.contains(&pane.id) && !t.worktrees.contains(&path) {
                continue;
            }
            for list in [&mut t.worktrees, &mut t.merged] {
                if !list.contains(&path) {
                    list.push(path.clone());
                }
            }
            t.updated = now_secs();
            changed = true;
        }
        if changed {
            self.save_home(cx);
            self.settle_home_tasks(cx);
            cx.notify();
        }
    }

    pub(super) fn save_home(&mut self, cx: &mut Context<Self>) {
        let tasks = serde_json::to_value(&self.home.tasks).unwrap_or_default();
        self.update_state(cx, |s| {
            let entry = s.rest.entry("home".to_string()).or_insert_with(|| json!({}));
            if !entry.is_object() {
                *entry = json!({});
            }
            entry["tasks"] = tasks;
        });
        self.mobile_home_status(cx);
    }

    pub(super) fn task(&self, id: &str) -> Option<&HomeTask> {
        self.home.tasks.iter().find(|t| t.id == id)
    }

    fn task_mut(&mut self, id: &str) -> Option<&mut HomeTask> {
        self.home.tasks.iter_mut().find(|t| t.id == id)
    }

    /// Changes a task's stage and summary; true when something changed.
    pub(super) fn set_task(&mut self, id: &str, stage: Option<Stage>, summary: Option<&str>, cx: &mut Context<Self>) -> bool {
        if stage.is_some_and(Stage::open) {
            reopen(&mut self.home.tasks, id);
        }
        let Some(t) = self.task_mut(id) else { return false };
        let mut changed = false;
        if let Some(stage) = stage.filter(|s| *s != t.stage) {
            t.stage = stage;
            changed = true;
        }
        if let Some(summary) = summary.map(str::trim).filter(|s| *s != t.summary) {
            t.summary = summary.chars().take(240).collect();
            changed = true;
        }
        if changed {
            t.updated = now_secs();
            self.save_home(cx);
            self.settle_home_tasks(cx);
            cx.notify();
        }
        changed
    }

    // ---- orchestrator sessions ----

    fn orch(&self, pane_id: &str) -> Option<&Orch> {
        match home_pane(pane_id)? {
            HomePane::Conductor => self.home.conductor.as_ref(),
            HomePane::Task(t) => self.home.forks.get(t),
        }
    }

    fn orch_mut(&mut self, pane_id: &str) -> Option<&mut Orch> {
        match home_pane(pane_id)? {
            HomePane::Conductor => self.home.conductor.as_mut(),
            HomePane::Task(t) => self.home.forks.get_mut(t),
        }
    }

    /// The chat model of a Home orchestrator that runs, for the Home view.
    pub(super) fn home_model(&self, pane_id: &str) -> Option<Entity<CodeModel>> {
        self.orch(pane_id).map(|o| o.model.clone())
    }

    /// Starts a hidden winfer code session for an orchestrator.
    #[allow(clippy::too_many_arguments)]
    fn start_orch(&mut self, pane_id: &str, cwd: String, workspace_id: String, model: Option<String>, role: &str, resume: Option<String>, cx: &mut Context<Self>) -> Result<Entity<CodeModel>, String> {
        let (pane, settings) = {
            let s = self.state.read(cx);
            if !s.providers.iter().any(|p| p.id == "winfer" && p.installed) {
                return Err("winfer code isn't available, so Home's orchestrator can't start.".into());
            }
            let title = if pane_id == CONDUCTOR { "Home".to_string() } else { "Task".to_string() };
            let pane = Pane { id: pane_id.to_string(), provider: "winfer".into(), title, cwd, workspace_id, model, resume: resume.is_some(), session_id: resume, ..Default::default() };
            let mut settings = terminal::SpawnSettings::of(s, &pane);
            // Autonomous: merges, discards and closing still need the user's yes (see `orchestrator_action`).
            settings.auto_approve = true;
            let guide = routing_guide();
            settings.system_prompt = [role.trim(), guide.trim(), settings.system_prompt.trim()].into_iter().filter(|p| !p.is_empty()).collect::<Vec<_>>().join("\n\n");
            (pane, settings)
        };
        let session = SESSIONS.get_or_create(&pane.id, &self.events);
        session.resize(200, 50);
        let (events, app) = (self.events.clone(), self.state.clone());
        let model = cx.new(|cx| CodeModel::new(pane, settings, session, events, app, cx));
        let (changed, turn) = (pane_id.to_string(), pane_id.to_string());
        let subscriptions = vec![
            cx.observe(&model, move |this, _, cx| this.on_orch_changed(&changed, cx)),
            cx.subscribe(&model, move |this, _, _: &CodeEvent, cx| this.on_orch_turn(&turn, cx)),
        ];
        let orch = Orch { model: model.clone(), queue: Vec::new(), owned: HashMap::new(), handled: HashSet::new(), step: None, _subscriptions: subscriptions };
        match home_pane(pane_id) {
            Some(HomePane::Task(t)) => {
                self.home.forks.insert(t.to_string(), orch);
            }
            _ => self.home.conductor = Some(orch),
        }
        Ok(model)
    }

    /// The conductor, started on first use (resuming its last conversation).
    pub(super) fn conductor(&mut self, cx: &mut Context<Self>) -> Result<Entity<CodeModel>, String> {
        if let Some(o) = &self.home.conductor {
            return Ok(o.model.clone());
        }
        self.load_home(cx);
        let s = self.state.read(cx);
        let settings = home_settings(s);
        let resume = s.rest.get("home").and_then(|h| h["conductorSession"].as_str()).filter(|v| !v.is_empty()).map(str::to_string);
        let cwd = home_dir();
        let model = self.start_orch(CONDUCTOR, cwd, String::new(), settings.conductor_model, CONDUCTOR_ROLE, resume, cx)?;
        self.sync_home_mentions(cx);
        // Its light tasks' agents: it hears of them again.
        let panes: Vec<String> = self.home.tasks.iter().filter(|t| t.mode == TaskMode::Light && t.stage.open()).flat_map(|t| t.panes.clone()).collect();
        for p in panes {
            self.home_owns(CONDUCTOR, &p, cx);
        }
        Ok(model)
    }

    /// A forked task's orchestrator here, started (or resumed) on first use.
    fn fork(&mut self, task_id: &str, cx: &mut Context<Self>) -> Result<Entity<CodeModel>, String> {
        if let Some(o) = self.home.forks.get(task_id) {
            return Ok(o.model.clone());
        }
        let task = self.task(task_id).cloned().ok_or("That task is gone.")?;
        let s = self.state.read(cx);
        let ws = task.workspace_id.as_ref().and_then(|w| s.workspaces.iter().find(|x| x.id == *w)).ok_or_else(|| format!("Workspace {} isn't open anymore.", task.workspace))?;
        let (cwd, ws_id) = (ws.root_dir.clone(), ws.id.clone());
        let model = home_settings(s).task_model;
        let model = self.start_orch(&task_pane(task_id), cwd, ws_id, model, TASK_ROLE, task.session.clone(), cx)?;
        for p in &task.panes {
            self.home_owns(&task_pane(task_id), p, cx);
        }
        Ok(model)
    }

    /// Starts (or resumes) a forked task's orchestrator, e.g. when the user opens its thread.
    pub(super) fn open_fork(&mut self, task_id: &str, cx: &mut Context<Self>) -> Result<(), String> {
        self.fork(task_id, cx).map(|_| ())
    }

    /// Ends the conductor; the next request starts a new conversation.
    pub(super) fn stop_conductor(&mut self, cx: &mut Context<Self>) {
        if self.home.conductor.take().is_some() {
            terminal::close(CONDUCTOR);
            SESSIONS.remove(CONDUCTOR);
        }
        self.home.views.remove(CONDUCTOR);
        self.home.follows = Default::default();
        self.update_state(cx, |s| {
            if let Some(h) = s.rest.get_mut("home").and_then(Value::as_object_mut) {
                h.remove("conductorSession");
            }
        });
        cx.notify();
    }

    fn stop_fork(&mut self, task_id: &str, cx: &mut Context<Self>) {
        let pane = task_pane(task_id);
        if self.home.forks.remove(task_id).is_some() {
            terminal::close(&pane);
            SESSIONS.remove(&pane);
        }
        self.home.views.remove(&pane);
        if self.home.thread.as_deref() == Some(task_id) {
            self.home.thread = None;
        }
        cx.notify();
    }

    /// Hands a request the user made by voice to the conductor.
    pub(super) fn orchestrate(&mut self, request: &str, fresh: bool, cx: &mut Context<Self>) -> Result<String, String> {
        if fresh {
            self.stop_conductor(cx);
        }
        let request = request.trim();
        if request.is_empty() {
            return if fresh { Ok("Home's orchestrator starts fresh with the next request.".into()) } else { Err("What should the orchestrator do?".into()) };
        }
        let model = self.conductor(cx)?;
        let busy = model.read(cx).state.busy;
        self.home.voice_turn = true;
        model.update(cx, |m, cx| m.submit(&format!("User (via voice): {request}"), None, cx));
        cx.notify();
        Ok(if busy {
            "Passed on to Home's orchestrator while it works; it answers in an [winfer] Orchestrator message.".into()
        } else {
            "Handed to Home's orchestrator; it answers in an [winfer] Orchestrator message.".into()
        })
    }

    pub(super) fn has_orchestrator(&self, _cx: &App) -> bool {
        self.home.conductor.is_some()
    }

    /// What the conductor is doing, while it works.
    pub(super) fn orchestrator_step(&self, cx: &App) -> Option<String> {
        let o = self.home.conductor.as_ref()?;
        o.model.read(cx).state.busy.then(|| o.step.clone().unwrap_or_else(|| "thinking".into()))
    }

    /// The step a forked task's orchestrator is on, while it works.
    pub(super) fn fork_step(&self, task_id: &str, cx: &App) -> Option<String> {
        let o = self.home.forks.get(task_id)?;
        o.model.read(cx).state.busy.then(|| o.step.clone().unwrap_or_else(|| "thinking".into()))
    }

    /// Whether the user sees `pane_id`'s chat right now.
    fn home_thread_on_screen(&self, pane_id: &str, cx: &App) -> bool {
        self.state.read(cx).view() == View::Home && status::window_focused() && self.home_thread_pane() == pane_id
    }

    pub(super) fn home_thread_pane(&self) -> String {
        self.home.thread.as_deref().map_or_else(|| CONDUCTOR.to_string(), task_pane)
    }

    /// Answers what the session asks when nobody can, and follows its step.
    fn on_orch_changed(&mut self, pane_id: &str, cx: &mut Context<Self>) {
        // The conductor's questions wait for the user on Home, unless it works for voice.
        let user_answers = pane_id == CONDUCTOR && !self.home.voice_turn;
        let on_screen = self.home_thread_on_screen(pane_id, cx);
        let Some(o) = self.orch_mut(pane_id) else { return };
        let (asks, step, session) = {
            let st = &o.model.read(cx).state;
            let asks: Vec<(String, usize)> = st.approvals.iter().filter(|a| !o.handled.contains(&a.id)).map(|a| (a.id.clone(), a.questions.len())).collect();
            (asks, if st.busy { current_step(&st.items) } else { None }, st.backend_session.clone())
        };
        let mut asked = false;
        for (id, questions) in asks {
            if questions > 0 && user_answers {
                asked = true;
                o.handled.insert(id);
                continue;
            }
            o.handled.insert(id.clone());
            let cmd = if questions == 0 { Cmd::Approve { id, decision: Decision::Allow } } else { Cmd::Answer { id, answers: vec![NO_PICKER.to_string(); questions] } };
            o.model.update(cx, |m, cx| m.send(cmd, cx));
        }
        let step_changed = o.step != step;
        if step_changed {
            o.step = step;
        }
        if asked && !on_screen {
            self.orchestrator_says(pane_id, "Home's orchestrator has a question for you.", cx);
        }
        self.remember_session(pane_id, session, cx);
        if pane_id == CONDUCTOR {
            if step_changed {
                self.home.touched_ms = now_secs() * 1000;
            }
            self.mobile_home_changed(cx);
        }
        if step_changed {
            cx.notify();
        }
    }

    /// Keeps an orchestrator's session id, to resume it after a restart.
    fn remember_session(&mut self, pane_id: &str, session: Option<String>, cx: &mut Context<Self>) {
        let Some(session) = session else { return };
        match home_pane(pane_id) {
            Some(HomePane::Conductor) => {
                let saved = self.state.read(cx).rest.get("home").and_then(|h| h["conductorSession"].as_str()).map(str::to_string);
                if saved.as_deref() != Some(session.as_str()) {
                    self.update_state(cx, |s| {
                        let entry = s.rest.entry("home".to_string()).or_insert_with(|| json!({}));
                        if !entry.is_object() {
                            *entry = json!({});
                        }
                        entry["conductorSession"] = json!(session);
                    });
                }
            }
            Some(HomePane::Task(t)) => {
                let t = t.to_string();
                if let Some(task) = self.task_mut(&t).filter(|x| x.session.as_deref() != Some(session.as_str())) {
                    task.session = Some(session);
                    self.save_home(cx);
                }
            }
            None => {}
        }
    }

    /// An orchestrator ended a turn: pass its answer on, then hand it what happened meanwhile.
    fn on_orch_turn(&mut self, pane_id: &str, cx: &mut Context<Self>) {
        let Some(o) = self.orch_mut(pane_id) else { return };
        o.step = None;
        let answer = answer_of(&o.model.read(cx).state.items);
        match home_pane(pane_id) {
            Some(HomePane::Conductor) => {
                if let Some(text) = answer {
                    self.orchestrator_says(CONDUCTOR, &text, cx);
                }
                self.home.voice_turn = false;
            }
            Some(HomePane::Task(t)) => {
                let t = t.to_string();
                if let Some(text) = answer {
                    self.task_reports(&t, &text, cx);
                }
            }
            None => {}
        }
        self.deliver_home_events(pane_id, cx);
        cx.notify();
    }

    /// A task's orchestrator reported: the conductor hears it (and the card shows it).
    fn task_reports(&mut self, task_id: &str, text: &str, cx: &mut Context<Self>) {
        let Some(task) = self.task(task_id).cloned() else { return };
        // A remote orchestrator has no update_task: it starts its report with the stage.
        let stage = text.trim().strip_prefix("Stage:").and_then(|r| Stage::parse(r.split(['.', '\n', ',']).next().unwrap_or_default()));
        let first: String = text.lines().map(str::trim).find(|l| !l.is_empty() && !l.starts_with("Stage:")).unwrap_or_default().chars().take(200).collect();
        self.set_task(task_id, stage, (!first.is_empty()).then_some(first.as_str()), cx);
        let note = format!("[winfer] {} ({}): {}", task.title(), task.workspace, text.chars().take(MAX_SPOKEN).collect::<String>());
        self.queue_for(CONDUCTOR, note, cx);
    }

    /// Puts an event in an orchestrator's queue (starting the conductor if needed) and delivers it
    /// once it's free.
    pub(super) fn queue_for(&mut self, pane_id: &str, text: String, cx: &mut Context<Self>) {
        if pane_id == CONDUCTOR && self.home.conductor.is_none() && self.conductor(cx).is_err() {
            return;
        }
        if let Some(o) = self.orch_mut(pane_id) {
            o.queue.push(text);
        }
        self.deliver_home_events(pane_id, cx);
    }

    /// Tells the user what the conductor said: spoken while it works for live voice; else in its
    /// chat on Home, plus a toast (and a notification when winfer isn't in front) when the user
    /// isn't looking at it.
    pub(super) fn orchestrator_says(&mut self, pane_id: &str, text: &str, cx: &mut Context<Self>) {
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        #[cfg(debug_assertions)]
        if std::env::var_os("WINFER_NATIVE_ORCHESTRATE").is_some() {
            eprintln!("[orchestrator] {text}");
        }
        let who = match home_pane(pane_id) {
            Some(HomePane::Task(t)) => self.task(t).map_or_else(|| "Task".to_string(), |t| format!("{} · {}", t.title(), t.workspace)),
            _ => "Home".to_string(),
        };
        // While live voice runs, it speaks what the user isn't reading on Home.
        if self.live_voice_on() && (self.home.voice_turn || !self.home_thread_on_screen(pane_id, cx)) {
            self.voice_note(format!("Orchestrator: {text}"));
            return;
        }
        if self.home_thread_on_screen(pane_id, cx) {
            return;
        }
        let short: String = text.chars().take(MAX_TOAST).collect();
        self.toast(format!("{who}: {short}"), cx);
        if !status::window_focused() {
            let notification = json!({ "paneId": pane_id, "title": who, "body": short, "kind": "info" });
            backend::background(move || {
                if let Ok(n) = serde_json::from_value(notification) {
                    let _ = winfer_backend::notify::notify_action(backend::app().clone(), n);
                }
            });
        }
    }

    /// Writes the waiting events into an orchestrator once it's free.
    fn deliver_home_events(&mut self, pane_id: &str, cx: &mut Context<Self>) {
        let Some(o) = self.orch_mut(pane_id) else { return };
        if o.queue.is_empty() || o.model.read(cx).state.busy {
            return;
        }
        let text = std::mem::take(&mut o.queue).join("\n");
        let model = o.model.clone();
        model.update(cx, |m, cx| m.submit(&text, None, cx));
    }

    /// An orchestrator follows `pane_id`: it hears when that agent finishes or needs something.
    pub(super) fn home_owns(&mut self, orch_pane: &str, pane_id: &str, cx: &App) {
        let status = self.state.read(cx).pane(pane_id).map_or(AgentStatus::Idle, |p| p.status);
        if let Some(o) = self.orch_mut(orch_pane) {
            o.owned.entry(pane_id.to_string()).or_insert_with(|| Watch::new(status));
        }
    }

    /// A note for a Home orchestrator (a background task of its own exited); false when
    /// `pane_id` isn't one.
    pub(super) fn orchestrator_note(&mut self, pane_id: &str, text: String, cx: &mut Context<Self>) -> bool {
        if home_pane(pane_id).is_none() {
            return false;
        }
        if let Some(o) = self.orch_mut(pane_id) {
            o.queue.push(text);
        }
        self.deliver_home_events(pane_id, cx);
        true
    }

    /// A Home orchestrator's process exited; false when `pane_id` isn't one.
    pub(super) fn orchestrator_exited(&mut self, pane_id: &str, code: u32, cx: &mut Context<Self>) -> bool {
        let Some(which) = home_pane(pane_id) else { return false };
        let gone = match which {
            HomePane::Conductor => self.home.conductor.take(),
            HomePane::Task(t) => self.home.forks.remove(t),
        };
        if let Some(o) = gone {
            SESSIONS.remove(pane_id);
            self.home.views.remove(pane_id);
            let busy = o.model.read(cx).state.busy;
            if code != 0 || busy {
                let text = format!("The orchestrator stopped (exit code {code}). The next message starts it again.");
                self.orchestrator_says(pane_id, &text, cx);
            }
            cx.notify();
        }
        true
    }

    /// Every second: tells orchestrators what their agents did, follows remote tasks, keeps the
    /// cards' stages current, and hands over what waited for the orchestrators to be free.
    pub(super) fn tick_home(&mut self, cx: &mut Context<Self>) {
        self.load_home(cx);
        self.sync_home_mentions(cx);
        self.tick_models(cx);
        self.tick_follows(cx);
        // Catches what closed or was removed by other ways (another machine, git, the folder).
        self.settle_home_tasks(cx);
        if self.home.conductor.is_none() && self.home.forks.is_empty() && !self.home.tasks.iter().any(|t| t.stage.open()) {
            return;
        }
        // Forks of tasks that are gone or done (once idle), and of workspaces that closed.
        let open: HashSet<String> = self.state.read(cx).workspaces.iter().map(|w| w.id.clone()).collect();
        let stale: Vec<String> = self
            .home
            .forks
            .keys()
            .filter(|t| {
                let busy = self.home.forks.get(*t).is_some_and(|o| o.model.read(cx).state.busy);
                self.task(t).is_none_or(|x| (!x.stage.open() && !busy) || x.workspace_id.as_ref().is_none_or(|w| !open.contains(w)))
            })
            .cloned()
            .collect();
        for t in stale {
            self.stop_fork(&t, cx);
        }
        let mut orchs: Vec<String> = self.home.forks.keys().map(|t| task_pane(t)).collect();
        if self.home.conductor.is_some() {
            orchs.push(CONDUCTOR.to_string());
        }
        for pane in orchs {
            let events = self.watch_home_panes(&pane, cx);
            if let Some(o) = self.orch_mut(&pane) {
                o.queue.extend(events);
            }
            self.deliver_home_events(&pane, cx);
        }
        self.tick_light_stages(cx);
        self.tick_remote_tasks(cx);
    }

    fn watch_home_panes(&mut self, orch_pane: &str, cx: &mut Context<Self>) -> Vec<String> {
        let conductor = orch_pane == CONDUCTOR;
        let tasks = self.home.tasks.clone();
        let s = self.state.read(cx);
        let Some(o) = (match home_pane(orch_pane) {
            Some(HomePane::Conductor) => self.home.conductor.as_mut(),
            Some(HomePane::Task(t)) => self.home.forks.get_mut(t),
            None => None,
        }) else {
            return Vec::new();
        };
        o.owned.retain(|id, _| s.pane(id).is_some());
        let mut events = Vec::new();
        for (id, watch) in o.owned.iter_mut() {
            let Some(pane) = s.pane(id) else { continue };
            let Some(what) = watch.tick(pane.status) else { continue };
            let Some(ws) = s.workspaces.iter().find(|w| w.id == pane.workspace_id) else { continue };
            let place = if conductor { address(ws, id) } else { format!("pane {}", numbered(ws).iter().position(|p| p == id).map_or(0, |n| n + 1)) };
            let task = conductor.then(|| tasks.iter().find(|t| t.panes.contains(id))).flatten();
            let message = s.status_messages.get(id).map(String::as_str);
            events.push(pane_event(task, &place, &s.provider_name(&pane.provider), pane.worktree.as_ref().map(|t| t.branch.as_str()), what, message));
        }
        events
    }

    /// Light tasks' cards follow their agent: running while it works, needs-you when it asks.
    /// (Checking and ready are the conductor's call, through update_task.)
    fn tick_light_stages(&mut self, cx: &mut Context<Self>) {
        let s = self.state.read(cx);
        let updates: Vec<(String, Stage)> = self
            .home
            .tasks
            .iter()
            .filter(|t| t.mode == TaskMode::Light && t.stage.open() && t.stage != Stage::Ready)
            .filter_map(|t| {
                let statuses: Vec<AgentStatus> = t.panes.iter().filter_map(|p| s.pane(p)).map(|p| p.status).collect();
                light_stage(t.stage, &statuses).map(|next| (t.id.clone(), next))
            })
            .collect();
        for (id, stage) in updates {
            self.set_task(&id, Some(stage), None, cx);
        }
    }

    /// Remote tasks: finds their orchestrator on the other machine once it shows up there, and
    /// passes on what it reports.
    fn tick_remote_tasks(&mut self, cx: &mut Context<Self>) {
        let remote: Vec<HomeTask> = self.home.tasks.iter().filter(|t| t.mode == TaskMode::Remote && t.stage.open()).cloned().collect();
        for task in remote {
            let Some(peer) = task.machine.clone() else { continue };
            let Some(mirror) = self.connect.mirrors.get(&peer) else { continue };
            let found = mirror.panes.iter().find(|p| task.orchestrated_by(p)).cloned();
            let Some(pane) = found else {
                // Asked to start a while ago and never showed up.
                if let Some(at) = self.home.pending_remote.get(&task.id)
                    && at.elapsed() > Duration::from_secs(120)
                {
                    self.home.pending_remote.remove(&task.id);
                    self.set_task(&task.id, Some(Stage::Failed), Some("Its orchestrator didn't start on the other machine."), cx);
                }
                continue;
            };
            self.home.pending_remote.remove(&task.id);
            if task.remote_pane.is_none() {
                if let Some(t) = self.task_mut(&task.id) {
                    t.remote_pane = Some(pane.id.clone());
                }
                self.save_home(cx);
            }
            let watch = self.home.remote_watch.entry(task.id.clone()).or_insert_with(|| Watch::new(pane.status));
            let what = watch.tick(pane.status);
            if pane.status == AgentStatus::Working && task.stage == Stage::Planning {
                self.set_task(&task.id, Some(Stage::Running), None, cx);
            }
            let Some(what) = what else { continue };
            match what {
                "finished" => {
                    let report = pane.reply.clone().or(pane.message.clone()).unwrap_or_default();
                    if report.trim().is_empty() || report.trim().trim_end_matches('.').eq_ignore_ascii_case(SILENT) {
                        continue;
                    }
                    self.task_reports(&task.id, &report, cx);
                }
                other => {
                    let stage = if other == "needs input" { Stage::NeedsYou } else { Stage::Failed };
                    self.set_task(&task.id, Some(stage), None, cx);
                    let name = crate::connect::peer_name(&peer).unwrap_or_else(|| "the other machine".into());
                    let detail = pane.request_text.clone().or(pane.message.clone());
                    let event = pane_event(Some(&task), &format!("its orchestrator on {name}"), "winfer", None, other, detail.as_deref());
                    self.queue_for(CONDUCTOR, event, cx);
                }
            }
        }
    }

    /// A paired machine answered a remote task's tool call; false when `id` isn't one of Home's.
    pub(super) fn home_tool_result(&mut self, id: &str, ok: bool, text: &str, cx: &mut Context<Self>) -> bool {
        let Some(task) = self.home.remote_calls.remove(id) else { return false };
        if !ok {
            self.home.pending_remote.remove(&task);
            let title = self.task(&task).map(HomeTask::title).unwrap_or_default();
            self.set_task(&task, Some(Stage::Failed), Some(text), cx);
            self.queue_for(CONDUCTOR, format!("[winfer] {title}: the other machine said: {text}"), cx);
        }
        true
    }

    /// Every workspace here and on the paired machines, as `@` mentions for Home's chat.
    pub(super) fn workspace_mentions(&self, cx: &App) -> Vec<Mention> {
        let s = self.state.read(cx);
        let agents = |statuses: &mut dyn Iterator<Item = AgentStatus>| -> String {
            let (mut n, mut working, mut waiting) = (0, 0, 0);
            for st in statuses {
                n += 1;
                working += usize::from(st == AgentStatus::Working);
                waiting += usize::from(st == AgentStatus::NeedsInput);
            }
            let mut text = match n {
                0 => "no agents".to_string(),
                1 => "1 agent".to_string(),
                n => format!("{n} agents"),
            };
            if working > 0 {
                text.push_str(&format!(", {working} working"));
            }
            if waiting > 0 {
                text.push_str(&format!(", {waiting} need input"));
            }
            text
        };
        let mut out: Vec<Mention> = Vec::new();
        for w in s.workspaces.iter().filter(|w| w.remote.is_none()) {
            let count = agents(&mut w.panes.iter().filter(|p| !p.archived && p.provider != "shell").map(|p| p.status));
            let open = self.home.tasks.iter().filter(|t| t.stage.open() && t.workspace_id.as_deref() == Some(w.id.as_str())).map(HomeTask::title).collect::<Vec<_>>();
            let tasks = if open.is_empty() { String::new() } else { format!("; open: {}", open.join(", ")) };
            out.push(Mention {
                token: slug(&w.name),
                detail: format!("this machine · {count}"),
                note: format!("workspace \"{}\" on this machine, folder {} ({count}{tasks}). Pass workspace: \"{}\"; its panes are \"{}/N\".", w.name, w.root_dir, w.name, w.name),
            });
        }
        if let Some(connect) = crate::connect::get() {
            for p in connect.peers() {
                let Some(m) = self.connect.mirrors.get(&p.peer_id) else { continue };
                let up = if connect.is_up(&p.peer_id) { "online" } else { "offline" };
                for (id, name) in &m.workspaces {
                    let count = agents(&mut m.panes.iter().filter(|x| x.workspace_id == *id && !x.archived && x.provider != "shell").map(|x| x.status));
                    let root = m.workspace_roots.get(id).map(|r| format!(", folder {r}")).unwrap_or_default();
                    out.push(Mention {
                        token: format!("{}/{}", slug(&p.name), slug(name)),
                        detail: format!("{} ({up}) · {count}", p.name),
                        note: format!("workspace \"{name}\" on the paired machine \"{}\" ({up}{root}; {count}). Pass workspace: \"{}/{name}\"; coding work there is a remote task (start_task forks it there).", p.name, p.name),
                    });
                }
            }
        }
        // Two local workspaces with the same name: keep the first (the other by its full name).
        let mut seen = HashSet::new();
        out.retain(|m| !m.token.is_empty() && seen.insert(m.token.clone()));
        out
    }

    /// Keeps the conductor's chat offering the current workspaces after `@`.
    fn sync_home_mentions(&mut self, cx: &mut Context<Self>) {
        let Some(model) = self.home.conductor.as_ref().map(|o| o.model.clone()) else { return };
        let mentions = self.workspace_mentions(cx);
        if model.read(cx).mentions != mentions {
            model.update(cx, |m, cx| {
                m.mentions = mentions;
                cx.notify();
            });
        }
    }

    /// Whether the conductor may ask with a picker: the user reads its chat (not voice).
    pub(super) fn home_can_ask(&self) -> bool {
        !self.home.voice_turn
    }

    // ---- models ----

    /// The models WinferCode offers (every session lists the same ones); else the last known.
    pub(super) fn known_models(&self, pane_id: &str, cx: &App) -> Vec<crate::code::protocol::ModelInfo> {
        self.live_models(pane_id, cx).unwrap_or_else(|| self.home.models.clone())
    }

    fn live_models(&self, pane_id: &str, cx: &App) -> Option<Vec<crate::code::protocol::ModelInfo>> {
        [pane_id, CONDUCTOR].iter().filter_map(|p| self.home_model(p)).chain(self.code_models.values().cloned()).map(|m| m.read(cx).state.models.clone()).find(|m| !m.is_empty())
    }

    /// A list_models call before WinferCode reported any models: answered once they arrive.
    pub(super) fn wait_for_models(&mut self, call: &str, pane_id: &str) {
        self.home.waiting_models.push((call.to_string(), pane_id.to_string(), Instant::now()));
    }

    /// Keeps the last model list (for the next start) and answers list_models calls waiting for it.
    fn tick_models(&mut self, cx: &mut Context<Self>) {
        if let Some(live) = self.live_models(CONDUCTOR, cx)
            && live.iter().map(|m| &m.spec).ne(self.home.models.iter().map(|m| &m.spec))
        {
            let saved: Vec<Value> = live.iter().map(|m| json!({ "spec": m.spec, "label": m.label, "detail": m.detail, "efforts": m.efforts })).collect();
            self.home.models = live;
            self.update_state(cx, |s| {
                let entry = s.rest.entry("home".to_string()).or_insert_with(|| json!({}));
                if entry.is_object() {
                    entry["models"] = Value::Array(saved);
                }
            });
        }
        if self.home.waiting_models.is_empty() {
            return;
        }
        let ready = !self.home.models.is_empty();
        let waiting = std::mem::take(&mut self.home.waiting_models);
        for (call, pane, since) in waiting {
            if ready {
                super::agent_api::respond(&call, true, self.models_text(&pane, cx));
            } else if since.elapsed() > Duration::from_secs(45) {
                super::agent_api::respond(&call, false, "WinferCode hasn't reported its models. Start the task without model (the agent's default).".into());
            } else {
                self.home.waiting_models.push((call, pane, since));
            }
        }
    }

    /// A model (and effort) a WinferCode agent can run; the matching spec.
    fn check_model(&self, model: &str, effort: Option<&str>, cx: &App) -> Result<String, String> {
        let models = self.known_models(CONDUCTOR, cx);
        if models.is_empty() {
            return Ok(model.to_string());
        }
        let info = crate::code::names::match_model(&models, model).ok_or_else(|| format!("No model “{model}”. list_models lists them."))?;
        if let Some(e) = effort
            && !crate::code::names::effort_levels(info.efforts.clone()).iter().any(|l| l == e)
        {
            return Err(format!("{} doesn't take effort “{e}”. It offers: {}.", info.spec, crate::code::names::effort_levels(info.efforts.clone()).join(", ")));
        }
        Ok(info.spec.clone())
    }

    /// list_models: what task agents can run on, by provider, with each provider's usage left.
    pub(super) fn models_text(&self, pane_id: &str, cx: &App) -> String {
        let models = self.known_models(pane_id, cx);
        let s = self.state.read(cx);
        let agent = home_settings(s).task_agent;
        let mut out = Vec::new();
        match agent.as_deref() {
            Some("winfer") => out.push("Task agents: WinferCode (runs any model below, and switches between them keeping context).".to_string()),
            Some(a) => out.push(format!("Task agents: {} (the user's setting): it can only use its own provider's models; the WinferCode list below is for WinferCode agents.", s.provider_name(a))),
            None => out.push("Task agents: your pick; WinferCode runs any model below.".to_string()),
        }
        if models.is_empty() {
            out.push("WinferCode hasn't reported its models yet: try again in a moment.".into());
            return out.join("\n");
        }
        let left = self.usage_by_provider(cx);
        let mut groups: Vec<(String, Vec<&crate::code::protocol::ModelInfo>)> = Vec::new();
        for m in &models {
            let provider = m.spec.split('/').next().unwrap_or_default().to_string();
            match groups.iter_mut().find(|(p, _)| *p == provider) {
                Some((_, list)) => list.push(m),
                None => groups.push((provider, vec![m])),
            }
        }
        for (provider, list) in groups {
            let usage = left.get(&provider).map(|l| format!(" · {:.0}% usage left", l.round())).unwrap_or_default();
            out.push(format!("\n{}{usage}:", s.provider_name(&provider)));
            for m in list {
                let name = if m.label.is_empty() { crate::code::names::model_name(&m.spec, None) } else { m.label.clone() };
                let detail = if m.detail.is_empty() { String::new() } else { format!(" · {}", m.detail) };
                let efforts = m.efforts.as_ref().filter(|e| !e.is_empty()).map(|e| format!(" · effort: {}", e.join("/"))).unwrap_or_default();
                out.push(format!("- {} · {name}{detail}{efforts}", m.spec));
            }
        }
        out.join("\n")
    }

    /// set_task_model: switches a task's agents (all, or `pane`) to another model mid-run.
    fn set_task_model(&mut self, task: &HomeTask, a: &Value, conductor: bool, cx: &mut Context<Self>) -> Result<String, String> {
        let text = |k: &str| a[k].as_str().map(str::trim).filter(|v| !v.is_empty()).map(str::to_string);
        let (model, effort, reason) = (text("model"), text("effort"), text("reason").unwrap_or_default());
        if model.is_none() && effort.is_none() {
            return Err("Give model and/or effort.".into());
        }
        let label = |m: &str| crate::code::names::model_name(m, None);
        let target = match (&model, &effort) {
            (Some(m), Some(e)) => format!("{} ({e})", label(m)),
            (Some(m), None) => label(m),
            (None, Some(e)) => format!("effort {e}"),
            (None, None) => unreachable!(),
        };
        if task.mode == TaskMode::Remote {
            // Its orchestrator runs on the other machine: it switches them there (set_model).
            let spec = model.clone().unwrap_or_default();
            let ask = format!("[Home] Switch this task's agents to {}{}: {reason}. Use set_model for each (their context carries over).", if spec.is_empty() { String::new() } else { format!("model {spec}") }, effort.as_ref().map(|e| format!(" effort {e}")).unwrap_or_default());
            self.task_say(task, &ask, cx)?;
            self.note_model(&task.id, model.as_deref(), effort.as_deref(), &reason, cx);
            return Ok(format!("Asked {}'s orchestrator to switch its agents to {target}.", task.title()));
        }
        let spec = match &model {
            Some(m) => Some(self.check_model(m, effort.as_deref(), cx)?),
            None => None,
        };
        let ws = task.workspace_id.as_ref().and_then(|w| self.state.read(cx).workspaces.iter().find(|x| x.id == *w).cloned());
        let panes: Vec<String> = match a["pane"].as_str().or_else(|| a["pane"].as_u64().map(|_| "")).map(str::trim) {
            Some(_) => {
                let ws = ws.as_ref().ok_or("The task's workspace isn't open anymore.")?;
                let reference = if a["pane"].is_u64() { a["pane"].clone() } else { json!(a["pane"].as_str().unwrap_or_default().rsplit('/').next().unwrap_or_default()) };
                let n = reference.as_u64().or_else(|| reference.as_str().and_then(|r| r.parse().ok())).ok_or("pane: an agent's number, e.g. \"api/2\".")?;
                vec![numbered(ws).get((n as usize).wrapping_sub(1)).cloned().ok_or("No such pane. list_panes shows them.")?]
            }
            None => task.panes.clone(),
        };
        if panes.is_empty() {
            return Err(format!("{} has no agents yet: give spawn_agent (or start_task) the model instead.", task.title()));
        }
        let mut done = Vec::new();
        let mut errors = Vec::new();
        for id in &panes {
            let at = ws.as_ref().map_or_else(|| id.clone(), |w| address(w, id));
            match self.code_models.get(id).cloned() {
                Some(chat) => {
                    chat.update(cx, |c, cx| {
                        if let Some(m) = &spec {
                            c.send(Cmd::SetModel(m.clone()), cx);
                        }
                        if let Some(e) = &effort {
                            c.send(Cmd::SetEffort((e != "default").then(|| e.clone())), cx);
                        }
                    });
                    if let Some(m) = &spec {
                        self.update_state(cx, |s| {
                            if let Some(p) = s.pane_mut(id) {
                                p.model = Some(m.clone());
                            }
                        });
                    }
                    done.push(at);
                }
                None => {
                    let provider = self.state.read(cx).pane(id).map(|p| self.state.read(cx).provider_name(&p.provider)).unwrap_or_default();
                    errors.push(format!("{at} is {provider}, not WinferCode: change_model restarts it on another of {provider}'s own models"));
                }
            }
        }
        if done.is_empty() {
            return Err(errors.join("; "));
        }
        self.note_model(&task.id, spec.as_deref(), effort.as_deref(), &reason, cx);
        let ws_id = task.workspace_id.clone().unwrap_or_default();
        let who = if conductor { "Home".to_string() } else { format!("{}'s orchestrator", task.title()) };
        self.update_state(cx, |s| s.log_activity(&ws_id, format!("{who} switched {} to {target}: {reason}", done.join(", ")), None));
        // A forked task's orchestrator should know its agents changed under it.
        if conductor && task.mode == TaskMode::Fork {
            self.queue_for(&task_pane(&task.id), format!("[winfer] Home switched your agents ({}) to {target}: {reason}", done.join(", ")), cx);
        }
        let mut reply = format!("{} switches to {target} from its next turn; WinferCode hands its conversation over.", done.join(", "));
        if !errors.is_empty() {
            reply.push_str(&format!(" Not switched: {}.", errors.join("; ")));
        }
        Ok(reply)
    }

    /// Remembers a task's model on its card.
    fn note_model(&mut self, task_id: &str, model: Option<&str>, effort: Option<&str>, reason: &str, cx: &mut Context<Self>) {
        let Some(t) = self.task_mut(task_id) else { return };
        if let Some(m) = model {
            t.model = Some(m.to_string());
        }
        if let Some(e) = effort {
            t.effort = (e != "default").then(|| e.to_string());
        }
        t.model_note = reason.to_string();
        t.updated = now_secs();
        self.save_home(cx);
        cx.notify();
    }

    /// The model a task's new agents start on, when it has one.
    pub(super) fn task_model(&self, pane_id: &str) -> Option<(String, Option<String>)> {
        match crate::voice::home_pane(pane_id) {
            Some(crate::voice::HomePane::Task(t)) => self.task(t).and_then(|t| t.model.clone().map(|m| (m, t.effort.clone()))),
            _ => None,
        }
    }

    /// The agent a task's orchestrator must start, when the task has one set.
    pub(super) fn task_agent(&self, pane_id: &str) -> Option<String> {
        match crate::voice::home_pane(pane_id) {
            Some(crate::voice::HomePane::Task(t)) => self.task(t).and_then(|t| t.agent.clone()),
            _ => None,
        }
    }

    /// A task's orchestrator started an agent for it.
    pub(super) fn task_gets_pane(&mut self, task_id: &str, pane_id: &str, cx: &mut Context<Self>) {
        if self.task(task_id).is_none_or(|t| t.panes.iter().any(|p| p == pane_id)) {
            return;
        }
        // A new agent for a done task: it's being worked on again.
        reopen(&mut self.home.tasks, task_id);
        let Some(t) = self.task_mut(task_id) else { return };
        t.panes.push(pane_id.to_string());
        t.updated = now_secs();
        if t.stage == Stage::Planning || !t.stage.open() {
            t.stage = Stage::Running;
        }
        self.save_home(cx);
    }

    /// list_workspaces: every workspace here and on the paired machines, with their agents.
    pub(super) fn workspaces_text(&self, cx: &App) -> String {
        let s = self.state.read(cx);
        let count = |statuses: &mut dyn Iterator<Item = AgentStatus>| -> String {
            let (mut n, mut working, mut waiting, mut failed) = (0, 0, 0, 0);
            for st in statuses {
                n += 1;
                match st {
                    AgentStatus::Working => working += 1,
                    AgentStatus::NeedsInput => waiting += 1,
                    AgentStatus::Error => failed += 1,
                    _ => {}
                }
            }
            let mut parts = vec![format!("{n} pane{}", if n == 1 { "" } else { "s" })];
            for (k, label) in [(working, "working"), (waiting, "needs input"), (failed, "error")] {
                if k > 0 {
                    parts.push(format!("{k} {label}"));
                }
            }
            parts.join(", ")
        };
        let me = crate::connect::get().map(|c| c.status().name).filter(|n| !n.is_empty()).unwrap_or_else(|| "this machine".into());
        let mut out = vec![format!("{me} (this machine):")];
        for w in s.workspaces.iter().filter(|w| w.remote.is_none()) {
            let active = if w.id == s.active_workspace_id { " · the user is in it" } else { "" };
            out.push(format!("- {} · {} · {}{active}", w.name, w.root_dir, count(&mut w.panes.iter().filter(|p| !p.archived).map(|p| p.status))));
        }
        if let Some(connect) = crate::connect::get() {
            for p in connect.peers() {
                let up = connect.is_up(&p.peer_id);
                out.push(format!("{} ({}):", p.name, if up { "online" } else { "offline, last known" }));
                let Some(m) = self.connect.mirrors.get(&p.peer_id) else {
                    out.push("- (nothing known yet)".into());
                    continue;
                };
                if m.workspaces.is_empty() {
                    out.push("- (no workspaces)".into());
                }
                for (id, name) in &m.workspaces {
                    let root = m.workspace_roots.get(id).map(String::as_str).unwrap_or("");
                    out.push(format!("- {}/{name} · {root} · {}", p.name, count(&mut m.panes.iter().filter(|x| x.workspace_id == *id && !x.archived).map(|x| x.status))));
                }
            }
        }
        out.push("Address panes as \"workspace/N\" here and \"machine/workspace/N\" on other machines.".into());
        out.join("\n")
    }

    // ---- the opening brief ----

    /// The user opened Home: after a while away, the conductor sums up what happened.
    pub(super) fn home_opened(&mut self, cx: &mut Context<Self>) {
        let away = self.home.left_at.take();
        let s = self.state.read(cx);
        if !home_settings(s).brief || self.home.last_brief.is_some_and(|b| b.elapsed() < BRIEF_EVERY) {
            return;
        }
        // Only after being away a while (or the first visit), and only with something to tell.
        if away.is_some_and(|a| a.elapsed() < Duration::from_secs(20 * 60)) {
            return;
        }
        let waiting = s.workspaces.iter().flat_map(|w| &w.panes).filter(|p| p.status == AgentStatus::NeedsInput).count();
        let open_tasks = self.home.tasks.iter().filter(|t| t.stage.open()).count();
        let reviews = self.review.reviews.len();
        if waiting + open_tasks + reviews == 0 {
            return;
        }
        self.home.last_brief = Some(Instant::now());
        let text = format!(
            "[winfer] The user opened Home. Give a short brief (3-6 bullets) of what needs them and what happened: agents waiting ({waiting}), reviews ready ({reviews}), open tasks ({open_tasks}), usage left, failures. Look before you write (list_tasks, list_workspaces, usage_status; list_reviews if you have it). No preamble."
        );
        self.home.voice_turn = false;
        self.queue_for(CONDUCTOR, text, cx);
    }

    /// The user left Home.
    pub(super) fn home_closed(&mut self) {
        self.home.left_at = Some(Instant::now());
    }

    // ---- UI actions ----

    /// One of winfer's UI actions an orchestrator called; `None` when `tool` isn't one.
    pub(super) fn orchestrator_action(&mut self, orch_pane: &str, ws_id: &str, tool: &str, args: &Value, window: &mut Window, cx: &mut Context<Self>) -> Option<Result<String, String>> {
        if !voice_tools::is_registry_tool(tool) {
            return None;
        }
        if voice_tools::needs_confirmation(tool, args) {
            return Some(Ok("Not done: ask the user in your reply, and call again with confirmed: true once they said yes.".into()));
        }
        // The actions work on the workspace on screen; app-wide ones leave the user where they are.
        const APP_WIDE: &[&str] = &["switch_workspace", "create_workspace", "open_settings", "set_theme", "show_shortcuts", "toggle_sidebar", "toggle_browser", "show_side_panel", "show_overview", "show_history", "usage_status"];
        let mut note = String::new();
        let s = self.state.read(cx);
        if (s.active_workspace_id != ws_id || s.view() != View::Workspace) && !APP_WIDE.contains(&tool) && !ws_id.is_empty() {
            let name = s.workspaces.iter().find(|w| w.id == ws_id).map(|w| w.name.clone()).unwrap_or_default();
            self.execute(Command::WorkspaceSwitch(ws_id.to_string()), window, cx);
            self.update_state(cx, |s| s.set_view(View::Workspace));
            note = format!(" (switched the user's view to workspace {name})");
        }
        let panes = |s: &AppState| -> HashSet<String> { s.workspaces.iter().find(|w| w.id == ws_id).map(|w| w.panes.iter().map(|p| p.id.clone()).collect()).unwrap_or_default() };
        let before = panes(self.state.read(cx));
        let commands = match voice_tools::registry_commands(tool, args, &self.voice_context(cx)) {
            Ok(c) => c,
            Err(e) => return Some(Err(e)),
        };
        if commands.is_empty() {
            return Some(Ok("Nothing to do with those arguments.".into()));
        }
        for c in commands {
            self.execute(c, window, cx);
        }
        let started: Vec<String> = panes(self.state.read(cx)).difference(&before).cloned().collect();
        for id in &started {
            self.home_owns(orch_pane, id, cx);
            if let Some(HomePane::Task(t)) = home_pane(orch_pane) {
                self.task_gets_pane(t, id, cx);
            }
        }
        Some(Ok(if started.is_empty() { format!("Done{note}.") } else { format!("Done{note}: started {} pane(s); list_panes shows them.", started.len()) }))
    }

    /// The MCP tools a Home orchestrator gets on top of the agent tools.
    pub(super) fn orchestrator_mcp_tools(&self, pane_id: &str, cx: &App) -> Vec<Value> {
        let agent = home_settings(self.state.read(cx)).task_agent;
        let mut tools = home_tools(pane_id == CONDUCTOR, agent.as_deref());
        tools.extend(ui_tools(&self.voice_context(cx).providers));
        tools
    }

    // ---- Home's task tools ----

    /// Home's own tools; `None` when `tool` isn't one.
    pub(super) fn home_task_tool(&mut self, pane_id: &str, tool: &str, a: &Value, cx: &mut Context<Self>) -> Option<Result<String, String>> {
        let conductor = pane_id == CONDUCTOR;
        let mine = match home_pane(pane_id) {
            Some(HomePane::Task(t)) => Some(t.to_string()),
            _ => None,
        };
        let pick = |this: &Self| -> Result<HomeTask, String> {
            match &mine {
                Some(t) => this.task(t).cloned().ok_or_else(|| "Your task is gone.".to_string()),
                None => find_task(&this.home.tasks, &a["task"]).cloned().ok_or_else(|| format!("No task {}. list_tasks shows them.", a["task"])),
            }
        };
        Some(match tool {
            "list_tasks" => Ok(self.tasks_text(a["all"] == true, cx)),
            "update_task" => pick(self).and_then(|t| {
                let stage = match a["stage"].as_str().filter(|s| !s.is_empty()) {
                    Some(s) => Some(Stage::parse(s).ok_or_else(|| format!("Unknown stage {s}."))?),
                    None => None,
                };
                self.set_task(&t.id, stage, a["summary"].as_str(), cx);
                Ok(format!("{} updated.", t.title()))
            }),
            "start_task" if conductor => self.start_task(a, cx),
            "list_models" => Ok(self.models_text(pane_id, cx)),
            "set_task_model" => pick(self).and_then(|t| self.set_task_model(&t, a, conductor, cx)),
            "fork_task" if conductor => pick(self).and_then(|t| self.fork_task(&t.id, a["context"].as_str().unwrap_or_default(), cx)),
            "task_say" if conductor => pick(self).and_then(|t| self.task_say(&t, a["text"].as_str().unwrap_or_default(), cx)),
            "close_task" if conductor => pick(self).map(|t| {
                let stage = if a["stage"] == "cancelled" { Stage::Cancelled } else { Stage::Merged };
                self.close_task(&t.id, stage, cx);
                format!("{} closed ({}).", t.title(), stage.label().to_lowercase())
            }),
            _ => return None,
        })
    }

    pub(super) fn close_task(&mut self, id: &str, stage: Stage, cx: &mut Context<Self>) {
        // Read first: a task with nothing left is removed as soon as its stage is set.
        let panes = self.task(id).map(|t| t.panes.clone()).unwrap_or_default();
        self.set_task(id, Some(stage), None, cx);
        self.stop_fork(id, cx);
        if let Some(o) = self.home.conductor.as_mut() {
            for p in &panes {
                o.owned.remove(p);
            }
        }
        self.home.remote_watch.remove(id);
        self.settle_home_tasks(cx);
    }

    pub(super) fn tasks_text(&self, all: bool, cx: &App) -> String {
        let s = self.state.read(cx);
        let rows: Vec<String> = self
            .home
            .tasks
            .iter()
            .filter(|t| all || t.stage.open())
            .map(|t| {
                let how = match t.mode {
                    TaskMode::Light => "light",
                    TaskMode::Fork => "fork",
                    TaskMode::Remote => "remote fork",
                };
                let agents: Vec<String> = t
                    .panes
                    .iter()
                    .filter_map(|p| {
                        let pane = s.pane(p)?;
                        let ws = s.workspaces.iter().find(|w| w.id == pane.workspace_id)?;
                        Some(format!("{} {} ({})", address(ws, p), s.provider_name(&pane.provider), super::agent_api::status_name(pane.status)))
                    })
                    .collect();
                let mut line = format!("{} · {} · {} · {how} · {}", t.title(), t.stage.label(), t.workspace, t.goal);
                if let Some(m) = &t.model {
                    line.push_str(&format!(" · model {m}{}", t.effort.as_ref().map(|e| format!(" ({e})")).unwrap_or_default()));
                }
                if !agents.is_empty() {
                    line.push_str(&format!(" · agents: {}", agents.join(", ")));
                }
                if let Some(step) = self.fork_step(&t.id, cx) {
                    line.push_str(&format!(" · now: {step}"));
                }
                if !t.summary.is_empty() {
                    line.push_str(&format!("\n    {}", t.summary));
                }
                line
            })
            .collect();
        if rows.is_empty() { "No open tasks.".into() } else { rows.join("\n") }
    }

    /// The conductor's start_task.
    fn start_task(&mut self, a: &Value, cx: &mut Context<Self>) -> Result<String, String> {
        let goal = a["goal"].as_str().map(str::trim).filter(|g| !g.is_empty()).ok_or("goal is empty.")?.to_string();
        let target = a["workspace"].as_str().map(str::trim).filter(|w| !w.is_empty()).ok_or("Which workspace? list_workspaces lists them.")?;
        let prompt = a["prompt"].as_str().map(str::trim).unwrap_or_default().to_string();
        let number = free_number(&self.home.tasks);
        // The agent: one the user asked for in this request, else Settings → Home's.
        let agent = a["provider"].as_str().map(str::trim).filter(|p| !p.is_empty()).map(str::to_string).or_else(|| home_settings(self.state.read(cx)).task_agent);
        let text = |k: &str| a[k].as_str().map(str::trim).filter(|v| !v.is_empty() && *v != "default").map(str::to_string);
        let (model, effort) = (text("model"), text("effort"));
        if let Some(m) = &model
            && agent.as_deref() == Some("winfer")
        {
            self.check_model(m, effort.as_deref(), cx)?;
        }
        let mut task = HomeTask { id: new_task_id(number), number, goal: goal.clone(), agent: agent.clone(), model: model.clone(), effort: effort.clone(), model_note: text("reason").unwrap_or_default(), created: now_secs(), updated: now_secs(), ..Default::default() };
        let place = self.find_workspace(target, cx)?;
        let reply = match place {
            Place::Local(ws_id, name) => {
                task.workspace = name.clone();
                task.workspace_id = Some(ws_id.clone());
                if a["fork"] == true {
                    task.mode = TaskMode::Fork;
                    self.add_task(task.clone(), cx);
                    let model = self.fork(&task.id, cx).inspect_err(|_| self.drop_task(&task.id, cx))?;
                    let text = fork_brief(&task, &prompt);
                    model.update(cx, |m, cx| m.submit(&text, None, cx));
                    format!("{} started in {name}: a task orchestrator plans and runs it, and reports to you.", task.title())
                } else {
                    let provider = agent.as_deref().ok_or("A light task needs provider (the agent to start).")?;
                    if prompt.is_empty() {
                        return Err("A light task needs the agent's prompt.".into());
                    }
                    task.mode = TaskMode::Light;
                    task.stage = Stage::Running;
                    let pane = self.spawn_for_task(&ws_id, provider, &prompt, a["worktree"].as_bool().or(Some(true)), model.as_deref(), effort.as_deref(), &task, cx)?;
                    task.panes.push(pane.clone());
                    self.add_task(task.clone(), cx);
                    self.home_owns(CONDUCTOR, &pane, cx);
                    let s = self.state.read(cx);
                    let at = s.workspaces.iter().find(|w| w.id == ws_id).map_or_else(String::new, |w| address(w, &pane));
                    format!("{} started: {} as {at}. You follow it; keep its stage current with update_task.", task.title(), s.provider_name(provider))
                }
            }
            Place::Remote { peer, machine, workspace } => {
                task.workspace = format!("{machine}/{workspace}");
                task.machine = Some(peer.clone());
                task.mode = TaskMode::Remote;
                let prefix = format!("{} ·", task.title());
                task.earlier_panes = self.connect.mirrors.get(&peer).map(|m| m.panes.iter().filter(|p| p.title.starts_with(&prefix)).map(|p| p.id.clone()).collect()).unwrap_or_default();
                // The orchestrator there runs on the task orchestrators' model; the agents' model is in its brief.
                let model = home_settings(self.state.read(cx)).task_model;
                let args = json!({ "provider": "winfer", "prompt": fork_brief(&task, &prompt), "worktree": false, "workspace": workspace, "role": "task", "title": format!("{} · {goal}", task.title()), "model": model });
                let me = crate::connect::get().map(|c| c.status().name).unwrap_or_default();
                if !crate::connect::get().is_some_and(|c| c.is_up(&peer)) {
                    return Err(format!("{machine} is offline."));
                }
                let call = self.connect_request(&peer, json!({"t": "tool.call", "name": "spawn_agent", "args": args, "caller": format!("Home on {me}")}), None);
                self.home.remote_calls.insert(call, task.id.clone());
                self.home.pending_remote.insert(task.id.clone(), Instant::now());
                self.add_task(task.clone(), cx);
                format!("{} started on {machine}: a task orchestrator there plans and runs it in {workspace}, and reports to you.", task.title())
            }
        };
        let mode = match task.mode {
            TaskMode::Light => "light",
            TaskMode::Fork => "fork",
            TaskMode::Remote => "remote",
        };
        // A fork's own agent is its orchestrator (winfer code on the task orchestrators' model);
        // the agents it starts report `pane_opened` themselves.
        match task.mode {
            TaskMode::Light => crate::telemetry::home_task_started(mode, agent.as_deref(), model.as_deref()),
            TaskMode::Fork | TaskMode::Remote => crate::telemetry::home_task_started(mode, Some("winfer"), home_settings(self.state.read(cx)).task_model.as_deref()),
        }
        Ok(reply)
    }

    fn add_task(&mut self, task: HomeTask, cx: &mut Context<Self>) {
        self.home.tasks.push(task);
        self.save_home(cx);
        cx.notify();
    }

    fn drop_task(&mut self, id: &str, cx: &mut Context<Self>) {
        self.home.tasks.retain(|t| t.id != id);
        self.save_home(cx);
    }

    /// Starts an agent pane for a task in a workspace here; its id.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn spawn_for_task(&mut self, ws_id: &str, provider: &str, prompt: &str, worktree: Option<bool>, model: Option<&str>, effort: Option<&str>, task: &HomeTask, cx: &mut Context<Self>) -> Result<String, String> {
        let s = self.state.read(cx);
        let Some(p) = s.providers.iter().find(|p| p.id == provider && p.installed).cloned() else {
            let available: Vec<&str> = s.providers.iter().filter(|p| p.installed).map(|p| p.id.as_str()).collect();
            return Err(format!("Unknown or uninstalled provider “{provider}”. Available: {}.", available.join(", ")));
        };
        let opts = AddPane { workspace_id: Some(ws_id.to_string()), prompt: Some(prompt.to_string()), worktree, model: model.map(str::to_string).filter(|m| !m.is_empty()), effort: effort.map(str::to_string), ..Default::default() };
        let title = task.title();
        let id = self.state.update(cx, |s, cx| {
            let id = s.add_pane(&p.id, opts);
            s.log_activity(ws_id, format!("Home started {} for {title}", p.name), None);
            cx.notify();
            id
        });
        let s = self.state.read(cx);
        if ws_id != s.active_workspace_id || s.view() != View::Workspace {
            self.start_detached(&id, cx);
        }
        Ok(id)
    }

    /// Promotes a light task: a task orchestrator takes over its agents.
    fn fork_task(&mut self, id: &str, context: &str, cx: &mut Context<Self>) -> Result<String, String> {
        let task = self.task(id).cloned().ok_or("That task is gone.")?;
        if task.mode != TaskMode::Light {
            return Err(format!("{} already has its own orchestrator.", task.title()));
        }
        if let Some(t) = self.task_mut(id) {
            t.mode = TaskMode::Fork;
            t.updated = now_secs();
        }
        if let Some(o) = self.home.conductor.as_mut() {
            for p in &task.panes {
                o.owned.remove(p);
            }
        }
        let model = match self.fork(id, cx) {
            Ok(m) => m,
            Err(e) => {
                if let Some(t) = self.task_mut(id) {
                    t.mode = TaskMode::Light;
                }
                return Err(e);
            }
        };
        self.save_home(cx);
        let s = self.state.read(cx);
        let agents: Vec<String> = task
            .panes
            .iter()
            .filter_map(|p| {
                let pane = s.pane(p)?;
                let ws = s.workspaces.iter().find(|w| w.id == pane.workspace_id)?;
                let n = numbered(ws).iter().position(|x| x == p).map_or(0, |n| n + 1);
                Some(format!("pane {n} ({}{})", s.provider_name(&pane.provider), pane.worktree.as_ref().map(|w| format!(", branch {}", w.branch)).unwrap_or_default()))
            })
            .collect();
        let text = format!(
            "{}\n\nThis task ran as a single agent until now and grew: you take it over.\nIts agents: {}.\nSo far: {}",
            fork_brief(&task, ""),
            if agents.is_empty() { "none left".to_string() } else { agents.join(", ") },
            context.trim()
        );
        model.update(cx, |m, cx| m.submit(&text, None, cx));
        cx.notify();
        Ok(format!("{} now has its own orchestrator; it reports to you.", task.title()))
    }

    /// The conductor's message for a task's orchestrator.
    fn task_say(&mut self, task: &HomeTask, text: &str, cx: &mut Context<Self>) -> Result<String, String> {
        let text = text.trim();
        if text.is_empty() {
            return Err("text is empty".into());
        }
        let message = format!("[Home] {text}");
        match task.mode {
            TaskMode::Fork => {
                let model = self.fork(&task.id, cx)?;
                if model.read(cx).state.busy {
                    if let Some(o) = self.home.forks.get_mut(&task.id) {
                        o.queue.push(message);
                    }
                    Ok(format!("{}'s orchestrator is busy; it gets this when it's free.", task.title()))
                } else {
                    model.update(cx, |m, cx| m.submit(&message, None, cx));
                    Ok(format!("Sent to {}'s orchestrator.", task.title()))
                }
            }
            TaskMode::Remote => {
                let (Some(peer), Some(pane)) = (task.machine.clone(), task.remote_pane.clone()) else {
                    return Err(format!("{}'s orchestrator hasn't shown up on the other machine yet.", task.title()));
                };
                if !crate::connect::get().is_some_and(|c| c.is_up(&peer)) {
                    return Err("That machine is offline.".into());
                }
                let me = crate::connect::get().map(|c| c.status().name).unwrap_or_default();
                let call = self.connect_request(&peer, json!({"t": "tool.call", "name": "send_to_pane", "args": {"pane": pane, "text": message}, "caller": format!("Home on {me}")}), None);
                self.home.remote_calls.insert(call, task.id.clone());
                Ok(format!("Sent to {}'s orchestrator.", task.title()))
            }
            TaskMode::Light => Err(format!("{} is light: talk to its agent with send_to_pane, or fork_task it.", task.title())),
        }
    }

    /// A workspace by "name" (this machine, else a paired one's) or "machine/name".
    pub(super) fn find_workspace(&self, target: &str, cx: &App) -> Result<Place, String> {
        let s = self.state.read(cx);
        let target = target.trim().trim_start_matches('@');
        if let Some(w) = s.workspace_named(target) {
            return Ok(Place::Local(w.id.clone(), w.name.clone()));
        }
        // An @mention token: "my-app", "mac-mini/web".
        let wanted = target.to_lowercase();
        if let Some(w) = s.workspaces.iter().filter(|w| w.remote.is_none()).find(|w| slug(&w.name) == wanted) {
            return Ok(Place::Local(w.id.clone(), w.name.clone()));
        }
        if let Some((m, n)) = wanted.split_once('/')
            && let Some(connect) = crate::connect::get()
        {
            for p in connect.peers().into_iter().filter(|p| slug(&p.name) == m) {
                if let Some((_, wname)) = self.connect.mirrors.get(&p.peer_id).and_then(|x| x.workspaces.iter().find(|(_, w)| slug(w) == n)) {
                    return Ok(Place::Remote { peer: p.peer_id.clone(), machine: p.name.clone(), workspace: wname.clone() });
                }
            }
        }
        let (machine, name) = match target.split_once('/') {
            Some((m, n)) if !m.trim().is_empty() && !n.trim().is_empty() => (Some(m.trim()), n.trim()),
            _ => (None, target),
        };
        let peers: Vec<(String, String)> = match machine {
            Some(m) => {
                let (peer, pname, _) = self.machine(m)?;
                vec![(peer, pname)]
            }
            None => crate::connect::get().map(|c| c.peers().into_iter().map(|p| (p.peer_id, p.name)).collect()).unwrap_or_default(),
        };
        let mut hits = Vec::new();
        for (peer, pname) in &peers {
            if let Some(m) = self.connect.mirrors.get(peer)
                && let Some((_, wname)) = m.workspaces.iter().find(|(_, n)| n.eq_ignore_ascii_case(name))
            {
                hits.push(Place::Remote { peer: peer.clone(), machine: pname.clone(), workspace: wname.clone() });
            }
        }
        match hits.len() {
            1 => Ok(hits.remove(0)),
            0 => Err(format!("No workspace “{target}”. list_workspaces lists them.")),
            _ => Err(format!("“{target}” is on several machines: say which, as machine/{name}.")),
        }
    }
}

/// A light task's stage from its agents' status: running while one works, needs-you while one
/// asks, checking once they're done (the conductor then checks and sets ready or failed).
pub fn light_stage(current: Stage, statuses: &[AgentStatus]) -> Option<Stage> {
    let next = if statuses.contains(&AgentStatus::NeedsInput) {
        Stage::NeedsYou
    } else if statuses.contains(&AgentStatus::Working) {
        Stage::Running
    } else if matches!(current, Stage::Running | Stage::NeedsYou) && statuses.contains(&AgentStatus::Done) {
        Stage::Checking
    } else if current == Stage::NeedsYou && !statuses.is_empty() {
        // Answered, and not working (yet): back to running.
        Stage::Running
    } else {
        return None;
    };
    (next != current).then_some(next)
}

/// Where a task runs.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Place {
    /// This machine's workspace: (id, name).
    Local(String, String),
    Remote { peer: String, machine: String, workspace: String },
}

/// The first message a task's orchestrator gets.
fn fork_brief(task: &HomeTask, context: &str) -> String {
    let context = context.trim();
    let mut text = format!("[Home] {}: {}", task.title(), task.goal);
    if let Some(agent) = &task.agent {
        text.push_str(&format!("\n\nStart this task's agents with provider \"{agent}\" (the user's choice)."));
    }
    if let Some(model) = &task.model {
        let effort = task.effort.as_ref().map(|e| format!(", effort {e}")).unwrap_or_default();
        let why = if task.model_note.is_empty() { String::new() } else { format!(" ({})", task.model_note) };
        text.push_str(&format!("\nRun its agents on model {model}{effort}{why}; switch them mid-run when your model guide says so."));
    }
    if !context.is_empty() {
        text.push_str(&format!("\n\n{context}"));
    }
    text
}

/// The conductor's working folder: a scratch folder of its own, not a project.
pub(super) fn home_dir() -> String {
    let dir = crate::backend::data_dir().join("home");
    let _ = std::fs::create_dir_all(&dir);
    dir.to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::{AgentStatus, HomeTask, Item, Mirror, Stage, TaskMode, Watch, all_merged, answer_of, finished, free_number, new_task_id, remote_pane_open, reopen, current_step, find_task, fork_brief, home_tools, light_stage, listed_tasks, pane_event, ui_tools};
    use serde_json::json;

    fn user(text: &str) -> Item {
        Item::User { key: String::new(), text: text.into(), at: 1, images: Vec::new() }
    }
    fn said(text: &str) -> Item {
        Item::Text { key: String::new(), text: text.into(), streaming: false }
    }
    fn tool(title: &str, ended: Option<u64>) -> Item {
        Item::Tool { key: String::new(), name: "Bash".into(), title: title.into(), input: json!({}), output: String::new(), ok: None, diff: None, started: 0, ended }
    }
    fn task(n: u32) -> HomeTask {
        HomeTask { id: format!("t{n}"), number: n, goal: "Fix it".into(), workspace: "api".into(), ..Default::default() }
    }

    #[test]
    fn the_answer_is_what_it_said_after_its_last_tool() {
        let items = [user("old"), said("old answer"), user("What changed?"), said("Let me check git."), tool("git log -3", Some(2)), said("The last commit adds telemetry."), said("It's on main.")];
        assert_eq!(answer_of(&items).as_deref(), Some("The last commit adds telemetry. It's on main."));
        assert_eq!(answer_of(&[user("x"), said("Starting three agents."), tool("spawn", Some(1))]).as_deref(), Some("Starting three agents."));
        assert_eq!(answer_of(&[user("x"), tool("spawn", Some(1))]), None);
    }

    #[test]
    fn silence_is_not_passed_on() {
        assert_eq!(answer_of(&[user("[winfer] Pane 2 finished."), tool("read", Some(1)), said("[silent]")]), None);
        assert_eq!(answer_of(&[user("x"), said(" [SILENT]. ")]), None);
    }

    #[test]
    fn the_step_is_the_running_tool() {
        assert_eq!(current_step(&[tool("git log", Some(1)), tool("cargo test", None)]).as_deref(), Some("cargo test"));
        assert_eq!(current_step(&[tool("git log", Some(1))]), None);
    }

    #[test]
    fn panes_report_settled_changes_only() {
        let mut w = Watch::new(AgentStatus::Idle);
        assert_eq!(w.tick(AgentStatus::Working), None);
        assert_eq!(w.tick(AgentStatus::Done), None);
        assert_eq!(w.tick(AgentStatus::Working), None);
        assert_eq!(w.tick(AgentStatus::Done), None);
        assert_eq!(w.tick(AgentStatus::Done), Some("finished"));
        assert_eq!(w.tick(AgentStatus::Done), None);
        let mut w = Watch::new(AgentStatus::Done);
        assert_eq!(w.tick(AgentStatus::Idle), None);
        assert_eq!(w.tick(AgentStatus::Idle), None);
        let mut w = Watch::new(AgentStatus::Working);
        assert_eq!(w.tick(AgentStatus::NeedsInput), None);
        assert_eq!(w.tick(AgentStatus::NeedsInput), Some("needs input"));
        assert_eq!(w.tick(AgentStatus::Error), None);
        assert_eq!(w.tick(AgentStatus::Error), Some("hit an error"));
    }

    #[test]
    fn events_name_the_task_and_pane() {
        assert_eq!(pane_event(None, "pane 2", "Claude Code", Some("winfer/api"), "finished", None), "[winfer] Pane 2 (Claude Code, branch winfer/api) finished.");
        assert_eq!(pane_event(Some(&task(3)), "api/1", "Codex", None, "needs input", Some(" Allow edit? ")), "[winfer] Task 3 · api/1 (Codex) needs input: Allow edit?");
    }

    #[test]
    fn open_tasks_are_listed_first_newest_first() {
        let mut tasks: Vec<HomeTask> = (1..=14).map(|n| HomeTask { updated: n.into(), ..task(n) }).collect();
        tasks[13].stage = Stage::Merged;
        tasks[1].stage = Stage::Cancelled;
        let numbers: Vec<u32> = listed_tasks(&tasks).iter().map(|t| t.number).collect();
        assert_eq!(numbers, [13, 12, 11, 10, 9, 8, 7, 6, 5, 4, 3, 1]);
        tasks.truncate(3);
        assert_eq!(listed_tasks(&tasks).iter().map(|t| t.number).collect::<Vec<_>>(), [3, 1, 2]);
    }

    #[test]
    fn tasks_are_found_by_number_or_id() {
        let tasks = [task(1), task(12)];
        assert_eq!(find_task(&tasks, &json!(12)).map(|t| t.number), Some(12));
        assert_eq!(find_task(&tasks, &json!("Task 1")).map(|t| t.number), Some(1));
        assert_eq!(find_task(&tasks, &json!("t12")).map(|t| t.number), Some(12));
        assert_eq!(find_task(&tasks, &json!("task 3")), None);
    }

    #[test]
    fn numbers_count_over_open_tasks_only() {
        assert_eq!(free_number(&[]), 1);
        let mut tasks = vec![task(1), task(2), task(4)];
        assert_eq!(free_number(&tasks), 3);
        tasks[0].stage = Stage::Merged;
        assert_eq!(free_number(&tasks), 1);
        for t in &mut tasks {
            t.stage = Stage::Cancelled;
        }
        assert_eq!(free_number(&tasks), 1);
        let (a, b) = (new_task_id(1), new_task_id(1));
        assert!(a.starts_with("t1-") && a != b);
    }

    #[test]
    fn a_number_means_the_open_task() {
        let mut old = task(1);
        old.stage = Stage::Merged;
        old.updated = 9;
        let mut new = task(1);
        new.id = "t1-abc".into();
        let mut tasks = vec![old, new];
        for key in [json!(1), json!("Task 1"), json!("t1"), json!("t1-abc")] {
            assert_eq!(find_task(&tasks, &key).map(|t| t.id.as_str()), Some("t1-abc"), "{key}");
        }
        // Both done: the latest.
        tasks[1].stage = Stage::Cancelled;
        assert_eq!(find_task(&tasks, &json!(1)).map(|t| t.id.as_str()), Some("t1"));
    }

    #[test]
    fn a_reopened_task_leaves_a_taken_number() {
        let mut old = task(1);
        old.stage = Stage::Merged;
        old.merged = vec!["/a".into()];
        let mut new = task(1);
        new.id = "t1-abc".into();
        let mut tasks = vec![old, new];
        reopen(&mut tasks, "t1");
        assert_eq!((tasks[0].number, tasks[0].merged.len(), tasks[1].number), (2, 0, 1));
    }

    #[test]
    fn light_tasks_follow_their_agent() {
        use AgentStatus::*;
        // Started, not working yet (a shell, or before its first turn): stays running.
        assert_eq!(light_stage(Stage::Running, &[Idle]), None);
        assert_eq!(light_stage(Stage::Running, &[Working]), None);
        assert_eq!(light_stage(Stage::Running, &[NeedsInput]), Some(Stage::NeedsYou));
        assert_eq!(light_stage(Stage::NeedsYou, &[Working]), Some(Stage::Running));
        assert_eq!(light_stage(Stage::Running, &[Done]), Some(Stage::Checking));
        // The conductor's own calls (checking, ready) aren't undone by an idle agent.
        assert_eq!(light_stage(Stage::Checking, &[Idle]), None);
        assert_eq!(light_stage(Stage::Checking, &[Done]), None);
        assert_eq!(light_stage(Stage::Checking, &[Working]), Some(Stage::Running));
    }

    #[test]
    fn tasks_run_as_winfer_code_unless_set() {
        use super::home_settings;
        let mut s = winfer_core::model::AppState::default();
        let winfer = winfer_backend::providers::builtin().iter().find(|p| p.id == "winfer").cloned().unwrap();
        s.providers = vec![winfer_core::model::AgentProvider { info: winfer, installed: true }];
        assert_eq!(home_settings(&s).task_agent.as_deref(), Some("winfer"));
        s.rest.insert("home".into(), json!({ "taskAgent": "codex" }));
        assert_eq!(home_settings(&s).task_agent.as_deref(), Some("codex"));
        s.rest.insert("home".into(), json!({ "taskAgent": "auto" }));
        assert_eq!(home_settings(&s).task_agent, None);
        // Not installed: the orchestrators pick rather than fail.
        s.rest.clear();
        s.providers[0].installed = false;
        assert_eq!(home_settings(&s).task_agent, None);
    }

    #[test]
    fn the_brief_names_the_tasks_agent() {
        let mut t = task(4);
        assert!(!fork_brief(&t, "").contains("provider"));
        t.agent = Some("codex".into());
        assert!(fork_brief(&t, "ctx").contains("provider \"codex\""));
        assert!(!fork_brief(&t, "").contains("model"));
        t.model = Some("agy/gemini-4-neon".into());
        t.effort = Some("low".into());
        t.model_note = "lots of logs to read".into();
        assert!(fork_brief(&t, "").contains("model agy/gemini-4-neon, effort low (lots of logs to read)"));
    }

    #[test]
    fn the_routing_guide_covers_the_families() {
        // Hidden from the user, but every orchestrator's system prompt carries it.
        for needle in ["claude/", "codex/", "agy/", "set_task_model", "list_models", "usage"] {
            assert!(super::ROUTING.contains(needle), "{needle}");
        }
    }

    #[test]
    fn stages_parse_loosely() {
        assert_eq!(Stage::parse("Needs you"), Some(Stage::NeedsYou));
        assert_eq!(Stage::parse(" ready"), Some(Stage::Ready));
        assert_eq!(Stage::parse("nope"), None);
        assert!(!Stage::Merged.open() && Stage::Ready.open());
    }

    #[test]
    fn done_tasks_go_once_nothing_is_left() {
        let mut t = task(1);
        t.panes = vec!["p".into()];
        t.worktrees = vec!["/w".into()];
        let (yes, no) = (|_: &str| true, |_: &str| false);
        // Open tasks stay, whatever is left of them.
        assert!(!finished(&t, no, no, false));
        for stage in [Stage::Merged, Stage::Cancelled] {
            t.stage = stage;
            assert!(finished(&t, no, no, false));
            assert!(!finished(&t, yes, no, false), "an agent pane is open");
            assert!(!finished(&t, no, yes, false), "a worktree is left");
            assert!(!finished(&t, no, no, true), "its orchestrator elsewhere is open");
        }
        // However old, one with leftovers is still saved with its worktrees.
        let saved: HomeTask = serde_json::from_value(serde_json::to_value(&t).unwrap()).unwrap();
        assert_eq!(saved.worktrees, t.worktrees);
    }

    #[test]
    fn tasks_are_merged_once_every_worktree_is_in_or_gone() {
        let mut t = task(1);
        t.worktrees = vec!["/a".into(), "/b".into()];
        assert!(!all_merged(&t, |_| false), "nothing was merged");
        t.merged = vec!["/a".into()];
        assert!(!all_merged(&t, |_| true), "/b isn't in yet");
        assert!(all_merged(&t, |w| w == "/a"), "/b was discarded");
        t.merged.push("/b".into());
        assert!(all_merged(&t, |_| true));
    }

    #[test]
    fn remote_panes_are_open_until_their_machine_says_otherwise() {
        let mut t = task(3);
        assert!(!remote_pane_open(&t, None, false), "not a remote task");
        t.mode = TaskMode::Remote;
        t.remote_pane = Some("r".into());
        assert!(remote_pane_open(&t, None, false), "offline: unknown");
        let mut mirror = Mirror { loaded: true, ..Default::default() };
        assert!(!remote_pane_open(&t, Some(&mirror), false));
        let pane = super::super::connect_ui::RemotePane { id: "r".into(), title: "Task 3 · fix it".into(), workspace: "web".into(), workspace_id: "w".into(), archived: false, cwd: String::new(), provider: "winfer".into(), status: AgentStatus::Idle, message: None, request_kind: None, request_text: None, reply: None, branch: None };
        mirror.panes.push(pane);
        assert!(remote_pane_open(&t, Some(&mirror), false));
        // Not seen yet: found by its title, or still starting.
        t.remote_pane = None;
        assert!(remote_pane_open(&t, Some(&mirror), false));
        // An earlier Task 3's orchestrator isn't this one.
        t.earlier_panes = vec!["r".into()];
        assert!(!remote_pane_open(&t, Some(&mirror), false));
        mirror.panes.clear();
        assert!(!remote_pane_open(&t, Some(&mirror), false));
        assert!(remote_pane_open(&t, Some(&mirror), true));
    }

    #[test]
    fn tasks_round_trip_through_the_saved_state() {
        let mut t = task(4);
        t.mode = TaskMode::Remote;
        t.stage = Stage::NeedsYou;
        let v = serde_json::to_value(&t).unwrap();
        assert_eq!(v["mode"], "remote");
        assert_eq!(v["stage"], "needs-you");
        assert_eq!(serde_json::from_value::<HomeTask>(v).unwrap(), t);
        // Older or partial entries still load.
        assert_eq!(serde_json::from_value::<HomeTask>(json!({ "id": "t1", "number": 1 })).unwrap().stage, Stage::Planning);
    }

    #[test]
    fn tool_sets_differ_by_role() {
        let names = |c: bool| home_tools(c, None).iter().map(|t| t["name"].as_str().unwrap().to_string()).collect::<Vec<_>>();
        assert!(names(true).contains(&"start_task".to_string()));
        assert!(!names(false).contains(&"start_task".to_string()));
        assert!(names(false).contains(&"update_task".to_string()));
        // Both pick and switch models; only the conductor names the task.
        for c in [true, false] {
            assert!(names(c).contains(&"list_models".to_string()) && names(c).contains(&"set_task_model".to_string()));
        }
        let required = |c: bool| home_tools(c, None).into_iter().find(|t| t["name"] == "set_task_model").unwrap()["inputSchema"]["required"].clone();
        assert_eq!(required(true), json!(["task", "reason"]));
        assert_eq!(required(false), json!(["reason"]));
        let ui = ui_tools(&[("claude".into(), "Claude Code".into())]);
        assert!(ui.iter().find(|t| t["name"] == "merge_worktree").unwrap()["inputSchema"]["properties"]["confirmed"].is_object());
    }
}
