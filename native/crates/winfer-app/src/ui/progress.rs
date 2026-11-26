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
    /// The Sessions panel's filter: all, working, needs you, done.
    filter: SessionFilter,
}

#[derive(Clone, Copy, Default, PartialEq)]
enum SessionFilter {
    #[default]
    All,
    Working,
    Waiting,
    Done,
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
        self.state.read(cx).rest.get("progress").is_some_and(|p| p["enabled"] == true)
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
        // A paired machine's tiles keep the titles that machine gives them (naming them here would
        // spend this machine's model on another machine's sessions).
        let unnamed: Vec<(String, String, Option<String>)> = s.all_panes().filter(|p| p.remote.is_none()).filter_map(|p| {
            let progress = s.progress.get(&p.id)?;
            Some((p.id.clone(), progress.task.clone()?, progress.title.clone()))
        }).collect();
        if sessions != self.progress_ui.sessions_key || !gone.is_empty() {
            self.progress_ui.sessions_key = sessions;
            self.state.update(cx, |s, _| {
                gone.iter().for_each(|id| s.forget_progress(id));
                s.sync_session_titles();
            });
        }
        for (id, task, title) in unnamed {
            if self.progress_ui.naming.contains_key(&id) || self.progress_ui.named_from.get(&id) == Some(&task) {
                continue;
            }
            // A title restored from the last run stays until the agent gets a new request.
            let first_seen = !self.progress_ui.named_from.contains_key(&id);
            self.progress_ui.named_from.insert(id.clone(), task.clone());
            if first_seen && title.is_some() {
                continue;
            }
            self.name_session(id, task, title, cx);
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

    /// Names a session from its message. With a `current` title, the LLM first checks whether
    /// that title still fits the new request and keeps it if so.
    fn name_session(&mut self, id: String, message: String, current: Option<String>, cx: &mut Context<Self>) {
        let Some((provider, model)) = self.llm_provider(cx) else { return };
        self.progress_ui.seq += 1;
        let seq = self.progress_ui.seq;
        self.progress_ui.naming.insert(id.clone(), seq);
        let language = reply_language(&message);
        let system = match &current {
            Some(title) => format!(
                "{NAMING} The user just sent a new request in a session already titled \"{title}\". If that title still describes the work (a follow-up, fix or refinement of the same thing), reply with exactly KEEP. Only if the request moves to clearly different work, reply with a new title. Write it in {language}."
            ),
            None => format!("{NAMING} Write it in {language}."),
        };
        let label = if current.is_some() { "New request" } else { "First message" };
        let first: String = message.chars().take(2000).collect();
        cx.spawn(async move |this, cx| {
            let answer = compose(provider, model, system, format!("{label}:\n{first}"), true).await;
            let _ = this.update(cx, |this, cx| {
                if this.progress_ui.naming.get(&id) != Some(&seq) {
                    return;
                }
                this.progress_ui.naming.remove(&id);
                let Ok(answer) = answer else { return };
                if answer.trim().trim_matches(|c: char| c == '"' || c == '\'' || c == '`' || c == '*' || c == '.').eq_ignore_ascii_case("keep") {
                    return;
                }
                let Some(title) = clean_title(&answer).filter(|t| !wrong_script(t, &message)) else { return };
                // A title changed meanwhile (restored from the resumed session) wins.
                this.update_state(cx, |s| {
                    if s.progress.get(&id).is_some_and(|p| p.title == current) {
                        super::title_swap::arm(&id, &title);
                        s.patch_progress(&id, |p| p.title = Some(title.clone()));
                    }
                });
            });
        })
        .detach();
    }

    fn estimate(&mut self, pane: Pane, cx: &mut Context<Self>) {
        // Not another machine's agents (this machine's model would be spent on them).
        if pane.remote.is_some() {
            return;
        }
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
        let tracked = |p: &Pane| p.provider != "shell" && !p.archived && (matches!(p.status, AgentStatus::Working | AgentStatus::NeedsInput | AgentStatus::Done) || s.progress.get(&p.id).is_some_and(|e| e.percent.is_some()));
        let all: Vec<Pane> = s.all_panes().filter(|p| tracked(p)).cloned().collect();
        let count = |st: AgentStatus| all.iter().filter(|p| p.status == st).count();
        let (working, waiting, done) = (count(AgentStatus::Working), count(AgentStatus::NeedsInput), count(AgentStatus::Done));
        let known: Vec<u32> = all.iter().filter(|p| p.status == AgentStatus::Working).filter_map(|p| s.progress.get(&p.id).and_then(|e| e.percent)).collect();
        let average = (!known.is_empty()).then(|| known.iter().sum::<u32>() / known.len() as u32);
        let filter = self.progress_ui.filter;
        let mono = crate::fonts::mono(cx);

        // Totals: one tile each, the "needs you" one tinted when it isn't zero.
        let stat = |label: &'static str, value: String, color: Hsla| {
            div().flex_1().flex().flex_col().gap_0p5().p_2p5().rounded_lg().border_1().border_color(c.edge).bg(c.panel)
                .child(div().text_size(px(18.0)).font_weight(FontWeight::SEMIBOLD).text_color(color).child(value))
                .child(div().text_size(px(11.0)).text_color(c.subtle).child(label))
        };
        let totals = div().flex().gap_2()
            .child(stat("working", working.to_string(), c.fg))
            .child(stat("need you", waiting.to_string(), if waiting > 0 { c.warning } else { c.fg }))
            .child(stat("done", done.to_string(), if done > 0 { c.success } else { c.fg }))
            .child(stat("avg progress", average.map_or("—".into(), |a| format!("{a}%")), c.fg));
        let tab = |id: &'static str, label: String, value: SessionFilter, cx: &mut Context<Self>| {
            let on = filter == value;
            div().id(id).flex_1().h(px(26.0)).flex().items_center().justify_center().rounded(px(6.0)).text_size(px(12.0)).cursor_pointer()
                .text_color(if on { c.fg } else { c.muted }).when(on, |d| d.bg(c.active)).hover(move |d| d.text_color(c.fg))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.progress_ui.filter = value;
                    cx.notify();
                }))
                .child(label)
        };
        let tabs = div().flex().gap_0p5().p(px(2.0)).rounded(px(8.0)).border_1().border_color(c.edge).bg(c.app)
            .child(tab("sessions-all", format!("All {}", all.len()), SessionFilter::All, cx))
            .child(tab("sessions-working", format!("Working {working}"), SessionFilter::Working, cx))
            .child(tab("sessions-waiting", format!("Needs you {waiting}"), SessionFilter::Waiting, cx))
            .child(tab("sessions-done", format!("Done {done}"), SessionFilter::Done, cx));

        let mut body = div().flex().flex_col().gap_1();
        if self.llm_provider(cx).is_none() {
            body = body.child(div().px_1().pb_2().text_xs().text_color(c.warning).child("Progress estimates use the prompt writer LLM, which is off in Settings."));
        }
        let s = self.state.read(cx);
        let shown: Vec<&Pane> = all.iter().filter(|p| match filter {
            SessionFilter::All => true,
            SessionFilter::Working => p.status == AgentStatus::Working,
            SessionFilter::Waiting => p.status == AgentStatus::NeedsInput,
            SessionFilter::Done => p.status == AgentStatus::Done,
        }).collect();
        // Waiting agents first, then working ones by progress, then the rest.
        let mut shown = shown;
        shown.sort_by_key(|p| (match p.status { AgentStatus::NeedsInput => 0, AgentStatus::Working => 1, _ => 2 }, std::cmp::Reverse(s.progress.get(&p.id).and_then(|e| e.percent).unwrap_or(0))));
        for pane in shown {
            let progress = s.progress.get(&pane.id);
            let title = s.pane_label(pane);
            let task = progress_title(progress).or_else(|| pane.prompt.clone()).unwrap_or_default();
            let workspace_name = s.workspaces.iter().find(|w| w.id == pane.workspace_id).map(|w| w.name.clone()).unwrap_or_default();
            let percent = progress.and_then(|p| p.percent);
            let detail = match pane.status {
                AgentStatus::NeedsInput => progress.and_then(|p| p.step.clone()).unwrap_or_else(|| "Waiting for you".into()),
                AgentStatus::Done => "Finished — ready to review".to_string(),
                _ => progress.and_then(|p| p.step.clone()).unwrap_or_else(|| if progress.is_some_and(|p| p.error.is_some()) { "Couldn't estimate".into() } else { "Estimating…".into() }),
            };
            let updated = progress.and_then(|p| p.updated_at).and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| relative_time(d.as_millis() as f64));
            let (id, workspace, approve) = (pane.id.clone(), pane.workspace_id.clone(), pane.id.clone());
            let status = pane.status;
            let action = match status {
                AgentStatus::NeedsInput => Some(div().id(SharedString::from(format!("session-approve-{id}"))).h(px(24.0)).px_2p5().flex().items_center().rounded_md().bg(c.accent)
                    .text_size(px(11.5)).font_weight(FontWeight::MEDIUM).text_color(c.on_accent).cursor_pointer().hover(|d| d.opacity(0.9))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        this.execute(Command::Approve(commands::PaneRef::Id(approve.clone())), window, cx);
                    }))
                    .child("Approve")),
                AgentStatus::Done => Some(div().id(SharedString::from(format!("session-review-{id}"))).h(px(24.0)).px_2p5().flex().items_center().rounded_md().border_1().border_color(c.edge)
                    .text_size(px(11.5)).text_color(c.fg).cursor_pointer().hover(move |d| d.bg(c.hover))
                    .on_click(cx.listener(|this, _, window, cx| {
                        cx.stop_propagation();
                        this.execute(Command::Overlay { overlay: Overlay::Review, provider: None }, window, cx);
                    }))
                    .child("Review")),
                _ => None,
            };
            body = body.child(
                div().id(SharedString::from(format!("session-{id}"))).flex().gap_3().rounded_lg().p_2p5().cursor_pointer()
                    .when(status == AgentStatus::NeedsInput, |d| d.bg(Hsla { a: 0.06, ..c.warning }))
                    .hover(move |d| d.bg(c.hover))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.update_state(cx, |s| s.overlay = Overlay::None);
                        this.focused_view = None;
                        this.execute(Command::WorkspaceSwitch(workspace.clone()), window, cx);
                        this.execute(Command::Focus(commands::PaneRef::Id(id.clone())), window, cx);
                    }))
                    .child(Self::agent_tile(&pane.provider, status, c))
                    .child(div().flex_1().min_w_0().flex().flex_col().gap_1()
                        .child(div().flex().items_center().gap_2()
                            .child(div().flex_1().min_w_0().truncate().text_size(px(13.0)).font_weight(FontWeight::MEDIUM).text_color(c.fg).child(title))
                            .child(div().flex_shrink_0().font_family(mono.clone()).text_size(px(11.0)).text_color(c.muted).child(percent.map_or("—".into(), |p| format!("{p}%")))))
                        .when(!task.is_empty(), |d| d.child(div().truncate().text_size(px(11.5)).text_color(c.subtle).child(format!("{workspace_name} · {task}"))))
                        .child(progress_bar(progress, status, &c).h(px(3.0)).bg(c.hover))
                        .child(div().flex().items_center().gap_2().text_size(px(11.5))
                            .child(div().flex_1().min_w_0().truncate().text_color(if status == AgentStatus::NeedsInput { c.warning } else { c.muted }).child(detail))
                            .when_some(updated, |d, u| d.child(div().flex_shrink_0().text_color(c.subtle).child(u)))
                            .children(action))),
            );
        }
        if all.is_empty() {
            body = body.child(div().py_10().flex().flex_col().items_center().gap_1().text_color(c.subtle)
                .child(Icon::new(IconName::Activity).size(px(20.0)))
                .child(div().text_size(px(13.0)).text_color(c.fg).child("No agent is working right now"))
                .child(div().text_xs().child("Started agents show up here with how far they are.")));
        }
        let refresh = small_button(c, "sessions-refresh", "Estimate again").on_click(cx.listener(|this, _, _, cx| {
            let working: Vec<Pane> = this.state.read(cx).all_panes().filter(|p| p.provider != "shell" && p.status == AgentStatus::Working).cloned().collect();
            for pane in working {
                this.update_state(cx, |s| s.patch_progress(&pane.id, |p| p.last_output = None));
                this.estimate(pane, cx);
            }
        }));
        let keys = commands::keys_for(&Command::Overlay { overlay: Overlay::Progress, provider: None }, self.state.read(cx));
        // A panel on the right, so the panes stay in view.
        Some(
            div().id("sessions-backdrop").absolute().size_full().flex().justify_end().p_2().bg(hsla(0.0, 0.0, 0.0, 0.25)).occlude()
                .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| this.dismiss_overlay(cx)))
                .child(
                    div().id("sessions").w(px(440.0)).max_w(relative(0.9)).h_full().flex().flex_col().overflow_hidden().rounded(px(12.0)).border_1().border_color(c.edge).bg(c.raised).shadow_2xl()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(div().flex().flex_col().gap_3().px_4().pt_4().pb_3()
                            .child(div().flex().items_center().gap_2()
                                .child(Icon::new(IconName::Activity).size(px(15.0)).text_color(c.accent))
                                .child(div().flex_1().text_size(px(15.0)).font_weight(FontWeight::SEMIBOLD).text_color(c.fg).child("Sessions"))
                                .when_some(keys, |d, k| d.child(div().rounded_sm().border_1().border_color(c.edge).px_1().text_size(px(10.5)).text_color(c.subtle).child(k)))
                                .child(refresh))
                            .child(totals)
                            .child(tabs))
                        .child(div().id("sessions-list").flex_1().min_h_0().overflow_y_scroll().p_2().child(body)),
                )
                .into_any_element(),
        )
    }
}
