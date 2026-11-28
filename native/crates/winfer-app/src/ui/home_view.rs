//! The Home view: the conductor's chat (or a forked task's) in the middle, and a live rail with
//! what needs the user, the tasks, the reviews, every machine's workspaces and usage left.
//! The rail is built from app state; no model is involved.

use super::commands::{Command, PaneRef, ViewTarget};
use super::home::{HomeTask, Stage, TaskMode, home_settings};
use super::home_nav::{self, Refs, Who};
use super::menu::MenuItem;
use super::sidebar::{card_bg, monogram_tile};
use super::*;
use crate::model::AgentStatus;
use crate::voice::{CONDUCTOR, task_pane};
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

/// Something waiting for the user in a task: (task id, the agent pane to open, what it asks).
type Waiting = (String, Option<String>, String);

/// An agent's status dot.
fn status_color(status: AgentStatus, c: &Colors) -> Hsla {
    match status {
        AgentStatus::Working => c.success,
        AgentStatus::NeedsInput => c.warning,
        AgentStatus::Error => c.danger,
        AgentStatus::Done => c.subtle,
        AgentStatus::Idle => c.border_strong,
    }
}

/// A paired machine's name, by its peer id.
fn machine_name(peer: &str) -> Option<String> {
    crate::connect::peer_name(peer).or_else(|| (cfg!(debug_assertions) && peer == DEMO_PEER).then(|| "mac-studio".to_string()))
}

/// The machine a remote task of the Home demo runs on (`WINFER_NATIVE_HOME_DEMO`).
const DEMO_PEER: &str = "demo-peer";

/// A paired machine's name with a server icon (task cards, a remote task's chat).
fn machine_chip(name: &str, c: Colors) -> Div {
    div().flex_shrink_0().flex().items_center().gap_1().px_1p5().h(px(18.0)).rounded(px(5.0)).bg(Hsla { a: 0.14, ..c.accent }).text_color(c.accent).text_size(px(10.5)).font_weight(FontWeight::MEDIUM)
        .child(Icon::new(IconName::Server).size(px(10.5)))
        .child(name.to_string())
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

    /// Home's task numbers and paired machines, which its chats turn into links and chips.
    fn home_refs(&self) -> Refs {
        let mut tasks: Vec<u32> = self.home.tasks.iter().map(|t| t.number).collect();
        tasks.sort_unstable();
        tasks.dedup();
        let mut machines: Vec<String> = crate::connect::get().map(|c| c.peers().into_iter().map(|p| p.name).filter(|n| !n.trim().is_empty()).collect()).unwrap_or_default();
        if cfg!(debug_assertions) && self.home.tasks.iter().any(|t| t.machine.as_deref() == Some(DEMO_PEER)) {
            machines.push("mac-studio".into());
        }
        Refs { tasks, machines }
    }

    fn render_home_chat(&mut self, c: Colors, window: &mut Window, cx: &mut Context<Self>) -> Div {
        if let Some(t) = self.home.thread.clone()
            && self.task(&t).is_none()
        {
            self.home.thread = None;
        }
        // The conductor runs whenever Home shows: its latest reply is quoted above a task's chat.
        let started = self.conductor(cx).map(|_| ());
        let refs = self.home_refs();
        for v in self.home.views.values() {
            v.update(cx, |v, cx| v.set_refs(refs.clone(), cx));
        }
        let waiting = self.home_waiting(cx);
        let panel = |d: Div| d.flex_1().min_w_0().h_full().flex().flex_col().overflow_hidden().rounded_2xl().border_1().border_color(c.edge).bg(c.panel);
        if let Some(task_id) = self.home.thread.clone()
            && let Some(t) = self.task(&task_id).cloned()
        {
            let waiting: Vec<_> = waiting.into_iter().filter(|w| w.0 != task_id).collect();
            return panel(div()).children(self.render_task_chat(&t, &waiting, c, window, cx));
        }

        let body: AnyElement = match started.and_then(|_| self.home_view_for(CONDUCTOR, window, cx).ok_or_else(|| "Couldn't open the chat.".to_string())) {
            Ok(view) => div().flex_1().min_h_0().child(view).into_any_element(),
            Err(e) => div().flex_1().flex().items_center().justify_center().p_6().text_size(px(13.0)).text_color(c.muted).child(e).into_any_element(),
        };
        let step = self.orchestrator_step(cx);
        // The conductor's running subagents and background shells (e.g. a release it watches), as on agent panes.
        let bg_chip = self.render_bg_chip(CONDUCTOR, c, cx);
        let title = div().flex().items_center().gap_1p5().h(px(26.0)).px_2p5().rounded(px(7.0)).text_size(px(12.0)).bg(c.active).text_color(c.fg).font_weight(FontWeight::MEDIUM).child(Icon::new(IconName::LayoutDashboard).size(px(12.0))).child("Home");
        let header = div()
            .flex()
            .items_center()
            .gap_2()
            .h(px(40.0))
            .flex_shrink_0()
            .px_2()
            .border_b_1()
            .border_color(c.edge)
            .child(title)
            .child(div().flex_1())
            .children(step.map(|s| div().max_w(px(260.0)).truncate().text_size(px(11.5)).text_color(c.subtle).child(s)))
            .children(bg_chip)
            .child(
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
            );
        panel(div()).child(header).children(self.render_waiting(&waiting, c, cx)).child(body)
    }

    /// What a task's chat shows: its picked agent, else its main chat. A remote task's
    /// orchestrator gets a tile here (as the sidebar opens one), streamed like any.
    fn home_who(&mut self, t: &HomeTask, cx: &mut Context<Self>) -> Who {
        let who = {
            let s = self.state.read(cx);
            let remote = t.machine.clone().and_then(|peer| {
                let pane = t.remote_pane.clone().or_else(|| self.connect.mirrors.get(&peer)?.panes.iter().find(|p| t.orchestrated_by(p)).map(|p| p.id.clone()))?;
                Some((peer, pane))
            });
            let tile = remote.as_ref().and_then(|(peer, pane)| s.all_panes().find(|p| p.remote.as_ref().is_some_and(|r| r.peer == *peer && r.pane == *pane)).map(|p| p.id.clone()));
            let picked = self.home.agent.get(&t.id).map(String::as_str);
            home_nav::who(t.mode, picked, &t.panes, |p| s.pane(p).is_some(), tile, remote)
        };
        match who {
            // The demo's remote task: a local chat stands in for that machine's tile.
            Who::Remote(peer, pane) if cfg!(debug_assertions) && peer == DEMO_PEER => if self.state.read(cx).pane(&pane).is_some() { Who::Pane(pane) } else { Who::Nothing },
            Who::Remote(peer, pane) => self.remote_tile(&peer, &pane, cx).map_or(Who::Nothing, Who::Pane),
            w => w,
        }
    }

    /// What to call an agent of a task: its pane's name, or "Orchestrator".
    fn agent_label(&self, t: &HomeTask, who: &Who, cx: &App) -> String {
        match who {
            Who::Pane(_) if t.mode == TaskMode::Remote => match t.machine.as_deref().and_then(machine_name) {
                Some(m) => format!("Orchestrator on {m}"),
                None => "Orchestrator".into(),
            },
            Who::Pane(id) => {
                let s = self.state.read(cx);
                s.pane(id).map_or_else(|| "Agent".into(), |p| s.pane_label(p))
            }
            _ => "Orchestrator".into(),
        }
    }

    /// A task's chat inside Home: breadcrumb, its agents (a fork), Home's latest reply, the chat.
    fn render_task_chat(&mut self, t: &HomeTask, waiting: &[Waiting], c: Colors, window: &mut Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let who = self.home_who(t, cx);
        let label = self.agent_label(t, &who, cx);
        let conductor_items = self.home_model(CONDUCTOR).map(|m| m.read(cx).state.items.clone()).unwrap_or_default();
        let new = home_nav::new_replies(&conductor_items, self.home.seen);
        let latest = home_nav::latest_reply(&conductor_items);
        let picked = matches!(who, Who::Pane(_)) && t.mode == TaskMode::Fork;
        let crumb = |id: &'static str| div().id(id).flex().items_center().gap_1p5().h(px(26.0)).px_2().rounded(px(7.0)).text_size(px(12.5)).min_w_0().cursor_pointer().text_color(c.muted).hover(move |d| d.bg(c.hover).text_color(c.fg));
        let sep = || Icon::new(IconName::ChevronRight).size(px(11.0)).text_color(c.subtle);
        let tip = super::commands::keys_for(&Command::View(ViewTarget::Set(View::Home)), self.state.read(cx)).map_or("Back to Home (Esc, ⌘[)".to_string(), |k| format!("Back to Home (Esc, ⌘[, {k})"));
        let home_crumb = crumb("home-crumb-home")
            .flex_shrink_0()
            .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
            .on_click(cx.listener(|this, _, _, cx| this.home_back(cx)))
            .child(Icon::new(IconName::ChevronLeft).size(px(13.0)))
            .child(Icon::new(IconName::LayoutDashboard).size(px(12.0)))
            .child("Home")
            .when(new > 0, |d| d.child(div().px_1p5().rounded(px(5.0)).bg(Hsla { a: 0.16, ..c.accent }).text_color(c.accent).text_size(px(10.5)).child(format!("{new} new"))));
        let task_id = t.id.clone();
        let task_crumb = crumb("home-crumb-task")
            .when(!picked, |d| d.text_color(c.fg).font_weight(FontWeight::MEDIUM))
            .on_click(cx.listener(move |this, _, _, cx| this.home_drill(&task_id, Some(None), cx)))
            .child(div().flex_shrink_0().child(t.title()))
            .child(div().min_w_0().max_w(px(320.0)).truncate().text_color(if picked { c.subtle } else { c.muted }).font_weight(FontWeight::NORMAL).child(t.goal.clone()));
        let crumbs = div().flex().items_center().gap_0p5().min_w_0().overflow_hidden().child(home_crumb).child(sep()).child(task_crumb).when(picked, |d| {
            let status = match &who {
                Who::Pane(id) => self.state.read(cx).pane(id).map(|p| p.status).unwrap_or_default(),
                _ => AgentStatus::Idle,
            };
            d.child(sep()).child(div().flex().items_center().gap_1p5().px_2().flex_shrink_0().text_size(px(12.5)).text_color(c.fg).font_weight(FontWeight::MEDIUM).child(div().size(px(6.0)).rounded_full().bg(status_color(status, &c))).child(label.clone()))
        });

        let shown = match &who {
            Who::Pane(id) => Some(id.clone()),
            _ => None,
        };
        let step = match &who {
            Who::Orchestrator => self.fork_step(&t.id, cx),
            _ => None,
        };
        let bg_chip = self.render_bg_chip(shown.as_deref().map_or_else(|| task_pane(&t.id), str::to_string).as_str(), c, cx);
        let (pane_btn, menu_btn) = (shown.clone(), t.clone());
        let ghost = |id: &'static str| div().id(id).flex().items_center().gap_1().h(px(24.0)).px_2().rounded_md().text_size(px(11.5)).text_color(c.muted).cursor_pointer().hover(move |d| d.bg(c.hover).text_color(c.fg));
        let header = div()
            .flex()
            .items_center()
            .gap_2()
            .h(px(40.0))
            .flex_shrink_0()
            .px_2()
            .border_b_1()
            .border_color(c.edge)
            .child(crumbs)
            .child(div().flex_1())
            .children(step.map(|s| div().max_w(px(220.0)).truncate().text_size(px(11.5)).text_color(c.subtle).child(s)))
            .children(bg_chip)
            .child(pill(t.stage.label(), stage_color(t.stage, &c)))
            .children(t.machine.as_deref().and_then(machine_name).map(|m| machine_chip(&m, c)))
            .child(
                ghost("home-task-pane")
                    .tooltip(|window, cx| Tooltip::new("Leave Home and show this agent's pane").build(window, cx))
                    .on_click(cx.listener(move |this, _, window, cx| match &pane_btn {
                        Some(p) => this.home_focus_pane(p, window, cx),
                        None => this.home_show_panes(&menu_btn.clone(), window, cx),
                    }))
                    .child(Icon::new(IconName::ExternalLink).size(px(12.0)))
                    .child("Pane"),
            )
            .child({
                let (t2, shown2) = (t.clone(), shown.clone());
                ghost("home-task-more").px_1().tooltip(|window, cx| Tooltip::new("More").build(window, cx)).on_click(cx.listener(move |this, _, window, cx| {
                    let items = this.task_menu(&t2, shown2.as_deref(), cx);
                    let at = window.mouse_position();
                    this.open_menu(at, None, items, cx);
                }))
                .child(Icon::new(IconName::Ellipsis).size(px(14.0)))
            });

        let mut out: Vec<AnyElement> = vec![header.into_any_element()];
        if t.mode == TaskMode::Fork {
            out.push(self.render_agent_chips(t, &who, c, cx).into_any_element());
        }
        if let Some(text) = latest {
            out.push(
                div()
                    .id("home-strip")
                    .flex_shrink_0()
                    .mx_3()
                    .mt_2()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_2p5()
                    .py_1p5()
                    .rounded_lg()
                    .border_1()
                    .border_color(c.edge)
                    .text_size(px(12.0))
                    .text_color(c.muted)
                    .cursor_pointer()
                    .hover(move |d| d.bg(c.hover))
                    .on_click(cx.listener(|this, _, _, cx| this.home_back(cx)))
                    .child(Icon::new(IconName::LayoutDashboard).size(px(12.0)).text_color(c.subtle))
                    .child(div().flex_shrink_0().text_color(c.fg).font_weight(FontWeight::MEDIUM).child("Home:"))
                    .child(div().flex_1().min_w_0().truncate().child(text))
                    .child(div().flex_shrink_0().text_size(px(11.0)).text_color(c.subtle).child("Esc to go back"))
                    .into_any_element(),
            );
        }
        out.extend(self.render_waiting(waiting, c, cx).map(IntoElement::into_any_element));
        out.push(self.render_task_body(t, &who, &label, c, window, cx));
        out
    }

    /// A fork's orchestrator and agents, to switch between.
    fn render_agent_chips(&self, t: &HomeTask, who: &Who, c: Colors, cx: &mut Context<Self>) -> Div {
        let s = self.state.read(cx);
        let chip = |id: SharedString, on: bool| {
            div().id(id).flex().items_center().gap_1p5().h(px(26.0)).px_2p5().rounded(px(7.0)).text_size(px(12.0)).cursor_pointer().border_1()
                .when(on, |d| d.bg(c.active).border_color(c.edge).text_color(c.fg))
                .when(!on, |d| d.border_color(gpui::transparent_black()).text_color(c.muted).hover(move |d| d.bg(c.hover).text_color(c.fg)))
        };
        let busy = self.fork_step(&t.id, cx).is_some();
        let needs = self.home_model(&task_pane(&t.id)).is_some_and(|m| !m.read(cx).state.approvals.is_empty());
        let orch_color = if needs { c.warning } else if busy { c.success } else { c.subtle };
        let task_id = t.id.clone();
        let mut row = div().flex().flex_wrap().items_center().gap_1().flex_shrink_0().px_2().py_1p5().border_b_1().border_color(c.edge)
            .child(div().px_1().mr_1().text_size(px(10.5)).font_weight(FontWeight::SEMIBOLD).text_color(c.subtle).child("AGENTS"))
            .child(chip("home-agent-orch".into(), *who == Who::Orchestrator)
                .on_click(cx.listener(move |this, _, _, cx| this.home_drill(&task_id, Some(None), cx)))
                .child(div().size(px(6.0)).rounded_full().bg(orch_color))
                .child("Orchestrator"));
        for p in t.panes.iter().filter_map(|p| s.pane(p)) {
            let on = *who == Who::Pane(p.id.clone());
            let (task_id, pane_id) = (t.id.clone(), p.id.clone());
            let approvals = self.code_models.get(&p.id).is_some_and(|m| !m.read(cx).state.approvals.is_empty());
            let status = if approvals { AgentStatus::NeedsInput } else { p.status };
            row = row.child(chip(SharedString::from(format!("home-agent-{}", p.id)), on)
                .on_click(cx.listener(move |this, _, _, cx| this.home_drill(&task_id, Some(Some(pane_id.clone())), cx)))
                .child(div().size(px(6.0)).rounded_full().bg(status_color(status, &c)))
                .child(div().max_w(px(180.0)).truncate().child(s.pane_label(p))));
        }
        row
    }

    fn render_task_body(&mut self, t: &HomeTask, who: &Who, label: &str, c: Colors, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let hint = format!("Message {label} ({})…", t.title());
        let note = |text: String| div().flex_1().flex().flex_col().items_center().justify_center().gap_3().p_6().text_size(px(13.0)).text_color(c.muted).child(text);
        match who {
            Who::Orchestrator => {
                if self.home_model(&task_pane(&t.id)).is_none() && !t.stage.open() {
                    let id = t.id.clone();
                    return note(format!("{} is closed; its orchestrator isn't running.", t.title()))
                        .child(div().id("home-resume").h(px(28.0)).px_3().flex().items_center().rounded_lg().border_1().border_color(c.edge).text_size(px(12.5)).text_color(c.fg).cursor_pointer().hover(move |d| d.bg(c.hover))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Err(e) = this.open_fork(&id, cx) {
                                    this.toast(e, cx);
                                }
                                cx.notify();
                            }))
                            .child("Resume its orchestrator"))
                        .into_any_element();
                }
                let pane = task_pane(&t.id);
                match self.home_fork_model(&t.id, cx).and_then(|_| self.home_view_for(&pane, window, cx).ok_or_else(|| "Couldn't open the chat.".to_string())) {
                    Ok(view) => {
                        view.update(cx, |v, cx| v.set_placeholder(&hint, window, cx));
                        div().flex_1().min_h_0().child(view).into_any_element()
                    }
                    Err(e) => note(e).into_any_element(),
                }
            }
            Who::Pane(id) => {
                let provider = self.state.read(cx).pane(id).map(|p| p.provider.clone()).unwrap_or_default();
                if self.uses_code(&provider, id, cx)
                    && let Some(view) = self.code_view(id, window, cx)
                {
                    view.update(cx, |v, cx| v.set_placeholder(&hint, window, cx));
                    return div().flex_1().min_h_0().child(view).into_any_element();
                }
                // An agent CLI: its terminal, answered right here.
                div().flex_1().min_h_0().p_2().child(self.terminal(id, cx)).into_any_element()
            }
            Who::Remote(..) | Who::Nothing => {
                let why = match t.mode {
                    TaskMode::Remote => format!("{}'s orchestrator isn't reachable: its machine is offline or the pane closed there.", t.title()),
                    _ => format!("No agent of {} is open anymore.", t.title()),
                };
                note(why).into_any_element()
            }
        }
    }

    /// The ⋯ menu of a task's chat: the pane, the worktree, all the task's panes, closing it.
    fn task_menu(&self, t: &HomeTask, shown: Option<&str>, cx: &App) -> Vec<MenuItem> {
        let s = self.state.read(cx);
        let mut items = Vec::new();
        let pane = shown.and_then(|p| s.pane(p));
        if let Some(p) = pane {
            let id = p.id.clone();
            items.push(MenuItem::run(format!("Open {}'s pane", s.pane_label(p)), move |this, window, cx| this.home_focus_pane(&id, window, cx)));
        }
        let worktree = pane.and_then(|p| p.worktree.as_ref().map(|w| w.path.clone())).or_else(|| t.worktrees.iter().find(|w| std::path::Path::new(w).exists()).cloned());
        if let Some(p) = pane.filter(|p| p.worktree.is_some()) {
            items.push(MenuItem::command("Show changes", Command::ShowChanges(PaneRef::Id(p.id.clone()))));
        }
        if let Some(path) = worktree {
            items.push(MenuItem::run(if cfg!(target_os = "macos") { "Open worktree in Finder" } else { "Open worktree in Explorer" }, move |_, _, _| super::sidebar::reveal_path(&path)));
        }
        let all = t.clone();
        let n = t.panes.iter().filter(|p| s.pane(p).is_some()).count();
        items.push(MenuItem::run(if n > 1 { format!("Show all {n} panes") } else { "Show in its workspace".to_string() }, move |this, window, cx| this.home_show_panes(&all, window, cx)));
        if t.stage.open() {
            let id = t.id.clone();
            items.push(MenuItem::Separator);
            items.push(MenuItem::run("Close task (its agents stay)", move |this, _, cx| this.close_task(&id, Stage::Cancelled, cx)).danger());
        }
        items
    }

    /// Leaves Home for the task's workspace (or its machine's tile), where all its panes are.
    fn home_show_panes(&mut self, t: &HomeTask, window: &mut Window, cx: &mut Context<Self>) {
        match (t.mode, t.machine.clone()) {
            (TaskMode::Remote, Some(_)) => {
                if let Who::Pane(id) = self.home_who(t, cx) {
                    self.home_focus_pane(&id, window, cx);
                }
            }
            _ => match t.panes.iter().find(|p| self.state.read(cx).pane(p).is_some()).cloned() {
                Some(p) => self.home_focus_pane(&p, window, cx),
                None => {
                    if let Some(ws) = t.workspace_id.clone() {
                        self.update_state(cx, |s| s.set_view(View::Workspace));
                        self.execute(Command::WorkspaceSwitch(ws), window, cx);
                    }
                }
            },
        }
    }

    /// Opens a task's chat in Home. `pick`: None keeps the agent picked before; Some(None) its
    /// main chat; Some(Some(pane)) that agent.
    pub(super) fn home_drill(&mut self, task_id: &str, pick: Option<Option<String>>, cx: &mut Context<Self>) {
        if self.home.thread.is_none() {
            self.home.seen = self.home_model(CONDUCTOR).map_or(0, |m| m.read(cx).state.items.len());
        }
        self.home.thread = Some(task_id.to_string());
        match pick {
            Some(Some(p)) => {
                self.home.agent.insert(task_id.to_string(), p);
            }
            Some(None) => {
                self.home.agent.remove(task_id);
            }
            None => {}
        }
        if self.state.read(cx).view() != View::Home {
            self.update_state(cx, |s| s.set_view(View::Home));
        }
        cx.notify();
    }

    /// Back from a task's chat to Home's.
    pub(super) fn home_back(&mut self, cx: &mut Context<Self>) {
        if self.home.thread.take().is_some() {
            cx.notify();
        }
    }

    /// Esc or ⌘[ in a task's chat: back to Home's. Esc only when the chat wouldn't use it (nothing
    /// typed, not working, nothing asked; a terminal agent keeps Esc).
    pub(super) fn home_back_key(&mut self, e: &KeyDownEvent, cx: &mut Context<Self>) -> bool {
        let k = &e.keystroke;
        if self.home.thread.is_none() || self.state.read(cx).view() != View::Home || self.menu.is_some() {
            return false;
        }
        let cmd_bracket = k.key == "[" && k.modifiers.platform && !k.modifiers.shift && !k.modifiers.alt && !k.modifiers.control;
        let esc = k.key == "escape" && !k.modifiers.modified();
        if !cmd_bracket && !esc {
            return false;
        }
        if esc && !self.home_chat_idle(cx) {
            return false;
        }
        self.home_back(cx);
        true
    }

    /// The task chat on screen is idle: a chat with nothing typed, working or asked.
    fn home_chat_idle(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(t) = self.home.thread.clone().and_then(|t| self.task(&t).cloned()) else { return false };
        let (model, view) = match self.home_who(&t, cx) {
            Who::Orchestrator => (self.home_model(&task_pane(&t.id)), self.home.views.get(&task_pane(&t.id)).cloned()),
            Who::Pane(id) => match self.code_models.get(&id) {
                Some(m) => (Some(m.clone()), self.code_views.get(&id).cloned()),
                None => return false,
            },
            _ => return true,
        };
        let quiet = model.is_none_or(|m| {
            let st = &m.read(cx).state;
            !st.busy && st.approvals.is_empty() && st.sheet.is_none()
        });
        quiet && view.is_none_or(|v| v.read(cx).composer_empty(cx))
    }

    /// What waits for the user in each task: approvals and questions of its agents and
    /// orchestrator, agents needing input, or the task's own "needs you".
    fn home_waiting(&self, cx: &App) -> Vec<Waiting> {
        let s = self.state.read(cx);
        let ask = |a: &crate::code::protocol::Approval| match a.questions.first() {
            Some(q) => if q.question.is_empty() { q.header.clone() } else { q.question.clone() },
            None => format!("wants to run {}", if a.title.is_empty() { &a.tool } else { &a.title }),
        };
        let mut out = Vec::new();
        for t in super::home::listed_tasks(&self.home.tasks).into_iter().filter(|t| t.stage.open()) {
            let before = out.len();
            if let Some(a) = self.home_model(&task_pane(&t.id)).and_then(|m| m.read(cx).state.approvals.first().map(ask)) {
                out.push((t.id.clone(), None, format!("{} · Orchestrator: {a}", t.title())));
            }
            for p in t.panes.iter().filter_map(|p| s.pane(p)) {
                let name = s.pane_label(p);
                match self.code_models.get(&p.id).and_then(|m| m.read(cx).state.approvals.first().map(ask)) {
                    Some(a) => out.push((t.id.clone(), Some(p.id.clone()), format!("{} · {name}: {a}", t.title()))),
                    None if p.status == AgentStatus::NeedsInput => out.push((t.id.clone(), Some(p.id.clone()), format!("{} · {name} needs input", t.title()))),
                    None => {}
                }
            }
            if t.mode == TaskMode::Remote
                && let (Some(peer), Some(pane)) = (&t.machine, &t.remote_pane)
                && let Some(tile) = s.all_panes().find(|p| p.remote.as_ref().is_some_and(|r| r.peer == *peer && r.pane == *pane))
            {
                match self.code_models.get(&tile.id).and_then(|m| m.read(cx).state.approvals.first().map(ask)) {
                    Some(a) => out.push((t.id.clone(), None, format!("{}: {a}", t.title()))),
                    None if tile.status == AgentStatus::NeedsInput => out.push((t.id.clone(), None, format!("{} needs input", t.title()))),
                    None => {}
                }
            }
            if out.len() == before && t.stage == Stage::NeedsYou {
                let why = if t.summary.is_empty() { "needs you".to_string() } else { t.summary.clone() };
                out.push((t.id.clone(), None, format!("{}: {why}", t.title())));
            }
        }
        out
    }

    /// The "needs you" banner: what waits in the tasks, each a click away.
    fn render_waiting(&self, waiting: &[Waiting], c: Colors, cx: &mut Context<Self>) -> Option<Div> {
        if waiting.is_empty() {
            return None;
        }
        let mut row = div().flex_1().min_w_0().flex().flex_wrap().items_center().gap_1p5();
        for (i, (task, agent, text)) in waiting.iter().take(4).enumerate() {
            let (task, agent) = (task.clone(), agent.clone());
            row = row.child(
                div()
                    .id(SharedString::from(format!("home-wait-{i}")))
                    .max_w(px(360.0))
                    .truncate()
                    .h(px(22.0))
                    .px_2()
                    .flex()
                    .items_center()
                    .rounded_md()
                    .bg(Hsla { a: 0.10, ..c.warning })
                    .text_color(c.fg)
                    .cursor_pointer()
                    .hover(move |d| d.bg(Hsla { a: 0.2, ..c.warning }))
                    .on_click(cx.listener(move |this, _, _, cx| this.home_drill(&task, Some(agent.clone()), cx)))
                    .child(text.clone()),
            );
        }
        if waiting.len() > 4 {
            row = row.child(div().text_color(c.subtle).child(format!("+{} more", waiting.len() - 4)));
        }
        Some(
            div()
                .flex_shrink_0()
                .mx_3()
                .mt_2()
                .flex()
                .items_center()
                .gap_2()
                .px_2p5()
                .py_1p5()
                .rounded_lg()
                .border_1()
                .border_color(Hsla { a: 0.35, ..c.warning })
                .bg(Hsla { a: 0.06, ..c.warning })
                .text_size(px(12.0))
                .child(Icon::new(IconName::ShieldQuestionMark).size(px(14.0)).text_color(c.warning))
                .child(div().flex_shrink_0().font_weight(FontWeight::SEMIBOLD).text_color(c.warning).child(format!("{} waiting", waiting.len())))
                .child(row),
        )
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
        rail = rail.child(self.home_tasks(c, cx));
        div().w(px(RAIL_WIDTH)).h_full().flex_shrink_0().flex().flex_col().overflow_hidden().rounded_2xl().border_1().border_color(c.edge).bg(c.panel).child(rail)
    }

    /// Focuses an agent pane in its workspace (leaving Home).
    pub(super) fn home_focus_pane(&mut self, pane_id: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.update_state(cx, |s| s.set_view(View::Workspace));
        self.execute(Command::Focus(PaneRef::Id(pane_id.to_string())), window, cx);
    }

    fn home_tasks(&mut self, c: Colors, cx: &mut Context<Self>) -> Div {
        let tasks: Vec<HomeTask> = super::home::listed_tasks(&self.home.tasks).into_iter().cloned().collect();
        let open = self.home.tasks.iter().filter(|t| t.stage.open()).count();
        let mut sec = div().flex().flex_col().gap_1p5().child(section(c, "Tasks", Some(open)));
        if tasks.is_empty() {
            return sec.child(div().px_1().text_size(px(12.0)).text_color(c.subtle).child("Ask Home for something to build: it becomes a task here."));
        }
        for t in tasks {
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
                            .children(t.machine.as_deref().and_then(machine_name).map(|m| machine_chip(&m, c)))
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
                    .child(div().flex().items_center().gap_1p5().text_size(px(11.0)).text_color(c.subtle).child(monogram_tile(t.workspace.rsplit('/').next().unwrap_or(&t.workspace)).size(px(16.0)).text_size(px(9.0))).child(t.workspace.clone()).child("·").child(how)
                        .children(t.model.as_deref().map(|m| {
                            let name = crate::code::names::model_name(m, None);
                            let name = match &t.effort { Some(e) => format!("{name} · {e}"), None => name };
                            let tip = if t.model_note.is_empty() { name.clone() } else { format!("{name}: {}", t.model_note) };
                            div().id(SharedString::from(format!("home-task-model-{}", t.id))).flex().items_center().gap_1p5().child("·").child(div().text_color(c.muted).child(name))
                                .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
                        })))
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
        } else if let Some(name) = target.strip_prefix("machine/") {
            // A machine chip: its task (the one needing the user first), else its workspaces.
            let peer = crate::connect::get().and_then(|c| c.peers().into_iter().find(|p| p.name == name)).map(|p| p.peer_id);
            let task = peer.as_ref().and_then(|peer| {
                let mut on: Vec<&HomeTask> = super::home::listed_tasks(&self.home.tasks).into_iter().filter(|t| t.machine.as_ref() == Some(peer) && t.stage.open()).collect();
                on.sort_by_key(|t| t.stage != Stage::NeedsYou);
                on.first().map(|t| t.id.clone())
            });
            match (task, peer) {
                (Some(t), _) => self.home_drill(&t, None, cx),
                (None, Some(peer)) => {
                    self.connect.expanded.insert(peer);
                    self.toast(format!("{name} has no open task; its workspaces are in the sidebar."), cx);
                }
                (None, None) => self.toast(format!("{name} isn't paired anymore."), cx),
            }
        } else if let Some(name) = target.strip_prefix("workspace/") {
            if let Some(id) = self.state.read(cx).workspace_named(name).map(|w| w.id.clone()) {
                self.execute(Command::WorkspaceSwitch(id), window, cx);
            }
        }
    }

    /// A task card (or a link to it) was clicked: its chat opens inside Home.
    fn open_task_card(&mut self, t: &HomeTask, _window: &mut Window, cx: &mut Context<Self>) {
        self.home_drill(&t.id, None, cx);
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

    /// Development aid (`WINFER_NATIVE_HOME_DEMO=1`, with `WINFER_NATIVE_CODE_DEMO=home`): Home with
    /// a light, a forked, a remote and a merged task whose chats play scripts;
    /// `WINFER_NATIVE_HOME_TASK=2` (or `2/backend`) opens that task's chat.
    #[cfg(debug_assertions)]
    pub(super) fn home_demo(&mut self, cx: &mut Context<Self>) {
        if std::env::var_os("WINFER_NATIVE_HOME_DEMO").is_none() {
            return;
        }
        self.load_home(cx);
        if self.home.tasks.is_empty() {
            let (ws_id, ws) = {
                let s = self.state.read(cx);
                (s.active_workspace_id.clone(), s.workspaces.iter().find(|w| w.id == s.active_workspace_id).map_or("web".to_string(), |w| w.name.clone()))
            };
            let add = |this: &mut Self, title: &str, cx: &mut Context<Self>| -> String {
                this.state.update(cx, |s, cx| {
                    let id = s.add_pane("winfer", crate::model::AddPane { worktree: Some(false), custom_title: Some(title.to_string()), ..Default::default() });
                    cx.notify();
                    id
                })
            };
            let light = add(self, "fix-login-flake", cx);
            let fork: Vec<String> = ["backend", "frontend", "tests"].iter().map(|t| add(self, t, cx)).collect();
            let remote = add(self, "Task 3 · mac-studio", cx);
            let now = super::home::now_secs();
            let task = |n: u32, goal: &str, mode: TaskMode, stage: Stage, panes: Vec<String>, model: &str| HomeTask {
                id: format!("t{n}-demo"), number: n, goal: goal.into(), workspace: ws.clone(), workspace_id: Some(ws_id.clone()), mode, stage, panes, model: Some(model.into()), created: now, updated: now + u64::from(10 - n), ..Default::default()
            };
            let mut r = task(3, "Bump tokio to 1.48 and fix the deprecations", TaskMode::Remote, Stage::Ready, Vec::new(), "claude/opus");
            r.machine = Some(DEMO_PEER.into());
            r.remote_pane = Some(remote);
            r.workspace = "mac-studio/core".into();
            r.workspace_id = None;
            self.home.tasks = vec![
                task(2, "Add CSV export to the reports page", TaskMode::Fork, Stage::NeedsYou, fork, "claude/opus"),
                task(1, "Fix the flaky login e2e test on CI", TaskMode::Light, Stage::Running, vec![light], "claude/sonnet"),
                r,
                HomeTask { worktrees: vec!["/tmp".into()], ..task(4, "Dark mode toggle on the settings page", TaskMode::Light, Stage::Merged, Vec::new(), "claude/sonnet") },
            ];
            // Their chats start now (as when the workspace showed them), so what they ask shows.
            for p in self.home.tasks.iter().flat_map(|t| t.panes.clone().into_iter().chain(t.remote_pane.clone())).collect::<Vec<_>>() {
                self.code_model(&p, cx);
            }
        }
        self.update_state(cx, |s| s.set_view(View::Home));
        if let Ok(target) = std::env::var("WINFER_NATIVE_HOME_TASK") {
            let (n, agent) = target.split_once('/').map_or((target.as_str(), None), |(n, a)| (n, Some(a)));
            if let Some(t) = super::home::find_task(&self.home.tasks, &serde_json::json!(n)).cloned() {
                let s = self.state.read(cx);
                let pane = agent.and_then(|a| t.panes.iter().find(|p| s.pane(p).is_some_and(|p| p.custom_title.as_deref() == Some(a))).cloned());
                self.home_drill(&t.id, Some(pane), cx);
            }
        }
    }

    /// Opens Home at launch when the user wants it (Settings → Home).
    pub(super) fn home_on_launch(&mut self, cx: &mut Context<Self>) {
        let s = self.state.read(cx);
        if home_settings(s).open_on_launch && s.view() == View::Workspace {
            self.update_state(cx, |s| s.set_view(View::Home));
        }
    }
}
