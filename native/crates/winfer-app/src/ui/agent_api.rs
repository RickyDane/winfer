//! Tool calls from the winfer MCP server (`src-tauri/src/mcp.rs`), scoped to the calling pane's
//! workspace (port of `src/features/mcp/agentApi.ts`, `questions.ts` and `bgNotices.ts`).

use super::*;
use crate::model::Workspace;
use serde_json::json;
use std::time::Instant;
use winfer_backend::mcp;

/// How long an `ask_user` question waits (the backend's own limit is an hour).
const ASK_TIMEOUT: Duration = Duration::from_secs(59 * 60);

#[derive(Clone, Debug)]
pub struct AskOption {
    pub label: String,
    pub description: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Ask {
    pub header: Option<String>,
    pub question: String,
    pub options: Vec<AskOption>,
    pub multi: bool,
}

pub struct PendingQuestions {
    pub pane_id: String,
    pub questions: Vec<Ask>,
    pub answers: Vec<String>,
    pub asked_at: Instant,
    pub call: String,
}

pub fn parse_questions(raw: &Value) -> Vec<Ask> {
    let text = |v: &Value| v.as_str().map(str::trim).unwrap_or("").to_string();
    raw.as_array()
        .map(|list| {
            list.iter()
                .filter_map(|q| {
                    let question = text(&q["question"]);
                    if question.is_empty() {
                        return None;
                    }
                    let options = q["options"]
                        .as_array()
                        .map(|o| {
                            o.iter()
                                .filter_map(|o| {
                                    let (label, description) = match o.as_str() {
                                        Some(s) => (s.trim().to_string(), None),
                                        None => (text(&o["label"]), Some(text(&o["description"])).filter(|d| !d.is_empty())),
                                    };
                                    (!label.is_empty()).then_some(AskOption { label, description })
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    let header = Some(text(&q["header"])).filter(|h| !h.is_empty());
                    Some(Ask { header, question, options, multi: q["multiSelect"] == true })
                })
                .collect()
        })
        .unwrap_or_default()
}

pub fn answers_text(questions: &[Ask], answers: &[String]) -> String {
    let lines: Vec<String> = questions.iter().zip(answers).map(|(q, a)| format!("\"{}\" → \"{a}\"", q.question)).collect();
    format!("The user answered:\n{}", lines.join("\n"))
}

pub const DISMISSED: &str = "The user dismissed the questions without answering. Don't ask again; go on with your best judgement or ask in plain text.";

pub(super) fn respond(id: &str, ok: bool, text: String) {
    if let Ok(reply) = serde_json::from_value(json!({ "ok": ok, "text": text })) {
        mcp::mcp_respond(id.to_string(), reply);
    }
}

/// Tiled panes in layout order, then archived ones (still running, so still reachable).
fn order(ws: &Workspace) -> Vec<&Pane> {
    let mut panes: Vec<&Pane> = ws.layout.as_ref().map(leaves).unwrap_or_default().iter().filter_map(|id| ws.panes.iter().find(|p| p.id == *id)).collect();
    panes.extend(ws.panes.iter().filter(|p| p.archived));
    panes
}

fn resolve<'a>(ws: &'a Workspace, reference: &Value) -> Result<&'a Pane, String> {
    let panes = order(ws);
    let number = reference.as_u64().or_else(|| reference.as_str().filter(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())).and_then(|s| s.parse().ok()));
    let pane = match number {
        Some(n) => panes.get((n as usize).wrapping_sub(1)).copied(),
        None => panes.iter().find(|p| Some(p.id.as_str()) == reference.as_str()).copied(),
    };
    pane.ok_or_else(|| format!("No pane {} in this workspace. Use list_panes to see them.", reference.as_str().map_or(reference.to_string(), str::to_string)))
}

fn describe(state: &AppState, ws: &Workspace, pane: &Pane) -> String {
    let index = order(ws).iter().position(|p| p.id == pane.id).unwrap_or(0) + 1;
    format!("pane {index} ({})", state.provider_name(&pane.provider))
}

/// The reply a tool gave, or a call that finishes later (`open_url`, `ask_user`).
enum Outcome {
    Done(Result<String, String>),
    Later,
}

impl Root {
    pub(super) fn on_mcp_call(&mut self, payload: Value, window: &mut Window, cx: &mut Context<Self>) {
        let id = payload["id"].as_str().unwrap_or_default().to_string();
        let tool = payload["tool"].as_str().unwrap_or_default().to_string();
        let pane_id = payload["paneId"].as_str().unwrap_or_default().to_string();
        let args = payload["args"].clone();
        match self.run_tool(&id, &tool, &pane_id, &args, window, cx) {
            Outcome::Done(Ok(text)) => respond(&id, true, text),
            Outcome::Done(Err(text)) => respond(&id, false, text),
            Outcome::Later => {}
        }
    }

    fn run_tool(&mut self, call: &str, tool: &str, pane_id: &str, a: &Value, window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        if tool == "__plugin_tools" {
            return Outcome::Done(Ok(self.plugin_agent_tool_list().to_string()));
        }
        let (ws, me) = {
            let s = self.state.read(cx);
            let Some(ws) = s.workspaces.iter().find(|w| w.panes.iter().any(|p| p.id == pane_id)) else {
                return Outcome::Done(Err("The calling pane isn't open in winfer anymore.".to_string()));
            };
            (ws.clone(), ws.panes.iter().find(|p| p.id == pane_id).cloned().unwrap())
        };
        let who = describe(self.state.read(cx), &ws, &me);
        match tool {
            "open_url" => {
                let url = a["url"].as_str().unwrap_or_default().to_string();
                if url.is_empty() {
                    return Outcome::Done(Err("url is empty".to_string()));
                }
                self.tool_open_url(call.to_string(), url, a["new_tab"] == true, &ws.id, who, me.provider.clone(), cx);
                Outcome::Later
            }
            "open_file" => {
                let path = a["path"].as_str().unwrap_or_default().trim().to_string();
                if path.is_empty() {
                    return Outcome::Done(Err("path is empty".to_string()));
                }
                let on_screen = self.state.read(cx).view() == View::Workspace && ws.id == self.state.read(cx).active_workspace_id;
                self.tool_open_file(call.to_string(), path, me.cwd.clone(), a["line"].as_u64().map(|l| l as usize), on_screen, ws.id.clone(), who, me.provider.clone(), cx);
                Outcome::Later
            }
            "ask_user" => {
                let questions = parse_questions(&a["questions"]);
                if questions.is_empty() {
                    return Outcome::Done(Err("questions is empty: give 1-4 questions, each with its options.".to_string()));
                }
                if questions.len() > 4 {
                    return Outcome::Done(Err("Ask at most 4 questions at once.".to_string()));
                }
                let first = questions[0].question.clone();
                status::report(&self.state, pane_id, AgentStatus::NeedsInput, Some(first.clone()), cx);
                let text = format!("{who} asks: {}", first.chars().take(80).collect::<String>());
                self.update_state(cx, |s| s.log_activity(&ws.id, text, Some(me.provider.clone())));
                self.question_serial += 1;
                self.questions.push_back(PendingQuestions { pane_id: pane_id.to_string(), questions, answers: Vec::new(), asked_at: Instant::now(), call: call.to_string() });
                self.reset_question_input(window, cx);
                cx.notify();
                Outcome::Later
            }
            t if self.run_plugin_agent_tool(call, t, a.clone(), pane_id, cx) => Outcome::Later,
            _ => Outcome::Done(self.run_sync_tool(tool, &ws, &me, a, &who, cx)),
        }
    }

    /// The agent tools for live voice: no calling pane, so the user's workspace stands in.
    pub(super) fn run_voice_agent_tool(&mut self, tool: &str, a: &Value, cx: &mut Context<Self>) -> Result<String, String> {
        let (ws, me) = {
            let s = self.state.read(cx);
            let ws = s.active().cloned().ok_or("There's no workspace open.")?;
            let focused = ws.panes.iter().find(|p| s.focused_pane_id.as_deref() == Some(p.id.as_str())).or_else(|| ws.panes.iter().find(|p| p.provider != "shell"));
            let me = Pane { id: "voice".into(), provider: focused.map_or_else(String::new, |p| p.provider.clone()), account_id: focused.and_then(|p| p.account_id.clone()), cwd: ws.root_dir.clone(), ..Default::default() };
            (ws, me)
        };
        self.run_sync_tool(tool, &ws, &me, a, "Voice", cx)
    }

    fn run_sync_tool(&mut self, tool: &str, ws: &Workspace, me: &Pane, a: &Value, who: &str, cx: &mut Context<Self>) -> Result<String, String> {
        match tool {
            "list_panes" => {
                let s = self.state.read(cx);
                let rows: Vec<String> = order(ws)
                    .iter()
                    .enumerate()
                    .map(|(i, p)| {
                        let mut parts = vec![format!("{}. {}", i + 1, s.provider_name(&p.provider)), status_name(p.status).to_string()];
                        if let Some(t) = &p.worktree {
                            parts.push(format!("branch {}", t.branch));
                        }
                        if p.archived {
                            parts.push("archived".to_string());
                        }
                        if p.id == me.id {
                            parts.push("(you)".to_string());
                        }
                        parts.join(" · ")
                    })
                    .collect();
                Ok(format!("Workspace “{}” ({}):\n{}", ws.name, ws.root_dir, rows.join("\n")))
            }
            "read_pane" => {
                let pane = resolve(ws, &a["pane"])?;
                let lines = a["lines"].as_f64().unwrap_or(80.0).clamp(1.0, 400.0) as usize;
                let session = SESSIONS.get(&pane.id).ok_or_else(|| format!("{} hasn't started yet.", describe(self.state.read(cx), ws, pane)))?;
                let text = session.read_text(lines);
                Ok(if text.is_empty() { "(no output yet)".to_string() } else { text })
            }
            "send_to_pane" => {
                let pane = resolve(ws, &a["pane"])?.clone();
                if pane.id == me.id {
                    return Err("That's your own pane.".to_string());
                }
                let text = a["text"].as_str().unwrap_or_default();
                if text.is_empty() {
                    return Err("text is empty".to_string());
                }
                self.type_into(&pane.id, text, a["submit"] != false, cx);
                let target = describe(self.state.read(cx), ws, &pane);
                let line: String = text.lines().next().unwrap_or("").chars().take(80).collect();
                self.update_state(cx, |s| s.log_activity(&ws.id, format!("{who} → {target}: {line}"), Some(me.provider.clone())));
                Ok(format!("Sent to {target}."))
            }
            "spawn_agent" => {
                let wanted = a["provider"].as_str().unwrap_or_default();
                let s = self.state.read(cx);
                let Some(provider) = s.providers.iter().find(|p| p.id == wanted && p.installed).cloned() else {
                    let available: Vec<&str> = s.providers.iter().filter(|p| p.installed).map(|p| p.id.as_str()).collect();
                    return Err(format!("Unknown or uninstalled provider “{wanted}”. Available: {}.", available.join(", ")));
                };
                let opts = AddPane {
                    workspace_id: Some(ws.id.clone()),
                    prompt: a["prompt"].as_str().map(str::to_string),
                    worktree: a["worktree"].as_bool(),
                    split_from: Some(me.id.clone()).filter(|id| ws.panes.iter().any(|p| p.id == *id)),
                    ..Default::default()
                };
                let new_id = self.state.update(cx, |s, cx| {
                    let id = s.add_pane(&provider.id, opts);
                    s.log_activity(&ws.id, format!("{who} started {}", provider.name), Some(me.provider.clone()));
                    cx.notify();
                    id
                });
                let s = self.state.read(cx);
                let fresh = s.workspaces.iter().find(|w| w.id == ws.id).cloned().unwrap_or_default();
                if ws.id != s.active_workspace_id {
                    self.start_detached(&new_id, cx);
                }
                let index = order(&fresh).iter().position(|p| p.id == new_id).unwrap_or(0) + 1;
                Ok(format!("Started {} as pane {index}.", provider.name))
            }
            "notify_user" => {
                let message = a["message"].as_str().unwrap_or("An agent needs you").to_string();
                status::report(&self.state, &me.id, AgentStatus::NeedsInput, Some(message), cx);
                Ok("The user has been notified.".to_string())
            }
            "usage_status" => Ok(self.tool_usage_status(me, cx)),
            "wait_for_usage_reset" => self.tool_wait(me, a, cx),
            other => Err(format!("Unknown tool {other}")),
        }
    }

    fn tool_open_url(&mut self, call: String, url: String, new_tab: bool, workspace: &str, who: String, provider: String, cx: &mut Context<Self>) {
        let opened = self.browser.update(cx, |b, cx| b.open_url(url.clone(), new_tab, Some(workspace.to_string()), cx));
        let workspace = workspace.to_string();
        cx.spawn(async move |this, cx| {
            let result = opened.await;
            let (text, activity) = match result {
                Ok(Some(previous)) => (
                    format!(
                        "Took over the open tab \"{0}\" ({1}), which already showed this site, and opened {url} in it. To keep working there, select that tab with your browser tools (list the tabs, then select the one now on {url}) instead of opening a new tab.",
                        previous.0, previous.1
                    ),
                    format!("{who} took over {} for {url}", previous.0),
                ),
                Ok(None) => (format!("Opened {url} in a new tab of the browser panel."), format!("{who} opened {url}")),
                Err(err) => {
                    respond(&call, true, format!("Couldn't open {url} in the browser panel: {err}"));
                    return;
                }
            };
            respond(&call, true, text);
            let _ = this.update(cx, |this, cx| this.update_state(cx, |s| s.log_activity(&workspace, activity, Some(provider))));
        })
        .detach();
    }

    #[allow(clippy::too_many_arguments)]
    fn tool_open_file(&mut self, call: String, path: String, cwd: String, line: Option<usize>, on_screen: bool, workspace: String, who: String, provider: String, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let (query, base) = (path.clone(), cwd.clone());
            let found = tauri::async_runtime::spawn(async move { winfer_backend::files::files_resolve(base, vec![query], None).await }).await.ok().and_then(|r| r.into_iter().next().flatten());
            let Some(found) = found else {
                respond(&call, false, format!("No file at {path} (relative paths start from {cwd})."));
                return;
            };
            let _ = this.update(cx, |this, cx| {
                this.viewer.update(cx, |v, cx| v.open(found.clone(), line, !on_screen, cx));
                this.update_state(cx, |s| s.log_activity(&workspace, format!("{who} opened {found}"), Some(provider)));
                respond(&call, true, if on_screen { format!("Showing {found} in the file viewer.") } else { format!("Added {found} to the file viewer (the user is in another workspace).") });
            });
        })
        .detach();
    }

    /// A chat handed over its plan for approval: show the plan file in the file viewer (in the
    /// background when the user is elsewhere), like an agent's `open_file`.
    pub(super) fn show_handed_plan(&mut self, pane_id: &str, cx: &mut Context<Self>) {
        let Some(model) = self.code_models.get(pane_id).cloned() else { return };
        let Some(file) = model.update(cx, |m, _| m.state.show_plan.take()) else { return };
        let s = self.state.read(cx);
        let Some(ws) = s.workspaces.iter().find(|w| w.panes.iter().any(|p| p.id == pane_id)) else { return };
        let ws_id = ws.id.clone();
        let provider = ws.panes.iter().find(|p| p.id == pane_id).map(|p| p.provider.clone());
        let on_screen = s.view() == View::Workspace && ws_id == s.active_workspace_id;
        let name = std::path::Path::new(&file).file_name().map_or_else(|| file.clone(), |n| n.to_string_lossy().into_owned());
        self.viewer.update(cx, |v, cx| v.open(file, None, !on_screen, cx));
        self.update_state(cx, |s| s.log_activity(&ws_id, format!("Plan ready for approval: {name}"), provider));
    }

    /// Starts a pane's process without showing it (an agent started in a background workspace).
    pub(super) fn start_detached(&mut self, id: &str, cx: &mut Context<Self>) {
        let s = self.state.read(cx);
        let Some(pane) = s.pane(id).cloned() else { return };
        let settings = terminal::SpawnSettings::of(s, &pane);
        if self.uses_code(&pane.provider, id, cx) {
            self.code_model(id, cx);
            return;
        }
        let session = SESSIONS.get_or_create(id, &self.events);
        session.resize(120, 32);
        terminal::spawn(session, pane, settings, self.events.clone());
    }

    pub(super) fn answer_question(&mut self, text: String, window: &mut Window, cx: &mut Context<Self>) {
        let text = text.trim().to_string();
        if text.is_empty() {
            return;
        }
        let Some(pending) = self.questions.front_mut() else { return };
        pending.answers.push(text);
        if pending.answers.len() < pending.questions.len() {
            self.reset_question_input(window, cx);
            return cx.notify();
        }
        let done = self.questions.pop_front().unwrap();
        respond(&done.call, true, answers_text(&done.questions, &done.answers));
        self.finish_question(&done.pane_id, window, cx);
    }

    pub(super) fn dismiss_question(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(done) = self.questions.pop_front() {
            respond(&done.call, true, DISMISSED.to_string());
            self.finish_question(&done.pane_id, window, cx);
        }
    }

    fn finish_question(&mut self, pane_id: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.state.read(cx).pane(pane_id).is_some_and(|p| p.status == AgentStatus::NeedsInput) {
            status::report(&self.state, pane_id, AgentStatus::Working, None, cx);
        }
        self.reset_question_input(window, cx);
        self.question_picked.clear();
        self.focused_view = None;
        cx.notify();
    }

    /// Questions of panes that closed, and questions nobody answered in time.
    pub(super) fn expire_questions(&mut self, cx: &mut Context<Self>) {
        let s = self.state.read(cx);
        let (mut keep, mut gone) = (std::collections::VecDeque::new(), Vec::new());
        for q in self.questions.drain(..) {
            if s.pane(&q.pane_id).is_some() && q.asked_at.elapsed() < ASK_TIMEOUT {
                keep.push_back(q);
            } else {
                gone.push(q);
            }
        }
        self.questions = keep;
        for q in gone {
            respond(&q.call, true, DISMISSED.to_string());
        }
    }

    /// Types queued background-task notes into panes whose agent is idle.
    pub(super) fn deliver_bg_notes(&mut self, cx: &mut Context<Self>) {
        if self.bg_notes.is_empty() {
            return;
        }
        let queued = std::mem::take(&mut self.bg_notes);
        for (id, notes) in queued {
            let Some(pane) = self.state.read(cx).pane(&id).cloned() else { continue };
            if matches!(pane.status, AgentStatus::Working | AgentStatus::NeedsInput) {
                self.bg_notes.insert(id, notes);
            } else {
                self.type_into(&id, &notes.join("\n\n"), false, cx);
                if let Some(session) = SESSIONS.get(&id) {
                    session.send("\r");
                }
            }
        }
    }
}

pub(super) fn status_name(status: AgentStatus) -> &'static str {
    match status {
        AgentStatus::Idle => "idle",
        AgentStatus::Working => "working",
        AgentStatus::NeedsInput => "needs-input",
        AgentStatus::Done => "done",
        AgentStatus::Error => "error",
    }
}

#[cfg(test)]
mod tests {
    use super::{answers_text, parse_questions};
    use serde_json::json;

    #[test]
    fn questions_are_cleaned() {
        let raw = json!([
            { "question": " Which? ", "header": "H", "multiSelect": true, "options": ["a", " ", { "label": "b", "description": "d" }] },
            { "question": "  " },
        ]);
        let parsed = parse_questions(&raw);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].question, "Which?");
        assert!(parsed[0].multi);
        assert_eq!(parsed[0].options.len(), 2);
        assert_eq!(parsed[0].options[1].description.as_deref(), Some("d"));
        assert_eq!(answers_text(&parsed, &["a".to_string()]), "The user answered:\n\"Which?\" → \"a\"");
    }
}
