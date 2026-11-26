//! The server's side of Winfer Connect: winfer-core's state, sessions and status with no window,
//! answering paired machines the way the desktop's mobile bridge and Connect host do
//! (docs/connect/protocol.md). A desktop shows these panes in its sidebar and opens them as tiles.

use parking_lot::Mutex;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};
use std::sync::atomic::Ordering;
use tauri::Manager;
use winfer_backend::{mcp, providers, pty, store};
use winfer_core::code::protocol::Cmd;
use winfer_core::layout::leaves;
use winfer_core::model::{AddPane, AgentProvider, AgentStatus, AppState, Pane, Workspace, clean_agent_title};
use winfer_core::terminal::{self, CodeHooks, Event, SESSIONS, SpawnSettings};
use winfer_core::{backend, connect, connect_term, status};

const MAX_REPLY: usize = 4000;

pub struct Host {
    state: Mutex<AppState>,
    events: async_channel::Sender<Event>,
    /// What the paired machines last got for each pane, to push only changes.
    sent: Mutex<HashMap<String, Value>>,
    /// Panes and workspaces as last pushed (a change sends the whole mirror).
    shape: Mutex<String>,
    last_saved: Mutex<String>,
    /// A chat pane's first message, sent once its process is up.
    first_messages: Mutex<HashMap<String, String>>,
    ticks: std::sync::atomic::AtomicU64,
}

static HOST: OnceLock<Arc<Host>> = OnceLock::new();

/// This server's agents' calls waiting on another machine: request id → MCP call id.
static TOOL_WAITS: std::sync::LazyLock<Mutex<HashMap<String, String>>> = std::sync::LazyLock::new(Default::default);

impl Host {
    /// The host, once started.
    pub fn get() -> &'static Host {
        HOST.get().expect("host started")
    }

    /// Loads the saved state and starts handling events. Panes start when a machine opens them.
    pub fn start() -> Arc<Host> {
        let (tx, rx) = async_channel::unbounded();
        // First of all: paired machines ask as soon as their link is up (handled once loaded).
        let forward = tx.clone();
        backend::app().listen_any(move |event, payload| {
            if matches!(event, "pty://exit" | "agent://status" | "connect://message" | "connect://peer" | "mcp://call" | "usage://report" | "mobile://request") {
                let _ = forward.try_send(Event::Backend(event.to_string(), payload.clone()));
            }
        });
        let mut state = AppState::new();
        let snapshot = store::state_load(backend::app().state()).ok().and_then(|s| serde_json::to_value(s).ok()).unwrap_or(Value::Null);
        state.hydrate(snapshot);
        let host = Arc::new(Host {
            state: Mutex::new(state),
            events: tx,
            sent: Mutex::default(),
            shape: Mutex::default(),
            last_saved: Mutex::default(),
            first_messages: Mutex::default(),
            ticks: Default::default(),
        });
        let _ = HOST.set(host.clone());
        let h = host.clone();
        std::thread::Builder::new()
            .name("winferd-events".into())
            .spawn(move || {
                while let Ok(event) = rx.recv_blocking() {
                    h.on_event(event);
                }
            })
            .expect("event thread");
        // Which agents are installed takes a few seconds (it runs each CLI); again every few
        // minutes, for agents installed since (in a shell tile, say).
        let h = host.clone();
        std::thread::spawn(move || loop {
            let found = tauri::async_runtime::block_on(providers::providers_list());
            h.state.lock().set_providers(found.into_iter().map(|p| AgentProvider { info: p.provider, installed: p.installed }).collect());
            std::thread::sleep(std::time::Duration::from_secs(120));
        });
        host
    }

    // ---- events ----

    fn on_event(&self, event: Event) {
        match event {
            Event::Title(id, raw) => {
                let mut s = self.state.lock();
                let Some(pane) = s.pane(&id) else { return };
                let provider = s.providers.iter().find(|p| p.id == pane.provider);
                let name = provider.map_or(String::new(), |p| p.name.clone());
                let command = provider.and_then(|p| p.command.clone()).unwrap_or_default();
                if let Some(title) = clean_agent_title(&raw, &[&name, &command, &pane.provider])
                    && let Some(pane) = s.pane_mut(&id)
                {
                    pane.agent_title = Some(title);
                }
            }
            Event::Notice(id, message) => status::notice(&mut *self.state.lock(), &id, message),
            Event::Output(id) => status::output(&mut *self.state.lock(), &id),
            // A dev server an agent started: its review gets a screenshot of it.
            Event::DevUrl(id, url) => {
                if std::env::var_os("WINFER_CONNECT_TRACE").is_some() {
                    eprintln!("[review] {id} serves {url}");
                }
                if let Some(pane) = self.state.lock().pane_mut(&id) {
                    pane.dev_url = Some(url);
                }
            }
            Event::FirstOutput(id) => {
                self.state.lock().starting.remove(&id);
                if let Some(text) = self.first_messages.lock().remove(&id) {
                    send_chat_text(&id, &text);
                }
            }
            Event::StartingProcess(id) => {
                self.state.lock().starting.insert(id, "process");
            }
            Event::Worktree { pane_id, info } => {
                let mut s = self.state.lock();
                if let Some(p) = s.pane_mut(&pane_id) {
                    p.wants_worktree = false;
                    if let Some(info) = info {
                        p.cwd = info.path.clone();
                        p.worktree = Some(info);
                    }
                }
            }
            Event::Spawned { pane_id, busy, task } => {
                let mut s = self.state.lock();
                if s.pane(&pane_id).is_some_and(|p| p.status == AgentStatus::Idle) {
                    s.set_pane_status(&pane_id, if busy { AgentStatus::Working } else { AgentStatus::Idle }, false);
                }
                if let Some(task) = task {
                    status::set_task(&mut *s, &pane_id, &task);
                }
            }
            Event::SpawnFailed(id) => {
                let mut s = self.state.lock();
                s.starting.remove(&id);
                s.set_pane_status(&id, AgentStatus::Error, false);
            }
            Event::Backend(name, payload) => self.on_backend(&name, &payload),
            _ => {}
        }
    }

    fn on_backend(&self, name: &str, payload: &Value) {
        match name {
            "agent://status" => {
                if status::hook(&mut *self.state.lock(), payload) {
                    crate::review::finished(payload["paneId"].as_str().unwrap_or_default());
                }
            }
            "pty://exit" => {
                let id = payload["paneId"].as_str().unwrap_or_default().to_string();
                let code = payload["code"].as_u64().unwrap_or(0) as u32;
                let pid = payload["pid"].as_u64().map(|p| p as u32);
                connect_term::exited(&id, code);
                if terminal::on_exit(&id, code, pid) {
                    let next = if code == 0 { AgentStatus::Done } else { AgentStatus::Error };
                    if status::report(&mut *self.state.lock(), &id, next, Some(format!("exited with code {code}"))) {
                        crate::review::finished(&id);
                    }
                }
            }
            "connect://message" => {
                let peer = payload["peer"].as_str().unwrap_or_default().to_string();
                self.on_message(&peer, &payload["msg"]);
            }
            "mcp://call" => self.on_mcp_call(payload),
            "connect://peer" if payload["up"] == false => crate::browser::drop_client(payload["peer"].as_str().unwrap_or_default()),
            // A phone paired with this server asks what a paired machine would.
            "mobile://request" => {
                let device = payload["device"].as_str().unwrap_or_default();
                self.on_message(&format!("phone:{device}"), &payload["msg"]);
            }
            "usage://report" => crate::limits::report(self, payload),
            _ => {}
        }
    }

    /// Once a second: quiet agents go idle, changes go out, the state is saved.
    pub fn tick(&self) {
        let n = self.ticks.fetch_add(1, Ordering::Relaxed);
        if n % 30 == 7 {
            // Linked sign-ins that refreshed here go to the machine they came from.
            winfer_core::accounts_sync::tick(|peer, msg| {
                let up = connect::get().is_some_and(|c| c.is_up(peer));
                if up {
                    connect::send(peer, msg);
                }
                up
            });
        }
        let finished = status::tick(&mut *self.state.lock());
        for id in &finished {
            crate::review::finished(id);
        }
        self.push_changes();
        if let Some(host) = HOST.get() {
            crate::review::tick(host);
        }
        self.save();
    }

    /// Installs `tag` (default: the newest release) over this binary and saves the state; the caller
    /// restarts. Returns the version installed.
    pub fn update(&self, tag: Option<&str>) -> Result<String, String> {
        let tag = match tag {
            Some(t) => t.to_string(),
            None => crate::update::available().map(Ok).unwrap_or_else(|| {
                let latest = crate::update::check()?;
                crate::update::available().ok_or(format!("Already up to date ({latest})."))
            })?,
        };
        let exe = crate::update::exe()?;
        eprintln!("updating to {tag}…");
        let version = crate::update::install(&tag, &exe)?;
        self.save();
        eprintln!("updated to {version}, restarting");
        Ok(version)
    }

    fn save(&self) {
        let snapshot = self.state.lock().snapshot();
        let text = snapshot.to_string();
        let mut last = self.last_saved.lock();
        if *last == text {
            return;
        }
        *last = text;
        if let Ok(snapshot) = serde_json::from_value(snapshot) {
            let _ = store::state_save(backend::app().state(), snapshot);
        }
    }

    // ---- the mirror ----

    fn pane_json(s: &AppState, pane: &Pane) -> Value {
        let mut status_text = status_name(pane.status).to_string();
        let mut message = s.status_messages.get(&pane.id).cloned();
        let request = (pane.status == AgentStatus::NeedsInput).then(|| {
            // A chat says exactly what it waits on.
            if let Some(r) = crate::chats::request(&pane.id) {
                return r;
            }
            let screen = SESSIONS.get(&pane.id).map(|x| x.read_text(40)).unwrap_or_default();
            winfer_core::request::describe_request(&screen, message.as_deref())
        });
        if pane.status == AgentStatus::NeedsInput && message.is_none() {
            message = request.as_ref().and_then(|r| r.text.lines().find(|l| !l.is_empty()).map(str::to_string));
        }
        if s.starting.contains_key(&pane.id) && pane.status == AgentStatus::Idle {
            status_text = "idle".into();
        }
        let reply = if pane.status == AgentStatus::Done { s.replies.get(&pane.id).map(|t| cut(t, MAX_REPLY)) } else { None };
        json!({
            "id": pane.id, "title": s.pane_label(pane),
            "workspace": s.workspaces.iter().find(|w| w.id == pane.workspace_id).map_or("", |w| w.name.as_str()),
            "workspaceId": pane.workspace_id, "archived": pane.archived,
            "provider": if pane.provider == "shell" { None } else { Some(pane.provider.clone()) },
            "status": status_text, "message": message, "request": request, "reply": reply,
            "config": crate::chats::config(&pane.id).map(|(c, _)| c),
            // Its own worktree's branch (none: it works in the project folder).
            "branch": pane.worktree.as_ref().map(|w| w.branch.clone()),
        })
    }

    fn mirror(&self) -> Value {
        let s = self.state.lock();
        let panes: Vec<Value> = s.workspaces.iter().flat_map(ordered).map(|p| Self::pane_json(&s, p)).collect();
        let workspaces: Vec<Value> = s.workspaces.iter().map(|w| json!({ "id": w.id, "name": w.name, "active": w.id == s.active_workspace_id })).collect();
        let providers: Vec<Value> = s.providers.iter().filter(|p| p.installed && p.id != "shell").map(|p| json!({ "id": p.id, "name": p.name })).collect();
        let server = json!({
            "version": crate::update::VERSION, "latest": crate::update::latest_known(), "update": crate::update::available(),
            "docker": crate::update::in_docker(), "os": std::env::consts::OS, "arch": std::env::consts::ARCH,
            "stats": crate::machine::stats(), "phones": crate::phone::paired(),
        });
        json!({ "panes": panes, "workspaces": workspaces, "providers": providers, "defaultProvider": s.default_provider_id(), "server": server })
    }

    fn shape_of(s: &AppState) -> String {
        let ws: Vec<String> = s.workspaces.iter().map(|w| format!("{}={}:{}", w.id, w.name, ordered(w).iter().map(|p| p.id.as_str()).collect::<Vec<_>>().join(","))).collect();
        let agents: Vec<&str> = s.providers.iter().filter(|p| p.installed).map(|p| p.id.as_str()).collect();
        let phones: Vec<String> = crate::phone::paired().iter().map(|p| format!("{}{}", p["deviceId"], p["push"])).collect();
        format!("{}|{}|{:?}|{}", ws.join(";"), agents.join(","), crate::update::available(), phones.join(","))
    }

    /// Sends what changed since the last push: the whole mirror when panes came, went or moved,
    /// else a `status` per changed pane (silent unless its status, message or question changed).
    fn push_changes(&self) {
        let (shape, panes) = {
            let s = self.state.lock();
            (Self::shape_of(&s), s.workspaces.iter().flat_map(ordered).map(|p| (p.id.clone(), Self::pane_json(&s, p))).collect::<Vec<_>>())
        };
        let mut sent = self.sent.lock();
        let reshaped = *self.shape.lock() != shape;
        if reshaped {
            *self.shape.lock() = shape;
            let mut all = self.mirror();
            all["t"] = json!("panes");
            broadcast(all);
        }
        for (id, pane) in &panes {
            let before = sent.get(id);
            if before == Some(pane) {
                continue;
            }
            if !reshaped {
                let loud = before.is_none_or(|b| b["status"] != pane["status"] || b["message"] != pane["message"] || b["request"] != pane["request"]);
                broadcast(json!({"t": "status", "pane": pane, "silent": !loud}));
            }
            sent.insert(id.clone(), pane.clone());
        }
        sent.retain(|id, _| panes.iter().any(|(p, _)| p == id));
    }

    // ---- requests from paired machines ----

    fn on_message(&self, peer: &str, msg: &Value) {
        let kind = msg["t"].as_str().unwrap_or_default();
        let reply = |mut v: Value| {
            if !msg["id"].is_null() {
                v["id"] = msg["id"].clone();
            }
            send_to(peer, v);
        };
        let ok = || json!({"t": "ack", "ok": true});
        let no = |e: &str| json!({"t": "ack", "ok": false, "error": e});
        match kind {
            "panes.get" => {
                if !peer.starts_with("phone:") {
                    connect::subscribe(peer);
                }
                let mut all = self.mirror();
                all["t"] = json!("panes");
                reply(all);
                if !peer.starts_with("phone:") {
                    connect::send(peer, crate::review::message());
                }
            }
            kind if kind.starts_with("review.") => {
                let Some(host) = HOST.get() else { return };
                // Discards and tests touch git: off the event thread.
                let (msg, peer) = (msg.clone(), peer.to_string());
                std::thread::spawn(move || {
                    let mut answer = crate::review::on_request(host, &msg);
                    answer["id"] = msg["id"].clone();
                    send_to(&peer, answer);
                });
            }
            "term.attach" | "term.in" | "term.resize" | "term.detach" => self.on_term(peer, msg),
            "browser.call" if !peer.starts_with("phone:") => {
                if let Some(host) = HOST.get() {
                    crate::browser::on_request(host, peer, msg);
                }
            }
            "tool.call" if msg["name"] == "add_workspace" => {
                // A clone takes a while: answered from its own thread.
                let (peer, msg) = (peer.to_string(), msg.clone());
                std::thread::spawn(move || {
                    let Some(host) = HOST.get() else { return };
                    let (ok, text) = match host.workspace_add(&msg["args"]) {
                        Ok(t) => (true, t),
                        Err(t) => (false, t),
                    };
                    send_to(&peer, json!({"t": "tool.result", "id": msg["id"], "ok": ok, "text": text}));
                });
            }
            "tool.call" => {
                let result = self.remote_tool(peer, msg["name"].as_str().unwrap_or_default(), &msg["args"]);
                let (ok, text) = match result {
                    Ok(t) => (true, t),
                    Err(t) => (false, t),
                };
                send_to(peer, json!({"t": "tool.result", "id": msg["id"], "ok": ok, "text": text}));
            }
            "tool.result" => {
                // A call this server's agent made on another machine.
                if let Some(call) = TOOL_WAITS.lock().remove(msg["id"].as_str().unwrap_or_default()) {
                    mcp_reply(&call, msg["ok"] == true, msg["text"].as_str().unwrap_or_default());
                }
            }
            "pane.new" => reply(self.new_pane(msg, peer)),
            "fs.list" => reply(crate::machine::list(msg["path"].as_str().unwrap_or("~"))),
            "workspace.new" => {
                let path = crate::machine::expand(msg["path"].as_str().unwrap_or("~"));
                if !path.is_dir() {
                    return reply(no("That folder doesn't exist on this server."));
                }
                let id = self.add_workspace(&path, msg["name"].as_str());
                let name = self.with_state(|s| s.workspaces.iter().find(|w| w.id == id).map(|w| w.name.clone()));
                reply(json!({"t": "ack", "ok": true, "workspaceId": id, "name": name}));
            }
            "workspace.remove" => {
                let key = msg["workspaceId"].as_str().or(msg["name"].as_str()).unwrap_or_default();
                match self.workspace_remove(key, &client_name(peer)) {
                    Ok(_) => reply(ok()),
                    Err(e) => reply(no(&e)),
                }
            }
            "server.key" if !peer.starts_with("phone:") => {
                let name = connect::get().map(|c| c.status().name).unwrap_or_default();
                match crate::machine::deploy_key(&name) {
                    Ok(key) => reply(json!({"t": "server.key", "key": key})),
                    Err(e) => reply(no(&format!("Couldn't make a deploy key: {e}"))),
                }
            }
            "git.clone" => {
                // A clone takes a while: answered from its own thread.
                let (peer, msg) = (peer.to_string(), msg.clone());
                std::thread::spawn(move || {
                    let Some(host) = HOST.get() else { return };
                    let url = msg["url"].as_str().unwrap_or_default();
                    let mut answer = match crate::machine::clone(url, msg["branch"].as_str(), msg["parent"].as_str().unwrap_or("~")) {
                        Ok(dest) => {
                            let id = host.add_workspace(&dest, None);
                            let name = host.with_state(|s| s.workspaces.iter().find(|w| w.id == id).map(|w| w.name.clone()));
                            json!({"t": "ack", "ok": true, "workspaceId": id, "name": name, "path": crate::machine::display(&dest)})
                        }
                        Err(e) => json!({"t": "ack", "ok": false, "error": e}),
                    };
                    answer["id"] = msg["id"].clone();
                    send_to(&peer, answer);
                });
            }
            "account.push" => {
                let answer = winfer_core::accounts_sync::on_push(peer, msg);
                if answer["ok"] == true {
                    eprintln!("signed in to {} as {} (pushed from {})", msg["provider"].as_str().unwrap_or_default(), answer["accountId"].as_str().unwrap_or_default(), client_name(peer));
                }
                reply(answer);
            }
            "account.sync" => winfer_core::accounts_sync::on_sync(peer, msg),
            "account.state" => winfer_core::accounts_sync::on_state(peer, msg),
            "server.update" => {
                // Takes a while (download): answered from its own thread, then the restart.
                let (peer, id) = (peer.to_string(), msg["id"].clone());
                std::thread::spawn(move || {
                    let Some(host) = HOST.get() else { return };
                    let result = host.update(None);
                    let mut answer = match &result {
                        Ok(v) => json!({"t": "ack", "ok": true, "version": v}),
                        Err(e) => json!({"t": "ack", "ok": false, "error": e}),
                    };
                    answer["id"] = id;
                    send_to(&peer, answer);
                    if result.is_ok() {
                        std::thread::sleep(std::time::Duration::from_secs(1));
                        crate::update::restart(&crate::update::exe().unwrap_or_default());
                    }
                });
            }
            "usage.get" => {
                // Polling and reading the logs take a moment: answered from its own thread.
                let (peer, msg) = (peer.to_string(), msg.clone());
                std::thread::spawn(move || {
                    let Some(host) = HOST.get() else { return };
                    let mut answer = crate::limits::answer(host, &msg);
                    answer["id"] = msg["id"].clone();
                    send_to(&peer, answer);
                });
            }
            "mobile.pair" if !peer.starts_with("phone:") => {
                let mut answer = match crate::phone::start_pairing() {
                    Ok(p) => p,
                    Err(e) => json!({"error": e}),
                };
                answer["t"] = json!("mobile.pairing");
                reply(answer);
            }
            "mobile.revoke" if !peer.starts_with("phone:") => {
                let answer = crate::phone::revoke(msg["deviceId"].as_str().unwrap_or_default());
                if answer["ok"] == true {
                    // The mirror's next push lists the phones without it.
                    reply(ok());
                } else {
                    reply(no(answer["error"].as_str().unwrap_or("Couldn't unpair it.")));
                }
            }
            "connect.presence" => {}
            "thread.get" => {
                let Some(pane) = msg["paneId"].as_str().and_then(|id| self.state.lock().pane(id).cloned()) else {
                    return reply(json!({"t": "error", "error": "That agent is gone."}));
                };
                // A chat that isn't running starts, and replays its history into the thread.
                if pane.provider == "winfer" && SESSIONS.get(&pane.id).is_none_or(|s| !s.spawned.load(Ordering::Relaxed)) {
                    self.start_pane(&pane.id);
                }
                // Its changes go to the reader live (also the history a chat just started replays).
                if pane.provider == "winfer" {
                    crate::chats::watch(&pane.id, peer);
                }
                // A chat's thread from its events.
                if let Some(messages) = crate::chats::thread(&pane.id, msg["limit"].as_u64().unwrap_or(200).clamp(1, 500) as usize) {
                    return reply(json!({"t": "thread", "paneId": pane.id, "messages": messages, "source": "winfer"}));
                }
                // A terminal agent's transcript, read off the thread that waits on nothing.
                if pane.provider != "shell" {
                    let (client, id, limit) = (peer.to_string(), msg["id"].clone(), msg["limit"].as_u64().unwrap_or(200).clamp(1, 500) as usize);
                    std::thread::spawn(move || {
                        let found = crate::limits::transcript(&pane).and_then(|(provider, source)| {
                            let list = tauri::async_runtime::block_on(winfer_backend::mobile::mobile_thread(provider.clone(), source, Some(limit)))?;
                            (!list.is_empty()).then(|| (provider, serde_json::to_value(list).unwrap_or_default()))
                        });
                        let mut answer = match found {
                            Some((provider, messages)) => json!({"t": "thread", "paneId": pane.id, "messages": messages, "source": provider}),
                            None => screen_thread(&pane.id),
                        };
                        answer["id"] = id;
                        send_to(&client, answer);
                    });
                    return;
                }
                reply(screen_thread(&pane.id));
            }
            "config.get" => {
                let pane = msg["paneId"].as_str().unwrap_or_default();
                match crate::chats::config(pane) {
                    Some((_, models)) => reply(json!({"t": "config", "paneId": pane, "models": models})),
                    None => reply(json!({"t": "config", "paneId": pane, "models": []})),
                }
            }
            "models.get" => {
                let provider = msg["provider"].as_str().filter(|p| !p.is_empty() && *p != "default").map_or_else(|| self.state.lock().default_provider_id(), str::to_string);
                let (client, id) = (peer.to_string(), msg["id"].clone());
                std::thread::spawn(move || {
                    let mut answer = crate::chats::models(&provider);
                    answer["id"] = id;
                    send_to(&client, answer);
                });
            }
            "pane.set" => {
                let Some(pane) = msg["paneId"].as_str().and_then(|id| self.state.lock().pane(id).cloned()) else { return reply(no("That agent is gone.")) };
                let (model, effort) = (msg["model"].as_str().filter(|m| !m.is_empty()), msg["effort"].as_str().filter(|e| !e.is_empty()));
                match crate::chats::set(&pane.id, model, effort) {
                    Ok(()) => {
                        let what: Vec<String> = [model.map(|m| winfer_core::code::names::model_name(m, None)), effort.map(|e| format!("{e} effort"))].into_iter().flatten().collect();
                        self.log(&pane, format!("Set {} from {}", what.join(" · "), client_name(peer)));
                        reply(ok());
                    }
                    Err(e) => reply(no(&e)),
                }
            }
            "choose" => {
                let Some(pane) = msg["paneId"].as_str().and_then(|id| self.state.lock().pane(id).cloned()) else { return reply(no("That agent is gone.")) };
                if !crate::chats::asks_question(&pane.id) {
                    return reply(no("A winfer server can only answer a chat's questions so far."));
                }
                match crate::chats::answer(&pane.id, msg["option"].as_u64().unwrap_or(0).max(1) as usize, msg["text"].as_str().unwrap_or_default()) {
                    Ok(answer) => {
                        self.log(&pane, format!("Chose \"{}\" from {}", cut(&answer, 60), client_name(peer)));
                        reply(ok());
                    }
                    Err(e) => reply(no(&e)),
                }
            }
            "send" | "approve" | "decline" | "interrupt" | "pane.close" => {
                let pane = msg["paneId"].as_str().and_then(|id| self.state.lock().pane(id).cloned());
                let Some(pane) = pane else { return reply(no("That agent is gone.")) };
                let via = client_name(peer);
                match kind {
                    "send" => {
                        let text = msg["text"].as_str().map(str::trim).unwrap_or_default();
                        if text.is_empty() {
                            return reply(no("Nothing to send."));
                        }
                        // A chat's question takes the text as its answer.
                        if crate::chats::asks_question(&pane.id) {
                            if let Err(e) = crate::chats::answer(&pane.id, 0, text) {
                                return reply(no(&e));
                            }
                        } else {
                            self.type_into(&pane, text);
                        }
                        self.log(&pane, format!("Answered from {via}: {}", cut(text, 80)));
                    }
                    "approve" if crate::chats::decide(&pane.id, true) => self.log(&pane, format!("Approved from {via}")),
                    "decline" if crate::chats::decide(&pane.id, false) => self.log(&pane, format!("Declined from {via}")),
                    "approve" => {
                        let input = self.state.lock().providers.iter().find(|p| p.id == pane.provider).and_then(|p| p.approve_input.clone()).unwrap_or_else(|| "\r".into());
                        if let Some(session) = SESSIONS.get(&pane.id) {
                            session.send(&input);
                        }
                        self.log(&pane, format!("Approved from {via}"));
                    }
                    "decline" | "interrupt" => {
                        if let Some(session) = SESSIONS.get(&pane.id) {
                            if session.is_code() {
                                write_chat(&pane.id, &Cmd::Interrupt);
                            } else {
                                session.send(if pane.provider == "shell" { "\x03" } else { "\x1b" });
                            }
                        }
                        self.log(&pane, format!("{} from {via}", if kind == "decline" { "Declined" } else { "Interrupted" }));
                    }
                    _ => {
                        terminal::close(&pane.id);
                        crate::chats::forget(&pane.id);
                        self.state.lock().remove_pane(&pane.id);
                    }
                }
                reply(ok());
            }
            // Requests carry an id; the rest (the desktop's own mirror, acks) isn't for a server.
            other if !msg["id"].is_null() => reply(json!({"t": "error", "error": format!("unknown request {other}")})),
            _ => {}
        }
    }

    /// A workspace for a folder on this server (the one already there, if any).
    fn add_workspace(&self, path: &std::path::Path, name: Option<&str>) -> String {
        let root = crate::machine::display(path);
        let mut s = self.state.lock();
        if let Some(ws) = s.workspaces.iter().find(|w| w.root_dir == root) {
            return ws.id.clone();
        }
        let name = name.map(str::trim).filter(|n| !n.is_empty()).map(str::to_string).unwrap_or_else(|| path.file_name().map_or(root.clone(), |n| n.to_string_lossy().to_string()));
        s.workspace_for_folder(&root, &name)
    }

    /// An agent's add_workspace (here or from another machine): a folder on this server, or a
    /// repository cloned first. Blocks while cloning.
    fn workspace_add(&self, a: &Value) -> Result<String, String> {
        let me = connect::get().map(|c| c.status().name).unwrap_or_else(|| "this server".into());
        let name = a["name"].as_str();
        let named = |id: &str| self.with_state(|s| s.workspaces.iter().find(|w| w.id == id).map(|w| w.name.clone())).unwrap_or_default();
        if let Some(url) = a["git_url"].as_str().filter(|u| !u.trim().is_empty()) {
            let dest = crate::machine::clone(url, a["branch"].as_str(), a["parent"].as_str().unwrap_or("~"))?;
            let id = self.add_workspace(&dest, name);
            return Ok(format!("Cloned into {} on {me}; it's workspace “{}” there.", crate::machine::display(&dest), named(&id)));
        }
        let path = a["path"].as_str().filter(|p| !p.trim().is_empty()).ok_or("Give a folder path or a git_url to clone.")?;
        let dir = crate::machine::expand(path);
        if !dir.is_dir() {
            return Err(format!("There's no folder {path} on {me}."));
        }
        let id = self.add_workspace(&dir, name);
        Ok(format!("Workspace “{}” ({}) is open on {me}.", named(&id), crate::machine::display(&dir)))
    }

    /// Removes a workspace (by id or name): its sessions close, the folder and worktrees stay.
    fn workspace_remove(&self, key: &str, by: &str) -> Result<String, String> {
        let me = connect::get().map(|c| c.status().name).unwrap_or_else(|| "this server".into());
        let Some((id, name, panes)) = self.with_state(|s| s.workspace_named(key).map(|w| (w.id.clone(), w.name.clone(), w.panes.iter().map(|p| p.id.clone()).collect::<Vec<_>>()))) else {
            let names = self.with_state(|s| s.workspaces.iter().filter(|w| w.remote.is_none()).map(|w| w.name.clone()).collect::<Vec<_>>().join(", "));
            return Err(format!("No workspace “{key}” on {me}. There: {names}."));
        };
        for pane in &panes {
            terminal::close(pane);
            crate::chats::forget(pane);
        }
        self.with_state(|s| s.remove_workspace(&id));
        let n = panes.len();
        eprintln!("removed workspace {name} ({n} session{}), asked by {by}", if n == 1 { "" } else { "s" });
        Ok(format!("Removed workspace “{name}” from {me}{}; its folder stays.", if n == 0 { String::new() } else { format!(" and closed its {n} session{}", if n == 1 { "" } else { "s" }) }))
    }

    pub(crate) fn with_state<R>(&self, f: impl FnOnce(&mut AppState) -> R) -> R {
        f(&mut self.state.lock())
    }

    pub(crate) fn log(&self, pane: &Pane, text: String) {
        self.state.lock().log_activity(&pane.workspace_id, text, Some(pane.provider.clone()));
    }

    pub(crate) fn type_into(&self, pane: &Pane, text: &str) {
        let running = SESSIONS.get(&pane.id).is_some_and(|s| s.spawned.load(Ordering::Relaxed) && !s.exited.load(Ordering::Relaxed));
        if !running {
            // Not started since the daemon started (or ended): start it, and the text goes in once
            // it's up (a chat takes it as its first message; a terminal agent once it has drawn).
            if pane.provider == "winfer" {
                self.first_messages.lock().insert(pane.id.clone(), text.to_string());
                self.start_pane(&pane.id);
            } else {
                self.start_pane(&pane.id);
                let (pane, text) = (pane.clone(), text.to_string());
                std::thread::spawn(move || {
                    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
                    while std::time::Instant::now() < deadline && SESSIONS.get(&pane.id).is_none_or(|s| s.read_text(5).trim().is_empty()) {
                        std::thread::sleep(std::time::Duration::from_millis(300));
                    }
                    std::thread::sleep(std::time::Duration::from_secs(3));
                    // Once: an agent that didn't come up (not installed, signed out) isn't
                    // started again and again.
                    let up = SESSIONS.get(&pane.id).is_some_and(|s| s.spawned.load(Ordering::Relaxed) && !s.exited.load(Ordering::Relaxed));
                    if up && let Some(host) = HOST.get() {
                        host.type_into(&pane, &text);
                    }
                });
            }
            status::submitted(&mut *self.state.lock(), &pane.id, text);
            return;
        }
        if pane.provider == "winfer" && SESSIONS.get(&pane.id).is_some_and(|s| s.is_code()) {
            send_chat_text(&pane.id, text);
        } else if let Some(session) = SESSIONS.get(&pane.id) {
            if text.contains('\n') {
                session.send(&format!("\x1b[200~{}\x1b[201~", text.replace("\r\n", "\r").replace('\n', "\r")));
            } else {
                session.send(text);
            }
            session.send("\r");
        } else {
            return;
        }
        status::submitted(&mut *self.state.lock(), &pane.id, text);
    }

    fn new_pane(&self, req: &Value, peer: &str) -> Value {
        let (id, name, ws) = {
            let mut s = self.state.lock();
            // A workspace asked for by id is that one or none; without one, the first.
            let ws = match req["workspaceId"].as_str().filter(|id| !id.is_empty()) {
                Some(id) => s.workspaces.iter().find(|w| w.id == id),
                None => s.workspaces.first(),
            }
            .map(|w| w.id.clone());
            let Some(ws) = ws else { return json!({"t": "ack", "ok": false, "error": "That workspace is gone."}) };
            let wanted = req["provider"].as_str().filter(|p| !p.is_empty() && *p != "default").map_or_else(|| s.default_provider_id(), str::to_string);
            let Some(name) = s.providers.iter().find(|p| p.id == wanted && p.installed).map(|p| p.name.clone()) else {
                return json!({"t": "ack", "ok": false, "error": format!("{wanted} isn't installed on this server.")});
            };
            let prompt = req["text"].as_str().map(str::trim).filter(|t| !t.is_empty()).map(str::to_string);
            let model = req["model"].as_str().filter(|m| !m.is_empty()).map(str::to_string);
            let effort = req["effort"].as_str().filter(|e| !e.is_empty() && *e != "default").map(str::to_string);
            let worktree = req["worktree"].as_bool();
            if wanted == "winfer"
                && let Some(p) = &prompt
            {
                // A chat takes its first message as a command once it's up.
                let id = s.add_pane(&wanted, AddPane { workspace_id: Some(ws.clone()), model, effort, worktree, ..Default::default() });
                self.first_messages.lock().insert(id.clone(), p.clone());
                (id, name, ws)
            } else {
                (s.add_pane(&wanted, AddPane { workspace_id: Some(ws.clone()), prompt, model, effort, worktree, ..Default::default() }), name, ws)
            }
        };
        let via = client_name(peer);
        self.state.lock().log_activity(&ws, format!("Started {name} from {via}"), None);
        self.start_pane(&id);
        json!({"t": "ack", "ok": true, "paneId": id})
    }

    /// Starts a pane's process (no window here: panes start when asked for).
    fn start_pane(&self, id: &str) {
        let (pane, settings) = {
            let s = self.state.lock();
            let Some(pane) = s.pane(id).cloned() else { return };
            let settings = SpawnSettings::of(&s, &pane);
            (pane, settings)
        };
        let session = SESSIONS.get_or_create(id, &self.events);
        if session.spawned.load(Ordering::Relaxed) {
            return;
        }
        if pane.provider == "winfer" {
            // `winferd code --serve`: JSON lines the desktop's chat view reads; typed text is a send.
            let pane_id = id.to_string();
            session.attach_code(CodeHooks {
                tap: Box::new({
                    let pane_id = pane_id.clone();
                    move |bytes| crate::chats::tap(&pane_id, bytes)
                }),
                input: Box::new(move |text| {
                    let text = text.trim_end_matches(['\r', '\n']);
                    if !text.trim().is_empty() {
                        send_chat_text(&pane_id, text);
                    }
                }),
            });
        }
        session.resize(120, 32);
        terminal::spawn(session, pane, settings, self.events.clone());
    }

    // ---- terminals ----

    fn on_term(&self, peer: &str, msg: &Value) {
        let pane_id = msg["paneId"].as_str().unwrap_or_default().to_string();
        let Some(pane) = self.state.lock().pane(&pane_id).cloned() else {
            if msg["t"] == "term.attach" {
                connect::send(peer, json!({"t": "term.exit", "paneId": pane_id, "code": 0, "gone": true}));
            }
            return;
        };
        let size = |m: &Value| (m["cols"].as_u64().unwrap_or(120).clamp(2, 1000) as usize, m["rows"].as_u64().unwrap_or(32).clamp(2, 500) as usize);
        match msg["t"].as_str().unwrap_or_default() {
            "term.attach" => {
                if SESSIONS.get(&pane_id).is_none_or(|s| !s.spawned.load(Ordering::Relaxed)) {
                    self.start_pane(&pane_id);
                }
                let chat = SESSIONS.get(&pane_id).is_some_and(|s| s.is_code());
                connect_term::attach(&pane_id, peer, chat, &pane.provider);
                let (cols, rows) = size(msg);
                connect_term::set_sized_by(&pane_id, peer);
                if let Some(session) = SESSIONS.get(&pane_id) {
                    session.force_resize(cols, rows);
                }
            }
            "term.in" => {
                let data = String::from_utf8_lossy(&connect_term::unb64(msg["data"].as_str().unwrap_or_default())).to_string();
                if !data.is_empty() && let Some(session) = SESSIONS.get(&pane_id) {
                    session.write_remote(&data, peer);
                }
            }
            "term.resize" => {
                let (cols, rows) = size(msg);
                connect_term::set_sized_by(&pane_id, peer);
                if let Some(session) = SESSIONS.get(&pane_id) {
                    session.force_resize(cols, rows);
                }
            }
            _ => connect_term::detach(&pane_id, peer),
        }
    }

    // ---- agent tools ----

    /// Another machine's agent calling into this one (`tool.call`).
    fn remote_tool(&self, peer: &str, tool: &str, a: &Value) -> Result<String, String> {
        // A pane by number: this machine's order (all workspaces), as list_panes_all shows it.
        let mut a = a.clone();
        if let Some(n) = a["pane"].as_u64().or_else(|| a["pane"].as_str().and_then(|t| t.trim().parse().ok())) {
            let s = self.state.lock();
            let id = s.workspaces.iter().flat_map(ordered).filter(|p| !p.archived).nth((n as usize).wrapping_sub(1)).map(|p| p.id.clone());
            a["pane"] = json!(id.ok_or_else(|| format!("No pane {n} here. list_remote_panes shows them."))?);
        }
        let a = &a;
        match tool {
            "remove_workspace" => self.workspace_remove(a["workspace"].as_str().unwrap_or_default(), &client_name(peer)),
            "list_panes_all" => {
                let s = self.state.lock();
                let me = connect::get().map(|c| c.status().name).unwrap_or_default();
                let spaces = s.workspaces.iter().map(|w| w.name.clone()).collect::<Vec<_>>().join(", ");
                let rows: Vec<String> = s
                    .workspaces
                    .iter()
                    .flat_map(|w| ordered(w).into_iter().filter(|p| !p.archived).map(move |p| (w, p)))
                    .enumerate()
                    .map(|(i, (w, p))| format!("{me}:{} · {} · {} · workspace {} · {}", i + 1, p.provider, status_name(p.status), w.name, s.pane_label(p)))
                    .collect();
                Ok(format!("{me} (online) · workspaces: {spaces}:\n{}", if rows.is_empty() { "(no panes)".into() } else { rows.join("\n") }))
            }
            "pane_action" => {
                let kind = match a["action"].as_str().unwrap_or_default() {
                    "approve" => "approve",
                    "decline" => "decline",
                    "interrupt" => "interrupt",
                    "close" => "pane.close",
                    other => return Err(format!("Unknown action “{other}”.")),
                };
                self.on_message(peer, &json!({"t": kind, "paneId": a["pane"], "id": "tool-action"}));
                Ok("Done.".into())
            }
            "read_pane" => {
                let id = a["pane"].as_str().unwrap_or_default();
                self.state.lock().pane(id).ok_or("No such pane on this machine.")?;
                let lines = a["lines"].as_f64().unwrap_or(80.0).clamp(1.0, 400.0) as usize;
                let session = SESSIONS.get(id).ok_or("That pane hasn't started yet.")?;
                let text = session.read_text(lines);
                Ok(if text.is_empty() { "(no output yet)".into() } else { text })
            }
            "send_to_pane" => {
                let id = a["pane"].as_str().unwrap_or_default();
                let pane = self.state.lock().pane(id).cloned().ok_or("No such pane on this machine.")?;
                let text = a["text"].as_str().unwrap_or_default();
                if text.is_empty() {
                    return Err("Nothing to send.".into());
                }
                // A pane that isn't running starts, and gets the text once it's up (type_into).
                let running = SESSIONS.get(id).is_some_and(|s| s.spawned.load(Ordering::Relaxed) && !s.exited.load(Ordering::Relaxed));
                self.type_into(&pane, text);
                Ok(if running { "Sent.".into() } else { "Starting it; the text goes in once it's up (a few seconds).".into() })
            }
            "spawn_agent" => {
                let ws = {
                    let s = self.state.lock();
                    // A workspace asked for by name is that one, or an error; without one, the first.
                    match a["workspace"].as_str().filter(|n| !n.trim().is_empty()) {
                        Some(n) => Some(s.workspaces.iter().find(|w| w.name.eq_ignore_ascii_case(n.trim())).map(|w| w.id.clone()).ok_or_else(|| {
                            format!("No workspace “{n}” here. Workspaces: {}.", s.workspaces.iter().map(|w| w.name.clone()).collect::<Vec<_>>().join(", "))
                        })?),
                        None => s.workspaces.first().map(|w| w.id.clone()),
                    }
                };
                let req = json!({"workspaceId": ws, "provider": a["provider"], "text": a["prompt"], "worktree": a["worktree"], "model": a["model"], "effort": a["effort"]});
                let r = self.new_pane(&req, "");
                if r["ok"] != true {
                    return Err(r["error"].as_str().unwrap_or("Couldn't start it.").to_string());
                }
                let name = connect::get().map(|c| c.status().name).unwrap_or_default();
                // Its address in this machine's order, as list_panes_all numbers it.
                let id = r["paneId"].as_str().unwrap_or_default();
                let n = self.with_state(|s| s.workspaces.iter().flat_map(ordered).filter(|p| !p.archived).position(|p| p.id == id));
                let what = a["provider"].as_str().unwrap_or("the agent");
                Ok(match n {
                    Some(i) => format!("Started {what} as {name}:{}", i + 1),
                    None => format!("Started {what} (on {name}; see list_remote_panes for its address)"),
                })
            }
            _ => Err(format!("{tool} can't be called from another machine.")),
        }
    }

    /// Connect's tools for this server's agents. None: not one of them; Some(None): answered later.
    fn connect_tool(&self, call: &str, tool: &str, me: &str, a: &Value) -> Option<Option<Result<String, String>>> {
        let connect = connect::get()?;
        let pane_ref = a["pane"].as_str().and_then(|t| t.rsplit_once(':')).filter(|(_, n)| n.trim().parse::<u32>().is_ok());
        let caller = || {
            let s = self.state.lock();
            let who = s.pane(me).map_or("An agent".to_string(), |p| s.provider_name(&p.provider));
            format!("{who} on {}", connect.status().name)
        };
        let result = match tool {
            "__plugin_tools" => Ok(Value::Array(server_tools()).to_string()),
            "list_machines" => {
                let rows: Vec<String> = connect
                    .status()
                    .peers
                    .iter()
                    .map(|p| {
                        let kind = if p.kind == winfer_connect::pairing::Kind::Server { "server" } else { "desktop" };
                        format!("{} · {kind} · {}", p.name, p.link.via.map_or("offline".to_string(), |v| format!("online ({v})")))
                    })
                    .collect();
                Ok(if rows.is_empty() { "No paired machines.".into() } else { rows.join("\n") })
            }
            "list_remote_panes" => return Some(self.forward(call, a["machine"].as_str().unwrap_or_default(), "list_panes_all", json!({}), &caller())),
            "add_workspace" => {
                let (call, a) = (call.to_string(), a.clone());
                std::thread::spawn(move || {
                    let Some(host) = HOST.get() else { return };
                    match host.workspace_add(&a) {
                        Ok(t) => mcp_reply(&call, true, &t),
                        Err(t) => mcp_reply(&call, false, &t),
                    }
                });
                return Some(None);
            }
            "remove_workspace" => {
                let key = a["workspace"].as_str().unwrap_or_default();
                let mine = self.with_state(|s| s.pane(me).map(|p| p.workspace_id.clone()));
                if mine.is_some() && self.with_state(|s| s.workspace_named(key).map(|w| w.id.clone())) == mine {
                    return Some(Some(Err("That's the workspace you're running in: it can't be removed from inside it.".into())));
                }
                self.workspace_remove(key, &caller())
            }
            "new_remote_workspace" => {
                let args = json!({"path": a["path"], "git_url": a["git_url"], "branch": a["branch"], "parent": a["parent"], "name": a["name"]});
                return Some(self.forward_for(call, a["machine"].as_str().unwrap_or_default(), "add_workspace", args, &caller(), 600));
            }
            "remove_remote_workspace" => {
                return Some(self.forward(call, a["machine"].as_str().unwrap_or_default(), "remove_workspace", json!({"workspace": a["workspace"]}), &caller()));
            }
            "spawn_remote_agent" => {
                let args = json!({"provider": a["provider"], "prompt": a["prompt"], "worktree": a["worktree"], "workspace": a["workspace"], "model": a["model"], "effort": a["effort"]});
                return Some(self.forward(call, a["machine"].as_str().unwrap_or_default(), "spawn_agent", args, &caller()));
            }
            "read_pane" | "send_to_pane" if pane_ref.is_some() => {
                let (machine, n) = pane_ref?;
                let mut args = a.clone();
                args["pane"] = json!(n.trim());
                return Some(self.forward(call, machine, tool, args, &caller()));
            }
            "remote_pane_action" => {
                let Some((machine, n)) = pane_ref else { return Some(Some(Err("Give the pane as \"<machine>:<number>\" (list_remote_panes).".into()))) };
                return Some(self.forward(call, machine, "pane_action", json!({"pane": n.trim(), "action": a["action"]}), &caller()));
            }
            "list_reviews" => {
                let msg = crate::review::message();
                let mut rows: Vec<String> = msg["items"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|r| {
                        let stats = &r["stats"];
                        let changes = stats["files"].as_array().map_or(String::new(), |f| format!(" · {} files +{} −{}", f.len(), stats["adds"], stats["dels"]));
                        let tests = match r["tests"]["run"]["ok"].as_bool() {
                            Some(ok) => format!(" · tests {}", if ok { "passed" } else { "failed" }),
                            None => String::new(),
                        };
                        let summary = r["summary"]["text"].as_str().map_or(String::new(), |t| format!("\n  {}", t.replace('\n', " ")));
                        format!("review {} · {} · {}{changes}{tests}{summary}", r["id"].as_str().unwrap_or_default(), r["provider"].as_str().unwrap_or_default(), r["branch"].as_str().unwrap_or("no worktree"))
                    })
                    .collect();
                if rows.is_empty() {
                    rows.push("No open reviews on this server.".into());
                }
                let queue: Vec<String> = msg["queue"].as_array().into_iter().flatten().map(|q| format!("{} → {}: {}", q["branch"].as_str().unwrap_or_default(), q["base"].as_str().unwrap_or_default(), q["state"].as_str().unwrap_or_default())).collect();
                if !queue.is_empty() {
                    rows.push(format!("Merge queue:\n{}", queue.join("\n")));
                }
                Ok(rows.join("\n"))
            }
            "review_action" => {
                let kind = match a["action"].as_str().unwrap_or_default() {
                    "merge" => "review.merge",
                    "tests" => "review.tests",
                    "send" => "review.send",
                    "discard" => "review.discard",
                    "dismiss" => "review.dismiss",
                    other => return Some(Some(Err(format!("Unknown action “{other}”: merge, tests, send, discard or dismiss.")))),
                };
                let host = HOST.get()?;
                let (call, msg) = (call.to_string(), json!({"t": kind, "reviewId": a["review"], "tests": a["tests"] == true, "text": a["text"]}));
                // Tests and merges take a while: off the event thread.
                std::thread::spawn(move || {
                    let answer = crate::review::on_request(host, &msg);
                    let ok = answer["ok"] != false;
                    let done = match msg["t"].as_str() {
                        Some("review.merge") => "Queued for merging; list_reviews shows the merge queue.",
                        Some("review.tests") => "Running its tests; list_reviews shows the result.",
                        _ => "Done.",
                    };
                    mcp_reply(&call, ok, answer["error"].as_str().unwrap_or(if ok { done } else { "That didn't work." }));
                });
                return Some(None);
            }
            _ => return None,
        };
        Some(Some(result))
    }

    /// Runs `tool` on a paired machine; the MCP call is answered when it replies (or in 30 s).
    fn forward(&self, call: &str, machine: &str, tool: &str, args: Value, caller: &str) -> Option<Result<String, String>> {
        self.forward_for(call, machine, tool, args, caller, 30)
    }

    /// `forward`, waiting up to `secs` (clones take longer).
    fn forward_for(&self, call: &str, machine: &str, tool: &str, args: Value, caller: &str, secs: u64) -> Option<Result<String, String>> {
        let connect = connect::get()?;
        let norm = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase();
        let peers = connect.peers();
        let Some(peer) = peers.iter().find(|p| p.peer_id == machine || norm(&p.name) == norm(machine)) else {
            return Some(Err(format!("No paired machine called “{machine}”. Paired: {}.", peers.iter().map(|p| p.name.clone()).collect::<Vec<_>>().join(", "))));
        };
        if !connect.is_up(&peer.peer_id) {
            return Some(Err(format!("{} is offline.", peer.name)));
        }
        let id = format!("st-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos()));
        TOOL_WAITS.lock().insert(id.clone(), call.to_string());
        connect::send(&peer.peer_id, json!({"t": "tool.call", "id": id, "name": tool, "args": args, "caller": caller}));
        let name = peer.name.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_secs(secs));
            if let Some(call) = TOOL_WAITS.lock().remove(&id) {
                mcp_reply(&call, false, &format!("{name} didn't answer in time."));
            }
        });
        None
    }

    /// An agent on this server calling winfer's tools (`winferd mcp-stdio`).
    fn on_mcp_call(&self, payload: &Value) {
        let call = payload["id"].as_str().unwrap_or_default().to_string();
        let tool = payload["tool"].as_str().unwrap_or_default();
        let me = payload["paneId"].as_str().unwrap_or_default();
        let a = &payload["args"];
        // Winfer Connect: other machines, their panes (`machine:N`), this server's reviews.
        match self.connect_tool(&call, tool, me, a) {
            Some(Some(result)) => {
                let (ok, text) = match result {
                    Ok(t) => (true, t),
                    Err(t) => (false, t),
                };
                return mcp_reply(&call, ok, &text);
            }
            Some(None) => return,
            None => {}
        }
        let result = match tool {
            "list_panes" => {
                let s = self.state.lock();
                let ws = s.pane(me).and_then(|p| s.workspaces.iter().find(|w| w.id == p.workspace_id)).or(s.workspaces.first());
                match ws {
                    Some(ws) => Ok(format!(
                        "Workspace “{}” ({}):\n{}",
                        ws.name,
                        ws.root_dir,
                        ordered(ws).iter().enumerate().map(|(i, p)| format!("{}. {} · {}{}", i + 1, s.provider_name(&p.provider), status_name(p.status), if p.id == me { " · (you)" } else { "" })).collect::<Vec<_>>().join("\n")
                    )),
                    None => Err("No workspace.".into()),
                }
            }
            "read_pane" | "send_to_pane" => {
                // Numbers are this workspace's order, as list_panes shows them.
                let id = {
                    let s = self.state.lock();
                    let ws = s.pane(me).and_then(|p| s.workspaces.iter().find(|w| w.id == p.workspace_id)).or(s.workspaces.first());
                    let n = a["pane"].as_u64().or_else(|| a["pane"].as_str().and_then(|t| t.parse().ok()));
                    ws.and_then(|w| n.and_then(|n| ordered(w).get((n as usize).wrapping_sub(1)).map(|p| p.id.clone())))
                };
                match id {
                    Some(id) => self.remote_tool("", tool, &json!({"pane": id, "lines": a["lines"], "text": a["text"]})),
                    None => Err("No such pane. list_panes shows them.".into()),
                }
            }
            "spawn_agent" => self.remote_tool("", tool, a),
            "notify_user" => Ok("Noted (a winfer server shows it on the machines it's paired with as the pane's status).".into()),
            _ => Err(format!("{tool} isn't available on a winfer server yet.")),
        };
        let (ok, text) = match result {
            Ok(t) => (true, t),
            Err(t) => (false, t),
        };
        if let Ok(reply) = serde_json::from_value(json!({ "ok": ok, "text": text })) {
            mcp::mcp_respond(call, reply);
        }
    }
}

/// A command to a chat pane's `winferd code --serve` process.
/// Sends to whoever asked: a paired machine (its peer id) or a phone paired with this server
/// (`phone:<device>`).
pub(crate) fn send_to(client: &str, msg: Value) {
    match client.strip_prefix("phone:") {
        Some(device) => {
            let _ = winfer_backend::mobile::mobile_send(Some(device.to_string()), msg);
        }
        None => connect::send(client, msg),
    }
}

/// To every paired machine and phone (a `status` that isn't silent also pushes to phones).
fn broadcast(msg: Value) {
    connect::broadcast(&msg);
    let _ = winfer_backend::mobile::mobile_send(None, msg);
}

/// "your phone", or the paired machine's name.
fn client_name(client: &str) -> String {
    if client.starts_with("phone:") { "your phone".into() } else { connect::peer_name(client).unwrap_or_else(|| "another machine".into()) }
}

/// A message to a chat. One line goes in as typed text, which the chat shows as your message to
/// everyone following it; text it would take for an approval answer (`y`, `n`, `1`, `3`) and
/// several lines go as a command, noted in the server's copy of the chat.
fn send_chat_text(pane_id: &str, text: &str) {
    let text = text.trim();
    if text.contains('\n') || matches!(text, "y" | "n" | "1" | "3") {
        crate::chats::note_user(pane_id, text);
        write_chat(pane_id, &Cmd::Send { text: text.to_string(), images: vec![], files: vec![], skill: None, steer: false });
    } else {
        let line = format!("{text}\n");
        let id = pane_id.to_string();
        backend::background(move || {
            let _ = pty::pty_write(backend::app().state(), id, line);
        });
    }
}

/// A thread of what the agent's terminal shows (no transcript to read).
fn screen_thread(pane_id: &str) -> Value {
    let screen = SESSIONS.get(pane_id).map(|s| s.read_text(120)).unwrap_or_default();
    let screen = screen.trim_end();
    let messages = if screen.is_empty() { json!([]) } else { json!([{"id": "screen", "role": "assistant", "text": format!("```\n{screen}\n```"), "ts": winfer_core::usage::now_ms()}]) };
    json!({"t": "thread", "paneId": pane_id, "messages": messages, "source": "screen"})
}

pub(crate) fn write_chat(pane_id: &str, cmd: &Cmd) {
    let line = format!("{}\n", cmd.json());
    let id = pane_id.to_string();
    backend::background(move || {
        let _ = pty::pty_write(backend::app().state(), id, line);
    });
}

/// Tiled panes in layout order, then the rest; never other machines' (remote tiles).
fn ordered(ws: &Workspace) -> Vec<&Pane> {
    let tiled = ws.layout.as_ref().map(leaves).unwrap_or_default();
    let mut panes: Vec<&Pane> = tiled.iter().filter_map(|id| ws.panes.iter().find(|p| p.id == *id)).collect();
    panes.extend(ws.panes.iter().filter(|p| !tiled.contains(&p.id)));
    panes.retain(|p| p.remote.is_none());
    panes
}

fn status_name(status: AgentStatus) -> &'static str {
    match status {
        AgentStatus::Idle => "idle",
        AgentStatus::Working => "working",
        AgentStatus::NeedsInput => "needs-input",
        AgentStatus::Done => "done",
        AgentStatus::Error => "error",
    }
}

fn cut(text: &str, n: usize) -> String {
    if text.chars().count() > n { format!("{}…", text.chars().take(n).collect::<String>()) } else { text.to_string() }
}

fn mcp_reply(call: &str, ok: bool, text: &str) {
    if let Ok(reply) = serde_json::from_value(json!({ "ok": ok, "text": text })) {
        mcp::mcp_respond(call.to_string(), reply);
    }
}

/// The Connect tools this server's agents get (with winfer's own).
/// add_workspace's and new_remote_workspace's arguments.
pub(crate) fn workspace_props(machine: Option<&Value>) -> Value {
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

fn server_tools() -> Vec<Value> {
    let machine = json!({"type": "string", "description": "Machine name from list_machines"});
    let pane = json!({"type": "string", "description": "\"<machine>:<number>\" from list_remote_panes"});
    vec![
        json!({"name": "list_machines", "description": "List the winfer machines paired with this server (Winfer Connect): name, desktop or server, online or not. Their panes are reachable with list_remote_panes, read_pane and send_to_pane (pane \"<machine>:<number>\"), spawn_remote_agent and remote_pane_action.", "inputSchema": {"type": "object", "properties": {}}}),
        json!({"name": "list_remote_panes", "description": "List a paired machine's workspaces and panes. Address panes as \"<machine>:<number>\" in read_pane, send_to_pane and remote_pane_action.", "inputSchema": {"type": "object", "properties": {"machine": machine}, "required": ["machine"]}}),
        json!({"name": "spawn_remote_agent", "description": "Start an agent pane on a paired machine, optionally with a first prompt, in a workspace there and its own git worktree.", "inputSchema": {"type": "object", "properties": {
            "machine": machine, "provider": {"type": "string", "description": "Agent id, e.g. claude, codex, winfer, or shell"},
            "prompt": {"type": "string"}, "worktree": {"type": "boolean"}, "workspace": {"type": "string", "description": "Workspace name on that machine (default: its first)"}
        }, "required": ["machine", "provider"]}}),
        json!({"name": "remote_pane_action", "description": "Act on an agent pane on a paired machine: approve or decline what it asks, interrupt it, or close the pane.", "inputSchema": {"type": "object", "properties": {
            "pane": pane, "action": {"type": "string", "enum": ["approve", "decline", "interrupt", "close"]}
        }, "required": ["pane", "action"]}}),
        json!({"name": "add_workspace", "description": "Open a workspace on this machine: an existing folder (path), or a git repository it clones first (git_url, optional branch and parent folder).", "inputSchema": {"type": "object", "properties": workspace_props(None)}}),
        json!({"name": "remove_workspace", "description": "Remove a workspace from this machine: its sessions close; the folder and worktrees stay. Not the one you run in.", "inputSchema": {"type": "object", "properties": {"workspace": {"type": "string", "description": "Workspace name"}}, "required": ["workspace"]}}),
        json!({"name": "new_remote_workspace", "description": "Open a workspace on a paired machine: an existing folder there (path), or a git repository it clones first (git_url). Returns the workspace's name for spawn_remote_agent.", "inputSchema": {"type": "object", "properties": workspace_props(Some(&machine)), "required": ["machine"]}}),
        json!({"name": "remove_remote_workspace", "description": "Remove a workspace from a paired machine: its sessions there close; the folder and worktrees stay.", "inputSchema": {"type": "object", "properties": {"machine": machine, "workspace": {"type": "string", "description": "Workspace name there (list_remote_panes)"}}, "required": ["machine", "workspace"]}}),
        json!({"name": "list_reviews", "description": "List the finished work waiting for review on this server, and its merge queue: review id, agent, branch, changes, tests, summary.", "inputSchema": {"type": "object", "properties": {}}}),
        json!({"name": "review_action", "description": "Act on a review from list_reviews: merge it (through the merge queue; tests: run the tests first), run its tests, send it back to its agent with text, discard the work, or dismiss the review.", "inputSchema": {"type": "object", "properties": {
            "review": {"type": "string"}, "action": {"type": "string", "enum": ["merge", "tests", "send", "discard", "dismiss"]},
            "tests": {"type": "boolean"}, "text": {"type": "string"}
        }, "required": ["review", "action"]}}),
    ]
}
