//! Winfer Connect, the client side: the panes of paired machines (their mobile-protocol mirror,
//! docs/mobile/protocol.md), the sidebar's Connected section, requests to them and notifications.
//! The host side is the mobile bridge itself (`mobile.rs`): peers are devices `connect:<peer>`.

use super::commands::{Command, PaneRef};
use super::menu::MenuItem;
use super::settings_kit::alpha;
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
    /// The branch of its worktree there; None in the project folder (or from an older server).
    pub branch: Option<String>,
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
            branch: v["branch"].as_str().map(str::to_string),
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
    /// A winferd's version and the newer release it found.
    pub server: Option<ServerInfo>,
}

#[derive(Clone, Debug, Default)]
pub struct ServerInfo {
    pub version: String,
    pub update: Option<String>,
    pub docker: bool,
    /// "load 0.4/8 · mem 2.1/7.8 GB · disk 41% · up 3 d", when it reports them.
    pub health: Option<String>,
    /// Phones paired with it directly: (device id, name, notifications on).
    pub phones: Vec<(String, String, bool)>,
}

fn gb(bytes: f64) -> String {
    format!("{:.1}", bytes / 1e9)
}

fn health(s: &Value) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(load) = s["load"].as_f64() {
        parts.push(format!("load {load:.2}/{}", s["cpus"].as_u64().unwrap_or(1)));
    }
    if let (Some(used), Some(total)) = (s["memUsed"].as_f64(), s["memTotal"].as_f64()) {
        parts.push(format!("mem {}/{} GB", gb(used), gb(total)));
    }
    if let (Some(used), Some(total)) = (s["diskUsed"].as_f64(), s["diskTotal"].as_f64().filter(|t| *t > 0.0)) {
        parts.push(format!("disk {:.0}% of {} GB", used / total * 100.0, gb(total)));
    }
    if let Some(up) = s["uptime"].as_u64() {
        parts.push(if up >= 86400 { format!("up {} d", up / 86400) } else { format!("up {} h", up / 3600) });
    }
    (!parts.is_empty()).then(|| parts.join(" · "))
}

impl ServerInfo {
    fn parse(v: &Value) -> Option<ServerInfo> {
        Some(ServerInfo {
            version: v["version"].as_str()?.to_string(),
            update: v["update"].as_str().map(str::to_string),
            docker: v["docker"].as_bool().unwrap_or(false),
            health: health(&v["stats"]),
            phones: v["phones"].as_array().into_iter().flatten().filter_map(|p| Some((p["deviceId"].as_str()?.to_string(), p["name"].as_str().unwrap_or("Phone").to_string(), p["push"] == true))).collect(),
        })
    }
}

#[derive(Default)]
pub(super) struct ConnectClient {
    pub mirrors: HashMap<String, Mirror>,
    /// Collapsed machines in the sidebar (expanded by default).
    pub collapsed: HashSet<String>,
    /// Workspace removals asked of another machine: request id → what to say once it's done.
    pub removing: HashMap<String, String>,
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
    #[cfg(debug_assertions)]
    dev_pushed: bool,
    #[cfg(debug_assertions)]
    dev_folder: bool,
    /// `WINFER_NATIVE_CONNECT_SPAWN`'s workspace request, once sent.
    dev_spawn: Option<String>,
    /// The dev spawn's `pane.new` request, then the pane it made (`WINFER_NATIVE_CONNECT_OPEN=spawned`).
    dev_spawn_pane: Option<String>,
    dev_spawned: Option<String>,
    dev_review: bool,
    dev_usage: bool,
    dev_phone: bool,
    dev_history: bool,
    dev_unpaired: bool,
    dev_browser: bool,
    dev_menu: bool,
    /// Development aid: the server whose History to show once it's open.
    pub dev_history_of: Option<String>,
    /// The `review.diff` request the Changes dialog waits on.
    pub diff_request: Option<String>,
    /// The remote tile asking, and since when (`WINFER_NATIVE_CONNECT_APPROVE`).
    dev_asked: Option<(String, Instant)>,
    /// Agents' calls waiting on another machine: our request id → their MCP call id.
    tool_calls: HashMap<String, String>,
    /// Agents' calls waiting on a request's reply (ack, usage): request id → (MCP call, what, machine).
    tool_waits: HashMap<String, (String, &'static str, String)>,
    /// Sign-ins pushed and not yet answered: request id → (account here, its fingerprint, machine name).
    pushes: HashMap<String, (String, String, String)>,
    /// The "New workspace on ‹machine›" dialog.
    pub folder: Option<super::connect_folder::FolderDialog>,
}

/// What a Connect agent tool did: answered, or waits for the other machine.
pub(super) enum ToolOutcome {
    Done(Result<String, String>),
    Later,
}

/// The agent tools Connect adds (listed with the plugins' tools, see `agent_api`).
pub(super) fn agent_tools() -> Vec<Value> {
    // This machine's workspaces: with or without Connect.
    let mut tools = vec![
        json!({"name": "add_workspace", "description": "Open a workspace on this machine: an existing folder (path), or a git repository it clones first (git_url, optional branch and parent folder).", "inputSchema": {"type": "object", "properties": workspace_props(None)}}),
        json!({"name": "remove_workspace", "description": "Remove a workspace from this machine: its panes close; the folder and worktrees stay. Not the one you run in.", "inputSchema": {"type": "object", "properties": {"workspace": {"type": "string", "description": "Workspace name"}}, "required": ["workspace"]}}),
    ];
    if crate::connect::get().is_none_or(|c| !c.status().enabled) {
        return tools;
    }
    let machine = json!({"type": "string", "description": "Machine name from list_machines"});
    let pane = json!({"type": "string", "description": "\"<machine>:<number>\" from list_remote_panes"});
    tools.extend([
        json!({"name": "list_machines", "description": "List the other winfer machines paired with this one (Winfer Connect): name, desktop or server, online or not, their workspaces and agents; also machines nearby that could be paired, and a pairing in progress. Their panes are reachable with list_remote_panes, read_pane, send_to_pane, spawn_remote_agent and remote_pane_action. You run on this machine, not on any listed one: work on another machine only through these tools.", "inputSchema": {"type": "object", "properties": {}}}),
        json!({"name": "list_remote_panes", "description": "List a paired machine's workspaces and panes: number, agent, status, workspace. Address panes as \"<machine>:<number>\" in read_pane, send_to_pane and remote_pane_action.", "inputSchema": {"type": "object", "properties": {"machine": machine}, "required": ["machine"]}}),
        json!({"name": "spawn_remote_agent", "description": "Start an agent pane on a paired machine, optionally with a first prompt, in a workspace there and its own git worktree. Returns its \"<machine>:<number>\" address. To run more commands on a machine, first look for a pane of yours there (list_remote_panes) and send them to it with send_to_pane; start another only when you need one running alongside.", "inputSchema": {"type": "object", "properties": {
            "machine": machine,
            "provider": {"type": "string", "description": "Agent id, e.g. claude, codex, winfer, or shell"},
            "prompt": {"type": "string"},
            "worktree": {"type": "boolean"},
            "workspace": {"type": "string", "description": "Workspace name on that machine (default: its first); new_remote_workspace makes one"}
        }, "required": ["machine", "provider"]}}),
        json!({"name": "remote_pane_action", "description": "Act on an agent pane on a paired machine: approve or decline what it asks, interrupt it, or close the pane.", "inputSchema": {"type": "object", "properties": {
            "pane": pane, "action": {"type": "string", "enum": ["approve", "decline", "interrupt", "close"]}
        }, "required": ["pane", "action"]}}),
        json!({"name": "new_remote_workspace", "description": "Open a workspace on a paired machine (server or desktop): an existing folder there (path), or a git repository it clones first (git_url; private ones on a server need its deploy key). Returns the workspace's name for spawn_remote_agent.", "inputSchema": {"type": "object", "properties": workspace_props(Some(&machine)), "required": ["machine"]}}),
        json!({"name": "remove_remote_workspace", "description": "Remove a workspace from a paired machine: its sessions there close; the folder and worktrees stay.", "inputSchema": {"type": "object", "properties": {"machine": machine, "workspace": {"type": "string", "description": "Workspace name there (list_remote_panes)"}}, "required": ["machine", "workspace"]}}),
        json!({"name": "list_reviews", "description": "List the finished work waiting for review on paired servers (and their merge queues): review id, agent, branch, changes, tests, summary.", "inputSchema": {"type": "object", "properties": {"machine": {"type": "string", "description": "Only this machine's"}}}}),
        json!({"name": "review_action", "description": "Act on a review from list_reviews, on the server it's on: merge it (through its merge queue; tests: run the tests first), run its tests, send it back to its agent with text, discard the work, or dismiss the review.", "inputSchema": {"type": "object", "properties": {
            "review": {"type": "string", "description": "Review id from list_reviews"},
            "action": {"type": "string", "enum": ["merge", "tests", "send", "discard", "dismiss"]},
            "tests": {"type": "boolean"},
            "text": {"type": "string", "description": "For send: what to tell the agent"}
        }, "required": ["review", "action"]}}),
        json!({"name": "machine_usage", "description": "A paired machine's agent usage limits: per agent and account, how much is left of each window and when it resets.", "inputSchema": {"type": "object", "properties": {"machine": machine}, "required": ["machine"]}}),
        json!({"name": "update_server", "description": "Update a paired winfer server (winferd) to the newest release; it restarts. list_machines shows which have an update.", "inputSchema": {"type": "object", "properties": {"machine": machine}, "required": ["machine"]}}),
        json!({"name": "pair_machine", "description": "Pair another machine with this one. mode \"link\": make a one-time pair link (10 minutes) and the commands that install and pair a server with it (run one on the server; the pairing then completes by itself). \"join\": join a pair link another winfer made. \"nearby\": ask a machine on this network (list_machines shows them) to pair; \"address\": the same by host:port. Nearby and address pairings show a code on both screens that the user confirms.", "inputSchema": {"type": "object", "properties": {
            "mode": {"type": "string", "enum": ["link", "join", "nearby", "address"]},
            "link": {"type": "string", "description": "For join: the winfer-connect: link"},
            "machine": {"type": "string", "description": "For nearby: its name"},
            "address": {"type": "string", "description": "For address: host:port"},
            "name": {"type": "string", "description": "For link: the server's name (default: its host name)"}
        }, "required": ["mode"]}}),
        json!({"name": "list_sign_ins", "description": "List this machine's agent sign-ins (Claude Code, Codex accounts): id, label, signed in or not, and the machines each is pushed to.", "inputSchema": {"type": "object", "properties": {}}}),
        json!({"name": "push_sign_in", "description": "Sign a paired machine in with one of this machine's agent accounts (from list_sign_ins). The two copies stay in step when either refreshes.", "inputSchema": {"type": "object", "properties": {
            "account": {"type": "string", "description": "Account id from list_sign_ins, e.g. claude:default"},
            "machine": machine
        }, "required": ["account", "machine"]}}),
    ]);
    tools
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
                let chat = SESSIONS.get(&pane_id).is_some_and(|s| s.is_code());
                crate::connect_term::attach(&pane_id, peer, chat, &pane.provider);
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
                if !data.is_empty() && let Some(session) = SESSIONS.get(&pane_id) {
                    session.write_remote(&data, peer);
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
            // In a workspace of that machine's workspace (listed under the machine), not the one
            // open here.
            let ws = s.remote_workspace(peer, &remote.workspace_id, &format!("{} · {machine}", remote.workspace));
            if s.view() != crate::model::View::Workspace {
                s.view = Some(crate::model::View::Workspace);
            }
            let id = s.add_pane(&remote.provider, crate::model::AddPane { workspace_id: Some(ws), worktree: Some(false), custom_title: Some(title), cwd: Some(String::new()), ..Default::default() });
            s.focus_pane(&id);
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
        if matches!(kind, "ack" | "error" | "usage") {
            self.tool_reply(msg);
        }
        if TOOL_REQUESTS.contains(&kind) {
            return self.on_tool_call(&peer, msg, window, cx);
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
        if matches!(kind, "fs" | "ack" | "server.key") && self.on_folder_reply(&peer, msg, cx) {
            return;
        }
        // Another machine's "New workspace on ‹this machine›" (as a server answers it).
        let answer = |mut a: Value| {
            a["id"] = msg["id"].clone();
            crate::connect::send(&peer, a);
        };
        match kind {
            "workspace.remove" => {
                let key = msg["workspaceId"].as_str().or(msg["name"].as_str()).unwrap_or_default().to_string();
                answer(match self.workspace_remove_here(&key, cx) {
                    Ok(_) => json!({"t": "ack", "ok": true}),
                    Err(e) => json!({"t": "ack", "ok": false, "error": e}),
                });
            }
            "fs.list" if !msg["id"].is_null() => {
                let path = msg["path"].as_str().unwrap_or("~").to_string();
                let (peer, id) = (peer.clone(), msg["id"].clone());
                cx.background_executor().spawn(async move {
                    let mut a = winfer_core::folders::list(&path);
                    a["id"] = id;
                    crate::connect::send(&peer, a);
                }).detach();
            }
            "workspace.new" if !msg["id"].is_null() => {
                let dir = winfer_core::folders::expand(msg["path"].as_str().unwrap_or("~"));
                if !dir.is_dir() {
                    return answer(json!({"t": "ack", "ok": false, "error": "That folder doesn't exist on this machine."}));
                }
                let (id, name) = self.workspace_for_dir(&dir, msg["name"].as_str(), cx);
                answer(json!({"t": "ack", "ok": true, "workspaceId": id, "name": name}));
            }
            "git.clone" if !msg["id"].is_null() => {
                let (url, branch, parent) = (msg["url"].as_str().unwrap_or_default().to_string(), msg["branch"].as_str().map(str::to_string), msg["parent"].as_str().unwrap_or("~").to_string());
                let (peer, id) = (peer.clone(), msg["id"].clone());
                cx.spawn(async move |this, cx| {
                    let cloned = cx.background_executor().spawn(async move { winfer_core::folders::clone(&url, branch.as_deref(), &parent) }).await;
                    let mut a = match cloned {
                        Ok(dest) => {
                            let made = this.update(cx, |this, cx| this.workspace_for_dir(&dest, None, cx)).ok();
                            made.map_or(json!({"t": "ack", "ok": false, "error": "winfer closed"}), |(ws, name)| json!({"t": "ack", "ok": true, "workspaceId": ws, "name": name, "path": winfer_core::folders::display(&dest)}))
                        }
                        Err(e) => json!({"t": "ack", "ok": false, "error": e}),
                    };
                    a["id"] = id;
                    crate::connect::send(&peer, a);
                }).detach();
            }
            "server.key" if !msg["id"].is_null() => {
                let me = crate::connect::get().map(|c| c.status().name).unwrap_or_default();
                answer(match winfer_core::folders::deploy_key(&me) {
                    Ok(key) => json!({"t": "server.key", "key": key}),
                    Err(e) => json!({"t": "ack", "ok": false, "error": format!("Couldn't make a deploy key: {e}")}),
                });
            }
            "account.push" => {
                let mut answer = winfer_core::accounts_sync::on_push(&peer, msg);
                answer["id"] = msg["id"].clone();
                if answer["ok"] == true {
                    let name = crate::connect::peer_name(&peer).unwrap_or_else(|| "another machine".into());
                    self.toast(format!("{name} shared its {} sign-in with this machine", msg["provider"].as_str().unwrap_or("agent")), cx);
                    self.reload_accounts(cx);
                }
                crate::connect::send(&peer, answer);
            }
            "account.sync" => {
                let peer = peer.clone();
                let msg = msg.clone();
                crate::backend::background(move || winfer_core::accounts_sync::on_sync(&peer, &msg));
            }
            "account.state" => {
                winfer_core::accounts_sync::on_state(&peer, msg);
                cx.notify();
            }
            "connect.presence" => {
                if msg["active"] == true {
                    self.connect.active_at.insert(peer, Instant::now());
                }
            }
            "reviews" => self.on_remote_reviews(&peer, msg, cx),
            "review.diff" => self.on_remote_diff(&peer, msg, cx),
            "review.screenshot" => self.on_remote_screenshot(&peer, msg, cx),
            "review.history" => self.on_remote_history(&peer, msg, cx),
            "usage" => self.on_remote_usage(&peer, msg, cx),
            "mobile.pairing" => self.on_server_pairing(&peer, msg, cx),
            "panes" => {
                let mirror = Mirror {
                    panes: msg["panes"].as_array().into_iter().flatten().filter_map(RemotePane::parse).collect(),
                    workspaces: msg["workspaces"].as_array().into_iter().flatten().map(|w| (w["id"].as_str().unwrap_or_default().to_string(), w["name"].as_str().unwrap_or_default().to_string())).collect(),
                    providers: msg["providers"].as_array().into_iter().flatten().map(|p| (p["id"].as_str().unwrap_or_default().to_string(), p["name"].as_str().unwrap_or_default().to_string())).collect(),
                    default_provider: msg["defaultProvider"].as_str().map(str::to_string),
                    loaded: true,
                    server: ServerInfo::parse(&msg["server"]),
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
                self.usage_request_failed(id);
                if self.connect.diff_request.as_deref() == Some(id) && msg["ok"] != true {
                    self.connect.diff_request = None;
                    self.on_remote_diff(&peer, msg, cx);
                }
                #[cfg(debug_assertions)]
                if self.connect.dev_spawn.as_deref() == Some(id)
                    && let Some(ws) = msg["workspaceId"].as_str()
                    && let Ok(what) = std::env::var("WINFER_NATIVE_CONNECT_SPAWN")
                {
                    let mut parts = what.split('|').skip(1);
                    let (provider, prompt) = (parts.next().unwrap_or("default"), parts.next().unwrap_or(""));
                    // `winfer@local/model`: a provider with its model.
                    let (provider, model) = provider.split_once('@').unwrap_or((provider, ""));
                    self.connect.dev_spawn_pane = Some(self.connect_request(&peer, json!({"t": "pane.new", "workspaceId": ws, "provider": provider, "model": model, "text": prompt, "worktree": true}), Some("Dev spawn: agent".into())));
                }
                #[cfg(debug_assertions)]
                if self.connect.dev_spawn_pane.as_deref() == Some(id) {
                    self.connect.dev_spawned = msg["paneId"].as_str().map(str::to_string);
                }
                if let Some((local, fp, name)) = self.connect.pushes.remove(id)
                    && msg["ok"] == true
                {
                    winfer_core::accounts_sync::pushed(&peer, &local, msg["accountId"].as_str().unwrap_or_default(), &fp);
                    self.toast(format!("Signed in on {name} as “{}”. The two stay in step when either refreshes.", msg["label"].as_str().unwrap_or("the account")), cx);
                }
                if let Some(done) = self.connect.removing.remove(id)
                    && msg["ok"] == true
                {
                    self.toast(done, cx);
                }
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

    /// Settings → Accounts → Push to ‹machine›: sends this account's sign-in there.
    pub(super) fn push_account(&mut self, account: &str, peer: &str, cx: &mut Context<Self>) {
        let name = crate::connect::peer_name(peer).unwrap_or_else(|| "the other machine".into());
        match winfer_core::accounts_sync::push_message(account) {
            Ok((msg, fp)) => {
                let id = self.connect_request(peer, msg, Some(format!("Couldn't sign in on {name}")));
                self.connect.pushes.insert(id, (account.to_string(), fp, name.clone()));
                self.toast(format!("Sending the sign-in to {name}…"), cx);
            }
            Err(e) => self.toast(e, cx),
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
        // Any case and any whitespace: macOS names often have a no-break space ("Mac\u{a0}Studio").
        let norm = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase();
        let lower = norm(name);
        let found = peers.iter().find(|p| p.name == name || p.peer_id == name)
            .or_else(|| peers.iter().find(|p| norm(&p.name) == lower))
            .or_else(|| {
                let mut hits = peers.iter().filter(|p| norm(&p.name).starts_with(&lower));
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

    /// `machine:N` → (peer, N, machine name); None when `reference` isn't one. The machine
    /// resolves N itself (its order, as its list_panes_all shows), so it's never stale here.
    fn remote_ref(&self, reference: &Value) -> Option<Result<(String, String, String), String>> {
        let text = reference.as_str()?;
        let (machine, n) = text.rsplit_once(':')?;
        let n: usize = n.trim().parse().ok()?;
        Some(self.machine(machine).and_then(|(peer, name, up)| if up { Ok((peer, n.to_string(), name)) } else { Err(format!("{name} is offline.")) }))
    }

    /// Runs `tool` on a peer; answers the MCP call when the result comes back (or times out).
    #[allow(clippy::too_many_arguments)]
    /// The workspace for a folder on this machine (made, not switched to, when there's none):
    /// `(id, name)`.
    fn workspace_for_dir(&mut self, dir: &std::path::Path, name: Option<&str>, cx: &mut Context<Self>) -> (String, String) {
        let root = dir.display().to_string();
        let name = name.map(str::trim).filter(|n| !n.is_empty()).map(str::to_string).unwrap_or_else(|| dir.file_name().map_or(root.clone(), |n| n.to_string_lossy().to_string()));
        let mut made = (String::new(), String::new());
        self.update_state(cx, |s| {
            let id = s.workspace_for_folder(&root, &name);
            made = (id.clone(), s.workspaces.iter().find(|w| w.id == id).map(|w| w.name.clone()).unwrap_or_default());
        });
        made
    }

    /// An agent's add_workspace (here or from another machine): a folder, or a repository
    /// cloned first; `done` gets the answer.
    pub(super) fn workspace_add_here(&mut self, a: &Value, cx: &mut Context<Self>, done: impl FnOnce(Result<String, String>) + 'static) {
        let me = crate::connect::get().map(|c| c.status().name).unwrap_or_else(|| "this machine".into());
        let name = a["name"].as_str().map(str::to_string);
        if let Some(url) = a["git_url"].as_str().filter(|u| !u.trim().is_empty()) {
            let (url, branch, parent) = (url.to_string(), a["branch"].as_str().map(str::to_string), a["parent"].as_str().unwrap_or("~").to_string());
            cx.spawn(async move |this, cx| {
                let cloned = cx.background_executor().spawn(async move { winfer_core::folders::clone(&url, branch.as_deref(), &parent) }).await;
                done(cloned.and_then(|dest| {
                    let (_, ws) = this.update(cx, |this, cx| this.workspace_for_dir(&dest, name.as_deref(), cx)).map_err(|_| "winfer closed".to_string())?;
                    Ok(format!("Cloned into {} on {me}; it's workspace “{ws}” there.", winfer_core::folders::display(&dest)))
                }));
            })
            .detach();
            return;
        }
        let Some(path) = a["path"].as_str().filter(|p| !p.trim().is_empty()) else {
            return done(Err("Give a folder path or a git_url to clone.".into()));
        };
        let dir = winfer_core::folders::expand(path);
        if !dir.is_dir() {
            return done(Err(format!("There's no folder {path} on {me}.")));
        }
        let (_, ws) = self.workspace_for_dir(&dir, name.as_deref(), cx);
        done(Ok(format!("Workspace “{ws}” ({}) is open on {me}.", winfer_core::folders::display(&dir))));
    }

    /// Removes a workspace here (by id or name): its panes close, the folder and worktrees stay.
    pub(super) fn workspace_remove_here(&mut self, key: &str, cx: &mut Context<Self>) -> Result<String, String> {
        let me = crate::connect::get().map(|c| c.status().name).unwrap_or_else(|| "this machine".into());
        let s = self.state.read(cx);
        let Some((id, name, n)) = s.workspace_named(key).map(|w| (w.id.clone(), w.name.clone(), w.panes.iter().filter(|p| !p.archived).count())) else {
            let names = s.workspaces.iter().filter(|w| w.remote.is_none()).map(|w| w.name.clone()).collect::<Vec<_>>().join(", ");
            return Err(format!("No workspace “{key}” on {me}. There: {names}."));
        };
        self.close_all_of(&id, cx);
        self.close_side_terminal(&id, cx);
        self.update_state(cx, |s| s.remove_workspace(&id));
        Ok(format!("Removed workspace “{name}” from {me}{}; its folder stays.", if n == 0 { String::new() } else { format!(" and closed its {n} pane{}", if n == 1 { "" } else { "s" }) }))
    }

    fn remote_tool(&mut self, call: &str, peer: &str, name: &str, tool: &str, args: Value, caller: &str, cx: &mut Context<Self>) -> ToolOutcome {
        self.remote_tool_for(call, peer, name, tool, args, caller, TOOL_TIMEOUT, cx)
    }

    /// `remote_tool`, waiting up to `wait` (clones take longer).
    #[allow(clippy::too_many_arguments)]
    fn remote_tool_for(&mut self, call: &str, peer: &str, name: &str, tool: &str, args: Value, caller: &str, wait: Duration, cx: &mut Context<Self>) -> ToolOutcome {
        let id = self.connect_request(peer, json!({"t": "tool.call", "name": tool, "args": args, "caller": caller}), None);
        self.connect.tool_calls.insert(id.clone(), call.to_string());
        let machine = name.to_string();
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(wait).await;
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
    pub(super) fn connect_tool(&mut self, call: &str, tool: &str, a: &Value, who: &str, pane: &str, cx: &mut Context<Self>) -> Option<ToolOutcome> {
        let me = crate::connect::get().map(|c| c.status().name).unwrap_or_default();
        let caller = format!("{who} on {me}");
        match tool {
            "list_machines" => {
                let connect = crate::connect::get()?;
                let status = connect.status();
                let here = self.state.read(cx).workspaces.iter().filter(|w| w.remote.is_none()).map(|w| w.name.clone()).collect::<Vec<_>>().join(", ");
                let mut rows = vec![format!("You're on {} (desktop, this machine; workspaces: {here}). Its files and commands are yours directly; the machines below only through these tools:", status.name)];
                rows.extend(status.peers.iter().map(|p| {
                    let kind = if p.kind == Kind::Server { "server" } else { "desktop" };
                    let state = match p.link.via { Some(v) => format!("online ({v})"), None => "offline".into() };
                    let m = self.connect.mirrors.get(&p.peer_id);
                    let agents = m.map_or(0, |m| m.panes.iter().filter(|x| !x.archived).count());
                    let workspaces = m.map(|m| m.workspaces.iter().map(|w| w.1.clone()).collect::<Vec<_>>().join(", ")).filter(|w| !w.is_empty()).map_or(String::new(), |w| format!(" · workspaces: {w}"));
                    let server = m.and_then(|m| m.server.as_ref()).map_or(String::new(), |s| match &s.update {
                        Some(u) => format!(" · winferd {} (update to {u} available)", s.version),
                        None => format!(" · winferd {}", s.version),
                    });
                    format!("{} · {kind} · {state} · {agents} pane{}{workspaces}{server}", p.name, if agents == 1 { "" } else { "s" })
                }));
                if rows.len() == 1 {
                    rows.push("No paired machines (pair_machine pairs one).".into());
                }
                if !status.nearby.is_empty() {
                    rows.push(format!("Nearby, not paired: {}", status.nearby.iter().map(|n| n.name.clone()).collect::<Vec<_>>().join(", ")));
                }
                if let Some(p) = &status.pairing {
                    rows.push(format!("Pairing: {}", pairing_text(p)));
                }
                Some(ToolOutcome::Done(Ok(rows.join("\n"))))
            }
            "list_remote_panes" => {
                let (peer, name, up) = match self.machine(a["machine"].as_str().unwrap_or_default()) {
                    Ok(m) => m,
                    Err(e) => return Some(ToolOutcome::Done(Err(e))),
                };
                if up {
                    return Some(self.remote_tool(call, &peer, &name, "list_panes_all", json!({}), &caller, cx));
                }
                let m = self.connect.mirrors.get(&peer).cloned().unwrap_or_default();
                let rows: Vec<String> = self.remote_order(&peer).iter().enumerate().map(|(i, p)| {
                    let ws = m.workspaces.iter().find(|w| w.0 == p.workspace_id).map_or("", |w| w.1.as_str());
                    format!("{name}:{} · {} · {} · workspace {ws} · {}", i + 1, p.provider, super::agent_api::status_name(p.status), p.title)
                }).collect();
                let spaces = m.workspaces.iter().map(|w| w.1.clone()).collect::<Vec<_>>().join(", ");
                let head = format!("{name} ({}) · workspaces: {}:", if up { "online" } else { "offline, last known" }, if spaces.is_empty() { "none" } else { &spaces });
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
                let mut args = json!({"provider": a["provider"], "prompt": a["prompt"], "worktree": a["worktree"], "model": a["model"], "effort": a["effort"]});
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
            "remote_pane_action" => {
                let r = self.remote_ref(&a["pane"]).unwrap_or_else(|| Err("Give the pane as \"<machine>:<number>\" (list_remote_panes).".into()));
                let (peer, pane, name) = match r {
                    Ok(r) => r,
                    Err(e) => return Some(ToolOutcome::Done(Err(e))),
                };
                if !matches!(a["action"].as_str(), Some("approve" | "decline" | "interrupt" | "close")) {
                    return Some(ToolOutcome::Done(Err("action is approve, decline, interrupt or close.".into())));
                }
                Some(self.remote_tool(call, &peer, &name, "pane_action", json!({"pane": pane, "action": a["action"]}), &caller, cx))
            }
            "add_workspace" => {
                let call = call.to_string();
                self.workspace_add_here(a, cx, move |r| match r {
                    Ok(t) => super::agent_api::respond(&call, true, t),
                    Err(t) => super::agent_api::respond(&call, false, t),
                });
                Some(ToolOutcome::Later)
            }
            "remove_workspace" => {
                let key = a["workspace"].as_str().unwrap_or_default();
                let s = self.state.read(cx);
                let mine = s.pane(pane).map(|p| p.workspace_id.clone()).or_else(|| crate::voice::orchestrator_workspace(pane).map(str::to_string));
                if mine.is_some() && s.workspace_named(key).map(|w| w.id.clone()) == mine {
                    return Some(ToolOutcome::Done(Err("That's the workspace you're running in: it can't be removed from inside it.".into())));
                }
                Some(ToolOutcome::Done(self.workspace_remove_here(key, cx)))
            }
            "new_remote_workspace" => {
                let (peer, name) = match self.online_machine(a) {
                    Ok(m) => m,
                    Err(e) => return Some(ToolOutcome::Done(Err(e))),
                };
                let args = json!({"path": a["path"], "git_url": a["git_url"], "branch": a["branch"], "parent": a["parent"], "name": a["name"]});
                Some(self.remote_tool_for(call, &peer, &name, "add_workspace", args, &caller, Duration::from_secs(600), cx))
            }
            "remove_remote_workspace" => {
                let (peer, name) = match self.online_machine(a) {
                    Ok(m) => m,
                    Err(e) => return Some(ToolOutcome::Done(Err(e))),
                };
                Some(self.remote_tool(call, &peer, &name, "remove_workspace", json!({"workspace": a["workspace"]}), &caller, cx))
            }
            "list_reviews" => {
                let only = match a["machine"].as_str().filter(|m| !m.is_empty()) {
                    Some(m) => match self.machine(m) {
                        Ok((peer, _, _)) => Some(peer),
                        Err(e) => return Some(ToolOutcome::Done(Err(e))),
                    },
                    None => None,
                };
                Some(ToolOutcome::Done(Ok(self.reviews_text(only.as_deref()))))
            }
            "review_action" => {
                let wanted = a["review"].as_str().unwrap_or_default().trim();
                let found = self.review.reviews.iter().find(|r| r.remote.as_ref().is_some_and(|(_, rid)| rid == wanted) || r.id == wanted).and_then(|r| r.remote.clone());
                let Some((peer, rid)) = found else {
                    return Some(ToolOutcome::Done(Err(format!("No review “{wanted}” on a paired server. list_reviews shows them."))));
                };
                let action = a["action"].as_str().unwrap_or_default();
                let msg = match action {
                    "merge" => json!({"t": "review.merge", "tests": a["tests"] == true}),
                    "tests" => json!({"t": "review.tests"}),
                    "send" => json!({"t": "review.send", "text": a["text"].as_str().unwrap_or_default()}),
                    "discard" => json!({"t": "review.discard"}),
                    "dismiss" => json!({"t": "review.dismiss"}),
                    other => return Some(ToolOutcome::Done(Err(format!("Unknown action “{other}”: merge, tests, send, discard or dismiss.")))),
                };
                let mut msg = msg;
                msg["reviewId"] = json!(rid);
                let name = crate::connect::peer_name(&peer).unwrap_or_else(|| "the server".into());
                let what = match action {
                    "merge" => "review-merge",
                    "tests" => "review-tests",
                    _ => "review",
                };
                Some(self.await_reply(call, &peer, &name, msg, what, cx))
            }
            "machine_usage" => {
                let (peer, name, up) = match self.machine(a["machine"].as_str().unwrap_or_default()) {
                    Ok(m) => m,
                    Err(e) => return Some(ToolOutcome::Done(Err(e))),
                };
                if !up {
                    return Some(ToolOutcome::Done(Err(format!("{name} is offline."))));
                }
                Some(self.await_reply(call, &peer, &name, json!({"t": "usage.get"}), "usage", cx))
            }
            "update_server" => {
                let (peer, name) = match self.online_server(a) {
                    Ok(m) => m,
                    Err(e) => return Some(ToolOutcome::Done(Err(e))),
                };
                Some(self.await_reply(call, &peer, &name, json!({"t": "server.update"}), "update", cx))
            }
            "pair_machine" => Some(ToolOutcome::Done(self.pair_tool(a))),
            "list_sign_ins" => {
                let call = call.to_string();
                let names: HashMap<String, String> = crate::connect::get().map(|c| c.peers().into_iter().map(|p| (p.peer_id, p.name)).collect()).unwrap_or_default();
                crate::backend::background(move || {
                    let links = winfer_core::accounts_sync::links();
                    let rows: Vec<String> = serde_json::to_value(winfer_backend::accounts::accounts_list()).unwrap_or_default().as_array().cloned().unwrap_or_default().iter().map(|e| {
                        let field = |k: &str| e[k].as_str().or_else(|| e["account"][k].as_str()).unwrap_or_default().to_string();
                        let id = field("id");
                        let pushed: Vec<String> = links.iter().filter(|l| l.local == id && !l.received).filter_map(|l| names.get(&l.peer).map(|n| if l.out_there { format!("{n} (signed out there)") } else { n.clone() })).collect();
                        let state = if e["signedIn"] == true { "signed in" } else { "signed out" };
                        let on = if pushed.is_empty() { String::new() } else { format!(" · pushed to {}", pushed.join(", ")) };
                        format!("{id} · “{}” · {state}{on}", field("label"))
                    }).collect();
                    super::agent_api::respond(&call, true, if rows.is_empty() { "No agent accounts.".into() } else { rows.join("\n") });
                });
                Some(ToolOutcome::Later)
            }
            "push_sign_in" => {
                let (peer, name, up) = match self.machine(a["machine"].as_str().unwrap_or_default()) {
                    Ok(m) => m,
                    Err(e) => return Some(ToolOutcome::Done(Err(e))),
                };
                if !up {
                    return Some(ToolOutcome::Done(Err(format!("{name} is offline."))));
                }
                let account = a["account"].as_str().unwrap_or_default().to_string();
                match winfer_core::accounts_sync::push_message(&account) {
                    Ok((msg, fp)) => {
                        let id = self.connect_request(&peer, msg, None);
                        self.connect.pushes.insert(id.clone(), (account, fp, name.clone()));
                        Some(self.wait_for(call, id, &name, "push", cx))
                    }
                    Err(e) => Some(ToolOutcome::Done(Err(e))),
                }
            }
            _ => None,
        }
    }

    /// A paired server named in `a["machine"]`, online.
    fn online_machine(&self, a: &Value) -> Result<(String, String), String> {
        let (peer, name, up) = self.machine(a["machine"].as_str().unwrap_or_default())?;
        if !up {
            return Err(format!("{name} is offline."));
        }
        Ok((peer, name))
    }

    fn online_server(&self, a: &Value) -> Result<(String, String), String> {
        let (peer, name, up) = self.machine(a["machine"].as_str().unwrap_or_default())?;
        if !up {
            return Err(format!("{name} is offline."));
        }
        let server = crate::connect::get().and_then(|c| c.peers().into_iter().find(|p| p.peer_id == peer)).is_some_and(|p| p.kind == Kind::Server);
        if !server {
            return Err(format!("{name} is a desktop; this works on winfer servers."));
        }
        Ok((peer, name))
    }

    /// Sends `msg` to `peer` and answers the MCP call with its reply (or a timeout).
    fn await_reply(&mut self, call: &str, peer: &str, name: &str, msg: Value, what: &'static str, cx: &mut Context<Self>) -> ToolOutcome {
        let id = self.connect_request(peer, msg, None);
        self.wait_for(call, id, name, what, cx)
    }

    fn wait_for(&mut self, call: &str, id: String, name: &str, what: &'static str, cx: &mut Context<Self>) -> ToolOutcome {
        self.connect.tool_waits.insert(id.clone(), (call.to_string(), what, name.to_string()));
        // Clones and updates download: they get longer.
        let wait = if matches!(what, "workspace" | "update") { Duration::from_secs(600) } else { TOOL_TIMEOUT };
        let machine = name.to_string();
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(wait).await;
            let _ = this.update(cx, |this, _| {
                if let Some((call, _, _)) = this.connect.tool_waits.remove(&id) {
                    super::agent_api::respond(&call, false, format!("{machine} didn't answer in time."));
                }
            });
        })
        .detach();
        ToolOutcome::Later
    }

    /// A reply to a request an agent's tool call waits on.
    fn tool_reply(&mut self, msg: &Value) {
        let Some(id) = msg["id"].as_str() else { return };
        let Some((call, what, machine)) = self.connect.tool_waits.remove(id) else { return };
        let ok = msg["ok"] != false && msg["t"] != "error";
        let text = if !ok {
            msg["error"].as_str().unwrap_or("That didn't work.").to_string()
        } else {
            match what {
                "usage" => usage_text(&machine, msg),
                "workspace" => {
                    let ws = msg["workspaceId"].as_str().unwrap_or_default();
                    let name = msg["name"].as_str().map(str::to_string).or_else(|| self.connect.mirrors.values().flat_map(|m| m.workspaces.iter()).find(|w| w.0 == ws).map(|w| w.1.clone()));
                    match (name, msg["path"].as_str()) {
                        (Some(n), _) => format!("Workspace “{n}” is open on {machine}."),
                        (None, Some(p)) => format!("Cloned into {p} on {machine}; it's a workspace there now (list_remote_panes names it)."),
                        (None, None) => format!("The workspace is open on {machine} (list_remote_panes names it)."),
                    }
                }
                "update" => format!("{machine} is updating to {} and restarts; it's back in a few seconds.", msg["version"].as_str().unwrap_or("the newest release")),
                "review-merge" => format!("Queued for merging on {machine}; list_reviews shows the merge queue."),
                "review-tests" => format!("Running its tests on {machine}; list_reviews shows the result."),
                "push" => format!("Signed in on {machine} as “{}”. The two stay in step when either refreshes.", msg["label"].as_str().unwrap_or("the account")),
                _ => "Done.".to_string(),
            }
        };
        super::agent_api::respond(&call, ok, text);
    }

    /// Open reviews on paired machines (and their merge queues), for agents.
    fn reviews_text(&self, only: Option<&str>) -> String {
        use super::review::Load;
        let mut rows = Vec::new();
        for r in &self.review.reviews {
            let Some((peer, rid)) = &r.remote else { continue };
            if only.is_some_and(|o| o != peer) {
                continue;
            }
            let machine = crate::connect::peer_name(peer).unwrap_or_default();
            let changes = r.stats.as_ref().map_or(String::new(), |s| format!(" · {} file{} +{} −{}", s.files.len(), if s.files.len() == 1 { "" } else { "s" }, s.adds, s.dels));
            let tests = match &r.tests {
                Load::Done(Some(t)) => format!(" · tests {} ({})", if t.ok { "passed" } else { "failed" }, t.command),
                Load::Loading => " · tests running".into(),
                Load::Error(e) => format!(" · tests error: {e}"),
                _ => String::new(),
            };
            let summary = match &r.summary { Load::Done(t) => format!("\n  {}", t.replace('\n', " ")), _ => String::new() };
            let conflict = if r.conflict.is_some() { " · conflicts with its base" } else { "" };
            rows.push(format!("review {rid} · {machine} · {} · {}{changes}{tests}{conflict}{summary}", r.provider, r.branch.clone().unwrap_or_else(|| "no worktree".into())));
        }
        let queue: Vec<String> = self.review.queue.iter().filter(|q| q.peer.as_deref().is_some_and(|p| only.is_none_or(|o| o == p))).map(|q| {
            let machine = q.peer.as_deref().and_then(crate::connect::peer_name).unwrap_or_default();
            format!("{} → {} on {machine}: {:?}{}", q.branch, q.base, q.state, q.detail.as_ref().map_or(String::new(), |d| format!(" ({d})")))
        }).collect();
        if rows.is_empty() {
            rows.push("No open reviews on paired servers.".into());
        }
        if !queue.is_empty() {
            rows.push(format!("Merge queue:\n{}", queue.join("\n")));
        }
        rows.join("\n")
    }

    /// `pair_machine`.
    fn pair_tool(&mut self, a: &Value) -> Result<String, String> {
        let connect = crate::connect::get().ok_or("Winfer Connect isn't running.")?;
        if !connect.status().enabled {
            return Err("Winfer Connect is off (Settings → Connect).".into());
        }
        match a["mode"].as_str().unwrap_or_default() {
            "link" => {
                // The agent hands the link on itself: its key proves the other side, so no code to compare.
                let link = connect.pair_create_link_with(true);
                let named = |cmd: String| match a["name"].as_str().filter(|n| !n.trim().is_empty()) {
                    Some(n) => cmd.replace("--pair", &format!("--name '{}' --pair", n.replace('\'', ""))).replace("WINFERD_NAME=\"$(hostname)\"", &format!("WINFERD_NAME='{}'", n.replace('\'', ""))),
                    None => cmd,
                };
                Ok(format!(
                    "Pair link (one-time, 10 minutes):\n{link}\n\nTo add a server, run on it either\n  {}\nor with Docker\n  {}\nAnother winfer desktop joins with pair_machine mode \"join\" or Settings → Connect. list_machines shows it once paired.",
                    named(super::settings_connect::server_command(&link, false)),
                    named(super::settings_connect::server_command(&link, true)),
                ))
            }
            "join" => {
                let link = a["link"].as_str().unwrap_or_default();
                connect.pair_join_link(link, true)?;
                Ok("Joining; list_machines shows the machine once paired (a few seconds).".into())
            }
            "nearby" => {
                let wanted = a["machine"].as_str().unwrap_or_default();
                let nearby = connect.status().nearby;
                let n = nearby.iter().find(|n| n.name.eq_ignore_ascii_case(wanted.trim()) || n.id == wanted).ok_or_else(|| format!("No unpaired machine called “{wanted}” on this network. Nearby: {}.", if nearby.is_empty() { "none".into() } else { nearby.iter().map(|n| n.name.clone()).collect::<Vec<_>>().join(", ") }))?;
                connect.pair_nearby(&n.id)?;
                Ok(format!("Asked {} to pair. Both screens now show a code: the user compares and confirms them in Settings → Connect (agents can't confirm a nearby pairing).", n.name))
            }
            "address" => {
                connect.pair_address(a["address"].as_str().unwrap_or_default())?;
                Ok("Asked it to pair. Both screens show a code that the user compares and confirms in Settings → Connect.".into())
            }
            other => Err(format!("Unknown mode “{other}”: link, join, nearby or address.")),
        }
    }

    /// A paired machine's agent calls a tool here (full trust: as if it were one of ours).
    fn on_tool_call(&mut self, peer: &str, msg: &Value, window: &mut Window, cx: &mut Context<Self>) {
        let id = msg["id"].clone();
        let tool = msg["name"].as_str().unwrap_or_default().to_string();
        let mut a = msg["args"].clone();
        let caller = msg["caller"].as_str().unwrap_or("An agent on another machine").to_string();
        let answer = |ok: bool, text: String| crate::connect::send(peer, json!({"t": "tool.result", "id": id, "ok": ok, "text": text}));
        // A pane by number: this machine's order (all workspaces), as list_panes_all shows it.
        let n = a["pane"].as_u64().or_else(|| a["pane"].as_str().and_then(|t| t.trim().parse().ok()));
        if let Some(n) = n {
            let s = self.state.read(cx);
            let found = s.workspaces.iter().flat_map(super::mobile::ordered).filter(|p| !p.archived).nth((n as usize).wrapping_sub(1)).map(|p| p.id.clone());
            match found {
                Some(pane) => a["pane"] = json!(pane),
                None => return answer(false, format!("No pane {n} here. list_remote_panes shows them.")),
            }
        }
        if tool == "list_panes_all" {
            let s = self.state.read(cx);
            let me = crate::connect::get().map(|c| c.status().name).unwrap_or_default();
            let spaces = s.workspaces.iter().filter(|w| w.remote.is_none()).map(|w| w.name.clone()).collect::<Vec<_>>().join(", ");
            let rows: Vec<String> = s.workspaces.iter().flat_map(|w| super::mobile::ordered(w).into_iter().filter(|p| !p.archived).map(move |p| (w, p))).enumerate()
                .map(|(i, (w, p))| format!("{me}:{} · {} · {} · workspace {} · {}", i + 1, p.provider, super::agent_api::status_name(p.status), w.name, s.pane_label(p)))
                .collect();
            return answer(true, format!("{me} (online) · workspaces: {spaces}:\n{}", if rows.is_empty() { "(no panes)".into() } else { rows.join("\n") }));
        }
        if tool == "add_workspace" {
            let (peer, id) = (peer.to_string(), id.clone());
            self.workspace_add_here(&a, cx, move |r| {
                let (ok, text) = match r {
                    Ok(t) => (true, t),
                    Err(t) => (false, t),
                };
                crate::connect::send(&peer, json!({"t": "tool.result", "id": id, "ok": ok, "text": text}));
            });
            return;
        }
        if tool == "remove_workspace" {
            return match self.workspace_remove_here(a["workspace"].as_str().unwrap_or_default(), cx) {
                Ok(t) => answer(true, t),
                Err(t) => answer(false, t),
            };
        }
        if tool == "pane_action" {
            let kind = match a["action"].as_str().unwrap_or_default() {
                "approve" => "approve",
                "decline" => "decline",
                "interrupt" => "interrupt",
                "close" => "pane.close",
                other => return answer(false, format!("Unknown action “{other}”.")),
            };
            let request = json!({"device": crate::connect::device(peer), "msg": {"t": kind, "paneId": a["pane"], "id": format!("tool-{}", id.as_str().unwrap_or_default())}});
            self.on_mobile_request(&request, window, cx);
            return answer(true, "Done.".into());
        }
        let s = self.state.read(cx);
        // The workspace: the target pane's, the one named, or the first.
        let ws = match (&tool[..], a["pane"].as_str(), a["workspace"].as_str()) {
            (_, Some(pane), _) => s.workspaces.iter().find(|w| w.panes.iter().any(|p| p.id == pane)),
            ("spawn_agent", None, Some(name)) => s.workspaces.iter().find(|w| w.remote.is_none() && w.name.eq_ignore_ascii_case(name)),
            _ => s.workspaces.iter().find(|w| w.id == s.active_workspace_id).or(s.workspaces.first()),
        }
        .cloned();
        let result = match ws {
            None => Err("No such pane or workspace on this machine.".to_string()),
            Some(_) if !matches!(&tool[..], "read_pane" | "send_to_pane" | "spawn_agent") => Err(format!("{tool} can't be called from another machine.")),
            Some(ws) => {
                let me = crate::model::Pane { id: format!("remote:{peer}"), provider: "winfer".into(), cwd: ws.root_dir.clone(), workspace_id: ws.id.clone(), ..Default::default() };
                let all = |s: &crate::model::AppState| s.workspaces.iter().flat_map(super::mobile::ordered).filter(|p| !p.archived).map(|p| p.id.clone()).collect::<Vec<_>>();
                let before = all(self.state.read(cx));
                let result = self.run_remote_sync_tool(&tool, &ws, &me, &a, &caller, cx);
                // Its address in this machine's order, as the caller writes it (machine:N).
                if tool == "spawn_agent" && result.is_ok() {
                    let me_name = crate::connect::get().map(|c| c.status().name).unwrap_or_default();
                    let after = all(self.state.read(cx));
                    match after.iter().position(|id| !before.contains(id)) {
                        Some(i) => Ok(format!("Started {} as {me_name}:{}", a["provider"].as_str().unwrap_or("the agent"), i + 1)),
                        None => Ok(format!("Started it (on {me_name}; see list_remote_panes for its address)")),
                    }
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
        // Linked sign-ins that refreshed here go to the other machine (every 30 s, off this thread).
        if self.connect.ticks % 30 == 7 && !winfer_core::accounts_sync::links().is_empty() {
            crate::backend::background(|| {
                winfer_core::accounts_sync::tick(|peer, msg| {
                    let up = crate::connect::get().is_some_and(|c| c.is_up(peer));
                    if up {
                        crate::connect::send(peer, msg);
                    }
                    up
                })
            });
        }
        // Development aid: open "New workspace on ‹server›" (`WINFER_NATIVE_CONNECT_FOLDER=1`, or a
        // git URL to clone it into the home folder).
        #[cfg(debug_assertions)]
        if let Ok(what) = std::env::var("WINFER_NATIVE_CONNECT_FOLDER")
            && !self.connect.dev_folder
            && let Some(server) = crate::connect::get().and_then(|c| c.status().peers.into_iter().find(|p| p.kind == Kind::Server && p.link.via.is_some()))
            && self.connect.mirrors.get(&server.peer_id).is_some_and(|m| m.loaded)
        {
            self.connect.dev_folder = true;
            self.open_remote_folder(&server.peer_id, window, cx);
            if what.contains("://") || what.starts_with("git@") {
                self.folder_clone_dev(&what, cx);
            }
        }
        // Development aid: open a folder on the first server as a workspace and start an agent there
        // in a worktree (`WINFER_NATIVE_CONNECT_SPAWN=path|provider[@model]|prompt`).
        #[cfg(debug_assertions)]
        if let Ok(what) = std::env::var("WINFER_NATIVE_CONNECT_SPAWN")
            && self.connect.dev_spawn.is_none()
            && let Some(server) = crate::connect::get().and_then(|c| c.status().peers.into_iter().find(|p| p.kind == Kind::Server && p.link.via.is_some()))
            && self.connect.mirrors.get(&server.peer_id).is_some_and(|m| m.loaded && !m.providers.is_empty())
        {
            let path = what.split('|').next().unwrap_or("~").to_string();
            self.connect.dev_spawn = Some(self.connect_request(&server.peer_id, json!({"t": "workspace.new", "path": path}), Some("Dev spawn: workspace".into())));
        }
        // Development aid: the first server's browser, as its menu's "Show its browser" does
        // (`WINFER_NATIVE_CONNECT_BROWSER=1`).
        #[cfg(debug_assertions)]
        if std::env::var_os("WINFER_NATIVE_CONNECT_BROWSER").is_some()
            && !self.connect.dev_browser
            && let Some((peer, ws)) = self.connect.mirrors.iter().find_map(|(p, m)| m.loaded.then(|| (p.clone(), m.workspaces.first().map(|w| w.0.clone()))))
        {
            self.connect.dev_browser = true;
            if self.browser.update(cx, |b, cx| b.show_server(&peer, ws.as_deref(), cx)) {
                self.update_state(cx, |s| {
                    s.browser_open = true;
                    s.side_panel = Some(crate::model::SidePanel::Browser);
                });
            }
        }
        // Development aid: unpair the first server's first phone, as its Unpair button does
        // (`WINFER_NATIVE_CONNECT_UNPAIR=1`).
        #[cfg(debug_assertions)]
        if std::env::var_os("WINFER_NATIVE_CONNECT_UNPAIR").is_some()
            && !self.connect.dev_unpaired
            && let Some((peer, device)) = self.connect.mirrors.iter().find_map(|(p, m)| Some((p.clone(), m.server.as_ref()?.phones.first()?.0.clone())))
        {
            self.connect.dev_unpaired = true;
            self.connect_request(&peer, json!({"t": "mobile.revoke", "deviceId": device}), Some("Dev unpair".into()));
        }
        // Development aid: History with the first server's merges (`WINFER_NATIVE_CONNECT_HISTORY=1`).
        #[cfg(debug_assertions)]
        if std::env::var_os("WINFER_NATIVE_CONNECT_HISTORY").is_some()
            && !self.connect.dev_history
            && let Some(server) = crate::connect::get().and_then(|c| c.status().peers.into_iter().find(|p| p.kind == Kind::Server && p.link.via.is_some()))
            && self.connect.mirrors.get(&server.peer_id).is_some_and(|m| m.loaded)
        {
            self.connect.dev_history = true;
            self.update_state(cx, |s| s.overlay = crate::model::Overlay::History);
            self.connect.dev_history_of = Some(server.peer_id.clone());
        }
        // Development aid: History showing that server's merges, once History is open.
        #[cfg(debug_assertions)]
        if let Some(peer) = self.connect.dev_history_of.clone()
            && self.dev_show_history_of(&peer, cx)
        {
            self.connect.dev_history_of = None;
        }
        // Development aid: open Review once a server's review arrives; with `merge` queue it there
        // with tests, `changes` shows its diff, `history` that server's History
        // (`WINFER_NATIVE_CONNECT_REVIEW=open|merge|changes|history`).
        #[cfg(debug_assertions)]
        if let Ok(what) = std::env::var("WINFER_NATIVE_CONNECT_REVIEW")
            && !self.connect.dev_review
            && let Some(first) = self.review.reviews.iter().find(|r| r.remote.is_some() && matches!(r.summary, super::review::Load::Done(_) | super::review::Load::Off | super::review::Load::Error(_))).map(|r| r.id.clone())
        {
            self.connect.dev_review = true;
            self.review.selected = Some(first.clone());
            self.update_state(cx, |s| s.overlay = crate::model::Overlay::Review);
            if what == "merge" {
                self.review.queue_tests = true;
                self.dev_enqueue(first, cx);
            } else if what == "changes" {
                let id = format!("review:{first}");
                self.update_state(cx, |s| {
                    s.overlay = crate::model::Overlay::None;
                    s.diff_pane_id = Some(id);
                });
            } else if what == "history" {
                let peer = first.split('|').next().unwrap_or_default().to_string();
                self.update_state(cx, |s| s.overlay = crate::model::Overlay::History);
                self.connect.dev_history_of = Some(peer);
            }
        }
        // Development aid: Settings → Usage showing the first server's (`WINFER_NATIVE_CONNECT_USAGE=limits|spend`).
        #[cfg(debug_assertions)]
        if let Ok(tab) = std::env::var("WINFER_NATIVE_CONNECT_USAGE")
            && !self.connect.dev_usage
            && let Some(server) = crate::connect::get().and_then(|c| c.status().peers.into_iter().find(|p| p.kind == Kind::Server && p.link.via.is_some()))
            && self.connect.mirrors.get(&server.peer_id).is_some_and(|m| m.loaded)
        {
            self.connect.dev_usage = true;
            self.execute(Command::Settings(Some("usage".into())), window, cx);
            if let Some(s) = self.settings.as_mut() {
                s.page_tabs.insert("usage", if tab == "spend" { "spend" } else { "limits" });
            }
            self.dev_usage_tab(if tab == "spend" { "spend" } else { "limits" }, cx);
            self.show_usage_of(Some(server.peer_id.clone()), cx);
        }
        // Development aid: Settings → Mobile with a pairing from the first server (`WINFER_NATIVE_CONNECT_PHONE=1`).
        #[cfg(debug_assertions)]
        if std::env::var_os("WINFER_NATIVE_CONNECT_PHONE").is_some()
            && !self.connect.dev_phone
            && let Some(server) = crate::connect::get().and_then(|c| c.status().peers.into_iter().find(|p| p.kind == Kind::Server && p.link.via.is_some()))
            && self.connect.mirrors.get(&server.peer_id).is_some_and(|m| m.loaded)
        {
            self.connect.dev_phone = true;
            self.execute(Command::Settings(Some("mobile".into())), window, cx);
            self.dev_pair_phone_with(&server.peer_id, cx);
        }
        // Development aid: push this account's sign-in to the first server that's up
        // (`WINFER_NATIVE_CONNECT_PUSH=codex:default`).
        #[cfg(debug_assertions)]
        if let Ok(account) = std::env::var("WINFER_NATIVE_CONNECT_PUSH")
            && !self.connect.dev_pushed
            && let Some(server) = crate::connect::get().and_then(|c| c.status().peers.into_iter().find(|p| p.kind == Kind::Server && p.link.via.is_some()))
        {
            self.connect.dev_pushed = true;
            self.push_account(&account, &server.peer_id, cx);
        }
        // Development aid: open a server workspace's menu (`WINFER_NATIVE_CONNECT_MENU=<name>`), or
        // remove it without asking (`WINFER_NATIVE_CONNECT_REMOVE=<name>`).
        #[cfg(debug_assertions)]
        if !self.connect.dev_menu {
            for (var, remove) in [("WINFER_NATIVE_CONNECT_MENU", false), ("WINFER_NATIVE_CONNECT_REMOVE", true)] {
                let Ok(want) = std::env::var(var) else { continue };
                let found = self.connect.mirrors.iter().filter(|(_, m)| m.loaded).find_map(|(peer, m)| m.workspaces.iter().find(|w| w.1 == want).map(|w| (peer.clone(), w.0.clone())));
                if let Some((peer, ws)) = found {
                    self.connect.dev_menu = true;
                    if remove {
                        self.do_remove_remote_workspace(&peer, &ws, cx);
                    } else {
                        let items = self.remote_workspace_menu(&peer, &ws);
                        self.open_menu(point(px(150.0), px(380.0)), Some(want.into()), items, cx);
                    }
                }
            }
        }
        // Development aid: open the first remote pane as a tile once it's known.
        #[cfg(debug_assertions)]
        if std::env::var_os("WINFER_NATIVE_CONNECT_OPEN").is_some() && !self.connect.dev_opened {
            let want = std::env::var("WINFER_NATIVE_CONNECT_OPEN").unwrap_or_default();
            let first = self.connect.mirrors.iter().find_map(|(peer, m)| m.panes.iter().find(|p| want == "1" || p.provider == want || (want == "spawned" && self.connect.dev_spawned.as_ref() == Some(&p.id))).map(|p| (peer.clone(), p.id.clone())));
            if let Some((peer, pane)) = first {
                self.connect.dev_opened = true;
                self.open_remote_pane(&peer, &pane, window, cx);
                // …shown in the overview (`WINFER_NATIVE_CONNECT_OVERVIEW`)
                if std::env::var_os("WINFER_NATIVE_CONNECT_OVERVIEW").is_some() {
                    self.update_state(cx, |s| s.view = Some(crate::model::View::Overview));
                }
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
        // Development aid: approve what a remote tile asks, as its sidebar button does, once it has
        // asked for a few seconds (`WINFER_NATIVE_CONNECT_APPROVE=1`).
        #[cfg(debug_assertions)]
        if std::env::var_os("WINFER_NATIVE_CONNECT_APPROVE").is_some() {
            let asking = self.state.read(cx).all_panes().find(|p| p.remote.is_some() && p.status == AgentStatus::NeedsInput).map(|p| p.id.clone());
            match asking {
                Some(id) if self.connect.dev_asked.as_ref().is_some_and(|(p, at)| *p == id && at.elapsed() > Duration::from_secs(4)) => {
                    self.connect.dev_asked = None;
                    self.execute(Command::Approve(PaneRef::Id(id)), window, cx);
                }
                Some(id) if self.connect.dev_asked.as_ref().is_none_or(|(p, _)| *p != id) => self.connect.dev_asked = Some((id, Instant::now())),
                _ => {}
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
            let about = mirror.server.as_ref().map(|sv| format!("\nwinferd {}{}", sv.version, sv.health.as_ref().map_or(String::new(), |h| format!("\n{h}")))).unwrap_or_default();
            let tip = format!("{}\n{label}{about}\nClick to show or hide its sessions{}", peer.name, if up && mirror.loaded { ", right-click for more" } else { "" });
            let menu_ready = up && mirror.loaded;
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
                .on_mouse_down(MouseButton::Right, cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                    if menu_ready {
                        let items = this.remote_machine_menu(&add_peer);
                        this.open_menu(e.position, None, items, cx);
                    }
                }))
                .child(div().relative().size(px(22.0)).flex_shrink_0().flex().items_center().justify_center().rounded(px(6.0)).bg(c.hover)
                    .child(Icon::new(icon).size(px(12.0)).text_color(c.muted))
                    .child(div().absolute().right(px(-2.0)).bottom(px(-2.0)).size(px(7.0)).rounded_full().bg(if up { c.success } else { c.subtle })))
                .child(div().flex_1().min_w_0().truncate().child(peer.name.clone()))
                .when_some(mirror.server.clone().filter(|_| up).and_then(|sv| sv.update.clone().map(|u| (sv, u))), |d, (sv, update)| {
                    let (peer, name) = (peer.peer_id.clone(), peer.name.clone());
                    let tip = if sv.docker { format!("winferd {} → {update}\nClick to copy the docker pull command", sv.version) } else { format!("winferd {} → {update}\nClick to update and restart it", sv.version) };
                    d.child(div().id(SharedString::from(format!("peer-update-{}", peer))).px_1p5().rounded_sm().text_size(px(10.5)).text_color(c.accent).bg(alpha(c.accent, 0.12)).hover(move |d| d.bg(alpha(c.accent, 0.22)))
                        .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
                        .on_mouse_down(MouseButton::Left, cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            if sv.docker {
                                cx.write_to_clipboard(ClipboardItem::new_string("docker pull ghcr.io/rickydane/winferd".to_string()));
                                this.toast(format!("{name} runs in Docker: pull the new image (copied) and recreate the container"), cx);
                            } else {
                                this.connect_request(&peer, json!({"t": "server.update"}), Some(format!("Couldn't update {name}")));
                                this.toast(format!("Updating {name} to {update}…"), cx);
                            }
                        }))
                        .child("Update"))
                })
                .when(waiting > 0, |d| d.child(div().text_size(px(10.5)).text_color(c.warning).child(format!("{waiting} need{} you", if waiting == 1 { "s" } else { "" }))))
                .when(waiting == 0 && agents > 0, |d| d.child(div().text_size(px(10.5)).text_color(c.subtle).font_family(mono.clone()).child(format!("{agents} agent{}", if agents == 1 { "" } else { "s" }))))
                .child(Icon::new(if collapsed { IconName::ChevronRight } else { IconName::ChevronDown }).size(px(13.0)).text_color(c.subtle));
            let mut group = div().flex().flex_col().child(header);
            if !collapsed {
                let mut rows = div().flex().flex_col().gap_0p5().pl_2();
                if !up && live.is_empty() {
                    rows = rows.child(div().px_2().py_1().text_size(px(11.5)).text_color(c.subtle).child(label.clone()));
                } else if live.is_empty() && mirror.loaded && mirror.workspaces.len() <= 1 {
                    rows = rows.child(div().px_2().py_1().text_size(px(11.5)).text_color(c.subtle).child("No sessions"));
                }
                // Finished work on that machine waiting to be merged: opens Review at its first.
                let to_review: Vec<String> = self.review.reviews.iter().filter(|r| r.status == super::review::ReviewStatus::Open && r.remote.as_ref().is_some_and(|(p, _)| p == &peer.peer_id)).map(|r| r.id.clone()).collect();
                if let Some(first) = to_review.first().cloned() {
                    let n = to_review.len();
                    rows = rows.child(div().id(SharedString::from(format!("peer-review-{}", peer.peer_id))).flex().items_center().gap_2().px_2().py_1().rounded_md().cursor_pointer().text_size(px(12.0)).text_color(c.muted)
                        .hover(move |d| d.bg(c.hover).text_color(c.fg))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.review.selected = Some(first.clone());
                            this.update_state(cx, |s| s.overlay = crate::model::Overlay::Review);
                        }))
                        .child(Icon::new(IconName::Inbox).size(px(12.0)).text_color(c.accent))
                        .child(div().flex_1().child(if n == 1 { "1 task to review".to_string() } else { format!("{n} tasks to review") })));
                }
                let many_workspaces = mirror.workspaces.len() > 1;
                for (ws_id, ws_name) in &mirror.workspaces {
                    let panes: Vec<&&RemotePane> = live.iter().filter(|p| &p.workspace_id == ws_id).collect();
                    // Each workspace folds on its own (click its name).
                    let fold_key = format!("{}/{ws_id}", peer.peer_id);
                    let folded = many_workspaces && self.connect.collapsed.contains(&fold_key);
                    if many_workspaces {
                        let (menu_peer, menu_ws, title) = (peer.peer_id.clone(), ws_id.clone(), ws_name.clone());
                        let tip = if up { "Click to show or hide its sessions, right-click for a new session, its browser or removing it" } else { "Click to show or hide its sessions" };
                        rows = rows.child(div().id(SharedString::from(format!("peer-ws-{}-{ws_id}", peer.peer_id))).flex().items_center().gap_1().px_2().pt_1().pb_0p5().rounded_md().cursor_pointer()
                            .text_size(px(10.5)).font_weight(FontWeight::SEMIBOLD).text_color(c.subtle)
                            .hover(move |d| d.text_color(c.fg))
                            .tooltip(move |window, cx| Tooltip::new(tip).build(window, cx))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if !this.connect.collapsed.remove(&fold_key) {
                                    this.connect.collapsed.insert(fold_key.clone());
                                }
                                cx.notify();
                            }))
                            .when(up, |d| d.on_mouse_down(MouseButton::Right, cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                                let items = this.remote_workspace_menu(&menu_peer, &menu_ws);
                                this.open_menu(e.position, Some(title.clone().into()), items, cx);
                            })))
                            .child(Icon::new(if folded { IconName::ChevronRight } else { IconName::ChevronDown }).size(px(10.0)))
                            .child(div().flex_1().min_w_0().truncate().child(ws_name.clone()))
                            .when(folded && !panes.is_empty(), |d| d.child(div().font_weight(FontWeight::NORMAL).child(panes.len().to_string()))));
                    }
                    if folded {
                        continue;
                    }
                    // A workspace with no sessions yet (just opened or cloned): start one there.
                    if panes.is_empty() && many_workspaces && up {
                        let (target, ws) = (peer.peer_id.clone(), ws_id.clone());
                        rows = rows.child(div().id(SharedString::from(format!("peer-ws-new-{}-{ws_id}", peer.peer_id))).flex().items_center().gap_2().px_2().py_1().rounded_md().cursor_pointer().text_size(px(12.0)).text_color(c.subtle)
                            .hover(move |d| d.bg(c.hover).text_color(c.fg))
                            .on_click(cx.listener(move |this, _, _, cx| this.remote_new_pane(&target, Some(ws.clone()), None, cx)))
                            .child(Icon::new(IconName::Plus).size(px(12.0)))
                            .child("New session"));
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
        // Its tile is the focused one, in its workspace or the overview (like a local pane's row).
        let s = self.state.read(cx);
        let selected = s.focused_pane_id.as_deref().and_then(|id| s.pane(id)).is_some_and(|t| t.remote.as_ref().is_some_and(|r| r.peer == peer && r.pane == pane.id));
        div().id(SharedString::from(format!("rrow-{peer}-{}", pane.id))).flex().items_center().gap(px(9.0)).rounded(px(7.0)).px_1p5().py(px(5.0))
            .text_size(px(12.5)).text_color(c.muted).when(!up, |d| d.opacity(0.6))
            .hover(move |d| d.bg(c.hover).text_color(c.fg))
            .when(selected, |d| d.bg(c.active).text_color(c.fg))
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

    /// A machine's menu (right-click on it): with one workspace, that workspace's menu; then its
    /// browser and a new workspace.
    fn remote_machine_menu(&self, peer: &str) -> Vec<MenuItem> {
        let Some(mirror) = self.connect.mirrors.get(peer) else { return Vec::new() };
        let mut items = Vec::new();
        if let [(ws, _)] = mirror.workspaces.as_slice() {
            items = self.remote_workspace_menu(peer, ws);
            items.push(MenuItem::Separator);
        } else {
            let (p, first) = (peer.to_string(), mirror.workspaces.first().map(|w| w.0.clone()));
            items.push(MenuItem::run("Show its browser", move |this: &mut Root, _: &mut Window, cx: &mut Context<Root>| this.show_remote_browser(&p, first.as_deref(), cx)));
        }
        let p = peer.to_string();
        items.push(MenuItem::run("New workspace (folder or git clone)…", move |this: &mut Root, window: &mut Window, cx: &mut Context<Root>| this.open_remote_folder(&p, window, cx)));
        items
    }

    /// A workspace's menu on a paired machine (right-click on it): a session with each agent
    /// installed there, its browser, and removing it.
    fn remote_workspace_menu(&self, peer: &str, ws: &str) -> Vec<MenuItem> {
        let Some(mirror) = self.connect.mirrors.get(peer) else { return Vec::new() };
        let mut items = Vec::new();
        for (id, name) in &mirror.providers {
            let (peer, ws, provider) = (peer.to_string(), ws.to_string(), id.clone());
            let label = if mirror.default_provider.as_deref() == Some(id.as_str()) { format!("{name} (default)") } else { name.clone() };
            items.push(MenuItem::run(label, move |this: &mut Root, _: &mut Window, cx: &mut Context<Root>| this.remote_new_pane(&peer, Some(ws.clone()), Some(provider.clone()), cx)));
        }
        items.push(MenuItem::Separator);
        let (p, w) = (peer.to_string(), ws.to_string());
        items.push(MenuItem::run("Show its browser", move |this: &mut Root, _: &mut Window, cx: &mut Context<Root>| this.show_remote_browser(&p, Some(&w), cx)));
        let (p, w) = (peer.to_string(), ws.to_string());
        items.push(MenuItem::run("Remove workspace…", move |this: &mut Root, window: &mut Window, cx: &mut Context<Root>| this.remove_remote_workspace(&p, &w, window, cx)).danger());
        items
    }

    fn show_remote_browser(&mut self, peer: &str, ws: Option<&str>, cx: &mut Context<Self>) {
        let shown = self.browser.update(cx, |b, cx| b.show_server(peer, ws, cx));
        if shown {
            self.update_state(cx, |s| {
                s.browser_open = true;
                s.side_panel = Some(crate::model::SidePanel::Browser);
            });
        } else {
            self.toast("That machine has no workspace yet.", cx);
        }
    }

    /// Asks, then removes a workspace on a paired machine (`workspace.remove`): its sessions
    /// there close; the folder stays.
    fn remove_remote_workspace(&mut self, peer: &str, ws: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(mirror) = self.connect.mirrors.get(peer) else { return };
        let name = mirror.workspaces.iter().find(|w| w.0 == ws).map_or_else(String::new, |w| w.1.clone());
        let machine = crate::connect::peer_name(peer).unwrap_or_else(|| "that machine".into());
        let panes = match mirror.panes.iter().filter(|p| p.workspace_id == ws && !p.archived).count() {
            0 => String::new(),
            1 => " Its 1 session there will be closed.".to_string(),
            n => format!(" Its {n} sessions there will be closed."),
        };
        let detail = format!("Remove workspace “{name}” from {machine}?{panes} Files and worktrees stay on disk.");
        let (peer, ws) = (peer.to_string(), ws.to_string());
        self.confirm("Remove workspace", detail, "Remove", window, cx, move |this, cx| this.do_remove_remote_workspace(&peer, &ws, cx));
    }

    fn do_remove_remote_workspace(&mut self, peer: &str, ws: &str, cx: &mut Context<Self>) {
        let Some(mirror) = self.connect.mirrors.get(peer) else { return };
        let name = mirror.workspaces.iter().find(|w| w.0 == ws).map_or_else(String::new, |w| w.1.clone());
        let machine = crate::connect::peer_name(peer).unwrap_or_else(|| "that machine".into());
        let (peer, ws) = (peer.to_string(), ws.to_string());
        {
            let this = self;
            let id = this.connect_request(&peer, json!({"t": "workspace.remove", "workspaceId": ws}), Some(format!("Couldn't remove “{name}” from {machine}")));
            this.connect.removing.insert(id.clone(), format!("Removed “{name}” from {machine}"));
            // An older winfer there doesn't know the request and never answers.
            let (failed, id) = (format!("{machine} didn't answer: it may need a winfer update to remove workspaces."), id);
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(Duration::from_secs(15)).await;
                let _ = this.update(cx, |this, cx| {
                    if this.connect.removing.remove(&id).is_some() {
                        this.connect.pending.remove(&id);
                        this.toast(failed, cx);
                    }
                });
            })
            .detach();
            // Its tiles here go with it.
            let local = this.state.read(cx).workspaces.iter().find(|w| w.remote.as_ref().is_some_and(|r| r.peer == peer && r.workspace == ws)).map(|w| w.id.clone());
            if let Some(local) = local {
                this.close_all_of(&local, cx);
                this.update_state(cx, |s| s.remove_workspace(&local));
            }
        }
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

/// add_workspace's and new_remote_workspace's arguments.
fn workspace_props(machine: Option<&Value>) -> Value {
    let mut props = json!({
        "path": {"type": "string", "description": "An existing folder, e.g. ~/code/app"},
        "git_url": {"type": "string", "description": "Repository to clone (into ~ unless parent is given)"},
        "branch": {"type": "string"}, "parent": {"type": "string", "description": "Folder to clone into (default ~)"},
        "name": {"type": "string", "description": "Workspace name (default: the folder's)"},
    });
    if let Some(m) = machine {
        props["machine"] = m.clone();
    }
    props
}

/// A pairing in progress, for agents.
fn pairing_text(p: &winfer_connect::PairStatus) -> String {
    use winfer_connect::PairStatus;
    match p {
        PairStatus::Waiting { side: "host", .. } => "waiting for the other machine to use the pair link".into(),
        PairStatus::Waiting { .. } => "joining".into(),
        PairStatus::Code { code, peer_name, confirmed, .. } => format!("with {peer_name}, code {code}{}", if *confirmed { " (confirmed here, waiting for the other side)" } else { ": waiting for the user to confirm in Settings → Connect" }),
        PairStatus::Done { peer_name } => format!("paired with {peer_name}"),
        PairStatus::Failed { error } => format!("failed: {error}"),
    }
}

/// A machine's `usage` reply, for agents: each agent and account, what's left of each window.
fn usage_text(machine: &str, msg: &Value) -> String {
    let now = msg["now"].as_f64().unwrap_or(0.0);
    let window = |w: &Value| {
        let left = w["left"].as_f64().map_or("?".to_string(), |l| format!("{l:.0}% left"));
        let reset = w["resetsAt"].as_f64().filter(|r| *r > now).map_or(String::new(), |r| {
            let mins = ((r - now) / 60_000.0).round() as i64;
            if mins >= 120 { format!(", resets in {} h", mins / 60) } else { format!(", resets in {mins} min") }
        });
        format!("{} {left}{reset}", w["label"].as_str().unwrap_or("window"))
    };
    let mut rows = vec![format!("Usage on {machine}:")];
    for p in msg["providers"].as_array().into_iter().flatten() {
        let name = p["name"].as_str().unwrap_or_default();
        for acc in p["accounts"].as_array().into_iter().flatten() {
            let windows: Vec<String> = acc["windows"].as_array().into_iter().flatten().map(window).collect();
            if !windows.is_empty() {
                rows.push(format!("{name} · {}: {}", acc["label"].as_str().unwrap_or("account"), windows.join("; ")));
            }
        }
    }
    if rows.len() == 1 {
        rows.push("No usage reported yet.".into());
    }
    rows.join("\n")
}
