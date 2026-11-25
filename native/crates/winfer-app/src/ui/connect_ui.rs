//! Winfer Connect, the client side: the panes of paired machines (their mobile-protocol mirror,
//! docs/mobile/protocol.md), the sidebar's Connected section, requests to them and notifications.
//! The host side is the mobile bridge itself (`mobile.rs`): peers are devices `connect:<peer>`.

use super::commands::{Command, PaneRef};
use super::menu::MenuItem;
use super::*;
use gpui_kit::component::tooltip::Tooltip;
use serde_json::{Value, json};
use std::collections::HashSet;
use std::time::Instant;
use winfer_connect::pairing::Kind;

/// How long after the user's last input on a machine its own notification covers it.
const ACTIVE_FOR: Duration = Duration::from_secs(120);
const NOTIFY_THROTTLE: Duration = Duration::from_secs(5);
/// Requests that are this instance's to answer (the rest are answers and pushes for us).
/// How long an agent's call on another machine may take.
const TOOL_TIMEOUT: Duration = Duration::from_secs(30);
const TERM_REQUESTS: [&str; 4] = ["term.attach", "term.in", "term.resize", "term.detach"];
const TOOL_REQUESTS: [&str; 1] = ["tool.call"];
const REQUESTS: [&str; 13] = ["panes.get", "thread.get", "send", "approve", "decline", "interrupt", "choose", "config.get", "models.get", "pane.set", "pane.new", "pane.close", "usage.get"];

#[derive(Clone, Debug)]
pub struct RemotePane {
    pub id: String,
    pub title: String,
    pub workspace: String,
    pub workspace_id: String,
    pub archived: bool,
    pub provider: String,
    pub status: AgentStatus,
    pub message: Option<String>,
    pub request_kind: Option<String>,
    pub request_text: Option<String>,
    pub reply: Option<String>,
}

impl RemotePane {
    fn parse(v: &Value) -> Option<RemotePane> {
        Some(RemotePane {
            id: v["id"].as_str()?.to_string(),
            title: v["title"].as_str().unwrap_or("Agent").to_string(),
            workspace: v["workspace"].as_str().unwrap_or_default().to_string(),
            workspace_id: v["workspaceId"].as_str().unwrap_or_default().to_string(),
            archived: v["archived"].as_bool().unwrap_or(false),
            provider: v["provider"].as_str().unwrap_or("shell").to_string(),
            status: parse_status(v["status"].as_str().unwrap_or("idle")),
            message: v["message"].as_str().map(str::to_string),
            request_kind: v["request"]["kind"].as_str().map(str::to_string),
            request_text: v["request"]["text"].as_str().map(str::to_string),
            reply: v["reply"].as_str().map(str::to_string),
        })
    }
}

pub fn parse_status(s: &str) -> AgentStatus {
    match s {
        "working" => AgentStatus::Working,
        "needs-input" => AgentStatus::NeedsInput,
        "done" => AgentStatus::Done,
        "error" => AgentStatus::Error,
        _ => AgentStatus::Idle,
    }
}

/// A paired machine's sessions as last heard.
#[derive(Clone, Debug, Default)]
pub struct Mirror {
    pub panes: Vec<RemotePane>,
    pub workspaces: Vec<(String, String)>,
    pub providers: Vec<(String, String)>,
    pub default_provider: Option<String>,
    pub loaded: bool,
}

#[derive(Default)]
pub(super) struct ConnectClient {
    pub mirrors: HashMap<String, Mirror>,
    /// Collapsed machines in the sidebar (expanded by default).
    pub collapsed: HashSet<String>,
    seq: u64,
    /// Requests waiting for an answer: id → what to say if it fails.
    pending: HashMap<String, String>,
    /// When each peer last told us its user was at it.
    active_at: HashMap<String, Instant>,
    last_notified: HashMap<String, Instant>,
    last_presence: Option<Instant>,
    ticks: u32,
    /// Caught up with the links once the app had loaded (events before that went nowhere).
    synced: bool,
    #[cfg(debug_assertions)]
    dev_opened: bool,
    /// Agents' calls waiting on another machine: our request id → their MCP call id.
    tool_calls: HashMap<String, String>,
}

/// What a Connect agent tool did: answered, or waits for the other machine.
pub(super) enum ToolOutcome {
    Done(Result<String, String>),
    Later,
}

/// The agent tools Connect adds (listed with the plugins' tools, see `agent_api`).
pub(super) fn agent_tools() -> Vec<Value> {
    if crate::connect::get().is_none_or(|c| c.peers().is_empty()) {
        return Vec::new();
    }
    vec![
        json!({"name": "list_machines", "description": "List the other winfer machines paired with this one (Winfer Connect): name, desktop or server, and whether it's online. Their panes are reachable with list_remote_panes, read_pane, send_to_pane and spawn_remote_agent.", "inputSchema": {"type": "object", "properties": {}}}),
        json!({"name": "list_remote_panes", "description": "List the panes on a paired machine: number, agent, status, workspace. Address them as \"<machine>:<number>\" in read_pane and send_to_pane.", "inputSchema": {"type": "object", "properties": {"machine": {"type": "string", "description": "Machine name from list_machines"}}, "required": ["machine"]}}),
        json!({"name": "spawn_remote_agent", "description": "Start an agent pane on a paired machine, optionally with a first prompt and its own git worktree. Returns its \"<machine>:<number>\" address.", "inputSchema": {"type": "object", "properties": {
            "machine": {"type": "string", "description": "Machine name from list_machines"},
            "provider": {"type": "string", "description": "Agent id, e.g. claude, codex, winfer, or shell"},
            "prompt": {"type": "string"},
            "worktree": {"type": "boolean"},
            "workspace": {"type": "string", "description": "Workspace name on that machine (default: its first)"}
        }, "required": ["machine", "provider"]}}),
    ]
}

impl Root {
    /// `connect://peer`: a paired machine came online or went away.
    pub(super) fn on_connect_peer(&mut self, payload: &Value, cx: &mut Context<Self>) {
        let peer = payload["peer"].as_str().unwrap_or_default().to_string();
        if payload["up"] == true {
            self.connect_request(&peer, json!({"t": "panes.get"}), None);
            // Tiles showing its panes attach again (a new link: nothing was replayed).
            for local in crate::connect_term::locals_of_peer(&peer) {
                if let (Some(session), Some((_, pane))) = (SESSIONS.get(&local), crate::connect_term::remote_of(&local)) {
                    let size = *session.size.lock();
                    crate::connect::send(&peer, json!({"t": "term.attach", "paneId": pane, "cols": size.cols, "rows": size.rows}));
                }
            }
            // Tell it whether we're at this machine, for its notifications.
            if let Some(at) = self.connect.last_presence.filter(|t| t.elapsed() < ACTIVE_FOR) {
                let _ = at;
                crate::connect::send(&peer, json!({"t": "connect.presence", "active": true}));
            }
        } else {
            crate::connect::unsubscribe(&peer);
            for local in crate::connect_term::locals_of_peer(&peer) {
                if let Some(session) = SESSIONS.get(&local) {
                    session.write_screen(&[]);
                }
            }
        }
        cx.notify();
    }

    // ---- terminals (host) ----

    /// A peer attaching to, typing into, sizing or leaving one of this machine's panes.
    fn on_term_request(&mut self, peer: &str, msg: &Value, cx: &mut Context<Self>) {
        let pane_id = msg["paneId"].as_str().unwrap_or_default().to_string();
        let pane = self.state.read(cx).pane(&pane_id).cloned().filter(|p| p.remote.is_none());
        let Some(pane) = pane else {
            if msg["t"] == "term.attach" {
                crate::connect::send(peer, json!({"t": "term.exit", "paneId": pane_id, "code": 0, "gone": true}));
            }
            return;
        };
        let size = |m: &Value| (m["cols"].as_u64().unwrap_or(120).clamp(2, 1000) as usize, m["rows"].as_u64().unwrap_or(32).clamp(2, 500) as usize);
        match msg["t"].as_str().unwrap_or_default() {
            "term.attach" => {
                // A pane that was never drawn here has no process yet.
                if SESSIONS.get(&pane_id).is_none_or(|s| !s.spawned.load(std::sync::atomic::Ordering::Relaxed)) {
                    self.start_detached(&pane_id, cx);
                }
                let snapshot = crate::connect_term::attach(&pane_id, peer);
                let chat = SESSIONS.get(&pane_id).is_some_and(|s| s.is_code());
                crate::connect::send(peer, json!({"t": "term.snapshot", "paneId": pane_id, "data": crate::connect_term::b64(&snapshot), "chat": chat, "provider": pane.provider}));
                let (cols, rows) = size(msg);
                crate::connect_term::set_sized_by(&pane_id, peer);
                if let Some(session) = SESSIONS.get(&pane_id) {
                    session.force_resize(cols, rows);
                    session.write_screen(&[]);
                }
                let name = crate::connect::peer_name(peer).unwrap_or_else(|| "another machine".into());
                self.update_state(cx, |s| s.log_activity(&pane.workspace_id, format!("Opened on {name}"), Some(pane.provider.clone())));
            }
            "term.in" => {
                let data = String::from_utf8_lossy(&crate::connect_term::unb64(msg["data"].as_str().unwrap_or_default())).to_string();
                if !data.is_empty() {
                    let id = pane_id.clone();
                    backend::background(move || {
                        let _ = winfer_backend::pty::pty_write(backend::app().state(), id, data);
                    });
                }
            }
            "term.resize" => {
                let (cols, rows) = size(msg);
                crate::connect_term::set_sized_by(&pane_id, peer);
                if let Some(session) = SESSIONS.get(&pane_id) {
                    session.force_resize(cols, rows);
                    session.write_screen(&[]);
                }
            }
            _ => {
                crate::connect_term::detach(&pane_id, peer);
                if let Some(session) = SESSIONS.get(&pane_id) {
                    session.write_screen(&[]);
                }
            }
        }
        cx.notify();
    }

    // ---- terminals (client) ----

    fn on_term_reply(&mut self, peer: &str, msg: &Value, cx: &mut Context<Self>) {
        let pane = msg["paneId"].as_str().unwrap_or_default();
        for local in crate::connect_term::locals_of(peer, pane) {
            let Some(session) = SESSIONS.get(&local) else { continue };
            match msg["t"].as_str().unwrap_or_default() {
                "term.snapshot" => {
                    crate::connect_term::set_reclaimed(&local, false);
                    if let Some(model) = self.code_models.get(&local).cloned() {
                        model.update(cx, |m, cx| m.reset(cx));
                    } else {
                        session.write_screen(b"\x1b[2J\x1b[3J\x1b[H");
                    }
                    session.on_output(&crate::connect_term::unb64(msg["data"].as_str().unwrap_or_default()));
                }
                "term.out" => session.on_output(&crate::connect_term::unb64(msg["data"].as_str().unwrap_or_default())),
                "term.reclaim" => {
                    crate::connect_term::set_reclaimed(&local, true);
                    session.write_screen(&[]);
                }
                _ => {
                    let code = msg["code"].as_u64().unwrap_or(0) as u32;
                    session.exited.store(true, std::sync::atomic::Ordering::Relaxed);
                    if let Some(model) = self.code_models.get(&local).cloned() {
                        model.update(cx, |m, cx| m.on_exit(code, cx));
                    } else {
                        let why = if msg["gone"] == true { "That session is gone on its machine." } else { "The process exited on its machine." };
                        session.write_screen(format!("\r\n\x1b[2m{why}\x1b[0m\r\n").as_bytes());
                    }
                    let status = if code == 0 { AgentStatus::Done } else { AgentStatus::Error };
                    let focused = status::window_focused();
                    self.state.update(cx, |s, cx| {
                        s.set_pane_status(&local, status, focused);
                        cx.notify();
                    });
                }
            }
        }
        cx.notify();
    }

    /// A remote pane's status, shown on the tiles that show it here.
    fn sync_remote_tiles(&mut self, peer: &str, pane: &RemotePane, cx: &mut Context<Self>) {
        let locals = crate::connect_term::locals_of(peer, &pane.id);
        if locals.is_empty() {
            return;
        }
        let focused = status::window_focused();
        self.state.update(cx, |s, cx| {
            for local in &locals {
                if s.pane(local).is_some_and(|p| p.status != pane.status) {
                    s.set_pane_status(local, pane.status, focused);
                }
            }
            cx.notify();
        });
    }

    /// Opens a paired machine's pane as a tile here (or focuses the one already open).
    pub(super) fn open_remote_pane(&mut self, peer: &str, pane_id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let existing = self.state.read(cx).all_panes().find(|p| p.remote.as_ref().is_some_and(|r| r.peer == peer && r.pane == pane_id)).map(|p| p.id.clone());
        if let Some(id) = existing {
            self.execute(Command::Focus(PaneRef::Id(id)), window, cx);
            return;
        }
        let Some(remote) = self.connect.mirrors.get(peer).and_then(|m| m.panes.iter().find(|p| p.id == pane_id)).cloned() else { return };
        let machine = crate::connect::peer_name(peer).unwrap_or_else(|| "another machine".into());
        let reference = crate::model::RemoteRef { peer: peer.to_string(), pane: pane_id.to_string(), machine: machine.clone() };
        let title = format!("{} · {machine}", remote.title);
        self.state.update(cx, |s, cx| {
            let id = s.add_pane(&remote.provider, crate::model::AddPane { worktree: Some(false), custom_title: Some(title), cwd: Some(String::new()), ..Default::default() });
            if let Some(p) = s.workspaces.iter_mut().flat_map(|w| w.panes.iter_mut()).find(|p| p.id == id) {
                p.remote = Some(reference);
                p.status = remote.status;
                p.model = None;
            }
            cx.notify();
        });
    }

    /// Sends a request to a peer; `failure`: what to toast if it comes back not ok.
    pub(super) fn connect_request(&mut self, peer: &str, mut msg: Value, failure: Option<String>) -> String {
        self.connect.seq += 1;
        let id = format!("c{}", self.connect.seq);
        msg["id"] = json!(id);
        if let Some(f) = failure {
            self.connect.pending.insert(id.clone(), f);
        }
        crate::connect::send(peer, msg);
        id
    }

    /// `connect://message`.
    pub(super) fn on_connect_message(&mut self, payload: &Value, window: &mut Window, cx: &mut Context<Self>) {
        let peer = payload["peer"].as_str().unwrap_or_default().to_string();
        let msg = &payload["msg"];
        let kind = msg["t"].as_str().unwrap_or_default();
        if TOOL_REQUESTS.contains(&kind) {
            return self.on_tool_call(&peer, msg, cx);
        }
        if TERM_REQUESTS.contains(&kind) {
            return self.on_term_request(&peer, msg, cx);
        }
        if REQUESTS.contains(&kind) {
            if kind == "panes.get" {
                crate::connect::subscribe(&peer);
            }
            let request = json!({"device": crate::connect::device(&peer), "msg": msg});
            self.on_mobile_request(&request, window, cx);
            return;
        }
        match kind {
            "connect.presence" => {
                if msg["active"] == true {
                    self.connect.active_at.insert(peer, Instant::now());
                }
            }
            "panes" => {
                let mirror = Mirror {
                    panes: msg["panes"].as_array().into_iter().flatten().filter_map(RemotePane::parse).collect(),
                    workspaces: msg["workspaces"].as_array().into_iter().flatten().map(|w| (w["id"].as_str().unwrap_or_default().to_string(), w["name"].as_str().unwrap_or_default().to_string())).collect(),
                    providers: msg["providers"].as_array().into_iter().flatten().map(|p| (p["id"].as_str().unwrap_or_default().to_string(), p["name"].as_str().unwrap_or_default().to_string())).collect(),
                    default_provider: msg["defaultProvider"].as_str().map(str::to_string),
                    loaded: true,
                };
                self.connect.mirrors.insert(peer, mirror);
                cx.notify();
            }
            "status" => {
                let Some(pane) = RemotePane::parse(&msg["pane"]) else { return };
                let mirror = self.connect.mirrors.entry(peer.clone()).or_default();
                let before = mirror.panes.iter().find(|p| p.id == pane.id).map(|p| p.status);
                match mirror.panes.iter_mut().find(|p| p.id == pane.id) {
                    Some(p) => *p = pane.clone(),
                    None => mirror.panes.push(pane.clone()),
                }
                if msg["silent"] != true && before != Some(pane.status) {
                    self.connect_notify(&peer, &pane, cx);
                }
                self.sync_remote_tiles(&peer, &pane, cx);
                cx.notify();
            }
            "term.snapshot" | "term.out" | "term.exit" | "term.reclaim" => self.on_term_reply(&peer, msg, cx),
            "tool.result" => {
                if let Some(call) = self.connect.tool_calls.remove(msg["id"].as_str().unwrap_or_default()) {
                    super::agent_api::respond(&call, msg["ok"] == true, msg["text"].as_str().unwrap_or_default().to_string());
                }
            }
            "ack" | "error" => {
                let id = msg["id"].as_str().unwrap_or_default();
                if let Some(failure) = self.connect.pending.remove(id)
                    && msg["ok"] != true
                {
                    let why = msg["error"].as_str().unwrap_or("it didn't work");
                    self.toast(format!("{failure}: {why}"), cx);
                }
            }
            _ => {}
        }
    }

    /// Native notification for a remote pane that finished, asks or failed.
    fn connect_notify(&mut self, peer: &str, pane: &RemotePane, cx: &mut Context<Self>) {
        if !matches!(pane.status, AgentStatus::Done | AgentStatus::NeedsInput | AgentStatus::Error) || pane.provider == "shell" {
            return;
        }
        let Some(connect) = crate::connect::get() else { return };
        let Some(info) = connect.peers().into_iter().find(|p| p.peer_id == peer) else { return };
        if !info.notify {
            return;
        }
        // The user is at that machine: its own notification covers it.
        if self.connect.active_at.get(peer).is_some_and(|t| t.elapsed() < ACTIVE_FOR) {
            return;
        }
        let key = format!("{peer}/{}", pane.id);
        if self.connect.last_notified.get(&key).is_some_and(|t| t.elapsed() < NOTIFY_THROTTLE) {
            return;
        }
        self.connect.last_notified.insert(key.clone(), Instant::now());
        let verb = match pane.status {
            AgentStatus::NeedsInput => "needs input",
            AgentStatus::Done => "finished",
            _ => "error",
        };
        let title = format!("{} · {verb} on {}", pane.title, info.name);
        let body = match pane.status {
            AgentStatus::NeedsInput => pane.request_text.clone().or(pane.message.clone()).unwrap_or_else(|| "Needs your input".into()),
            AgentStatus::Done => pane.reply.clone().or(pane.message.clone()).unwrap_or_else(|| "Finished".into()),
            _ => pane.message.clone().unwrap_or_else(|| "Stopped with an error".into()),
        };
        let body: String = body.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(240).collect();
        if status::window_focused() {
            self.toast(title, cx);
            return;
        }
        let kind = if pane.status == AgentStatus::NeedsInput && pane.request_kind.as_deref() == Some("approval") { "approval" } else if pane.status == AgentStatus::NeedsInput { "question" } else { "info" };
        let notification = json!({ "paneId": format!("connect:{key}"), "title": title, "body": body, "kind": kind });
        backend::background(move || {
            if let Ok(n) = serde_json::from_value(notification) {
                let _ = winfer_backend::notify::notify_action(backend::app().clone(), n);
            }
        });
    }

    /// A click on a remote pane's notification (`connect:<peer>/<pane>`); false when it isn't one.
    pub(super) fn on_remote_notification(&mut self, payload: &Value, cx: &mut Context<Self>) -> bool {
        let Some((peer, pane)) = payload["paneId"].as_str().and_then(|id| id.strip_prefix("connect:")).and_then(|r| r.split_once('/')) else { return false };
        let (peer, pane) = (peer.to_string(), pane.to_string());
        let text = payload["text"].as_str().filter(|t| !t.is_empty()).map(str::to_string);
        match (payload["action"].as_str().unwrap_or_default(), text) {
            ("approve", _) => self.remote_action(&peer, &pane, "approve", None, cx),
            ("decline", _) => self.remote_action(&peer, &pane, "decline", None, cx),
            ("reply", Some(text)) => self.remote_action(&peer, &pane, "send", Some(text), cx),
            _ => {
                self.connect.collapsed.remove(&peer);
                cx.notify();
            }
        }
        true
    }

    /// approve / decline / interrupt / send / pane.close on a remote pane.
    pub(super) fn remote_action(&mut self, peer: &str, pane: &str, kind: &str, text: Option<String>, cx: &mut Context<Self>) {
        let mut msg = json!({"t": kind, "paneId": pane});
        if let Some(text) = text {
            msg["text"] = json!(text);
        }
        let name = crate::connect::peer_name(peer).unwrap_or_else(|| "the other machine".into());
        self.connect_request(peer, msg, Some(format!("Couldn't reach {name}")));
        cx.notify();
    }

    /// Starts a session on a paired machine, in its first (or given) workspace.
    pub(super) fn remote_new_pane(&mut self, peer: &str, workspace: Option<String>, provider: Option<String>, cx: &mut Context<Self>) {
        let Some(mirror) = self.connect.mirrors.get(peer) else { return };
        let Some(ws) = workspace.or_else(|| mirror.workspaces.first().map(|w| w.0.clone())) else {
            self.toast("That machine has no workspace yet.", cx);
            return;
        };
        let mut msg = json!({"t": "pane.new", "workspaceId": ws});
        if let Some(p) = provider {
            msg["provider"] = json!(p);
        }
        let name = crate::connect::peer_name(peer).unwrap_or_else(|| "the other machine".into());
        self.connect_request(peer, msg, Some(format!("Couldn't start a session on {name}")));
    }

    /// Links that came up before the app was listening: ask for their panes, and hand them ours
    /// as if they had asked (their request may have come too early).
    fn connect_resync(&mut self, cx: &mut Context<Self>) {
        let Some(connect) = crate::connect::get() else { return };
        for p in connect.peers() {
            if connect.is_up(&p.peer_id) {
                self.connect_request(&p.peer_id, json!({"t": "panes.get"}), None);
                crate::connect::subscribe(&p.peer_id);
                let mut all = self.mobile_mirror(cx);
                all["t"] = json!("panes");
                crate::connect::send(&p.peer_id, all);
            }
        }
        cx.notify();
    }

    // ---- agents across machines ----

    /// A paired machine by name (or id): exact, then case-insensitive, then a unique prefix.
    fn machine(&self, name: &str) -> Result<(String, String, bool), String> {
        let connect = crate::connect::get().ok_or("Winfer Connect isn't running.")?;
        let peers = connect.peers();
        let lower = name.trim().to_lowercase();
        let found = peers.iter().find(|p| p.name == name || p.peer_id == name)
            .or_else(|| peers.iter().find(|p| p.name.to_lowercase() == lower))
            .or_else(|| {
                let mut hits = peers.iter().filter(|p| p.name.to_lowercase().starts_with(&lower));
                let first = hits.next();
                if hits.next().is_some() { None } else { first }
            });
        let p = found.ok_or_else(|| format!("No paired machine called “{name}”. Paired: {}.", peers.iter().map(|p| p.name.clone()).collect::<Vec<_>>().join(", ")))?;
        Ok((p.peer_id.clone(), p.name.clone(), connect.is_up(&p.peer_id)))
    }

    /// A paired machine's panes in the order list_remote_panes numbers them.
    fn remote_order(&self, peer: &str) -> Vec<RemotePane> {
        let Some(m) = self.connect.mirrors.get(peer) else { return Vec::new() };
        let mut out = Vec::new();
        for (ws, _) in &m.workspaces {
            out.extend(m.panes.iter().filter(|p| &p.workspace_id == ws && !p.archived).cloned());
        }
        out
    }

    /// `machine:N` → (peer, its pane id); None when `reference` isn't one.
    fn remote_ref(&self, reference: &Value) -> Option<Result<(String, String, String), String>> {
        let text = reference.as_str()?;
        let (machine, n) = text.rsplit_once(':')?;
        let n: usize = n.trim().parse().ok()?;
        Some(self.machine(machine).and_then(|(peer, name, up)| {
            if !up {
                return Err(format!("{name} is offline."));
            }
            let panes = self.remote_order(&peer);
            let pane = panes.get(n.wrapping_sub(1)).ok_or_else(|| format!("No pane {n} on {name}. Use list_remote_panes."))?;
            Ok((peer, pane.id.clone(), name))
        }))
    }

    /// Runs `tool` on a peer; answers the MCP call when the result comes back (or times out).
    #[allow(clippy::too_many_arguments)]
    fn remote_tool(&mut self, call: &str, peer: &str, name: &str, tool: &str, args: Value, caller: &str, cx: &mut Context<Self>) -> ToolOutcome {
        let id = self.connect_request(peer, json!({"t": "tool.call", "name": tool, "args": args, "caller": caller}), None);
        self.connect.tool_calls.insert(id.clone(), call.to_string());
        let machine = name.to_string();
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(TOOL_TIMEOUT).await;
            let _ = this.update(cx, |this, _| {
                if let Some(call) = this.connect.tool_calls.remove(&id) {
                    super::agent_api::respond(&call, false, format!("{machine} didn't answer in time."));
                }
            });
        })
        .detach();
        ToolOutcome::Later
    }

    /// Connect's agent tools, and read_pane/send_to_pane on `machine:N`; None when it isn't one.
    pub(super) fn connect_tool(&mut self, call: &str, tool: &str, a: &Value, who: &str, cx: &mut Context<Self>) -> Option<ToolOutcome> {
        let me = crate::connect::get().map(|c| c.status().name).unwrap_or_default();
        let caller = format!("{who} on {me}");
        match tool {
            "list_machines" => {
                let connect = crate::connect::get()?;
                let rows: Vec<String> = connect.status().peers.iter().map(|p| {
                    let kind = if p.kind == Kind::Server { "server" } else { "desktop" };
                    let state = match p.link.via { Some(v) => format!("online ({v})"), None => "offline".into() };
                    let agents = self.connect.mirrors.get(&p.peer_id).map_or(0, |m| m.panes.iter().filter(|x| !x.archived).count());
                    format!("{} · {kind} · {state} · {agents} pane{}", p.name, if agents == 1 { "" } else { "s" })
                }).collect();
                Some(ToolOutcome::Done(Ok(if rows.is_empty() { "No paired machines.".into() } else { rows.join("\n") })))
            }
            "list_remote_panes" => {
                let (peer, name, up) = match self.machine(a["machine"].as_str().unwrap_or_default()) {
                    Ok(m) => m,
                    Err(e) => return Some(ToolOutcome::Done(Err(e))),
                };
                let m = self.connect.mirrors.get(&peer).cloned().unwrap_or_default();
                let rows: Vec<String> = self.remote_order(&peer).iter().enumerate().map(|(i, p)| {
                    let ws = m.workspaces.iter().find(|w| w.0 == p.workspace_id).map_or("", |w| w.1.as_str());
                    format!("{name}:{} · {} · {} · workspace {ws} · {}", i + 1, p.provider, super::agent_api::status_name(p.status), p.title)
                }).collect();
                let head = if up { format!("{name} (online):") } else { format!("{name} (offline, last known):") };
                Some(ToolOutcome::Done(Ok(if rows.is_empty() { format!("{head}\n(no panes)") } else { format!("{head}\n{}", rows.join("\n")) })))
            }
            "spawn_remote_agent" => {
                let (peer, name, up) = match self.machine(a["machine"].as_str().unwrap_or_default()) {
                    Ok(m) => m,
                    Err(e) => return Some(ToolOutcome::Done(Err(e))),
                };
                if !up {
                    return Some(ToolOutcome::Done(Err(format!("{name} is offline."))));
                }
                let mut args = json!({"provider": a["provider"], "prompt": a["prompt"], "worktree": a["worktree"]});
                if let Some(ws) = a["workspace"].as_str() {
                    args["workspace"] = json!(ws);
                }
                Some(self.remote_tool(call, &peer, &name, "spawn_agent", args, &caller, cx))
            }
            "read_pane" | "send_to_pane" => {
                let (peer, pane, name) = match self.remote_ref(&a["pane"])? {
                    Ok(r) => r,
                    Err(e) => return Some(ToolOutcome::Done(Err(e))),
                };
                let mut args = a.clone();
                args["pane"] = json!(pane);
                Some(self.remote_tool(call, &peer, &name, tool, args, &caller, cx))
            }
            _ => None,
        }
    }

    /// A paired machine's agent calls a tool here (full trust: as if it were one of ours).
    fn on_tool_call(&mut self, peer: &str, msg: &Value, cx: &mut Context<Self>) {
        let id = msg["id"].clone();
        let tool = msg["name"].as_str().unwrap_or_default().to_string();
        let a = msg["args"].clone();
        let caller = msg["caller"].as_str().unwrap_or("An agent on another machine").to_string();
        let s = self.state.read(cx);
        // The workspace: the target pane's, the one named, or the first.
        let ws = match (&tool[..], a["pane"].as_str(), a["workspace"].as_str()) {
            (_, Some(pane), _) => s.workspaces.iter().find(|w| w.panes.iter().any(|p| p.id == pane)),
            ("spawn_agent", None, Some(name)) => s.workspaces.iter().find(|w| w.name.eq_ignore_ascii_case(name)),
            _ => s.workspaces.iter().find(|w| w.id == s.active_workspace_id).or(s.workspaces.first()),
        }
        .cloned();
        let result = match ws {
            None => Err("No such pane or workspace on this machine.".to_string()),
            Some(_) if !matches!(&tool[..], "read_pane" | "send_to_pane" | "spawn_agent") => Err(format!("{tool} can't be called from another machine.")),
            Some(ws) => {
                let me = crate::model::Pane { id: format!("remote:{peer}"), provider: "winfer".into(), cwd: ws.root_dir.clone(), workspace_id: ws.id.clone(), ..Default::default() };
                let result = self.run_remote_sync_tool(&tool, &ws, &me, &a, &caller, cx);
                // Numbers mean this machine's order; the caller addresses panes as machine:N.
                if tool == "spawn_agent" && let Ok(text) = &result {
                    let me_name = crate::connect::get().map(|c| c.status().name).unwrap_or_default();
                    Ok(format!("{text} (on {me_name}; see list_remote_panes for its address)"))
                } else {
                    result
                }
            }
        };
        let (ok, text) = match result {
            Ok(t) => (true, t),
            Err(t) => (false, t),
        };
        crate::connect::send(peer, json!({"t": "tool.result", "id": id, "ok": ok, "text": text}));
    }

    /// Called with the user's input here: peers hold back their notifications while we're active.
    pub(super) fn connect_note_activity(&mut self) {
        if self.connect.last_presence.is_some_and(|t| t.elapsed() < Duration::from_secs(20)) {
            return;
        }
        self.connect.last_presence = Some(Instant::now());
        if let Some(connect) = crate::connect::get() {
            for p in connect.peers() {
                connect.send(&p.peer_id, json!({"t": "connect.presence", "active": true}));
            }
        }
    }

    /// Every second: refresh mirrors now and then (pushes keep them current in between).
    pub(super) fn tick_connect(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Development aid: open the first remote pane as a tile once it's known.
        #[cfg(debug_assertions)]
        if std::env::var_os("WINFER_NATIVE_CONNECT_OPEN").is_some() && !self.connect.dev_opened {
            let want = std::env::var("WINFER_NATIVE_CONNECT_OPEN").unwrap_or_default();
            let first = self.connect.mirrors.iter().find_map(|(peer, m)| m.panes.iter().find(|p| want == "1" || p.provider == want).map(|p| (peer.clone(), p.id.clone())));
            if let Some((peer, pane)) = first {
                self.connect.dev_opened = true;
                self.open_remote_pane(&peer, &pane, window, cx);
                // …and type into it (`WINFER_NATIVE_CONNECT_TYPE`, `\n` for Enter).
                if let Ok(text) = std::env::var("WINFER_NATIVE_CONNECT_TYPE") {
                    cx.spawn(async move |_, cx| {
                        cx.background_executor().timer(Duration::from_secs(3)).await;
                        let local = crate::connect_term::locals_of(&peer, &pane).into_iter().next();
                        if let Some(session) = local.and_then(|l| SESSIONS.get(&l)) {
                            session.send(&text.replace("\\n", "\r"));
                        }
                    })
                    .detach();
                }
            }
        }
        if !self.connect.synced && self.state.read(cx).hydrated {
            self.connect.synced = true;
            self.connect_resync(cx);
        }
        self.connect.ticks += 1;
        if !self.connect.ticks.is_multiple_of(60) {
            return;
        }
        let Some(connect) = crate::connect::get() else { return };
        for p in connect.peers() {
            if connect.is_up(&p.peer_id) {
                self.connect_request(&p.peer_id, json!({"t": "panes.get"}), None);
            }
        }
        let _ = cx;
    }

    /// The sidebar's Connected section: each paired machine with its sessions.
    pub(super) fn render_connected(&mut self, c: Colors, cx: &mut Context<Self>) -> Option<Div> {
        let connect = crate::connect::get()?;
        let status = connect.status();
        if status.peers.is_empty() {
            return None;
        }
        let mono = crate::fonts::mono(cx);
        let mut list = div().flex().flex_col().gap_1p5();
        for peer in &status.peers {
            let up = peer.link.via.is_some();
            let mirror = self.connect.mirrors.get(&peer.peer_id).cloned().unwrap_or_default();
            let live: Vec<&RemotePane> = mirror.panes.iter().filter(|p| !p.archived).collect();
            let agents = live.iter().filter(|p| p.provider != "shell").count();
            let waiting = live.iter().filter(|p| p.status == AgentStatus::NeedsInput).count();
            let collapsed = self.connect.collapsed.contains(&peer.peer_id);
            let icon = if peer.kind == Kind::Server { IconName::Server } else { IconName::Monitor };
            let (label, _) = super::settings_connect::link_label(peer, now_ms());
            let toggle = peer.peer_id.clone();
            let add_peer = peer.peer_id.clone();
            let tip = format!("{}\n{label}\nClick to show or hide its sessions", peer.name);
            let header = div().id(SharedString::from(format!("peer-{}", peer.peer_id))).flex().items_center().gap_2().px_2().py(px(6.0)).rounded(px(10.0)).cursor_pointer().text_size(px(13.0))
                .text_color(if up { c.fg } else { c.muted })
                .hover(move |d| d.bg(c.hover).text_color(c.fg))
                .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
                .on_click(cx.listener(move |this, _, _, cx| {
                    if !this.connect.collapsed.remove(&toggle) {
                        this.connect.collapsed.insert(toggle.clone());
                    }
                    cx.notify();
                }))
                .child(div().relative().size(px(22.0)).flex_shrink_0().flex().items_center().justify_center().rounded(px(6.0)).bg(c.hover)
                    .child(Icon::new(icon).size(px(12.0)).text_color(c.muted))
                    .child(div().absolute().right(px(-2.0)).bottom(px(-2.0)).size(px(7.0)).rounded_full().bg(if up { c.success } else { c.subtle })))
                .child(div().flex_1().min_w_0().truncate().child(peer.name.clone()))
                .when(waiting > 0, |d| d.child(div().text_size(px(10.5)).text_color(c.warning).child(format!("{waiting} need{} you", if waiting == 1 { "s" } else { "" }))))
                .when(waiting == 0 && agents > 0, |d| d.child(div().text_size(px(10.5)).text_color(c.subtle).font_family(mono.clone()).child(format!("{agents} agent{}", if agents == 1 { "" } else { "s" }))))
                .when(up && mirror.loaded, |d| d.child(div().id(SharedString::from(format!("peer-new-{}", peer.peer_id))).rounded_sm().p_0p5().text_color(c.subtle).hover(move |d| d.bg(c.active).text_color(c.fg))
                    .tooltip(|window, cx| Tooltip::new("New session on this machine").build(window, cx))
                    .on_mouse_down(MouseButton::Left, cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                        cx.stop_propagation();
                        let items = this.remote_new_menu(&add_peer);
                        this.open_menu(e.position, Some("New session on this machine".into()), items, cx);
                    }))
                    .child(Icon::new(IconName::Plus).size(px(12.0)))))
                .child(Icon::new(if collapsed { IconName::ChevronRight } else { IconName::ChevronDown }).size(px(13.0)).text_color(c.subtle));
            let mut group = div().flex().flex_col().child(header);
            if !collapsed {
                let mut rows = div().flex().flex_col().gap_0p5().pl_2();
                if !up && live.is_empty() {
                    rows = rows.child(div().px_2().py_1().text_size(px(11.5)).text_color(c.subtle).child(label.clone()));
                } else if live.is_empty() && mirror.loaded {
                    rows = rows.child(div().px_2().py_1().text_size(px(11.5)).text_color(c.subtle).child("No sessions"));
                }
                let many_workspaces = mirror.workspaces.len() > 1;
                for (ws_id, ws_name) in &mirror.workspaces {
                    let panes: Vec<&&RemotePane> = live.iter().filter(|p| &p.workspace_id == ws_id).collect();
                    if panes.is_empty() {
                        continue;
                    }
                    if many_workspaces {
                        rows = rows.child(div().px_2().pt_1().text_size(px(10.5)).font_weight(FontWeight::SEMIBOLD).text_color(c.subtle).child(ws_name.clone()));
                    }
                    for p in panes {
                        rows = rows.child(self.render_remote_row(&peer.peer_id, p, up, c, cx));
                    }
                }
                group = group.child(rows);
            }
            list = list.child(group);
        }
        Some(div().flex().flex_col()
            .child(div().flex().h(px(26.0)).mt_2().items_center().pl_1p5().text_size(px(11.0)).font_weight(FontWeight::SEMIBOLD).text_color(c.subtle).child("Connected"))
            .child(list))
    }

    fn render_remote_row(&self, peer: &str, pane: &RemotePane, up: bool, c: Colors, cx: &mut Context<Self>) -> AnyElement {
        let meta = match pane.status {
            AgentStatus::NeedsInput => Some(("needs you", c.warning)),
            AgentStatus::Done => Some(("done", c.success)),
            AgentStatus::Error => Some(("error", c.danger)),
            _ => None,
        };
        let detail = if pane.status == AgentStatus::NeedsInput { pane.request_text.clone().or(pane.message.clone()) } else { pane.message.clone() }
            .map(|t| t.lines().find(|l| !l.trim().is_empty()).unwrap_or_default().to_string())
            .filter(|t| !t.is_empty());
        let (menu_peer, menu_pane, menu_status) = (peer.to_string(), pane.id.clone(), pane.status);
        let (open_peer, open_pane) = (peer.to_string(), pane.id.clone());
        let asking = pane.status == AgentStatus::NeedsInput && pane.request_kind.as_deref() == Some("approval");
        let (approve_peer, approve_pane) = (peer.to_string(), pane.id.clone());
        let tip = format!("{} · {}{}", pane.title, pane.workspace, detail.as_ref().map_or(String::new(), |d| format!("\n{d}")));
        div().id(SharedString::from(format!("rrow-{peer}-{}", pane.id))).flex().items_center().gap(px(9.0)).rounded(px(7.0)).px_1p5().py(px(5.0))
            .text_size(px(12.5)).text_color(c.muted).when(!up, |d| d.opacity(0.6))
            .hover(move |d| d.bg(c.hover).text_color(c.fg))
            .when(up, |d| d.cursor_pointer())
            .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
            .on_click(cx.listener(move |this, _, window, cx| {
                if up {
                    this.open_remote_pane(&open_peer, &open_pane, window, cx);
                }
            }))
            .on_mouse_down(MouseButton::Right, cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                let items = this.remote_pane_menu(&menu_peer, &menu_pane, menu_status);
                this.open_menu(e.position, None, items, cx);
            }))
            .child(Self::agent_tile(&pane.provider, pane.status, c))
            .child(div().flex_1().min_w_0().flex().flex_col()
                .child(div().flex().items_center().gap_1p5().line_height(px(15.0))
                    .child(div().flex_1().min_w_0().truncate().child(pane.title.clone()))
                    .when_some(meta, |d, (text, color)| d.child(div().flex_shrink_0().text_size(px(10.5)).text_color(color).child(text))))
                .when_some(detail, |d, t| d.child(div().truncate().text_size(px(11.0)).text_color(if pane.status == AgentStatus::NeedsInput { c.warning } else { c.subtle }).child(t))))
            .when(asking && up, |d| d.child(div().id(SharedString::from(format!("rapprove-{peer}-{}", pane.id))).flex_shrink_0().h(px(22.0)).px_2().flex().items_center().rounded_md().bg(c.accent)
                .text_size(px(11.0)).font_weight(FontWeight::MEDIUM).text_color(c.on_accent).cursor_pointer().hover(|d| d.opacity(0.9))
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.remote_action(&approve_peer, &approve_pane, "approve", None, cx);
                }))
                .child("Approve")))
            .into_any_element()
    }

    /// One entry per agent installed on the machine, in each of its workspaces when it has several.
    fn remote_new_menu(&self, peer: &str) -> Vec<MenuItem> {
        let Some(mirror) = self.connect.mirrors.get(peer) else { return Vec::new() };
        let mut items = Vec::new();
        for (ws_id, ws_name) in &mirror.workspaces {
            if mirror.workspaces.len() > 1 {
                if !items.is_empty() {
                    items.push(MenuItem::Separator);
                }
                items.push(MenuItem::run(ws_name.clone(), |_: &mut Root, _: &mut Window, _: &mut Context<Root>| {}).disabled(true));
            }
            for (id, name) in &mirror.providers {
                let (peer, ws, provider) = (peer.to_string(), ws_id.clone(), id.clone());
                let label = if mirror.default_provider.as_deref() == Some(id.as_str()) { format!("{name} (default)") } else { name.clone() };
                items.push(MenuItem::run(label, move |this: &mut Root, _: &mut Window, cx: &mut Context<Root>| this.remote_new_pane(&peer, Some(ws.clone()), Some(provider.clone()), cx)));
            }
        }
        items
    }

    fn remote_pane_menu(&self, peer: &str, pane: &str, status: AgentStatus) -> Vec<MenuItem> {
        let mut items = Vec::new();
        let action = |label: &str, kind: &'static str| {
            let (peer, pane) = (peer.to_string(), pane.to_string());
            MenuItem::run(label, move |this: &mut Root, _window: &mut Window, cx: &mut Context<Root>| this.remote_action(&peer, &pane, kind, None, cx))
        };
        if status == AgentStatus::NeedsInput {
            items.push(action("Approve", "approve"));
            items.push(action("Decline", "decline"));
        }
        items.push(action("Interrupt", "interrupt"));
        items.push(action("Close on that machine", "pane.close"));
        items
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

#[cfg(test)]
mod tests {
    use super::{AgentStatus, RemotePane, Value, parse_status};

    #[test]
    fn remote_panes_parse_from_the_mobile_protocol() {
        let v: Value = serde_json::from_str(r#"{"id": "p1", "title": "fix bug", "workspace": "web", "workspaceId": "w1", "archived": false, "provider": "claude",
            "status": "needs-input", "message": "Allow Bash?", "request": {"kind": "approval", "text": "Run tests"}, "reply": null}"#).unwrap();
        let p = RemotePane::parse(&v).unwrap();
        assert_eq!(p.status, AgentStatus::NeedsInput);
        assert_eq!(p.request_kind.as_deref(), Some("approval"));
        assert_eq!(p.provider, "claude");
        assert!(RemotePane::parse(&serde_json::from_str(r#"{"title": "no id"}"#).unwrap()).is_none());
        assert_eq!(parse_status("bogus"), AgentStatus::Idle);
    }
}
