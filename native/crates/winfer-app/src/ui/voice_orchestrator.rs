//! The voice orchestrator: a hidden `winfer code --serve` session per workspace that live voice
//! hands requests to. It looks into things itself (git, files, panes, chats, the browser), gives
//! coding work to agents, follows them to the end and reports back for voice to speak.
//!
//! It runs like a winfer code chat pane, under a made-up pane id (`crate::voice::orchestrator_pane`)
//! that no workspace lists. Its MCP calls come in with that id (`agent_api.rs`), where it also gets
//! winfer's UI actions as tools.

use super::voice_tools;
use super::*;
use crate::code::protocol::{Cmd, Decision};
use crate::code::session::{CodeEvent, CodeModel};
use crate::code::state::Item;
use crate::model::Workspace;
use crate::voice::{orchestrator_pane, orchestrator_workspace};
use serde_json::{Value, json};
use std::collections::HashSet;

const ROLE: &str = include_str!("../../assets/orchestrator-role.txt");
/// What the orchestrator replies when there's nothing to tell the user.
const SILENT: &str = "[silent]";
/// Seconds (ticks) a pane's new status must hold before the orchestrator hears of it: hooks fire
/// per turn and idle detection can flap.
const SETTLE_TICKS: u8 = 2;
const MAX_SPOKEN: usize = 1500;
const MAX_TOAST: usize = 280;
const NO_PICKER: &str = "Nobody sees questions asked here. Put the question in your reply instead; the voice assistant asks the user.";

#[derive(Default)]
pub(super) struct Orchestrators {
    by_workspace: HashMap<String, Orch>,
}

struct Orch {
    model: Entity<CodeModel>,
    /// Events waiting for the session to be free.
    queue: Vec<String>,
    /// Agent panes it started, and what it last heard of their status.
    owned: HashMap<String, Watch>,
    /// Approvals and questions already answered.
    handled: HashSet<String>,
    /// The step shown in the voice HUD.
    step: Option<String>,
    _subscriptions: Vec<Subscription>,
}

/// Follows one pane's status for its orchestrator.
#[derive(Clone, Copy, Debug)]
struct Watch {
    seen: AgentStatus,
    /// A different status, and for how many ticks it has held.
    next: Option<(AgentStatus, u8)>,
    /// It worked since the orchestrator last heard of it.
    worked: bool,
}

impl Watch {
    fn new(status: AgentStatus) -> Self {
        Self { seen: status, next: None, worked: status == AgentStatus::Working }
    }

    /// One tick with the pane at `now`: what to tell the orchestrator, once a change has settled.
    fn tick(&mut self, now: AgentStatus) -> Option<&'static str> {
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

/// What the orchestrator said at the end of its turn (its text after its last tool call), or
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

/// The tool the orchestrator is running, for the HUD.
pub fn current_step(items: &[Item]) -> Option<String> {
    items.iter().rev().find_map(|i| match i {
        Item::Tool { name, title, ended: None, .. } => Some(if title.trim().is_empty() { name.clone() } else { title.trim().chars().take(60).collect() }),
        _ => None,
    })
}

/// An event about one of the orchestrator's panes.
pub fn pane_event(number: usize, agent: &str, branch: Option<&str>, what: &str, message: Option<&str>) -> String {
    let branch = branch.map(|b| format!(", branch {b}")).unwrap_or_default();
    let message = message.map(str::trim).filter(|m| !m.is_empty()).map(|m| format!(": {}", m.chars().take(200).collect::<String>())).unwrap_or_default();
    let text = format!("[winfer] Pane {number} ({agent}{branch}) {what}{message}");
    if text.ends_with(['.', '?', '!']) { text } else { format!("{text}.") }
}

/// winfer's UI actions as MCP tools, for orchestrators.
pub fn mcp_tools(providers: &[(String, String)]) -> Vec<Value> {
    voice_tools::registry_tools(providers).into_iter().map(|t| json!({ "name": t["name"], "description": t["description"], "inputSchema": t["parameters"] })).collect()
}

/// Pane ids in the order agents number them: tiled, then archived.
fn numbered(ws: &Workspace) -> Vec<String> {
    let mut ids = ws.layout.as_ref().map(leaves).unwrap_or_default();
    ids.extend(ws.panes.iter().filter(|p| p.archived).map(|p| p.id.clone()));
    ids
}

impl Root {
    /// Hands a request the user made by voice to the active workspace's orchestrator.
    pub(super) fn orchestrate(&mut self, request: &str, fresh: bool, cx: &mut Context<Self>) -> Result<String, String> {
        let ws = self.state.read(cx).active_workspace_id.clone();
        if ws.is_empty() {
            return Err("There's no workspace open.".into());
        }
        if fresh {
            self.stop_orchestrator(&ws, cx);
        }
        let request = request.trim();
        if request.is_empty() {
            return if fresh { Ok("The orchestrator starts fresh with the next request.".into()) } else { Err("What should the orchestrator do?".into()) };
        }
        let model = self.orchestrator(&ws, cx)?;
        let busy = model.read(cx).state.busy;
        model.update(cx, |m, cx| m.submit(&format!("User (via voice): {request}"), None, cx));
        cx.notify();
        Ok(if busy {
            "Passed on to the orchestrator while it works; it answers in an [winfer] Orchestrator message.".into()
        } else {
            "Handed to the orchestrator; it answers in an [winfer] Orchestrator message.".into()
        })
    }

    /// The workspace's orchestrator, started on first use.
    fn orchestrator(&mut self, ws_id: &str, cx: &mut Context<Self>) -> Result<Entity<CodeModel>, String> {
        if let Some(o) = self.orch.by_workspace.get(ws_id) {
            return Ok(o.model.clone());
        }
        let (pane, settings) = {
            let s = self.state.read(cx);
            if !s.providers.iter().any(|p| p.id == "winfer" && p.installed) {
                return Err("winfer code isn't available, so the orchestrator can't start.".into());
            }
            let ws = s.workspaces.iter().find(|w| w.id == ws_id).ok_or("That workspace isn't open anymore.")?;
            let model = s.rest.get("voice").and_then(|v| v["orchestratorModel"].as_str()).map(str::trim).filter(|m| !m.is_empty() && *m != "default").map(str::to_string);
            let pane = Pane { id: orchestrator_pane(ws_id), provider: "winfer".into(), title: "Orchestrator".into(), cwd: ws.root_dir.clone(), workspace_id: ws.id.clone(), model, ..Default::default() };
            let mut settings = terminal::SpawnSettings::of(s, &pane);
            // Fully autonomous: merges, discards and closing still need the user's yes (see `orchestrator_action`).
            settings.auto_approve = true;
            settings.system_prompt = [ROLE.trim(), settings.system_prompt.trim()].into_iter().filter(|p| !p.is_empty()).collect::<Vec<_>>().join("\n\n");
            (pane, settings)
        };
        let session = SESSIONS.get_or_create(&pane.id, &self.events);
        session.resize(200, 50);
        let (events, app) = (self.events.clone(), self.state.clone());
        let model = cx.new(|cx| CodeModel::new(pane, settings, session, events, app, cx));
        let (changed, turn) = (ws_id.to_string(), ws_id.to_string());
        let subscriptions = vec![
            cx.observe(&model, move |this, _, cx| this.on_orchestrator_changed(&changed, cx)),
            cx.subscribe(&model, move |this, _, _: &CodeEvent, cx| this.on_orchestrator_turn(&turn, cx)),
        ];
        self.orch.by_workspace.insert(ws_id.to_string(), Orch { model: model.clone(), queue: Vec::new(), owned: HashMap::new(), handled: HashSet::new(), step: None, _subscriptions: subscriptions });
        Ok(model)
    }

    /// Ends a workspace's orchestrator; the next request starts a new one.
    pub(super) fn stop_orchestrator(&mut self, ws_id: &str, cx: &mut Context<Self>) {
        if self.orch.by_workspace.remove(ws_id).is_some() {
            terminal::close(&orchestrator_pane(ws_id));
            cx.notify();
        }
    }

    pub(super) fn has_orchestrator(&self, cx: &App) -> bool {
        self.orch.by_workspace.contains_key(&self.state.read(cx).active_workspace_id)
    }

    /// What the active workspace's orchestrator is doing, while it works.
    pub(super) fn orchestrator_step(&self, cx: &App) -> Option<String> {
        let o = self.orch.by_workspace.get(&self.state.read(cx).active_workspace_id)?;
        o.model.read(cx).state.busy.then(|| o.step.clone().unwrap_or_else(|| "thinking".into()))
    }

    /// Answers what the session asks (it runs without anyone watching) and follows its step.
    fn on_orchestrator_changed(&mut self, ws_id: &str, cx: &mut Context<Self>) {
        let Some(o) = self.orch.by_workspace.get_mut(ws_id) else { return };
        let (asks, step) = {
            let st = &o.model.read(cx).state;
            let asks: Vec<(String, usize)> = st.approvals.iter().filter(|a| !o.handled.contains(&a.id)).map(|a| (a.id.clone(), a.questions.len())).collect();
            (asks, if st.busy { current_step(&st.items) } else { None })
        };
        for (id, questions) in asks {
            o.handled.insert(id.clone());
            let cmd = if questions == 0 { Cmd::Approve { id, decision: Decision::Allow } } else { Cmd::Answer { id, answers: vec![NO_PICKER.to_string(); questions] } };
            o.model.update(cx, |m, cx| m.send(cmd, cx));
        }
        if o.step != step {
            o.step = step;
            cx.notify();
        }
    }

    /// The orchestrator ended a turn: say its answer, then hand it what happened meanwhile.
    fn on_orchestrator_turn(&mut self, ws_id: &str, cx: &mut Context<Self>) {
        let Some(o) = self.orch.by_workspace.get_mut(ws_id) else { return };
        o.step = None;
        if let Some(text) = answer_of(&o.model.read(cx).state.items) {
            self.orchestrator_says(ws_id, &text, cx);
        }
        self.deliver_orchestrator_events(ws_id, cx);
        cx.notify();
    }

    /// Tells the user what the orchestrator said: spoken while live voice is on, else a toast
    /// (and a notification when winfer isn't in front).
    pub(super) fn orchestrator_says(&mut self, ws_id: &str, text: &str, cx: &mut Context<Self>) {
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        #[cfg(debug_assertions)]
        if std::env::var_os("WINFER_NATIVE_ORCHESTRATE").is_some() {
            eprintln!("[orchestrator] {text}");
        }
        let (active, name) = {
            let s = self.state.read(cx);
            (s.active_workspace_id == ws_id, s.workspaces.iter().find(|w| w.id == ws_id).map_or(String::new(), |w| w.name.clone()))
        };
        if self.live_voice_on() {
            let place = if active { String::new() } else { format!(" (workspace {name})") };
            self.voice_note(format!("Orchestrator{place}: {text}"));
            return;
        }
        let short: String = text.chars().take(MAX_TOAST).collect();
        self.toast(format!("Orchestrator: {short}"), cx);
        if !status::window_focused() {
            let notification = json!({ "paneId": orchestrator_pane(ws_id), "title": format!("Orchestrator · {name}"), "body": short, "kind": "info" });
            backend::background(move || {
                if let Ok(n) = serde_json::from_value(notification) {
                    let _ = winfer_backend::notify::notify_action(backend::app().clone(), n);
                }
            });
        }
    }

    /// Writes the waiting events into the orchestrator once it's free.
    fn deliver_orchestrator_events(&mut self, ws_id: &str, cx: &mut Context<Self>) {
        let Some(o) = self.orch.by_workspace.get_mut(ws_id) else { return };
        if o.queue.is_empty() || o.model.read(cx).state.busy {
            return;
        }
        let text = std::mem::take(&mut o.queue).join("\n");
        let model = o.model.clone();
        model.update(cx, |m, cx| m.submit(&text, None, cx));
    }

    /// The orchestrator started `pane_id`: it hears when that agent finishes or needs something.
    pub(super) fn orchestrator_owns(&mut self, ws_id: &str, pane_id: &str, cx: &App) {
        let status = self.state.read(cx).pane(pane_id).map_or(AgentStatus::Idle, |p| p.status);
        if let Some(o) = self.orch.by_workspace.get_mut(ws_id) {
            o.owned.insert(pane_id.to_string(), Watch::new(status));
        }
    }

    /// A note for an orchestrator (a background task of its own exited); false when `pane_id`
    /// isn't an orchestrator.
    pub(super) fn orchestrator_note(&mut self, pane_id: &str, text: String, cx: &mut Context<Self>) -> bool {
        let Some(ws) = orchestrator_workspace(pane_id).map(str::to_string) else { return false };
        if let Some(o) = self.orch.by_workspace.get_mut(&ws) {
            o.queue.push(text);
        }
        self.deliver_orchestrator_events(&ws, cx);
        true
    }

    /// An orchestrator's process exited; false when `pane_id` isn't an orchestrator.
    pub(super) fn orchestrator_exited(&mut self, pane_id: &str, code: u32, cx: &mut Context<Self>) -> bool {
        let Some(ws) = orchestrator_workspace(pane_id).map(str::to_string) else { return false };
        if let Some(o) = self.orch.by_workspace.remove(&ws) {
            SESSIONS.remove(pane_id);
            let busy = o.model.read(cx).state.busy;
            if code != 0 || busy {
                self.orchestrator_says(&ws, &format!("The orchestrator stopped (exit code {code}). The next request starts a new one."), cx);
            }
            cx.notify();
        }
        true
    }

    /// Every second: ends orchestrators of closed workspaces, tells them what their agents did,
    /// and hands over what waited for them to be free.
    pub(super) fn tick_orchestrators(&mut self, cx: &mut Context<Self>) {
        if self.orch.by_workspace.is_empty() {
            return;
        }
        let open: HashSet<String> = self.state.read(cx).workspaces.iter().map(|w| w.id.clone()).collect();
        let gone: Vec<String> = self.orch.by_workspace.keys().filter(|w| !open.contains(*w)).cloned().collect();
        for ws in gone {
            self.stop_orchestrator(&ws, cx);
        }
        let workspaces: Vec<String> = self.orch.by_workspace.keys().cloned().collect();
        for ws in workspaces {
            let events = self.watch_orchestrator_panes(&ws, cx);
            if let Some(o) = self.orch.by_workspace.get_mut(&ws) {
                o.queue.extend(events);
            }
            self.deliver_orchestrator_events(&ws, cx);
        }
    }

    fn watch_orchestrator_panes(&mut self, ws_id: &str, cx: &App) -> Vec<String> {
        let s = self.state.read(cx);
        let (Some(ws), Some(o)) = (s.workspaces.iter().find(|w| w.id == ws_id), self.orch.by_workspace.get_mut(ws_id)) else { return Vec::new() };
        o.owned.retain(|id, _| ws.panes.iter().any(|p| p.id == *id));
        let order = numbered(ws);
        let mut events = Vec::new();
        for (id, watch) in o.owned.iter_mut() {
            let Some(pane) = ws.panes.iter().find(|p| p.id == *id) else { continue };
            if let Some(what) = watch.tick(pane.status) {
                let number = order.iter().position(|p| p == id).map_or(0, |n| n + 1);
                let message = s.status_messages.get(id).map(String::as_str);
                events.push(pane_event(number, &s.provider_name(&pane.provider), pane.worktree.as_ref().map(|t| t.branch.as_str()), what, message));
            }
        }
        events
    }

    /// One of winfer's UI actions an orchestrator called; `None` when `tool` isn't one.
    pub(super) fn orchestrator_action(&mut self, ws_id: &str, tool: &str, args: &Value, window: &mut Window, cx: &mut Context<Self>) -> Option<Result<String, String>> {
        if !voice_tools::is_registry_tool(tool) {
            return None;
        }
        if voice_tools::needs_confirmation(tool, args) {
            return Some(Ok("Not done: ask the user in your reply, and call again with confirmed: true once they said yes.".into()));
        }
        // The actions work on the workspace on screen.
        let mut note = String::new();
        if self.state.read(cx).active_workspace_id != ws_id && tool != "switch_workspace" {
            let name = self.state.read(cx).workspaces.iter().find(|w| w.id == ws_id).map(|w| w.name.clone()).unwrap_or_default();
            self.execute(Command::WorkspaceSwitch(ws_id.to_string()), window, cx);
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
            self.orchestrator_owns(ws_id, id, cx);
        }
        Some(Ok(if started.is_empty() { format!("Done{note}.") } else { format!("Done{note}: started {} pane(s); list_panes shows them.", started.len()) }))
    }

    /// The MCP tools an orchestrator gets on top of the agent tools: winfer's UI actions.
    pub(super) fn orchestrator_mcp_tools(&self, cx: &App) -> Vec<Value> {
        mcp_tools(&self.voice_context(cx).providers)
    }
}

#[cfg(test)]
mod tests {
    use super::{AgentStatus, Item, Watch, answer_of, current_step, mcp_tools, pane_event};
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

    #[test]
    fn the_answer_is_what_it_said_after_its_last_tool() {
        let items = [user("old"), said("old answer"), user("What changed?"), said("Let me check git."), tool("git log -3", Some(2)), said("The last commit adds telemetry."), said("It's on main.")];
        assert_eq!(answer_of(&items).as_deref(), Some("The last commit adds telemetry. It's on main."));
        // Nothing after the tools: its last words before them.
        assert_eq!(answer_of(&[user("x"), said("Starting three agents."), tool("spawn", Some(1))]).as_deref(), Some("Starting three agents."));
        assert_eq!(answer_of(&[user("x"), tool("spawn", Some(1))]), None);
    }

    #[test]
    fn silence_is_not_spoken() {
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
        // A flap back to working isn't reported.
        assert_eq!(w.tick(AgentStatus::Done), None);
        assert_eq!(w.tick(AgentStatus::Working), None);
        assert_eq!(w.tick(AgentStatus::Done), None);
        assert_eq!(w.tick(AgentStatus::Done), Some("finished"));
        assert_eq!(w.tick(AgentStatus::Done), None);
        // Idle without having worked isn't "finished".
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
    fn events_name_the_pane() {
        assert_eq!(pane_event(2, "Claude Code", Some("winfer/api"), "finished", None), "[winfer] Pane 2 (Claude Code, branch winfer/api) finished.");
        assert_eq!(pane_event(1, "Codex", None, "needs input", Some(" Allow edit? ")), "[winfer] Pane 1 (Codex) needs input: Allow edit?");
    }

    #[test]
    fn ui_actions_become_mcp_tools() {
        let tools = mcp_tools(&[("claude".into(), "Claude Code".into())]);
        let merge = tools.iter().find(|t| t["name"] == "merge_worktree").unwrap();
        assert!(merge["inputSchema"]["properties"]["confirmed"].is_object());
        assert!(tools.iter().all(|t| t["inputSchema"]["type"] == "object" && t.get("parameters").is_none()));
    }
}
