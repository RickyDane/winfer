//! The Home view: the conductor's chat (or a forked task's) in the middle, and a live rail with
//! what needs the user, the tasks, the reviews, every machine's workspaces and usage left.
//! The rail is built from app state; no model is involved.

use super::commands::{Command, PaneRef, ViewTarget};
use super::home::{HomeTask, Stage, TaskMode, home_settings};
use super::review::ReviewStatus;
use super::sidebar::{card_bg, monogram_tile};
use super::*;
use crate::model::Overlay;
use crate::voice::task_pane;
use gpui_kit::component::tooltip::Tooltip;

const RAIL_WIDTH: f32 = 320.0;

/// A card's stage pill colors.
fn stage_color(stage: Stage, c: &Colors) -> Hsla {
    match stage {
        Stage::Planning | Stage::Checking => c.accent,
        Stage::Running => c.success,
        Stage::NeedsYou => c.warning,
        Stage::Ready => c.success,
        Stage::Failed => c.danger,
        Stage::Merged | Stage::Cancelled => c.subtle,
    }
}

fn section(c: Colors, title: &str, count: Option<usize>) -> Div {
    div()
        .flex()
        .items_center()
        .gap_1p5()
        .h(px(26.0))
        .mt_3()
        .px_1()
        .text_size(px(11.0))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(c.subtle)
        .child(title.to_string())
        .children(count.filter(|n| *n > 0).map(|n| div().font_weight(FontWeight::NORMAL).child(n.to_string())))
}

fn pill(text: &str, color: Hsla) -> Div {
    div().flex_shrink_0().px_1p5().h(px(18.0)).flex().items_center().rounded(px(5.0)).bg(Hsla { a: 0.14, ..color }).text_color(color).text_size(px(10.5)).font_weight(FontWeight::MEDIUM).child(text.to_string())
}

impl Root {
    pub(super) fn render_home(&mut self, c: &Colors, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let c = *c;
        self.load_home(cx);
        let chat = self.render_home_chat(c, window, cx);
        let rail = self.render_home_rail(c, cx);
        div().size_full().flex().gap_2().child(chat).child(rail)
    }

    /// The thread's chat view, made on first use.
    fn home_view_for(&mut self, pane_id: &str, window: &mut Window, cx: &mut Context<Self>) -> Option<Entity<CodeView>> {
        if let Some(v) = self.home.views.get(pane_id) {
            return Some(v.clone());
        }
        let model = self.home_model(pane_id)?;
        let app = self.state.clone();
        let view = cx.new(|cx| CodeView::new(model, app, window, cx));
        let pane = pane_id.to_string();
        cx.subscribe_in(&view, window, move |this, _, OpenLink(href), window, cx| {
            // winfer://task/3 and winfer://pane/api/2: the orchestrator's links to its tasks and agents.
            if let Some(target) = href.strip_prefix("winfer://") {
                this.open_home_link(target, window, cx);
                return;
            }
            if href.starts_with("http://") || href.starts_with("https://") {
                this.open_link(pane.clone(), terminal::Link::Web(href.clone()), false, window, cx);
                return;
            }
            let (name, line) = crate::code::links::split_line(href);
            let path = crate::ui::viewer::percent_decode(name.trim_start_matches("file://"));
            this.viewer.update(cx, |v, cx| v.open(path, line, false, cx));
        })
        .detach();
        self.home.views.insert(pane_id.to_string(), view.clone());
        Some(view)
    }

    fn render_home_chat(&mut self, c: Colors, window: &mut Window, cx: &mut Context<Self>) -> Div {
        // The thread on screen: the conductor (started when Home first shows), or a forked task.
        if let Some(t) = self.home.thread.clone()
            && self.task(&t).is_none_or(|x| x.mode != TaskMode::Fork)
        {
            self.home.thread = None;
        }
        let pane = self.home_thread_pane();
        let started = match &self.home.thread {
            None => self.conductor(cx).map(|_| ()),
            Some(t) => {
                let t = t.clone();
                self.home_fork_model(&t, cx)
            }
        };
        let body: AnyElement = match started.and_then(|_| self.home_view_for(&pane, window, cx).ok_or_else(|| "Couldn't open the chat.".to_string())) {
            Ok(view) => div().flex_1().min_h_0().child(view).into_any_element(),
            Err(e) => div().flex_1().flex().items_center().justify_center().p_6().text_size(px(13.0)).text_color(c.muted).child(e).into_any_element(),
        };

        // Thread tabs: Home, then the forked tasks still open.
        let forks: Vec<HomeTask> = self.home.tasks.iter().filter(|t| t.mode == TaskMode::Fork && t.stage.open()).cloned().collect();
        let tab = |id: SharedString, label: String, on: bool| {
            div()
                .id(id)
                .flex()
                .items_center()
                .gap_1p5()
                .h(px(26.0))
                .px_2p5()
                .rounded(px(7.0))
                .text_size(px(12.0))
                .cursor_pointer()
                .when(on, |d| d.bg(c.active).text_color(c.fg).font_weight(FontWeight::MEDIUM))
                .when(!on, |d| d.text_color(c.muted).hover(move |d| d.bg(c.hover).text_color(c.fg)))
                .child(label)
        };
        let mut tabs = div().flex().items_center().gap_1().min_w_0().overflow_hidden().child(
            tab("home-tab-conductor".into(), "Home".into(), self.home.thread.is_none())
                .child(Icon::new(IconName::LayoutDashboard).size(px(12.0)))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.home.thread = None;
                    cx.notify();
                })),
        );
        for t in &forks {
            let on = self.home.thread.as_deref() == Some(t.id.as_str());
            let id = t.id.clone();
            let busy = self.fork_step(&t.id, cx).is_some();
            tabs = tabs.child(
                tab(SharedString::from(format!("home-tab-{}", t.id)), t.title(), on)
                    .tooltip({
                        let tip = format!("{} · {}", t.goal, t.workspace);
                        move |window, cx| Tooltip::new(tip.clone()).build(window, cx)
                    })
                    .when(busy, |d| d.child(div().size(px(6.0)).rounded_full().bg(c.success)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.home.thread = Some(id.clone());
                        cx.notify();
                    })),
            );
        }
        let step = match &self.home.thread {
            None => self.orchestrator_step(cx),
            Some(t) => self.fork_step(t, cx),
        };
        let on_conductor = self.home.thread.is_none();
        let header = div()
            .flex()
            .items_center()
            .gap_2()
            .h(px(40.0))
            .px_2()
            .border_b_1()
            .border_color(c.edge)
            .child(tabs)
            .child(div().flex_1())
            .children(step.map(|s| div().max_w(px(260.0)).truncate().text_size(px(11.5)).text_color(c.subtle).child(s)))
            .when(on_conductor, |d| {
                d.child(
                    div()
                        .id("home-fresh")
                        .flex()
                        .items_center()
                        .gap_1()
                        .h(px(24.0))
                        .px_2()
                        .rounded_md()
                        .text_size(px(11.5))
                        .text_color(c.muted)
                        .cursor_pointer()
                        .hover(move |d| d.bg(c.hover).text_color(c.fg))
                        .tooltip(|window, cx| Tooltip::new("Start a new conversation with Home's orchestrator (tasks stay)").build(window, cx))
                        .on_click(cx.listener(|this, _, _, cx| this.stop_conductor(cx)))
                        .child(Icon::new(IconName::RotateCcw).size(px(12.0)))
                        .child("New chat"),
                )
            });
        div().flex_1().min_w_0().h_full().flex().flex_col().overflow_hidden().rounded_2xl().border_1().border_color(c.edge).bg(c.panel).child(header).child(body)
    }

    /// A forked task's orchestrator for its thread, started (or resumed) when the user opens it.
    fn home_fork_model(&mut self, task_id: &str, cx: &mut Context<Self>) -> Result<(), String> {
        if self.home_model(&task_pane(task_id)).is_some() {
            return Ok(());
        }
        self.open_fork(task_id, cx)
    }

    fn render_home_rail(&mut self, c: Colors, cx: &mut Context<Self>) -> Div {
        let mut rail = div().id("home-rail").flex_1().min_h_0().overflow_y_scroll().px_2().pb_3();
        rail = rail.child(self.home_needs_you(c, cx));
        rail = rail.child(self.home_tasks(c, cx));
        rail = rail.children(self.home_reviews(c, cx));
        rail = rail.child(self.home_fleet(c, cx));
        rail = rail.children(self.home_usage(c, cx));
        div().w(px(RAIL_WIDTH)).h_full().flex_shrink_0().flex().flex_col().overflow_hidden().rounded_2xl().border_1().border_color(c.edge).bg(c.panel).child(rail)
    }

    /// Agents waiting for the user, here and on the paired machines.
    fn home_needs_you(&mut self, c: Colors, cx: &mut Context<Self>) -> Div {
        let s = self.state.read(cx);
        let local: Vec<Pane> = s.workspaces.iter().filter(|w| w.remote.is_none()).flat_map(|w| w.panes.iter()).filter(|p| !p.archived && p.provider != "shell" && matches!(p.status, AgentStatus::NeedsInput | AgentStatus::Error)).cloned().collect();
        let mut remote = Vec::new();
        for (peer, m) in &self.connect.mirrors {
            let name = crate::connect::peer_name(peer).unwrap_or_default();
            for p in m.panes.iter().filter(|p| !p.archived && p.status == AgentStatus::NeedsInput) {
                let ws = m.workspaces.iter().find(|w| w.0 == p.workspace_id).map_or("", |w| w.1.as_str());
                remote.push((format!("{name}/{ws}"), p.clone()));
            }
        }
        let mut sec = div().flex().flex_col().gap_1().child(section(c, "Needs you", Some(local.len() + remote.len())));
        if local.is_empty() && remote.is_empty() {
            return sec.child(div().px_1().text_size(px(12.0)).text_color(c.subtle).child("Nothing is waiting for you."));
        }
        for p in &local {
            let ws = s.workspaces.iter().find(|w| w.id == p.workspace_id);
            let place = ws.map(|w| super::home::address(w, &p.id)).unwrap_or_default();
            let ask = s.status_messages.get(&p.id).cloned().or_else(|| s.progress.get(&p.id).and_then(|e| e.step.clone())).unwrap_or_else(|| if p.status == AgentStatus::Error { "Hit an error".into() } else { "Waiting for your input".into() });
            let (open, approve) = (p.id.clone(), p.id.clone());
            let needs_input = p.status == AgentStatus::NeedsInput;
            sec = sec.child(
                div()
                    .id(SharedString::from(format!("home-need-{}", p.id)))
                    .flex()
                    .items_center()
                    .gap(px(9.0))
                    .rounded_lg()
                    .px_2()
                    .py(px(7.0))
                    .bg(Hsla { a: 0.06, ..c.warning })
                    .cursor_pointer()
                    .hover(move |d| d.bg(Hsla { a: 0.1, ..c.warning }))
                    .on_click(cx.listener(move |this, _, window, cx| this.home_focus_pane(&open, window, cx)))
                    .child(Self::agent_tile(&p.provider, p.status, c))
                    .child(div().flex_1().min_w_0().flex().flex_col().child(div().truncate().text_size(px(12.5)).text_color(c.fg).child(format!("{place} · {}", s.provider_name(&p.provider)))).child(div().truncate().text_size(px(11.5)).text_color(c.muted).child(ask)))
                    .when(needs_input, |d| {
                        d.child(
                            div()
                                .id(SharedString::from(format!("home-approve-{}", p.id)))
                                .flex_shrink_0()
                                .h(px(24.0))
                                .px_2p5()
                                .flex()
                                .items_center()
                                .rounded_md()
                                .bg(c.accent)
                                .text_size(px(11.5))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(c.on_accent)
                                .cursor_pointer()
                                .hover(|d| d.opacity(0.9))
                                .tooltip(|window, cx| Tooltip::new("Approve what it's asking (sends its approve key)").build(window, cx))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    cx.stop_propagation();
                                    this.execute(Command::Approve(PaneRef::Id(approve.clone())), window, cx);
                                }))
                                .child("Approve"),
                        )
                    }),
            );
        }
        for (place, p) in remote {
            let ask = p.request_text.clone().or(p.message.clone()).unwrap_or_else(|| "Waiting for your input".into());
            sec = sec.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(9.0))
                    .rounded_lg()
                    .px_2()
                    .py(px(7.0))
                    .bg(Hsla { a: 0.06, ..c.warning })
                    .child(Self::agent_tile(&p.provider, p.status, c))
                    .child(div().flex_1().min_w_0().flex().flex_col().child(div().truncate().text_size(px(12.5)).text_color(c.fg).child(format!("{place} · {}", p.title))).child(div().truncate().text_size(px(11.5)).text_color(c.muted).child(ask))),
            );
        }
        sec
    }

    /// Focuses an agent pane in its workspace (leaving Home).
    pub(super) fn home_focus_pane(&mut self, pane_id: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.update_state(cx, |s| s.set_view(View::Workspace));
        self.execute(Command::Focus(PaneRef::Id(pane_id.to_string())), window, cx);
    }

    fn home_tasks(&mut self, c: Colors, cx: &mut Context<Self>) -> Div {
        let mut tasks: Vec<HomeTask> = self.home.tasks.clone();
        // Open ones first (newest first), then the recently done.
        tasks.sort_by_key(|t| (!t.stage.open(), std::cmp::Reverse(t.updated)));
        let open = tasks.iter().filter(|t| t.stage.open()).count();
        let mut sec = div().flex().flex_col().gap_1p5().child(section(c, "Tasks", Some(open)));
        if tasks.is_empty() {
            return sec.child(div().px_1().text_size(px(12.0)).text_color(c.subtle).child("Ask Home for something to build: it becomes a task here."));
        }
        for t in tasks.into_iter().take(12) {
            let color = stage_color(t.stage, &c);
            let how = match t.mode {
                TaskMode::Light => "one agent",
                TaskMode::Fork => "orchestrated",
                TaskMode::Remote => "on another machine",
            };
            let step = self.fork_step(&t.id, cx);
            let on = self.home.thread.as_deref() == Some(t.id.as_str());
            let (click, close) = (t.clone(), t.id.clone());
            let done = !t.stage.open();
            sec = sec.child(
                div()
                    .id(SharedString::from(format!("home-task-{}", t.id)))
                    .group("home-task")
                    .flex()
                    .flex_col()
                    .gap_1()
                    .rounded_lg()
                    .px_2p5()
                    .py_2()
                    .bg(card_bg(c))
                    .border_1()
                    .border_color(if on { c.accent } else { c.edge })
                    .cursor_pointer()
                    .when(done, |d| d.opacity(0.6))
                    .hover(move |d| d.bg(c.hover))
                    .on_click(cx.listener(move |this, _, window, cx| this.open_task_card(&click, window, cx)))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(div().text_size(px(11.5)).font_weight(FontWeight::SEMIBOLD).text_color(c.muted).child(t.title()))
                            .child(pill(t.stage.label(), color))
                            .child(div().flex_1())
                            .child(
                                div()
                                    .id(SharedString::from(format!("home-task-close-{}", t.id)))
                                    .invisible()
                                    .group_hover("home-task", |d| d.visible())
                                    .rounded_sm()
                                    .p_0p5()
                                    .text_color(c.subtle)
                                    .hover(move |d| d.bg(c.hover).text_color(c.fg))
                                    .tooltip(move |window, cx| Tooltip::new(if done { "Remove the card" } else { "Close the task (its agents stay)" }).build(window, cx))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        if done {
                                            this.home.tasks.retain(|x| x.id != close);
                                            this.save_home(cx);
                                        } else {
                                            this.close_task(&close, Stage::Cancelled, cx);
                                        }
                                        cx.notify();
                                    }))
                                    .child(Icon::new(IconName::Close).size(px(12.0))),
                            ),
                    )
                    .child(div().text_size(px(12.5)).text_color(c.fg).line_clamp(2).child(t.goal.clone()))
                    .child(div().flex().items_center().gap_1p5().text_size(px(11.0)).text_color(c.subtle).child(monogram_tile(t.workspace.rsplit('/').next().unwrap_or(&t.workspace)).size(px(16.0)).text_size(px(9.0))).child(t.workspace.clone()).child("·").child(how))
                    .when(!t.summary.is_empty(), |d| d.child(div().text_size(px(11.5)).text_color(c.muted).line_clamp(2).child(t.summary.clone())))
                    .children(step.map(|s| div().flex().items_center().gap_1().text_size(px(11.0)).text_color(c.accent).child(div().size(px(5.0)).rounded_full().bg(c.accent)).child(div().truncate().child(s)))),
            );
        }
        sec
    }

    /// A winfer:// link in a Home chat: "task/3", "pane/api/2", "workspace/api".
    fn open_home_link(&mut self, target: &str, window: &mut Window, cx: &mut Context<Self>) {
        let target = crate::ui::viewer::percent_decode(target.trim_end_matches('/'));
        if let Some(n) = target.strip_prefix("task/") {
            match super::home::find_task(&self.home.tasks, &serde_json::json!(n)).cloned() {
                Some(t) => self.open_task_card(&t, window, cx),
                None => self.toast(format!("No task {n} anymore."), cx),
            }
        } else if let Some(rest) = target.strip_prefix("pane/") {
            let found = rest.rsplit_once('/').and_then(|(ws, n)| {
                let s = self.state.read(cx);
                let w = s.workspace_named(ws)?;
                let i: usize = n.parse().ok()?;
                super::home::numbered(w).get(i.checked_sub(1)?).cloned()
            });
            match found {
                Some(id) => self.home_focus_pane(&id, window, cx),
                None => self.toast(format!("No pane {rest} here."), cx),
            }
        } else if let Some(name) = target.strip_prefix("workspace/") {
            if let Some(id) = self.state.read(cx).workspace_named(name).map(|w| w.id.clone()) {
                self.execute(Command::WorkspaceSwitch(id), window, cx);
            }
        }
    }

    /// A task card was clicked: a fork's thread opens; a light task's agent gets focus.
    fn open_task_card(&mut self, t: &HomeTask, window: &mut Window, cx: &mut Context<Self>) {
        match t.mode {
            TaskMode::Fork if t.stage.open() => {
                self.home.thread = Some(t.id.clone());
                cx.notify();
            }
            TaskMode::Light | TaskMode::Fork => {
                if let Some(p) = t.panes.iter().find(|p| self.state.read(cx).pane(p).is_some()).cloned() {
                    self.home_focus_pane(&p, window, cx);
                } else if let Some(ws) = t.workspace_id.clone() {
                    self.update_state(cx, |s| s.set_view(View::Workspace));
                    self.execute(Command::WorkspaceSwitch(ws), window, cx);
                }
            }
            TaskMode::Remote => {
                // Its orchestrator's pane, shown here as a tile of that machine's workspace.
                let (Some(peer), Some(pane)) = (t.machine.clone(), t.remote_pane.clone()) else { return };
                if let Some(local) = self.state.read(cx).workspaces.iter().flat_map(|w| &w.panes).find(|p| p.remote.as_ref().is_some_and(|r| r.peer == peer && r.pane == pane)).map(|p| p.id.clone()) {
                    self.home_focus_pane(&local, window, cx);
                } else {
                    self.toast("Open that machine's workspace from the sidebar to see this task's orchestrator.", cx);
                }
            }
        }
    }

    fn home_reviews(&mut self, c: Colors, cx: &mut Context<Self>) -> Option<Div> {
        let open: Vec<(String, String, String)> = {
            let s = self.state.read(cx);
            self.review
                .reviews
                .iter()
                .filter(|r| r.status == ReviewStatus::Open)
                .map(|r| {
                    let place = match &r.remote {
                        Some((peer, _)) => crate::connect::peer_name(peer).unwrap_or_else(|| "server".into()),
                        None => s.workspaces.iter().find(|w| w.id == r.workspace_id).map_or_else(String::new, |w| w.name.clone()),
                    };
                    let what = r.title.clone().unwrap_or_else(|| s.provider_name(&r.provider));
                    let branch = r.branch.clone().unwrap_or_default();
                    (place, what, branch)
                })
                .collect()
        };
        if open.is_empty() {
            return None;
        }
        let mut sec = div().flex().flex_col().gap_1().child(section(c, "Ready to review", Some(open.len())));
        for (i, (place, what, branch)) in open.into_iter().take(8).enumerate() {
            sec = sec.child(
                div()
                    .id(SharedString::from(format!("home-review-{i}")))
                    .flex()
                    .items_center()
                    .gap_2()
                    .rounded_md()
                    .px_2()
                    .py(px(6.0))
                    .cursor_pointer()
                    .hover(move |d| d.bg(c.hover))
                    .on_click(cx.listener(|this, _, window, cx| this.execute(Command::Overlay { overlay: Overlay::Review, provider: None }, window, cx)))
                    .child(Icon::new(IconName::GitMerge).size(px(13.0)).text_color(c.success))
                    .child(div().flex_1().min_w_0().flex().flex_col().child(div().truncate().text_size(px(12.0)).text_color(c.fg).child(format!("{place} · {what}"))).when(!branch.is_empty(), |d| d.child(div().truncate().text_size(px(11.0)).text_color(c.subtle).child(branch)))),
            );
        }
        Some(sec)
    }

    /// Every machine's workspaces and how their agents are doing.
    fn home_fleet(&mut self, c: Colors, cx: &mut Context<Self>) -> Div {
        let s = self.state.read(cx);
        let mut sec = div().flex().flex_col().gap_0p5().child(section(c, "Workspaces", None));
        let summary = |statuses: Vec<AgentStatus>| -> String {
            if statuses.is_empty() {
                return "idle".into();
            }
            let working = statuses.iter().filter(|s| **s == AgentStatus::Working).count();
            let waiting = statuses.iter().filter(|s| **s == AgentStatus::NeedsInput).count();
            let mut parts = vec![format!("{} agent{}", statuses.len(), if statuses.len() == 1 { "" } else { "s" })];
            if working > 0 {
                parts.push(format!("{working} working"));
            }
            if waiting > 0 {
                parts.push(format!("{waiting} waiting"));
            }
            parts.join(" · ")
        };
        let me = crate::connect::get().map(|c| c.status().name).filter(|n| !n.is_empty()).unwrap_or_else(|| "This machine".into());
        sec = sec.child(div().flex().items_center().gap_1p5().px_1().pt_1().text_size(px(11.0)).text_color(c.muted).child(Icon::new(IconName::Laptop).size(px(12.0))).child(me));
        let workspaces: Vec<(String, String, String)> = s.workspaces.iter().filter(|w| w.remote.is_none()).map(|w| (w.id.clone(), w.name.clone(), summary(w.panes.iter().filter(|p| !p.archived && p.provider != "shell").map(|p| p.status).collect()))).collect();
        for (id, name, line) in workspaces {
            sec = sec.child(
                div()
                    .id(SharedString::from(format!("home-ws-{id}")))
                    .flex()
                    .items_center()
                    .gap_2()
                    .rounded_md()
                    .px_2()
                    .py(px(5.0))
                    .cursor_pointer()
                    .hover(move |d| d.bg(c.hover))
                    .tooltip(|window, cx| Tooltip::new("Open this workspace").build(window, cx))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.update_state(cx, |s| s.set_view(View::Workspace));
                        this.execute(Command::WorkspaceSwitch(id.clone()), window, cx);
                    }))
                    .child(monogram_tile(&name))
                    .child(div().flex_1().min_w_0().truncate().text_size(px(12.5)).text_color(c.fg).child(name))
                    .child(div().flex_shrink_0().text_size(px(11.0)).text_color(c.subtle).child(line)),
            );
        }
        if let Some(connect) = crate::connect::get() {
            for p in connect.peers() {
                let up = connect.is_up(&p.peer_id);
                sec = sec.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1p5()
                        .px_1()
                        .pt_2()
                        .text_size(px(11.0))
                        .text_color(c.muted)
                        .child(Icon::new(if p.kind == winfer_connect::pairing::Kind::Server { IconName::Server } else { IconName::Monitor }).size(px(12.0)))
                        .child(p.name.clone())
                        .child(div().size(px(6.0)).rounded_full().bg(if up { c.success } else { c.subtle })),
                );
                let Some(m) = self.connect.mirrors.get(&p.peer_id) else { continue };
                for (id, name) in &m.workspaces {
                    let line = summary(m.panes.iter().filter(|x| x.workspace_id == *id && !x.archived && x.provider != "shell").map(|x| x.status).collect());
                    sec = sec.child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .px_2()
                            .py(px(5.0))
                            .when(!up, |d| d.opacity(0.6))
                            .child(monogram_tile(name))
                            .child(div().flex_1().min_w_0().truncate().text_size(px(12.5)).text_color(c.fg).child(name.clone()))
                            .child(div().flex_shrink_0().text_size(px(11.0)).text_color(c.subtle).child(line)),
                    );
                }
            }
        }
        sec
    }

    /// What's left of each signed-in account's usage.
    fn home_usage(&mut self, c: Colors, cx: &mut Context<Self>) -> Option<Div> {
        let rows = self.usage_left_rows(cx);
        if rows.is_empty() {
            return None;
        }
        let mut sec = div().flex().flex_col().gap_1p5().child(section(c, "Usage left", None));
        for (label, left) in rows {
            let color = if left < 15.0 { c.danger } else if left < 40.0 { c.warning } else { c.success };
            sec = sec.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .px_1()
                    .child(div().flex().justify_between().text_size(px(11.5)).child(div().truncate().text_color(c.muted).child(label)).child(div().text_color(c.fg).child(format!("{:.0}%", left.round()))))
                    .child(div().h(px(4.0)).w_full().rounded_full().bg(c.hover).child(div().h_full().rounded_full().bg(color).w(relative((left / 100.0).clamp(0.0, 1.0) as f32)))),
            );
        }
        Some(sec)
    }

    /// Home's entry in the sidebar.
    pub(super) fn render_home_nav(&mut self, rail: bool, c: Colors, cx: &mut Context<Self>) -> Stateful<Div> {
        let on = self.state.read(cx).view() == View::Home;
        let waiting = self.home.tasks.iter().filter(|t| t.stage == Stage::NeedsYou).count();
        let busy = self.orchestrator_step(cx).is_some();
        let keys = super::commands::keys_for(&Command::View(ViewTarget::Set(View::Home)), self.state.read(cx));
        let tip = keys.map_or("Home: your orchestrator for every workspace".to_string(), |k| format!("Home: your orchestrator for every workspace ({k})"));
        let base = div()
            .id(if rail { "rail-home" } else { "nav-home" })
            .relative()
            .flex()
            .items_center()
            .cursor_pointer()
            .rounded(px(8.0))
            .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
            .on_click(cx.listener(|this, _, window, cx| this.execute(Command::View(ViewTarget::Set(View::Home)), window, cx)))
            .when(on, |d| d.bg(c.active).text_color(c.fg))
            .when(!on, |d| d.text_color(c.muted).hover(move |d| d.bg(c.hover).text_color(c.fg)));
        let dot = (busy || waiting > 0).then(|| div().size(px(6.0)).rounded_full().bg(if waiting > 0 { c.warning } else { c.success }));
        if rail {
            return base.size(px(36.0)).justify_center().child(Icon::new(IconName::LayoutDashboard).size(px(15.0))).children(dot.map(|d| d.absolute().top(px(5.0)).right(px(5.0))));
        }
        base.gap_2()
            .h(px(32.0))
            .px_2p5()
            .text_size(px(13.0))
            .font_weight(FontWeight::MEDIUM)
            .child(Icon::new(IconName::LayoutDashboard).size(px(14.0)))
            .child(div().flex_1().child("Home"))
            .children(dot)
    }

    /// Opens Home at launch when the user wants it (Settings → Home).
    pub(super) fn home_on_launch(&mut self, cx: &mut Context<Self>) {
        let s = self.state.read(cx);
        if home_settings(s).open_on_launch && s.view() == View::Workspace {
            self.update_state(cx, |s| s.set_view(View::Home));
        }
    }
}
