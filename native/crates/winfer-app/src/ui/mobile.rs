//! The mobile companion's desktop side (port of `features/mobile/mobile.ts`, protocol in
//! docs/mobile/protocol.md). The backend keeps the phones, the relay and push; panes live here,
//! so phone requests arrive as `mobile://request` and answers go back with `mobile_send`.

use super::commands::{Command, PaneRef};
use super::dialogs::ModelInfo as ProviderModelInfo;
use super::welcome::{HARNESS_BACKENDS, backend_default_efforts};
use crate::code::protocol::{Cmd, Decision};
use crate::code::session::CodeModel;
use std::collections::HashSet;
use super::*;
use crate::model::Workspace;
use crate::code::names::model_name;
use crate::request::{AgentRequest, RequestOption, describe_request};
use crate::ui::code_pickers::{effort_levels, match_model};
use serde_json::{Value, json};
use std::time::Instant;
use winfer_backend::{harness, mobile, providers};

const MAX_REPLY: usize = 4000;

#[derive(Default)]
pub(super) struct MobileBridge {
    /// Needs-input panes' requests, read from the screen once the prompt is drawn.
    requests: HashMap<String, AgentRequest>,
    changed_at: HashMap<String, f64>,
    seen: HashMap<String, AgentStatus>,
    last_sent: HashMap<String, String>,
    layout: String,
    started: bool,
    last_activity: Option<Instant>,
    asking: Vec<String>,
    questioned: Vec<String>,
    /// Chat panes that were waiting on an approval or question at the last look.
    chat_asking: HashSet<String>,
    /// A form's answers so far, per chat pane (its approval's id, one answer per question).
    answers: HashMap<String, (String, Vec<String>)>,
    /// Phones reading a chat pane's thread, with when each lease ends; they get its changes live.
    watching: HashMap<String, HashMap<String, Instant>>,
    push_pending: HashSet<String>,
    /// A chat pane's model, effort, mode and branch at the last look, to tell the phones when it changes.
    chat_cfg: HashMap<String, String>,
}

fn now_ms() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_millis() as f64)
}

fn send(device: Option<String>, msg: Value) {
    backend::background(move || {
        let _ = mobile::mobile_send(device, msg);
    });
}

/// A workspace's panes as the desktop shows them: tiles in layout order, then the rest.
fn ordered(ws: &Workspace) -> Vec<&Pane> {
    let tiled = ws.layout.as_ref().map(leaves).unwrap_or_default();
    let mut panes: Vec<&Pane> = tiled.iter().filter_map(|id| ws.panes.iter().find(|p| p.id == *id)).collect();
    panes.extend(ws.panes.iter().filter(|p| !tiled.contains(&p.id)));
    panes
}

fn status_text(status: AgentStatus) -> &'static str {
    agent_api::status_name(status)
}

fn cut_reply(text: Option<&String>) -> Option<String> {
    let text = text?;
    Some(if text.chars().count() > MAX_REPLY { format!("{}…", text.chars().take(MAX_REPLY).collect::<String>()) } else { text.clone() })
}

/// The first unanswered question as the phone shows it, plus a free-text row.
pub fn question_request(p: &agent_api::PendingQuestions) -> AgentRequest {
    let q = &p.questions[p.answers.len().min(p.questions.len() - 1)];
    let progress = if p.questions.len() > 1 { format!("({}/{})", p.answers.len() + 1, p.questions.len()) } else { String::new() };
    let head = match (&q.header, progress.is_empty()) {
        (Some(h), true) => format!("{h}\n"),
        (Some(h), false) => format!("{h} {progress}\n"),
        (None, false) => format!("{progress}\n"),
        (None, true) => String::new(),
    };
    let multi = if q.multi { "\n(Several apply? Type them, comma-separated.)" } else { "" };
    let mut options: Vec<RequestOption> = q.options.iter().enumerate().map(|(i, o)| RequestOption { n: i + 1, label: o.label.clone(), detail: o.description.clone() }).collect();
    options.push(RequestOption { n: options.len() + 1, label: "Type something".into(), detail: None });
    AgentRequest { kind: "question", text: format!("{head}{}{multi}", q.question), options: Some(options) }
}

/// A chat pane's model and effort as the phone shows them.
fn chat_config(state: &crate::code::state::CodeState) -> Value {
    let offered = match_model(&state.models, &state.model).and_then(|i| i.efforts.clone());
    json!({
        "model": state.model,
        "label": if state.model.is_empty() { String::new() } else { model_name(&state.model, state.actual_model.as_deref()) },
        "effort": if state.effort.is_empty() { "default" } else { state.effort.as_str() },
        "efforts": effort_levels(offered),
        "mode": state.mode.label(),
        "yolo": matches!(state.mode, crate::code::protocol::Mode::Yolo | crate::code::protocol::Mode::YoloPlan),
        "cwd": state.cwd,
        "branch": state.branch,
    })
}

/// The desktop's theme, so the phone wears the same colors.
fn theme_json(id: &str) -> Value {
    let def = crate::theme::theme_by_id(id);
    json!({ "id": def.id, "mode": def.mode, "ui": def.ui })
}

/// Effort list for a model on a provider: empty when the model explicitly supports none.
fn model_efforts(provider: &str, offered: Option<Vec<String>>) -> Vec<String> {
    match offered {
        Some(v) if v.is_empty() => Vec::new(),
        Some(v) => effort_levels(Some(v)),
        None => backend_default_efforts(provider).map(|v| effort_levels(Some(v))).unwrap_or_default(),
    }
}

/// Maps harness + provider model catalogs into the mobile `models` payload list.
fn provider_models_json(
    provider: &str,
    harness_models: Option<&[crate::code::protocol::ModelInfo]>,
    provider_models: Option<&[ProviderModelInfo]>,
) -> Vec<Value> {
    if provider == "winfer" {
        return harness_models
            .unwrap_or(&[])
            .iter()
            .map(|m| json!({
                "id": m.spec,
                "name": if m.label.is_empty() { model_name(&m.spec, None) } else { m.label.clone() },
                "detail": m.detail,
                "efforts": model_efforts(provider, m.efforts.clone()),
            }))
            .collect();
    }
    if HARNESS_BACKENDS.contains(&provider) {
        let prefix = format!("{provider}/");
        if let Some(hm) = harness_models {
            let matched: Vec<Value> = hm
                .iter()
                .filter_map(|m| {
                    let slug = m.spec.strip_prefix(&prefix)?;
                    Some(json!({
                        "id": slug,
                        "name": if m.label.is_empty() { model_name(&m.spec, None) } else { m.label.clone() },
                        "detail": m.detail,
                        "efforts": model_efforts(provider, m.efforts.clone()),
                    }))
                })
                .collect();
            if !matched.is_empty() {
                return matched;
            }
        }
    }
    let fallback_efforts = model_efforts(provider, None);
    provider_models
        .unwrap_or(&[])
        .iter()
        .map(|m| json!({
            "id": m.id,
            "name": m.label,
            "efforts": fallback_efforts,
        }))
        .collect()
}

/// The models a chat pane offers, each with the effort levels it takes.
fn chat_models(state: &crate::code::state::CodeState) -> Vec<Value> {
    state.models.iter().map(|m| json!({
        "id": m.spec,
        "name": if m.label.is_empty() { model_name(&m.spec, None) } else { m.label.clone() },
        "detail": m.detail,
        "efforts": effort_levels(m.efforts.clone()),
    })).collect()
}

impl Root {
    /// A chat pane's process side, when this pane is one that's running.
    fn chat(&self, id: &str) -> Option<Entity<CodeModel>> {
        self.code_models.get(id).cloned()
    }

    /// How many of the chat's pending form the phone has answered.
    fn chat_answered(&self, id: &str, cx: &App) -> usize {
        let Some(model) = self.code_models.get(id) else { return 0 };
        let first = model.read(cx).state.approvals.first().map(|a| a.id.clone());
        self.mobile.answers.get(id).filter(|(a, _)| Some(a) == first.as_ref()).map_or(0, |(_, v)| v.len())
    }

    /// A phone's answer to the chat's question; the last one of a form sends them all.
    fn chat_answer(&mut self, id: &str, answer: String, cx: &mut Context<Self>) {
        let Some(model) = self.chat(id) else { return };
        let Some((aid, total)) = model.read(cx).state.approvals.first().filter(|a| !a.questions.is_empty()).map(|a| (a.id.clone(), a.questions.len())) else { return };
        let entry = self.mobile.answers.entry(id.to_string()).or_insert_with(|| (aid.clone(), Vec::new()));
        if entry.0 != aid {
            *entry = (aid.clone(), Vec::new());
        }
        entry.1.push(answer);
        if entry.1.len() >= total {
            let answers = self.mobile.answers.remove(id).map(|(_, v)| v).unwrap_or_default();
            model.update(cx, |m, cx| m.send(Cmd::Answer { id: aid, answers }, cx));
        } else {
            self.mobile_broadcast(id, true, cx);
        }
    }

    /// The chat changed: tell the phones if what it waits on did, and stream the thread to the
    /// phones reading it.
    pub(super) fn on_code_changed(&mut self, id: &str, cx: &mut Context<Self>) {
        self.show_handed_plan(id, cx);
        let Some(pane) = self.state.read(cx).pane(id).cloned() else { return };
        let (asking, busy) = self.chat(id).map_or((false, false), |m| {
            let st = &m.read(cx).state;
            (!st.approvals.is_empty(), st.busy)
        });
        // The pane's status dot follows the session: asking, working a turn, or idle again (a finished
        // pane keeps its Done until something new happens).
        let want = if asking { AgentStatus::NeedsInput } else if busy { AgentStatus::Working } else { AgentStatus::Idle };
        let current = pane.status;
        if want != current && !(want == AgentStatus::Idle && !matches!(current, AgentStatus::Working | AgentStatus::NeedsInput)) {
            let finished = crate::status::report(&self.state, id, want, None, cx);
            if finished {
                self.agent_finished(id, cx);
            }
            cx.notify();
        }
        if !asking {
            self.mobile.answers.remove(id);
        }
        let cfg = self.chat(id).map(|m| chat_config(&m.read(cx).state).to_string());
        if let Some(cfg) = cfg
            && self.mobile.chat_cfg.get(id) != Some(&cfg)
            && self.mobile.chat_cfg.insert(id.to_string(), cfg).is_some()
        {
            self.mobile_broadcast(id, true, cx);
        }
        let was = self.mobile.chat_asking.contains(id);
        if asking != was {
            if asking {
                self.mobile.chat_asking.insert(id.to_string());
            } else {
                self.mobile.chat_asking.remove(id);
            }
            self.mobile_broadcast(id, false, cx);
        } else if asking {
            let snap = self.snapshot_pane(&pane, cx);
            if self.mobile.last_sent.get(id) != Some(&Self::signature(&snap)) {
                self.mobile_broadcast(id, true, cx);
            }
        }
        let now = Instant::now();
        if let Some(phones) = self.mobile.watching.get_mut(id) {
            phones.retain(|_, until| *until > now);
            if phones.is_empty() {
                self.mobile.watching.remove(id);
            } else if self.mobile.push_pending.insert(id.to_string()) {
                let id = id.to_string();
                cx.spawn(async move |this, cx| {
                    // Token bursts coalesce into one push.
                    cx.background_executor().timer(Duration::from_millis(50)).await;
                    let _ = this.update(cx, |this, cx| {
                        this.mobile.push_pending.remove(&id);
                        let Some(model) = this.chat(&id) else { return };
                        let messages = crate::code::mobile::live_thread(&model.read(cx).state, 200);
                        let now = Instant::now();
                        for (device, until) in this.mobile.watching.get(&id).into_iter().flatten() {
                            if *until > now {
                                send(Some(device.clone()), json!({ "t": "thread", "paneId": id, "messages": messages, "source": "winfer" }));
                            }
                        }
                    });
                })
                .detach();
            }
        }
    }

    fn pane_questions(&self, id: &str) -> Option<&agent_api::PendingQuestions> {
        self.questions.iter().find(|q| q.pane_id == id)
    }

    fn snapshot_pane(&self, pane: &Pane, cx: &App) -> Value {
        let s = self.state.read(cx);
        let mut status = status_text(pane.status).to_string();
        let mut message = s.status_messages.get(&pane.id).cloned();
        let mut updated = self.mobile.changed_at.get(&pane.id).copied().unwrap_or_else(now_ms);
        let mut request = (pane.status == AgentStatus::NeedsInput).then(|| self.mobile.requests.get(&pane.id).cloned()).flatten();
        let mut reply = if pane.status == AgentStatus::Done { cut_reply(s.replies.get(&pane.id)) } else { None };
        // A chat pane knows exactly what it waits on; then the agent's `ask_user` questions, then plugin approvals on its behalf.
        let chat_request = self.chat(&pane.id).and_then(|m| crate::code::mobile::request(&m.read(cx).state, self.chat_answered(&pane.id, cx)));
        if let Some(r) = chat_request {
            status = "needs-input".into();
            message = r.text.lines().find(|l| !l.is_empty()).map(str::to_string);
            request = Some(r);
            reply = None;
        } else if let Some(asked) = self.pane_questions(&pane.id) {
            status = "needs-input".into();
            message = asked.questions.get(asked.answers.len()).map(|q| q.question.clone());
            updated = now_ms() - asked.asked_at.elapsed().as_millis() as f64;
            request = Some(question_request(asked));
            reply = None;
        } else if let Some((_, title, text)) = self.approval_for(&pane.id) {
            status = "needs-input".into();
            message = Some(title.clone());
            request = Some(AgentRequest { kind: "approval", text: format!("{title}\n\n{text}"), options: None });
            reply = None;
        }
        let config = self.chat(&pane.id).map(|m| chat_config(&m.read(cx).state));
        json!({
            "id": pane.id, "title": s.pane_label(pane),
            "workspace": s.workspaces.iter().find(|w| w.id == pane.workspace_id).map_or("", |w| w.name.as_str()),
            "workspaceId": pane.workspace_id, "archived": pane.archived,
            "provider": if pane.provider == "shell" { None } else { Some(pane.provider.clone()) },
            "status": status, "message": message, "updatedAt": updated, "request": request, "reply": reply, "config": config,
        })
    }

    fn mobile_mirror(&self, cx: &App) -> Value {
        let s = self.state.read(cx);
        let panes: Vec<Value> = s.workspaces.iter().flat_map(ordered).map(|p| self.snapshot_pane(p, cx)).collect();
        let workspaces: Vec<Value> = s.workspaces.iter().map(|w| json!({ "id": w.id, "name": w.name, "active": w.id == s.active_workspace_id })).collect();
        let providers: Vec<Value> = s.providers.iter().filter(|p| p.installed && p.id != "shell").map(|p| json!({ "id": p.id, "name": p.name })).collect();
        let compact_tools = s.rest.get("codeWeb").and_then(|v| v["compactTools"].as_bool()).unwrap_or(true);
        json!({ "panes": panes, "workspaces": workspaces, "providers": providers, "defaultProvider": s.default_provider_id(), "theme": theme_json(&s.theme), "compactTools": compact_tools })
    }

    fn signature(snapshot: &Value) -> String {
        let mut v = snapshot.clone();
        v["updatedAt"] = json!(0);
        v.to_string()
    }

    fn mobile_broadcast(&mut self, id: &str, silent: bool, cx: &App) {
        let Some(pane) = self.state.read(cx).pane(id).cloned() else { return };
        let snap = self.snapshot_pane(&pane, cx);
        self.mobile.last_sent.insert(id.to_string(), Self::signature(&snap));
        send(None, if silent { json!({ "t": "status", "pane": snap, "silent": true }) } else { json!({ "t": "status", "pane": snap }) });
    }

    fn reread_request(&mut self, id: String, delay: u64, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(delay)).await;
            let _ = this.update(cx, |this, cx| {
                let s = this.state.read(cx);
                if s.pane(&id).is_none_or(|p| p.status != AgentStatus::NeedsInput) {
                    return;
                }
                let message = s.status_messages.get(&id).cloned();
                let screen = SESSIONS.get(&id).map(|session| session.read_text(40)).unwrap_or_default();
                let next = describe_request(&screen, message.as_deref());
                if this.mobile.requests.get(&id) != Some(&next) {
                    this.mobile.requests.insert(id.clone(), next);
                    this.mobile_broadcast(&id, false, cx);
                }
            });
        })
        .detach();
    }

    /// Lets the backend tell "at the desktop" from "away": pushes wait while the user works here.
    pub(super) fn note_user_activity(&mut self) {
        if self.mobile.last_activity.is_none_or(|t| t.elapsed() >= Duration::from_secs(20)) {
            self.mobile.last_activity = Some(Instant::now());
            backend::background(mobile::mobile_activity);
        }
    }

    /// After any state change: tells the phones what moved.
    pub(super) fn sync_mobile(&mut self, cx: &mut Context<Self>) {
        let s = self.state.read(cx);
        if !s.hydrated {
            return;
        }
        let panes: Vec<(String, AgentStatus)> = s.all_panes().map(|p| (p.id.clone(), p.status)).collect();
        let compact_tools = s.rest.get("codeWeb").and_then(|v| v["compactTools"].as_bool()).unwrap_or(true);
        let layout = json!([
            s.active_workspace_id,
            s.workspaces.iter().map(|w| json!([w.id, w.name, ordered(w).iter().map(|p| p.id.clone()).collect::<Vec<_>>()])).collect::<Vec<_>>(),
            s.providers.iter().filter(|p| p.installed).map(|p| p.id.clone()).collect::<Vec<_>>(),
            s.default_provider,
            s.theme,
            compact_tools,
        ]).to_string();
        if !self.mobile.started {
            self.mobile.started = true;
            self.mobile.seen = panes.iter().cloned().collect();
            self.mobile.layout = layout;
            return;
        }
        for (id, status) in &panes {
            if self.mobile.seen.get(id) == Some(status) {
                continue;
            }
            let known = self.mobile.seen.insert(id.clone(), *status).is_some();
            self.mobile.changed_at.insert(id.clone(), now_ms());
            self.mobile.requests.remove(id);
            if *status == AgentStatus::NeedsInput {
                // The hook fires as the prompt appears; give the terminal a moment to draw it.
                self.reread_request(id.clone(), 450, cx);
            } else if known {
                self.mobile_broadcast(id, false, cx);
            }
        }
        let gone: Vec<String> = self.mobile.seen.keys().filter(|id| !panes.iter().any(|p| p.0 == **id)).cloned().collect();
        for id in gone {
            self.mobile.seen.remove(&id);
            self.mobile.changed_at.remove(&id);
            self.mobile.requests.remove(&id);
            self.mobile.last_sent.remove(&id);
        }
        // Plugin approvals and agents' questions change what a pane shows on the phone.
        let asking: Vec<String> = self.plugin_approval_panes();
        let questioned: Vec<String> = self.questions.iter().map(|q| q.pane_id.clone()).collect();
        if asking != self.mobile.asking {
            let all: Vec<String> = asking.iter().chain(&self.mobile.asking).cloned().collect();
            self.mobile.asking = asking;
            all.iter().for_each(|id| self.mobile_broadcast(id, false, cx));
        }
        if questioned != self.mobile.questioned {
            let before = std::mem::replace(&mut self.mobile.questioned, questioned.clone());
            for id in questioned.iter().chain(&before).cloned().collect::<Vec<_>>() {
                self.mobile_broadcast(&id, before.contains(&id) && questioned.contains(&id), cx);
            }
        }
        // New, closed or moved panes, renamed workspaces: the phones get the whole mirror again.
        if layout != self.mobile.layout {
            self.mobile.layout = layout;
            let mut all = self.mobile_mirror(cx);
            for p in all["panes"].as_array().into_iter().flatten() {
                self.mobile.last_sent.insert(p["id"].as_str().unwrap_or_default().to_string(), Self::signature(p));
            }
            all["t"] = json!("panes");
            send(None, all);
        }
        // Silent updates (a new title, a moved pane).
        for (id, status) in panes {
            if status == AgentStatus::NeedsInput && !self.mobile.requests.contains_key(&id) && self.approval_for(&id).is_none() && self.pane_questions(&id).is_none() {
                continue;
            }
            let Some(pane) = self.state.read(cx).pane(&id).cloned() else { continue };
            let snap = self.snapshot_pane(&pane, cx);
            if self.mobile.last_sent.get(&id) != Some(&Self::signature(&snap)) {
                self.mobile_broadcast(&id, true, cx);
            }
        }
    }

    /// `mobile://request`.
    pub(super) fn on_mobile_request(&mut self, payload: &Value, window: &mut Window, cx: &mut Context<Self>) {
        let device = payload["device"].as_str().unwrap_or_default().to_string();
        let req = payload["msg"].clone();
        let kind = req["t"].as_str().unwrap_or_default().to_string();
        let (rid, reply_to) = (req["id"].clone(), device.clone());
        let reply = move |mut msg: Value| {
            msg["id"] = rid.clone();
            send(Some(reply_to.clone()), msg);
        };
        if kind == "panes.get" {
            let mut all = self.mobile_mirror(cx);
            all["t"] = json!("panes");
            return reply(all);
        }
        if kind == "models.get" {
            return self.mobile_models_get(&req, reply, cx);
        }
        if kind == "pane.new" {
            return reply(self.mobile_new_pane(&req, cx));
        }
        let pane = req["paneId"].as_str().and_then(|id| self.state.read(cx).pane(id).cloned());
        let Some(pane) = pane else {
            return reply(json!({ "t": if kind == "thread.get" { "error" } else { "ack" }, "ok": false, "error": "That agent is gone." }));
        };
        let target = PaneRef::Id(pane.id.clone());
        let ok = json!({ "t": "ack", "ok": true });
        let text = req["text"].as_str().map(str::trim).unwrap_or_default().to_string();
        let short = |t: &str, n: usize| t.chars().take(n).collect::<String>();
        let log = |this: &mut Self, text: String, cx: &mut Context<Self>| this.update_state(cx, |s| s.log_activity(&pane.workspace_id, text, Some(pane.provider.clone())));
        match kind.as_str() {
            "config.get" => {
                let Some(model) = self.chat(&pane.id) else {
                    return reply(json!({ "t": "config", "paneId": pane.id, "models": [] }));
                };
                reply(json!({ "t": "config", "paneId": pane.id, "models": chat_models(&model.read(cx).state) }));
            }
            "pane.set" => {
                let Some(model) = self.chat(&pane.id) else {
                    return reply(json!({ "t": "ack", "ok": false, "error": "Only WinferCode agents have a model to change." }));
                };
                let (wanted_model, wanted_effort) = (req["model"].as_str().filter(|m| !m.is_empty()), req["effort"].as_str().filter(|e| !e.is_empty()));
                let st = { let s = &model.read(cx).state; (s.model.clone(), s.models.clone()) };
                let spec = wanted_model.map_or(st.0.clone(), str::to_string);
                if wanted_model.is_some() && !st.1.iter().any(|m| m.spec == spec) {
                    return reply(json!({ "t": "ack", "ok": false, "error": "That model isn't available." }));
                }
                if let Some(level) = wanted_effort {
                    let offered = effort_levels(match_model(&st.1, &spec).and_then(|i| i.efforts.clone()));
                    if !offered.iter().any(|l| l == level) {
                        return reply(json!({ "t": "ack", "ok": false, "error": "That model doesn't take that effort." }));
                    }
                }
                model.update(cx, |m, cx| {
                    if wanted_model.is_some() {
                        m.send(Cmd::SetModel(spec.clone()), cx);
                    }
                    if let Some(level) = wanted_effort {
                        m.send(Cmd::SetEffort((level != "default").then(|| level.to_string())), cx);
                    }
                });
                log(self, format!("Set {} from your phone", [wanted_model.map(|m| model_name(m, None)), wanted_effort.map(|e| format!("{e} effort"))].into_iter().flatten().collect::<Vec<_>>().join(" · ")), cx);
                self.mobile_broadcast(&pane.id, true, cx);
                reply(ok);
            }
            "thread.get" => {
                let limit = req["limit"].as_u64().unwrap_or(200).clamp(1, 500) as usize;
                // A running chat has its transcript in memory: send that, and keep sending it as it grows.
                if let Some(model) = self.chat(&pane.id).filter(|_| pane.provider == "winfer") {
                    self.mobile.watching.entry(pane.id.clone()).or_default().insert(device.clone(), Instant::now() + Duration::from_secs(60));
                    let messages = crate::code::mobile::live_thread(&model.read(cx).state, limit);
                    return reply(json!({ "t": "thread", "paneId": pane.id, "messages": messages, "source": "winfer" }));
                }
                self.mobile_thread(pane, limit, reply, cx)
            }
            "send" => {
                if text.is_empty() {
                    return reply(json!({ "t": "ack", "ok": false, "error": "Nothing to send." }));
                }
                let asked_here = self.chat(&pane.id).is_some_and(|m| m.read(cx).state.approvals.first().is_some_and(|a| !a.questions.is_empty()));
                if let (Some(model), false) = (self.chat(&pane.id), asked_here) {
                    model.update(cx, |m, cx| m.submit(&text, None, cx));
                } else if asked_here {
                    self.chat_answer(&pane.id, text.clone(), cx);
                } else if self.front_question(&pane.id) {
                    self.answer_question(text.clone(), window, cx);
                } else {
                    self.execute(Command::SendText { target, text: text.clone(), submit: true }, window, cx);
                }
                log(self, format!("Answered from your phone: {}", short(&text, 80)), cx);
                reply(ok);
            }
            "choose" => {
                let n = req["option"].as_u64().unwrap_or(0) as usize;
                if let Some(model) = self.chat(&pane.id).filter(|m| m.read(cx).state.approvals.first().is_some_and(|a| !a.questions.is_empty())) {
                    let answer = crate::code::mobile::pick(&model.read(cx).state, self.chat_answered(&pane.id, cx), n, &text);
                    let Some(answer) = answer else {
                        return reply(json!({ "t": "ack", "ok": false, "error": "That choice is no longer on screen." }));
                    };
                    log(self, format!("Chose \"{}\" from your phone", short(&answer, 60)), cx);
                    self.chat_answer(&pane.id, answer, cx);
                    return reply(ok);
                }
                if self.front_question(&pane.id) {
                    let asked = self.questions.front().unwrap();
                    let q = &asked.questions[asked.answers.len()];
                    let answer = if (1..=q.options.len()).contains(&n) { q.options[n - 1].label.clone() } else if n == q.options.len() + 1 { text.clone() } else { String::new() };
                    if answer.is_empty() {
                        return reply(json!({ "t": "ack", "ok": false, "error": "That choice is no longer on screen." }));
                    }
                    self.answer_question(answer.clone(), window, cx);
                    log(self, format!("Chose \"{}\" from your phone", short(&answer, 60)), cx);
                    return reply(ok);
                }
                // Typing an option's number picks it in Claude Code's and winfer code's pickers.
                let label = self.mobile.requests.get(&pane.id).and_then(|r| r.options.as_ref()?.iter().find(|o| o.n == n).map(|o| o.label.clone()));
                let Some(label) = label.filter(|_| pane.status == AgentStatus::NeedsInput && n <= 9) else {
                    return reply(json!({ "t": "ack", "ok": false, "error": "That choice is no longer on screen." }));
                };
                if let Some(session) = SESSIONS.get(&pane.id) {
                    session.send(&n.to_string());
                }
                // "Type something" rows open a text field: type the answer into it.
                if !text.is_empty() {
                    let (id, typed) = (pane.id.clone(), text.clone());
                    cx.spawn(async move |this, cx| {
                        cx.background_executor().timer(Duration::from_millis(250)).await;
                        let _ = this.update(cx, |this, cx| this.type_into(&id, &typed, true, cx));
                    })
                    .detach();
                }
                log(self, format!("Chose \"{}\" from your phone", short(&label, 60)), cx);
                // A form with several questions shows the next one without a status change.
                self.reread_request(pane.id.clone(), 600, cx);
                reply(ok);
            }
            "approve" | "decline" | "interrupt" => {
                if let Some(model) = self.chat(&pane.id) {
                    let (pending, is_form) = { let m = model.read(cx); (m.state.approvals.first().map(|a| a.id.clone()), m.state.approvals.first().is_some_and(|a| !a.questions.is_empty())) };
                    match (pending, kind.as_str()) {
                        // A form is answered with its options, not approved.
                        (Some(_), "approve") if is_form => return reply(json!({ "t": "ack", "ok": false, "error": "Pick an answer instead." })),
                        (Some(id), "approve" | "decline") => {
                            let decision = if kind == "approve" { Decision::Allow } else { Decision::Deny };
                            model.update(cx, |m, cx| m.send(Cmd::Approve { id, decision }, cx));
                            log(self, format!("{} from your phone", if kind == "approve" { "Approved" } else { "Declined" }), cx);
                        }
                        _ => {
                            model.update(cx, |m, cx| m.send(Cmd::Interrupt, cx));
                            log(self, "Interrupted from your phone".into(), cx);
                        }
                    }
                    return reply(ok);
                }
                if kind != "approve" && self.front_question(&pane.id) {
                    self.dismiss_question(window, cx);
                    log(self, "Dismissed the agent's questions from your phone".into(), cx);
                    return reply(ok);
                }
                if let Some((id, title, _)) = self.approval_for(&pane.id) {
                    self.answer_approval_id(id, kind == "approve", cx);
                    log(self, format!("{} “{title}” from your phone", if kind == "approve" { "Approved" } else { "Declined" }), cx);
                    return reply(ok);
                }
                if kind == "approve" {
                    self.execute(Command::Approve(target), window, cx);
                    log(self, "Approved from your phone".into(), cx);
                } else {
                    // Esc cancels the pending request in Claude Code and Codex, and stops a running turn.
                    self.execute(Command::Interrupt(target), window, cx);
                    log(self, if kind == "decline" { "Declined from your phone".into() } else { "Interrupted from your phone".into() }, cx);
                }
                reply(ok);
            }
            "pane.close" => {
                self.execute(Command::Close(target), window, cx);
                reply(ok);
            }
            other => reply(json!({ "t": "error", "error": format!("unknown request {other}") })),
        }
    }

    /// Brings the pane's questions to the front of the queue; false if it has none.
    fn front_question(&mut self, pane: &str) -> bool {
        let Some(i) = self.questions.iter().position(|q| q.pane_id == pane) else { return false };
        if let Some(q) = self.questions.remove(i) {
            self.questions.push_front(q);
        }
        true
    }

    fn mobile_models_get(&mut self, req: &Value, reply: impl Fn(Value) + 'static, cx: &mut Context<Self>) {
        let s = self.state.read(cx);
        let provider = req["provider"]
            .as_str()
            .filter(|p| !p.is_empty() && *p != "default")
            .map_or_else(|| s.default_provider_id(), str::to_string);
        let has_model_arg = s.providers.iter().any(|p| p.id == provider && p.model_arg.is_some());
        let saved_agent_model = s.agent_models.get(&provider).cloned();
        let cached_harness = self.welcome.harness_models.clone();
        let cached_provider = self.models.get(&provider).cloned();
        let cached_defaults = self.welcome.harness_defaults.clone();
        let needs_harness = (provider == "winfer" || HARNESS_BACKENDS.contains(&provider.as_str())) && cached_harness.is_none();
        let needs_defaults = provider == "winfer" && cached_defaults.is_none();

        cx.spawn(async move |this, cx| {
            let defaults = if needs_defaults {
                let cfg = tauri::async_runtime::spawn_blocking(harness::config::Config::load).await.ok();
                cfg.map(|c| ((!c.model.is_empty()).then_some(c.model), c.effort))
            } else {
                cached_defaults
            };

            let harness_models = if needs_harness {
                let list = tauri::async_runtime::spawn_blocking(harness::backend::all_models).await.unwrap_or_default();
                let mapped: Vec<crate::code::protocol::ModelInfo> = list
                    .into_iter()
                    .map(|m| crate::code::protocol::ModelInfo { spec: m.spec, label: m.label, detail: m.detail, efforts: m.efforts })
                    .collect();
                Some(mapped)
            } else {
                cached_harness
            };

            let mut list = provider_models_json(&provider, harness_models.as_deref(), cached_provider.as_deref());
            let mut fetched_provider: Option<Vec<ProviderModelInfo>> = None;
            if list.is_empty() && provider != "winfer" && has_model_arg && cached_provider.is_none() {
                let p = provider.clone();
                let found = tauri::async_runtime::spawn(async move {
                    providers::providers_models(p).await.ok().and_then(|m| serde_json::to_value(m).ok())
                })
                .await
                .ok()
                .flatten();
                let parsed: Vec<ProviderModelInfo> = found.and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default();
                list = provider_models_json(&provider, None, Some(&parsed));
                fetched_provider = Some(parsed);
            }

            let default_model = if provider == "winfer" {
                defaults.as_ref().and_then(|(m, _)| m.clone())
            } else {
                saved_agent_model
            };
            let default_effort = if provider == "winfer" {
                defaults.as_ref().and_then(|(_, e)| e.clone())
            } else {
                None
            };
            let default_efforts = model_efforts(&provider, None);

            let _ = this.update(cx, |this, _| {
                if needs_defaults {
                    this.welcome.harness_defaults = defaults;
                }
                if needs_harness {
                    this.welcome.harness_models = harness_models;
                }
                if let Some(parsed) = fetched_provider {
                    this.models.insert(provider.clone(), parsed);
                }
            });

            reply(json!({
                "t": "models",
                "provider": provider,
                "models": list,
                "defaultModel": default_model,
                "defaultEffort": default_effort,
                "defaultEfforts": default_efforts,
            }));
        })
        .detach();
    }

    fn mobile_new_pane(&mut self, req: &Value, cx: &mut Context<Self>) -> Value {
        let s = self.state.read(cx);
        let Some(ws) = s.workspaces.iter().find(|w| Some(w.id.as_str()) == req["workspaceId"].as_str()).map(|w| w.id.clone()) else {
            return json!({ "t": "ack", "ok": false, "error": "That workspace is gone." });
        };
        let wanted = req["provider"].as_str().filter(|p| *p != "default").map_or_else(|| s.default_provider_id(), str::to_string);
        let Some(name) = s.providers.iter().find(|p| p.id == wanted && p.installed && p.id != "shell").map(|p| p.name.clone()) else {
            return json!({ "t": "ack", "ok": false, "error": "That agent isn't installed on the desktop." });
        };
        let prompt = req["text"].as_str().map(str::trim).filter(|t| !t.is_empty()).map(str::to_string);
        let model = req["model"].as_str().map(str::trim).filter(|m| !m.is_empty()).map(str::to_string);
        let effort = req["effort"].as_str().map(str::trim).filter(|e| !e.is_empty() && *e != "default").map(str::to_string);
        let what = prompt.as_ref().map_or(String::new(), |p| format!(": {}", p.chars().take(80).collect::<String>()));
        let id = self.state.update(cx, |s, cx| {
            let id = s.add_pane(&wanted, AddPane { workspace_id: Some(ws.clone()), prompt, model, effort, ..Default::default() });
            s.log_activity(&ws, format!("Started {name} from your phone{what}"), Some(wanted.clone()));
            cx.notify();
            id
        });
        // Started right away: the desktop may show another workspace, and a tile only starts when drawn.
        self.start_detached(&id, cx);
        json!({ "t": "ack", "ok": true, "paneId": id })
    }

    /// The phone's thread: the agent's session when winfer can read it, else the screen.
    fn mobile_thread(&mut self, pane: Pane, limit: usize, reply: impl Fn(Value) + 'static, cx: &mut Context<Self>) {
        let source = if pane.provider == "winfer" {
            pane.session_id.clone().map(|s| ("winfer".to_string(), s))
        } else {
            self.meters.context.get(&pane.id).and_then(|c| Some((c["provider"].as_str().unwrap_or(&pane.provider).to_string(), c["source"].as_str()?.to_string())))
        };
        cx.spawn(async move |_, _| {
            if let Some((provider, from)) = source {
                let found = tauri::async_runtime::spawn(mobile::mobile_thread(provider.clone(), from, Some(limit))).await.ok().flatten();
                if let Some(list) = found.filter(|l| !l.is_empty()) {
                    return reply(json!({ "t": "thread", "paneId": pane.id, "messages": list, "source": provider }));
                }
            }
            let screen = SESSIONS.get(&pane.id).map(|s| s.read_text(120)).unwrap_or_default();
            let screen = screen.trim_end();
            let messages = if screen.is_empty() { json!([]) } else { json!([{ "id": "screen", "role": "assistant", "text": format!("```\n{screen}\n```"), "ts": now_ms() }]) };
            reply(json!({ "t": "thread", "paneId": pane.id, "messages": messages, "source": "screen" }));
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::agent_api::{Ask, AskOption, PendingQuestions};
    use super::{model_efforts, provider_models_json, question_request};
    use crate::code::protocol::ModelInfo;
    use std::time::Instant;

    #[test]
    fn questions_show_progress_and_a_free_text_row() {
        let ask = |q: &str| Ask { header: Some("DB".into()), question: q.into(), options: vec![AskOption { label: "pg".into(), description: None }], multi: true };
        let p = PendingQuestions { pane_id: "p".into(), questions: vec![ask("Which?"), ask("Next?")], answers: vec![], asked_at: Instant::now(), call: "c".into() };
        let r = question_request(&p);
        assert_eq!(r.text, "DB (1/2)\nWhich?\n(Several apply? Type them, comma-separated.)");
        assert_eq!(r.options.unwrap()[1].label, "Type something");
    }

    #[test]
    fn provider_models_respect_explicit_empty_efforts_and_strip_prefix() {
        let harness = vec![
            ModelInfo { spec: "claude/opus".into(), label: "Opus".into(), detail: "Deep".into(), efforts: Some(vec!["low".into(), "high".into()]) },
            ModelInfo { spec: "claude/haiku".into(), label: "Haiku".into(), detail: String::new(), efforts: Some(vec![]) },
            ModelInfo { spec: "codex/gpt-5".into(), label: "GPT-5".into(), detail: String::new(), efforts: None },
        ];
        let claude = provider_models_json("claude", Some(&harness), None);
        assert_eq!(claude.len(), 2);
        assert_eq!(claude[0]["id"], "opus");
        assert_eq!(claude[0]["efforts"], serde_json::json!(["default", "low", "high"]));
        assert_eq!(claude[1]["id"], "haiku");
        assert_eq!(claude[1]["efforts"], serde_json::json!([]));

        let winfer = provider_models_json("winfer", Some(&harness), None);
        assert_eq!(winfer.len(), 3);
        assert_eq!(winfer[0]["id"], "claude/opus");
        assert_eq!(model_efforts("codex", None), vec!["default", "low", "medium", "high", "xhigh"]);
    }
}
