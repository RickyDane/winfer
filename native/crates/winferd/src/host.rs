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
            "tool.call" => {
                let result = self.remote_tool(msg["name"].as_str().unwrap_or_default(), &msg["args"]);
                let (ok, text) = match result {
                    Ok(t) => (true, t),
                    Err(t) => (false, t),
                };
                send_to(peer, json!({"t": "tool.result", "id": msg["id"], "ok": ok, "text": text}));
            }
            "pane.new" => reply(self.new_pane(msg, peer)),
            "fs.list" => reply(crate::machine::list(msg["path"].as_str().unwrap_or("~"))),
            "workspace.new" => {
                let path = crate::machine::expand(msg["path"].as_str().unwrap_or("~"));
                if !path.is_dir() {
                    return reply(no("That folder doesn't exist on this server."));
                }
                let id = self.add_workspace(&path, msg["name"].as_str());
                reply(json!({"t": "ack", "ok": true, "workspaceId": id}));
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
                        Ok(dest) => json!({"t": "ack", "ok": true, "workspaceId": host.add_workspace(&dest, None), "path": crate::machine::display(&dest)}),
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
        s.create_workspace(&name, &root)
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
                    if let Some(host) = HOST.get() {
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
            let ws = req["workspaceId"].as_str().and_then(|id| s.workspaces.iter().find(|w| w.id == id)).or(s.workspaces.first()).map(|w| w.id.clone());
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
                let snapshot = connect_term::attach(&pane_id, peer);
                let chat = SESSIONS.get(&pane_id).is_some_and(|s| s.is_code());
                connect::send(peer, json!({"t": "term.snapshot", "paneId": pane_id, "data": connect_term::b64(&snapshot), "chat": chat, "provider": pane.provider}));
                let (cols, rows) = size(msg);
                connect_term::set_sized_by(&pane_id, peer);
                if let Some(session) = SESSIONS.get(&pane_id) {
                    session.force_resize(cols, rows);
                }
            }
            "term.in" => {
                let data = String::from_utf8_lossy(&connect_term::unb64(msg["data"].as_str().unwrap_or_default())).to_string();
                if !data.is_empty() {
                    backend::background(move || {
                        let _ = pty::pty_write(backend::app().state(), pane_id, data);
                    });
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
    fn remote_tool(&self, tool: &str, a: &Value) -> Result<String, String> {
        match tool {
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
                if SESSIONS.get(id).is_none_or(|s| !s.spawned.load(Ordering::Relaxed)) {
                    self.start_pane(id);
                }
                self.type_into(&pane, text);
                Ok("Sent.".into())
            }
            "spawn_agent" => {
                let ws = {
                    let s = self.state.lock();
                    a["workspace"].as_str().and_then(|n| s.workspaces.iter().find(|w| w.name.eq_ignore_ascii_case(n))).or(s.workspaces.first()).map(|w| w.id.clone())
                };
                let req = json!({"workspaceId": ws, "provider": a["provider"], "text": a["prompt"], "worktree": a["worktree"]});
                let r = self.new_pane(&req, "");
                if r["ok"] != true {
                    return Err(r["error"].as_str().unwrap_or("Couldn't start it.").to_string());
                }
                let name = connect::get().map(|c| c.status().name).unwrap_or_default();
                Ok(format!("Started {} (on {name}; see list_remote_panes for its address)", a["provider"].as_str().unwrap_or("the agent")))
            }
            _ => Err(format!("{tool} can't be called from another machine.")),
        }
    }

    /// An agent on this server calling winfer's tools (`winferd mcp-stdio`).
    fn on_mcp_call(&self, payload: &Value) {
        let call = payload["id"].as_str().unwrap_or_default().to_string();
        let tool = payload["tool"].as_str().unwrap_or_default();
        let me = payload["paneId"].as_str().unwrap_or_default();
        let a = &payload["args"];
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
                    Some(id) => self.remote_tool(tool, &json!({"pane": id, "lines": a["lines"], "text": a["text"]})),
                    None => Err("No such pane. list_panes shows them.".into()),
                }
            }
            "spawn_agent" => self.remote_tool(tool, a),
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
