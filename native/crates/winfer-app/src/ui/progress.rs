//! Progress estimates (port of `features/progress`): an LLM periodically reads each working
//! agent's task and output, for the bars in pane headers, the sidebar and the Sessions overlay.

use super::dialogs::relative_time;
use super::commands::worktree_info;
use super::review::{reply_language, small_button, wrong_script};
use super::*;
use crate::model::Overlay;
use crate::model::progress::{Progress, clean_title, parse_estimate, progress_title};
use serde_json::json;
use winfer_backend::{llm, worktree};

const NAMING: &str = "You name a coding agent's session for a list of sessions, like a short issue title. Given the user's first message to the agent, reply with a title of 2 to 6 words summarizing the goal of the work, e.g. \"Fix login redirect loop\", \"Persist session titles\", \"Dark mode for settings\". Describe the work, don't quote or paraphrase the message: no filler (please, I noticed, can you), no trailing punctuation, no quotes. Reply with the title only.";

const SYSTEM: &str = "You estimate how far a coding agent is with its current task, from the task and the agent's recent terminal output. Evidence: its plan or todo list (count checked items), files it edited, tests or builds it ran and their results, what it says it is doing. 100 only when the task is complete and the agent stopped; exploring and planning is early (5–20), implementing is the middle, testing and fixing is late. An agent that stopped to ask a question or wait for approval isn't done. If the output shows no work on the task yet, answer 0. Reply with JSON only: {\"percent\": <integer 0-100>, \"step\": \"<what it is doing now, max 8 words>\", \"title\": \"<the task, max 6 words>\"}.";

#[derive(Default)]
pub(super) struct ProgressUi {
    /// Newest naming request per pane, so a slow answer doesn't overwrite a newer one.
    naming: HashMap<String, u64>,
    seq: u64,
    /// The task each pane was last named from, so a failed naming isn't retried every second.
    named_from: HashMap<String, String>,
    seconds: u64,
    sessions_key: String,
}

fn tail(text: &str, count: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    chars[chars.len().saturating_sub(count)..].iter().collect::<String>().trim().to_string()
}

async fn compose(provider: String, model: String, system: String, user: String, fast: bool) -> Result<String, String> {
    tauri::async_runtime::spawn(async move {
        let mut req = json!({ "provider": provider, "model": model, "system": system, "user": user });
        if fast {
            req["fast"] = json!(true);
        }
        llm::llm_compose(serde_json::from_value(req).map_err(|e| e.to_string())?).await
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Thin progress bar; an empty track while there's no estimate yet.
pub(super) fn progress_bar(progress: Option<&Progress>, status: AgentStatus, c: &Colors) -> Div {
    let percent = progress.and_then(|p| p.percent);
    let color = if status == AgentStatus::Done || percent == Some(100) {
        c.success
    } else if status == AgentStatus::NeedsInput {
        c.warning
    } else {
        c.accent
    };
    div().w_full().overflow_hidden().rounded_full().when_some(percent, |d, p| d.child(div().h_full().rounded_full().bg(color).w(relative(p.max(2) as f32 / 100.0))))
}

impl Root {
    fn progress_on(&self, cx: &App) -> bool {
        self.state.read(cx).rest.get("progress").is_none_or(|p| p["enabled"] != false)
    }

    /// Runs every second: restores titles when sessions change, names new sessions, and every
    /// five seconds re-estimates working agents while their output changes.
    pub(super) fn tick_progress(&mut self, cx: &mut Context<Self>) {
        let s = self.state.read(cx);
        if !s.hydrated {
            return;
        }
        let sessions = s.all_panes().map(|p| format!("{}={}", p.id, p.session_id.as_deref().unwrap_or_default())).collect::<Vec<_>>().join(",");
        let open: Vec<String> = s.all_panes().map(|p| p.id.clone()).collect();
        let gone: Vec<String> = s.progress.keys().chain(s.run_times.keys()).filter(|id| !open.contains(id)).cloned().collect();
        let unnamed: Vec<(String, String)> = s.all_panes().filter_map(|p| {
            let progress = s.progress.get(&p.id)?;
            let task = progress.task.clone().filter(|_| progress.title.is_none())?;
            Some((p.id.clone(), task))
        }).collect();
        if sessions != self.progress_ui.sessions_key || !gone.is_empty() {
            self.progress_ui.sessions_key = sessions;
            self.state.update(cx, |s, _| {
                gone.iter().for_each(|id| s.forget_progress(id));
                s.sync_session_titles();
            });
        }
        for (id, task) in unnamed {
            if !self.progress_ui.naming.contains_key(&id) && self.progress_ui.named_from.get(&id) != Some(&task) {
                self.progress_ui.named_from.insert(id.clone(), task.clone());
                self.name_session(id, task, cx);
            }
        }
        self.progress_ui.seconds += 1;
        if self.progress_ui.seconds % 5 != 0 || !self.progress_on(cx) {
            return;
        }
        let s = self.state.read(cx);
        let interval = s.rest.get("progress").and_then(|p| p["intervalSec"].as_u64()).unwrap_or(60);
        let due: Vec<Pane> = s.all_panes().filter(|p| p.provider != "shell" && p.status == AgentStatus::Working).filter(|p| {
            let is_due = s.progress.get(&p.id).and_then(|e| e.updated_at).and_then(|t| t.elapsed().ok()).is_none_or(|e| e.as_secs() >= interval);
            is_due && SESSIONS.get(&p.id).is_some_and(|session| session.output_age_ms() < interval * 1000)
        }).cloned().collect();
        for pane in due {
            self.estimate(pane, cx);
        }
    }

    /// An agent without completion hooks went quiet: let the LLM judge whether it's done.
    pub(super) fn estimate_now(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.progress_on(cx) && let Some(pane) = self.state.read(cx).pane(id).cloned() {
            self.estimate(pane, cx);
        }
    }

    /// Asks the LLM's quickest model for a short title, from the session's first message.
    fn name_session(&mut self, id: String, message: String, cx: &mut Context<Self>) {
        let Some((provider, model)) = self.llm_provider(cx) else { return };
        self.progress_ui.seq += 1;
        let seq = self.progress_ui.seq;
        self.progress_ui.naming.insert(id.clone(), seq);
        let system = format!("{NAMING} Write it in {}.", reply_language(&message));
        let first: String = message.chars().take(2000).collect();
        cx.spawn(async move |this, cx| {
            let answer = compose(provider, model, system, format!("First message:\n{first}"), true).await;
            let _ = this.update(cx, |this, cx| {
                if this.progress_ui.naming.get(&id) != Some(&seq) {
                    return;
                }
                this.progress_ui.naming.remove(&id);
                let Some(title) = answer.ok().and_then(|a| clean_title(&a)).filter(|t| !wrong_script(t, &message)) else { return };
                // A title restored from the resumed session meanwhile wins.
                this.update_state(cx, |s| {
                    if s.progress.get(&id).is_some_and(|p| p.title.is_none()) {
                        s.patch_progress(&id, |p| p.title = Some(title.clone()));
                    }
                });
            });
        })
        .detach();
    }

    fn estimate(&mut self, pane: Pane, cx: &mut Context<Self>) {
        let Some((provider, model)) = self.llm_provider(cx) else { return };
        let progress = self.state.read(cx).progress_of(&pane.id);
        // Without a task there's nothing to measure: a freshly started agent's welcome screen
        // would only get a made-up estimate.
        let Some(task) = progress.task.clone().or_else(|| pane.prompt.clone()).filter(|_| !progress.estimating) else { return };
        let output = tail(&SESSIONS.get(&pane.id).map(|s| s.read_text(120)).unwrap_or_default(), 3500);
        if output.is_empty() || progress.last_output.as_ref() == Some(&output) {
            return;
        }
        let name = self.state.read(cx).provider_name(&pane.provider);
        let working = pane.status == AgentStatus::Working;
        let id = pane.id.clone();
        self.update_state(cx, |s| {
            s.patch_progress(&id, |p| {
                p.last_output = Some(output.clone());
                p.estimating = true;
            });
        });
        let system = format!("{SYSTEM} Write step and title in {}.", reply_language(&task));
        cx.spawn(async move |this, cx| {
            // Just the names: the full diff can be megabytes, and this runs for every working agent.
            let mut changes = String::new();
            if let Some(info) = pane.worktree.as_ref().and_then(|t| worktree_info(t).ok()) {
                let files = tauri::async_runtime::spawn(worktree::worktree_changed_files(info)).await;
                if let Ok(Ok(files)) = files {
                    changes = format!("Files changed so far: {}", if files.is_empty() { "none".to_string() } else { files.join(", ") });
                }
            }
            let state = if working { "working" } else { "stopped, waiting for input" };
            let mut parts = vec![format!("Task: {task}"), format!("Agent: {name}"), format!("Agent state: {state}")];
            if !changes.is_empty() {
                parts.push(changes);
            }
            parts.push(format!("Recent output:\n{output}"));
            let answer = compose(provider, model, system, parts.join("\n\n"), false).await;
            let _ = this.update(cx, |this, cx| {
                // The naming call owns the title; the estimate's only fills in when there's none yet.
                let naming = this.progress_ui.naming.contains_key(&id);
                this.update_state(cx, |s| {
                    let working_now = s.pane(&id).is_some_and(|p| p.status == AgentStatus::Working);
                    match answer.and_then(|a| parse_estimate(&a).ok_or_else(|| "no estimate in the answer".to_string())) {
                        Ok(mut e) => {
                            e.step = e.step.filter(|t| !wrong_script(t, &task));
                            e.title = e.title.filter(|t| !wrong_script(t, &task));
                            // Estimates are noisy: within one task the bar only moves forward, and a
                            // working agent isn't finished.
                            s.patch_progress(&id, |p| {
                                let cap = if working_now { 99 } else { 100 };
                                p.percent = Some(p.percent.unwrap_or(0).max(e.percent).min(cap));
                                p.step = e.step.or(p.step.take());
                                if !naming {
                                    p.title = p.title.take().or(e.title);
                                }
                                p.updated_at = Some(std::time::SystemTime::now());
                                p.estimating = false;
                                p.error = None;
                            });
                        }
                        Err(err) => s.patch_progress(&id, |p| {
                            p.estimating = false;
                            p.error = Some(err);
                        }),
                    }
                });
            });
        })
        .detach();
    }

    /// Mod+O: how far each running agent is with its task, across workspaces.
    pub(super) fn render_sessions(&mut self, c: &Colors, cx: &mut Context<Self>) -> Option<AnyElement> {
        let s = self.state.read(cx);
        if s.overlay != Overlay::Progress {
            return None;
        }
        let c = *c;
        let tracked = |p: &Pane| p.provider != "shell" && (matches!(p.status, AgentStatus::Working | AgentStatus::NeedsInput) || s.progress.get(&p.id).is_some_and(|e| e.percent.is_some()));
        let mut body = div().flex().flex_col().gap_4();
        if self.llm_provider(cx).is_none() {
            body = body.child(div().text_xs().text_color(c.warning).child("Progress estimates use the prompt writer LLM, which is off in Settings."));
        }
        let mut any = false;
        let s = self.state.read(cx);
        let mono = crate::fonts::mono(cx);
        for w in &s.workspaces {
            let panes: Vec<&Pane> = w.panes.iter().filter(|p| tracked(p)).collect();
            if panes.is_empty() {
                continue;
            }
            any = true;
            let mut section = div().flex().flex_col().gap_1().child(div().px_1().text_xs().text_color(c.subtle).child(w.name.to_uppercase()));
            for pane in panes {
                let progress = s.progress.get(&pane.id);
                let task = progress_title(progress).or_else(|| pane.prompt.clone()).unwrap_or_else(|| "Task unknown".into());
                let percent = progress.and_then(|p| p.percent).map_or("—".to_string(), |p| format!("{p}%"));
                let detail = match pane.status {
                    AgentStatus::NeedsInput => "Waiting for you".to_string(),
                    AgentStatus::Done => "Finished".to_string(),
                    _ => progress.and_then(|p| p.step.clone()).unwrap_or_else(|| if progress.is_some_and(|p| p.error.is_some()) { "Couldn't estimate".into() } else { "Estimating…".into() }),
                };
                let updated = progress.and_then(|p| p.updated_at).and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| format!("updated {}", relative_time(d.as_millis() as f64)));
                let (id, workspace) = (pane.id.clone(), pane.workspace_id.clone());
                section = section.child(
                    div().id(SharedString::from(format!("session-{id}"))).flex().flex_col().gap_1p5().rounded_lg().px_3().py_2().cursor_pointer().hover(move |d| d.bg(c.hover))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.update_state(cx, |s| s.overlay = Overlay::None);
                            this.focused_view = None;
                            this.execute(Command::WorkspaceSwitch(workspace.clone()), window, cx);
                            this.execute(Command::Focus(commands::PaneRef::Id(id.clone())), window, cx);
                        }))
                        .child(div().flex().items_center().gap_2().text_xs()
                            .child(div().size_1p5().rounded_full().bg(status_color(pane.status, &c)))
                            .child(div().flex_shrink_0().text_color(c.fg).child(s.provider_name(&pane.provider)))
                            .child(div().min_w_0().flex_1().truncate().text_color(c.muted).child(task))
                            .child(div().w(px(40.0)).text_right().font_family(mono.clone()).text_color(c.fg).child(percent)))
                        .child(progress_bar(progress, pane.status, &c).h(px(4.0)).bg(c.hover))
                        .child(div().flex().items_center().gap_2().text_size(px(11.0)).text_color(c.subtle)
                            .child(div().flex_1().truncate().child(detail))
                            .children(updated)),
                );
            }
            body = body.child(section);
        }
        if !any {
            body = body.child(div().py_8().text_center().text_xs().text_color(c.subtle).child("No agent is working right now."));
        }
        let refresh = small_button(c, "sessions-refresh", "Estimate again").on_click(cx.listener(|this, _, _, cx| {
            let working: Vec<Pane> = this.state.read(cx).all_panes().filter(|p| p.provider != "shell" && p.status == AgentStatus::Working).cloned().collect();
            for pane in working {
                this.update_state(cx, |s| s.patch_progress(&pane.id, |p| p.last_output = None));
                this.estimate(pane, cx);
            }
        }));
        let keys = commands::keys_for(&Command::Overlay { overlay: Overlay::Progress, provider: None }, self.state.read(cx));
        Some(
            div().id("sessions-backdrop").absolute().size_full().flex().items_start().justify_center().pt(relative(0.1)).bg(hsla(0.0, 0.0, 0.0, 0.4)).occlude()
                .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| this.dismiss_overlay(cx)))
                .child(
                    div().id("sessions").w(px(760.0)).max_h(relative(0.8)).flex().flex_col().overflow_hidden().rounded_xl().border_1().border_color(c.border_strong).bg(c.panel)
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(div().flex().items_center().gap_2().border_b_1().border_color(c.border).px_5().py_3()
                            .child(div().flex_1().text_sm().child("Sessions"))
                            .when_some(keys, |d, k| d.child(div().text_xs().text_color(c.subtle).child(k)))
                            .child(refresh))
                        .child(div().id("sessions-list").flex_1().min_h_0().overflow_y_scroll().p_4().child(body)),
                )
                .into_any_element(),
        )
    }
}
