//! The sidebar (port of `src/features/workspaces/Sidebar.tsx`, `ActivityFeed.tsx`,
//! `sidebarMenus.ts` and `sidebarTooltips.ts`), Graphite layout: search and the Sessions ·
//! Review · History control, agents that need you, the workspaces with their sessions, the
//! "Latest" feed and the footer. Collapsed, it is a 60 px icon rail.

use super::commands::{Command, PaneRef, ViewTarget};
use super::menu::MenuItem;
use super::review::ReviewStatus;
use super::*;
use crate::model::{Overlay, Workspace};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::tooltip::Tooltip;
use std::collections::HashSet;
use std::sync::Arc;

static LOGO: std::sync::LazyLock<Arc<Image>> = std::sync::LazyLock::new(|| Arc::new(Image::from_bytes(ImageFormat::Svg, include_bytes!("../../assets/logo.svg").to_vec())));

/// A sidebar row being dragged to a new place.
#[derive(Clone)]
pub(super) struct DraggedRow {
    id: String,
    label: String,
    workspace: bool,
    c: Colors,
}

impl Render for DraggedRow {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = self.c;
        let _ = cx;
        div().rounded_md().border_1().border_color(c.accent).bg(c.raised).px_2().py_1().text_xs().text_color(c.fg).child(self.label.clone())
    }
}

/// `git worktree list` per folder, read when a row is hovered (for its tooltip).
#[derive(Default)]
pub(super) struct SidebarUi {
    pub renaming: Option<(String, Entity<InputState>)>,
    /// The rename field is drawn in the pane header, not the sidebar row (never both: one input can't be focused twice in a frame).
    pub rename_in_header: bool,
    show_archived: HashSet<String>,
    feed_expanded: bool,
    listings: HashMap<String, Option<Vec<Value>>>,
    pending: HashSet<String>,
}

/// The last word's first letter: "orbit-web" and "orbit-api" get W and A, not two O's.
pub fn monogram(name: &str) -> String {
    name.split(|c: char| "-_ ./".contains(c))
        .filter(|w| !w.is_empty())
        .last()
        .and_then(|w| w.chars().find(|c| c.is_ascii_alphanumeric()))
        .map_or("·".into(), |c| c.to_ascii_uppercase().to_string())
}

/// A stable hue per workspace name (same hash as the Tauri app's).
pub fn name_hue(name: &str) -> f32 {
    let mut hash: i32 = 0;
    for ch in name.encode_utf16() {
        hash = hash.wrapping_mul(31).wrapping_add(ch as i32);
    }
    (hash.unsigned_abs() % 360) as f32
}

/// Linear sRGB mix of `over` onto `base` (matches CSS `color-mix(in srgb, over t%, base)`).
fn mix_color(base: Hsla, over: Hsla, t: f32) -> Hsla {
    let a = Rgba::from(base);
    let b = Rgba::from(over);
    let inv = 1.0 - t;
    Rgba {
        r: a.r * inv + b.r * t,
        g: a.g * inv + b.g * t,
        b: a.b * inv + b.b * t,
        a: a.a * inv + b.a * t,
    }
    .into()
}

fn normalize(path: &str) -> String {
    let value = path.replace('\\', "/").trim_end_matches('/').to_string();
    let bytes = value.as_bytes();
    if bytes.len() > 2 && bytes[1] == b':' && bytes[2] == b'/' { value.to_lowercase() } else { value }
}

/// Branch and checkout lines for a tooltip. Matches the directory, never a saved branch.
pub fn checkout_details(dir: &str, listing: Option<&Option<Vec<Value>>>) -> Vec<String> {
    let Some(listing) = listing else { return vec!["Branch: checking…".into()] };
    let Some(list) = listing else { return vec!["Branch / worktree: unavailable".into()] };
    let path = normalize(dir);
    let checkout = list
        .iter()
        .filter(|wt| wt["prunable"] != true)
        .filter(|wt| {
            let p = normalize(wt["info"]["path"].as_str().unwrap_or_default());
            path == p || path.starts_with(&format!("{p}/"))
        })
        .max_by_key(|wt| wt["info"]["path"].as_str().map_or(0, str::len));
    let Some(wt) = checkout else {
        return vec![if list.is_empty() { "Git: not a repository".into() } else { "Branch / worktree: unavailable for this directory".into() }];
    };
    let main = wt["isMain"] == true;
    let mut lines = vec![
        format!("Branch: {}", wt["info"]["branch"].as_str().unwrap_or_default()),
        format!("{}: {}", if main { "Project checkout" } else { "Worktree" }, wt["info"]["path"].as_str().unwrap_or_default()),
    ];
    if !main {
        lines.push(format!("Base branch: {}", wt["info"]["base"].as_str().unwrap_or_default()));
    }
    lines.push(format!("Changes: {}", if wt["dirty"] == true { "uncommitted changes" } else { "clean" }));
    lines
}

fn status_label(status: AgentStatus) -> &'static str {
    match status {
        AgentStatus::Idle => "Idle",
        AgentStatus::Working => "Working",
        AgentStatus::NeedsInput => "Needs your input",
        AgentStatus::Done => "Done",
        AgentStatus::Error => "Error",
    }
}

/// Moves `id` to `index` in `items` (the Tauri app's `moveItem`).
pub fn move_item<T>(items: &mut Vec<T>, from: usize, index: usize) {
    if from >= items.len() || from == index {
        return;
    }
    let item = items.remove(from);
    items.insert(index.min(items.len()), item);
}

fn reveal_label() -> &'static str {
    if cfg!(target_os = "macos") { "Reveal in Finder" } else { "Show in Explorer" }
}


impl Root {
    fn refresh_listing(&mut self, dir: String, cx: &mut Context<Self>) {
        if !self.sidebar.pending.insert(dir.clone()) {
            return;
        }
        cx.spawn(async move |this, cx| {
            let probe = dir.clone();
            let found = tauri::async_runtime::spawn(winfer_backend::worktree::worktree_list(probe)).await;
            let found = found.ok().and_then(Result::ok).and_then(|l| serde_json::to_value(l).ok()).and_then(|v| v.as_array().cloned());
            let _ = this.update(cx, |this, cx| {
                this.sidebar.pending.remove(&dir);
                this.sidebar.listings.insert(dir, found);
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn start_rename(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let current = self.state.read(cx).pane(id).map(|p| p.custom_title.clone().unwrap_or_else(|| self.state.read(cx).pane_label(p))).unwrap_or_default();
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Name this session").default_value(current));
        let pane = id.to_string();
        cx.subscribe_in(&input, window, move |this, input, event: &InputEvent, _, cx| match event {
            InputEvent::PressEnter { .. } | InputEvent::Blur => {
                if this.sidebar.renaming.as_ref().is_some_and(|(p, _)| *p == pane) {
                    let name = input.read(cx).value().to_string();
                    this.sidebar.renaming = None;
                    this.focused_view = None;
                    this.update_state(cx, |s| s.rename_pane(&pane, &name));
                }
            }
            _ => {}
        })
        .detach();
        input.update(cx, |i, cx| {
            i.focus(window, cx);
            i.select_all(window, cx);
        });
        self.sidebar.renaming = Some((id.to_string(), input));
        self.sidebar.rename_in_header = false;
        cx.notify();
    }

    /// Escape while renaming: keep the old name.
    pub(super) fn cancel_rename(&mut self, cx: &mut Context<Self>) -> bool {
        let open = self.sidebar.renaming.take().is_some();
        if open {
            self.focused_view = None;
            cx.notify();
        }
        open
    }

    pub(super) fn workspace_menu(&self, ws: &Workspace, cx: &App) -> Vec<MenuItem> {
        let s = self.state.read(cx);
        let agent = s.provider_name(&s.default_provider_id());
        let id = ws.id.clone();
        let switch = Command::WorkspaceSwitch(id.clone());
        let spawn = |worktree: Option<bool>| Command::Spawn { provider: None, count: 1, prompt: None, worktree, model: None };
        let mut items = vec![
            MenuItem::command("Open", switch.clone()),
            MenuItem::commands(format!("New {agent} here"), vec![switch.clone(), spawn(None)]),
        ];
        if ws.use_worktrees {
            items.push(MenuItem::commands(format!("New {agent} without worktree"), vec![switch.clone(), spawn(Some(false))]));
        }
        items.push(MenuItem::commands("New worktree from a branch…", vec![switch.clone(), Command::Overlay { overlay: Overlay::Branch, provider: None }]));
        let toggle = id.clone();
        items.push(MenuItem::run(if ws.use_worktrees { "Stop giving new agents a worktree" } else { "Give every new agent a worktree" }, move |this, _, cx| {
            this.update_state(cx, |s| {
                if let Some(w) = s.workspaces.iter_mut().find(|w| w.id == toggle) {
                    w.use_worktrees = !w.use_worktrees;
                }
            });
        }));
        items.push(MenuItem::Separator);
        items.push(MenuItem::commands("Open in code editor", vec![switch, Command::EditorOpen { target: None, editor: None }]));
        let (reveal, copy) = (ws.root_dir.clone(), ws.root_dir.clone());
        items.push(MenuItem::run(reveal_label(), move |_, _, _| reveal_path(&reveal)));
        items.push(MenuItem::run("Copy folder path", move |this, _, cx| this.copy_with_toast(copy.clone(), cx)));
        items.push(MenuItem::Separator);
        items.push(MenuItem::command("Remove workspace…", Command::WorkspaceRemove(id)).danger());
        items
    }

    pub(super) fn pane_menu(&self, pane: &Pane, cx: &App) -> Vec<MenuItem> {
        let s = self.state.read(cx);
        let provider = s.providers.iter().find(|p| p.id == pane.provider);
        let target = PaneRef::Id(pane.id.clone());
        let keys = |cmd: Command| commands::keys_for(&cmd, s);
        let agent = pane.provider != "shell";
        let mut items = vec![MenuItem::command("Focus", Command::Focus(target.clone()))];
        if pane.archived {
            items.push(MenuItem::command("Restore from archive", Command::Archive { target: target.clone(), restore: true }));
        } else {
            items.push(MenuItem::command("Zoom", Command::Zoom(target.clone())).hint(keys(Command::Zoom(PaneRef::Focused))));
            items.push(MenuItem::commands("Show in overview", vec![Command::Focus(target.clone()), Command::View(ViewTarget::Set(View::Overview))]));
            items.push(MenuItem::command("Archive", Command::Archive { target: target.clone(), restore: false }));
        }
        let rename = pane.id.clone();
        items.push(MenuItem::run("Rename…", move |this, window, cx| this.start_rename(&rename, window, cx)));
        if pane.custom_title.is_some() {
            let reset = pane.id.clone();
            items.push(MenuItem::run("Reset name", move |this, _, cx| this.update_state(cx, |s| s.rename_pane(&reset, ""))));
        }
        items.push(MenuItem::Separator);
        if pane.status == AgentStatus::NeedsInput {
            items.push(MenuItem::command("Approve request", Command::Approve(target.clone())));
        }
        items.push(MenuItem::command(if agent { "Interrupt" } else { "Interrupt (Ctrl-C)" }, Command::Interrupt(target.clone())).hint(keys(Command::Interrupt(PaneRef::Focused))));
        if agent {
            let restart = pane.id.clone();
            let resumable = (pane.session_id.is_some() && provider.is_some_and(|p| !p.resume_session_args.is_empty()))
                || provider.is_some_and(|p| !p.resume_args.is_empty());
            items.push(MenuItem::run("Restart agent (resume session)", move |this, _, cx| {
                this.update_state(cx, |s| {
                    if let Some(p) = s.pane_mut(&restart) {
                        p.resume = resumable;
                        p.prompt = None;
                    }
                });
                this.restart_pane(&restart, cx);
            }));
            if provider.is_some_and(|p| p.model_arg.is_some()) {
                items.push(MenuItem::commands("Change model…", vec![Command::Focus(target.clone()), Command::Overlay { overlay: Overlay::Model, provider: None }]));
            }
        }
        items.push(MenuItem::Separator);
        let editor = MenuItem::command("Open in code editor", Command::EditorOpen { target: Some(target.clone()), editor: None }).hint(keys(Command::EditorOpen { target: Some(PaneRef::Focused), editor: None }));
        match &pane.worktree {
            Some(wt) => {
                items.push(MenuItem::command("Show changes", Command::ShowChanges(target.clone())));
                if self.review.reviews.iter().any(|r| r.pane_id == pane.id && r.status == ReviewStatus::Open) {
                    items.push(MenuItem::command("Open its review", Command::Overlay { overlay: Overlay::Review, provider: None }));
                }
                items.push(MenuItem::command(format!("Merge into {}…", wt.base), Command::MergeWorktree(target.clone())));
                items.push(MenuItem::Separator);
                items.push(editor);
                let (reveal, path, branch) = (wt.path.clone(), wt.path.clone(), wt.branch.clone());
                items.push(MenuItem::run(reveal_label(), move |_, _, _| reveal_path(&reveal)));
                items.push(MenuItem::run("Copy worktree path", move |this, _, cx| this.copy_with_toast(path.clone(), cx)));
                items.push(MenuItem::run("Copy branch name", move |this, _, cx| this.copy_with_toast(branch.clone(), cx)));
            }
            None => {
                items.push(editor);
                let (reveal, path) = (pane.cwd.clone(), pane.cwd.clone());
                items.push(MenuItem::run(reveal_label(), move |_, _, _| reveal_path(&reveal)));
                items.push(MenuItem::run("Copy folder path", move |this, _, cx| this.copy_with_toast(path.clone(), cx)));
            }
        }
        items.push(MenuItem::Separator);
        if pane.worktree.is_some() {
            items.push(MenuItem::command("Discard worktree…", Command::DiscardWorktree(target.clone())).danger());
        }
        items.push(MenuItem::command("Close", Command::Close(target)).hint(keys(Command::Close(PaneRef::Focused))).danger());
        items
    }

    fn pane_tooltip(&self, pane: &Pane, cx: &App) -> String {
        let s = self.state.read(cx);
        let name = s.provider_name(&pane.provider);
        let label = s.pane_label(pane);
        let progress = s.progress.get(&pane.id);
        let mut lines = vec![
            if label == name { name.clone() } else { format!("{name}: {label}") },
            format!("Status: {}{}", status_label(pane.status), if pane.archived { " · archived" } else { "" }),
        ];
        lines.extend(pane.model.as_ref().map(|m| format!("Model: {m}")));
        lines.extend(progress.and_then(|p| p.percent).map(|n| format!("Progress: {n}%")));
        lines.extend(progress.and_then(|p| p.step.clone()).map(|a| format!("Activity: {a}")));
        lines.push(format!("Directory: {}", pane.cwd));
        lines.extend(checkout_details(&pane.cwd, self.sidebar.listings.get(&pane.cwd)));
        if pane.wants_worktree {
            lines.push("Worktree: creation pending".into());
        }
        lines.join("\n")
    }

    /// The agent's brand mark on a rounded tile, with its status as a dot in the corner.
    pub(super) fn agent_tile(provider: &str, status: AgentStatus, c: Colors) -> Div {
        let shell = provider == "shell";
        div().relative().size(px(26.0)).flex_shrink_0().flex().items_center().justify_center().rounded(px(7.0)).bg(c.hover)
            .child(crate::ui::code_pickers::mark(c, provider, 14.0, 1.0))
            .when(!shell, |d| d.child(div().absolute().right(px(-2.0)).bottom(px(-2.0)).size(px(8.0)).rounded_full().bg(status_color(status, &c))))
    }

    fn render_pane_row(&self, pane: &Pane, index: usize, sortable: bool, c: Colors, cx: &mut Context<Self>) -> AnyElement {
        let s = self.state.read(cx);
        let focused = s.focused_pane_id.as_deref() == Some(&pane.id) && s.active_workspace_id == pane.workspace_id;
        let label = s.pane_label(pane);
        let percent = s.progress.get(&pane.id).filter(|_| pane.provider != "shell" && pane.status != AgentStatus::Idle).and_then(|e| e.percent);
        let branch = pane.worktree.as_ref().map(|t| t.branch.trim_start_matches("winfer/").to_string());
        let doing = if pane.status == AgentStatus::NeedsInput { Some("Needs your input".to_string()) } else { s.progress.get(&pane.id).and_then(|e| e.step.clone()) };
        let tooltip = self.pane_tooltip(pane, cx);
        let renaming = self.sidebar.renaming.as_ref().filter(|(id, _)| *id == pane.id && !self.sidebar.rename_in_header).map(|(_, input)| input.clone());
        let status = pane.status;
        let (focus, menu_pane, rename, hover_dir) = (pane.id.clone(), pane.clone(), pane.id.clone(), pane.cwd.clone());
        let mono = crate::fonts::mono(cx);
        let drag = DraggedRow { id: pane.id.clone(), label: label.clone(), workspace: false, c };
        let drop_ws = pane.workspace_id.clone();
        let emphasized = pane.unseen || focused;
        let meta = match status {
            AgentStatus::NeedsInput => Some(("needs you".to_string(), c.warning)),
            AgentStatus::Done => Some(("done".to_string(), c.success)),
            AgentStatus::Error => Some(("error".to_string(), c.danger)),
            _ => percent.map(|n| (format!("{n}%"), c.subtle)),
        };
        let mut detail = div().flex().items_center().gap_1().line_height(px(13.0)).text_size(px(11.0)).text_color(c.subtle).min_w_0();
        if let Some(b) = &branch {
            detail = detail.child(div().flex().items_center().gap_1().flex_shrink(1.0).min_w_0().font_family(mono.clone()).text_size(px(10.5))
                .child(Icon::new(IconName::GitBranch).size(px(10.0)))
                .child(div().truncate().child(b.clone())));
        }
        if branch.is_some() && doing.is_some() {
            detail = detail.child(div().opacity(0.6).child("·"));
        }
        if let Some(d) = &doing {
            detail = detail.child(div().flex_1().min_w_0().truncate().when(status == AgentStatus::NeedsInput, |x| x.text_color(c.warning)).child(d.clone()));
        }
        let row = div().id(SharedString::from(format!("row-{}", pane.id))).flex().items_center().gap(px(9.0)).rounded(px(7.0)).px_1p5().py(px(5.5)).cursor_pointer()
            .text_size(px(12.5))
            .when(focused, |d| d.bg(c.active).text_color(c.fg))
            .when(!focused, |d| d.text_color(c.muted).hover(move |d| d.bg(c.hover).text_color(c.fg)))
            .when(pane.archived && !focused, |d| d.opacity(0.7))
            .tooltip(move |window, cx| Tooltip::new(tooltip.clone()).build(window, cx))
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                if *hovered {
                    this.refresh_listing(hover_dir.clone(), cx);
                }
            }))
            .on_click(cx.listener(move |this, e: &ClickEvent, window, cx| {
                if e.click_count() == 2 {
                    this.start_rename(&rename, window, cx);
                } else {
                    this.execute(Command::Focus(PaneRef::Id(focus.clone())), window, cx);
                }
            }))
            .on_mouse_down(MouseButton::Right, cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                let items = this.pane_menu(&menu_pane, cx);
                let title = menu_pane.worktree.as_ref().map_or(menu_pane.cwd.clone(), |t| t.branch.clone());
                this.open_menu(e.position, Some(title), items, cx);
            }))
            .child(Self::agent_tile(&pane.provider, status, c))
            .child(
                div().flex_1().min_w_0().flex().flex_col()
                    .child(div().flex().items_center().gap_1p5().line_height(px(15.0))
                        .child(match renaming {
                            Some(input) => div().flex_1().child(Input::new(&input)).into_any_element(),
                            None => div().flex_1().min_w_0().truncate().when(emphasized, |d| d.text_color(c.fg).font_weight(FontWeight::MEDIUM)).child(super::title_swap::title("side", &pane.id, &label)).into_any_element(),
                        })
                        .when_some(meta, |d, (text, color)| d.child(div().flex_shrink_0().text_size(px(10.5)).text_color(color).when(percent.is_some() && status == AgentStatus::Working, |d| d.font_family(mono.clone())).child(text))))
                    .when(branch.is_some() || doing.is_some(), |d| d.child(detail)),
            );
        if !sortable {
            return row.into_any_element();
        }
        row.on_drag(drag, |row, _, _, cx| cx.new(|_| row.clone()))
            .drag_over::<DraggedRow>(move |d, dragged, _, _| if dragged.workspace { d } else { d })
            .on_drop(cx.listener(move |this, dragged: &DraggedRow, _, cx| {
                if dragged.workspace {
                    return;
                }
                let id = dragged.id.clone();
                this.update_state(cx, |s| {
                    let Some(w) = s.workspaces.iter_mut().find(|w| w.id == drop_ws) else { return };
                    let Some(pane) = w.panes.iter().find(|p| p.id == id).cloned() else { return };
                    let (mut group, rest): (Vec<Pane>, Vec<Pane>) = w.panes.drain(..).partition(|p| p.archived == pane.archived);
                    if let Some(from) = group.iter().position(|p| p.id == id) {
                        move_item(&mut group, from, index);
                    }
                    w.panes = if pane.archived { rest.into_iter().chain(group).collect() } else { group.into_iter().chain(rest).collect() };
                });
            }))
            .into_any_element()
    }

    /// One segment of the Sessions · Review · History control.
    /// One segment of the Sessions · Review · History control; a count takes the icon's place.
    /// Labels show when the sidebar is wide enough for all three tabs, else the icons alone.
    fn nav_tab(&self, id: &'static str, icon: IconName, label: &'static str, tip: String, count: usize, labels: bool, c: Colors) -> Stateful<Div> {
        div().id(id).flex_1().min_w_0().h(px(28.0)).flex().items_center().justify_center().gap_1p5().rounded(px(6.0)).text_size(px(12.0)).text_color(c.muted).cursor_pointer()
            .hover(move |d| d.bg(c.hover).text_color(c.fg))
            .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
            .child(if count > 0 { Self::count_badge(count, c).into_any_element() } else { Icon::new(icon).size(px(14.0)).flex_shrink_0().into_any_element() })
            .when(labels, |d| d.child(div().truncate().child(label)))
    }

    fn count_badge(n: usize, c: Colors) -> Div {
        div().min_w(px(16.0)).h(px(16.0)).px_1().flex().items_center().justify_center().rounded_full().bg(c.accent).text_size(px(10.0)).font_weight(FontWeight::SEMIBOLD).text_color(c.on_accent).child(n.to_string())
    }

    /// Agents waiting for you, in every workspace.
    fn waiting_panes(&self, cx: &App) -> Vec<Pane> {
        self.state.read(cx).all_panes().filter(|p| !p.archived && p.provider != "shell" && p.status == AgentStatus::NeedsInput).cloned().collect()
    }

    fn open_reviews(&self, cx: &App) -> usize {
        let s = self.state.read(cx);
        let everywhere = s.view() == View::Overview;
        self.review.reviews.iter().filter(|r| r.status == ReviewStatus::Open && (everywhere || r.remote.is_some() || r.workspace_id == s.active_workspace_id)).count()
    }

    pub(super) fn render_sidebar(&mut self, c: &Colors, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let c = *c;
        let s = self.state.read(cx);
        let keys = |cmd: Command| commands::keys_for(&cmd, s);
        let overlay = |o: Overlay| Command::Overlay { overlay: o, provider: None };
        let with_keys = |label: &str, k: Option<String>| k.map_or(label.to_string(), |k| format!("{label} ({k})"));
        let (progress_tip, review_tip, history_tip) = (
            with_keys("Session progress", keys(overlay(Overlay::Progress))),
            with_keys("Review finished work", keys(overlay(Overlay::Review))),
            with_keys("Merged features", keys(overlay(Overlay::History))),
        );
        let settings_tip = with_keys("Settings", keys(Command::Settings(None)));
        let new_keys = keys(overlay(Overlay::NewPane)).unwrap_or_default();
        let active_ws = s.active_workspace_id.clone();
        let review_count = self.open_reviews(cx);
        let mono = crate::fonts::mono(cx);

        // The three views: icon-only tabs (labels live in the tooltips).
        let labels = false;
        let sessions = self.nav_tab("nav-sessions", IconName::Activity, "Sessions", progress_tip, 0,labels, c)
            .on_click(cx.listener(|this, _, window, cx| this.execute(Command::Overlay { overlay: Overlay::Progress, provider: None }, window, cx)));
        let review = self.nav_tab("nav-review", IconName::Inbox, "Review", review_tip, review_count,labels, c)
            .on_click(cx.listener(|this, _, window, cx| this.execute(Command::Overlay { overlay: Overlay::Review, provider: None }, window, cx)));
        let history = self.nav_tab("nav-history", IconName::Clock, "History", history_tip, 0,labels, c)
            .on_click(cx.listener(|this, _, window, cx| this.execute(Command::Overlay { overlay: Overlay::History, provider: None }, window, cx)));
        let top = div().flex().flex_col().gap_2().px_2p5().pt_2p5().pb_2()
            .child(div().flex().gap_px().p(px(2.0)).rounded(px(8.0)).bg(c.app).border_1().border_color(c.edge).child(sessions).child(review).child(history));

        // Needs you: approve or open waiting agents without finding their pane.
        let waiting = self.waiting_panes(cx);
        let needs = (!waiting.is_empty()).then(|| {
            let tint = Hsla { a: 0.06, ..c.warning };
            let mut sec = div().mt_2p5().p_1().rounded_lg().bg(tint).border_1().border_color(Hsla { a: 0.16, ..c.warning })
                .child(div().flex().items_center().gap(px(7.0)).h(px(26.0)).px_1p5().text_size(px(11.0)).font_weight(FontWeight::SEMIBOLD).text_color(c.warning)
                    .child(div().size(px(7.0)).rounded_full().bg(c.warning))
                    .child("Needs you")
                    .child(div().font_weight(FontWeight::NORMAL).text_color(c.subtle).font_family(mono.clone()).child(waiting.len().to_string())));
            let s = self.state.read(cx);
            for p in &waiting {
                let ask = s.progress.get(&p.id).and_then(|e| e.step.clone()).unwrap_or_else(|| "Waiting for your input".into());
                let (focus, approve) = (p.id.clone(), p.id.clone());
                sec = sec.child(
                    div().id(SharedString::from(format!("need-{}", p.id))).flex().items_center().gap(px(9.0)).rounded_md().px_1p5().py(px(7.0)).cursor_pointer()
                        .hover(move |d| d.bg(Hsla { a: 0.08, ..c.warning }))
                        .on_click(cx.listener(move |this, _, window, cx| this.execute(Command::Focus(PaneRef::Id(focus.clone())), window, cx)))
                        .child(Self::agent_tile(&p.provider, p.status, c))
                        .child(div().flex_1().min_w_0().flex().flex_col()
                            .child(div().truncate().text_size(px(12.5)).text_color(c.fg).child(s.pane_label(p)))
                            .child(div().truncate().text_size(px(11.5)).text_color(c.muted).child(ask)))
                        .child(div().id(SharedString::from(format!("approve-{}", p.id))).flex_shrink_0().h(px(24.0)).px_2p5().flex().items_center().rounded_md().bg(c.accent)
                            .text_size(px(11.5)).font_weight(FontWeight::MEDIUM).text_color(c.on_accent).cursor_pointer().hover(|d| d.opacity(0.9))
                            .tooltip(|window, cx| Tooltip::new("Approve what it's asking (sends its approve key)").build(window, cx))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                cx.stop_propagation();
                                this.execute(Command::Approve(PaneRef::Id(approve.clone())), window, cx);
                            }))
                            .child("Approve")),
                );
            }
            sec
        });

        let s = self.state.read(cx);
        let workspaces = s.workspaces.clone();
        let expanded_ids = s.expanded_workspaces.clone();
        let card_bg = mix_color(c.panel, c.app, 0.68);
        let mut list = div().mt_1().flex().flex_col().gap_1p5();
        for (i, ws) in workspaces.iter().enumerate() {
            // Other machines' workspaces are listed under them (Connected).
            if ws.remote.is_some() {
                continue;
            }
            let active = ws.id == active_ws;
            let expanded = expanded_ids.contains(&ws.id);
            let hue = name_hue(&ws.name) / 360.0;
            let color = hsla(hue, 0.65, 0.62, 1.0);
            let live: Vec<&Pane> = ws.panes.iter().filter(|p| !p.archived).collect();
            let agents: Vec<&Pane> = live.iter().copied().filter(|p| p.provider != "shell").collect();
            let listing = self.sidebar.listings.get(&ws.root_dir);
            let mut tip = vec![ws.name.clone(), format!("Directory: {}", ws.root_dir)];
            tip.extend(checkout_details(&ws.root_dir, listing));
            tip.push(format!("New agents: {}", if ws.use_worktrees { "isolated worktrees" } else { "shared checkout" }));
            tip.push("Double-click to open · drag to reorder".into());
            let tip = tip.join("\n");
            let (toggle, menu_ws, hover_dir) = (ws.id.clone(), ws.clone(), ws.root_dir.clone());
            let count = |st: AgentStatus| agents.iter().filter(|p| p.status == st).count();
            let (working, waiting_here, done) = (count(AgentStatus::Working), count(AgentStatus::NeedsInput), count(AgentStatus::Done));
            let summary = (!expanded && !agents.is_empty()).then(|| {
                let mut parts: Vec<AnyElement> = vec![div().child(format!("{working} working")).into_any_element()];
                if waiting_here > 0 {
                    parts.push(div().text_color(c.warning).font_weight(FontWeight::MEDIUM).child(format!("{waiting_here} need{} you", if waiting_here == 1 { "s" } else { "" })).into_any_element());
                }
                if done > 0 {
                    parts.push(div().text_color(c.success).child(format!("{done} done")).into_any_element());
                }
                let mut line = div().flex().flex_wrap().gap_1().pl(px(30.0)).mt(px(2.0)).text_size(px(11.0)).font_weight(FontWeight::NORMAL).text_color(c.subtle);
                for (k, part) in parts.into_iter().enumerate() {
                    if k > 0 {
                        line = line.child("·");
                    }
                    line = line.child(part);
                }
                line
            });
            let top_row = div().flex().items_center().gap_2()
                .child(div().size(px(22.0)).flex_shrink_0().flex().items_center().justify_center().rounded(px(6.0)).bg(Hsla { a: 0.16, ..color }).text_size(px(11.0)).font_weight(FontWeight::SEMIBOLD).text_color(color).child(monogram(&ws.name)))
                .child(div().flex_1().min_w_0().truncate().child(ws.name.clone()))
                .when(active, |d| d.child(div().size(px(6.0)).flex_shrink_0().rounded_full().bg(color)))
                .when(!expanded && !live.is_empty(), |d| d.child(div().text_size(px(10.5)).font_weight(FontWeight::NORMAL).text_color(c.subtle).font_family(mono.clone())
                    .child(if agents.is_empty() { format!("{}", live.len()) } else { format!("{} agent{}", agents.len(), if agents.len() == 1 { "" } else { "s" }) })))
                .child(Icon::new(if expanded { IconName::ChevronDown } else { IconName::ChevronRight }).size(px(13.0)).text_color(c.subtle));
            let header = div().id(SharedString::from(format!("ws-{}", ws.id))).flex().flex_col().justify_center().px_2().py(px(6.0)).cursor_pointer().text_size(px(13.0))
                .when(expanded, |d| d.rounded_t(px(10.0)))
                .when(!expanded, |d| d.rounded(px(10.0)))
                .when(!expanded && active, |d| d.bg(card_bg))
                .when(active || expanded, |d| d.text_color(c.fg).font_weight(FontWeight::MEDIUM))
                .when(!(active || expanded), |d| d.text_color(c.muted))
                .hover(move |d| d.bg(c.hover).text_color(c.fg))
                .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
                .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                    if *hovered {
                        this.refresh_listing(hover_dir.clone(), cx);
                    }
                }))
                .on_click(cx.listener(move |this, e: &ClickEvent, window, cx| {
                    if e.click_count() == 2 {
                        this.execute(Command::WorkspaceSwitch(toggle.clone()), window, cx);
                    } else {
                        let id = toggle.clone();
                        this.update_state(cx, |s| s.toggle_workspace_expanded(&id));
                    }
                }))
                .on_mouse_down(MouseButton::Right, cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                    let items = this.workspace_menu(&menu_ws, cx);
                    this.open_menu(e.position, Some(menu_ws.root_dir.clone()), items, cx);
                }))
                .on_drag(DraggedRow { id: ws.id.clone(), label: ws.name.clone(), workspace: true, c }, |row, _, _, cx| cx.new(|_| row.clone()))
                .drag_over::<DraggedRow>(move |d, dragged, _, _| if dragged.workspace { d } else { d })
                .on_drop(cx.listener(move |this, dragged: &DraggedRow, _, cx| {
                    if !dragged.workspace {
                        return;
                    }
                    let id = dragged.id.clone();
                    this.update_state(cx, |s| {
                        if let Some(from) = s.workspaces.iter().position(|w| w.id == id) {
                            move_item(&mut s.workspaces, from, i);
                        }
                    });
                }))
                .child(top_row)
                .children(summary);
            let mut group = div().relative().rounded(px(10.0)).border_1()
                .border_color(if expanded || active { c.edge } else { gpui_kit::transparent_black() })
                .when(expanded, |d| d.bg(card_bg))
                .child(header);
            if expanded {
                let mut panes = div().flex().flex_col().gap_0p5().p_1();
                let live: Vec<Pane> = ws.panes.iter().filter(|p| !p.archived).cloned().collect();
                let archived: Vec<Pane> = ws.panes.iter().filter(|p| p.archived).cloned().collect();
                for (j, p) in live.iter().enumerate() {
                    panes = panes.child(self.render_pane_row(p, j, true, c, cx));
                }
                if !archived.is_empty() {
                    let showing = self.sidebar.show_archived.contains(&ws.id);
                    let flip = ws.id.clone();
                    panes = panes.child(
                        div().id(SharedString::from(format!("archived-{}", ws.id))).flex().items_center().gap_2().rounded(px(7.0)).px_2().h(px(26.0)).text_size(px(12.0)).text_color(c.subtle).cursor_pointer()
                            .hover(move |d| d.bg(c.hover).text_color(c.fg))
                            .tooltip(move |window, cx| Tooltip::new(if showing { "Hide archived sessions" } else { "Show archived sessions" }).build(window, cx))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if !this.sidebar.show_archived.remove(&flip) {
                                    this.sidebar.show_archived.insert(flip.clone());
                                }
                                cx.notify();
                            }))
                            .child(Icon::new(IconName::Archive).size(px(12.0)))
                            .child(div().flex_1().child(format!("Archived · {}", archived.len())))
                            .child(Icon::new(if showing { IconName::ChevronDown } else { IconName::ChevronRight }).size(px(12.0))),
                    );
                    if showing {
                        for (j, p) in archived.iter().enumerate() {
                            panes = panes.child(self.render_pane_row(p, j, false, c, cx));
                        }
                    }
                }
                if let Some(list) = self.render_worktree_list(ws, c, cx) {
                    panes = panes.child(list);
                }
                let target = ws.id.clone();
                let new_keys = new_keys.clone();
                panes = panes.child(
                    div().id(SharedString::from(format!("new-agent-{}", ws.id))).group("new-agent").flex().items_center().gap_2().rounded(px(7.0)).px_2().h(px(28.0)).text_size(px(12.0)).text_color(c.subtle).cursor_pointer()
                        .hover(move |d| d.bg(c.hover).text_color(c.fg))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            if this.state.read(cx).active_workspace_id != target {
                                this.execute(Command::WorkspaceSwitch(target.clone()), window, cx);
                            }
                            this.execute(Command::Overlay { overlay: Overlay::NewPane, provider: None }, window, cx);
                        }))
                        .child(Icon::new(IconName::Plus).size(px(12.0)))
                        .child(div().flex_1().child(if ws.panes.is_empty() { "No agents yet · start one" } else { "New agent" }))
                        .when(!new_keys.is_empty(), |d| d.child(div().invisible().group_hover("new-agent", |d| d.visible()).font_family(mono.clone()).text_size(px(10.0)).child(new_keys))),
                );
                group = group.child(panes);
            }
            list = list.child(group);
        }
        let any_expanded = !expanded_ids.is_empty();
        let label = div().flex().h(px(26.0)).mt_1().items_center().justify_between().pl_1p5()
            .child(div().text_size(px(11.0)).font_weight(FontWeight::SEMIBOLD).text_color(c.subtle).child("Workspaces"))
            .child(div().flex().items_center()
                .child(div().id("collapse-all").rounded_sm().p_1().text_color(c.subtle).when(!any_expanded, |d| d.opacity(0.4))
                    .when(any_expanded, |d| d.cursor_pointer().hover(move |d| d.bg(c.hover).text_color(c.fg)))
                    .tooltip(|window, cx| Tooltip::new("Collapse all workspaces").build(window, cx))
                    .on_click(cx.listener(|this, _, _, cx| this.update_state(cx, |s| s.expanded_workspaces.clear())))
                    .child(Icon::new(IconName::ChevronsDownUp).size(px(13.0))))
                .child(div().id("new-workspace").rounded_sm().p_1().text_color(c.subtle).cursor_pointer().hover(move |d| d.bg(c.hover).text_color(c.fg))
                    .tooltip(|window, cx| Tooltip::new("New workspace").build(window, cx))
                    .on_click(cx.listener(|this, _, window, cx| this.execute(Command::WorkspaceCreate, window, cx)))
                    .child(Icon::new(IconName::Plus).size(px(13.0)))));

        let connected = self.render_connected(c, cx);
        let feed = self.render_activity_feed(&active_ws, c, cx);
        let footer = self.render_sidebar_footer(false, &settings_tip, c, cx);
        let width = px(self.state.read(cx).sidebar_width as f32);
        div().w(width).h_full().flex_shrink_0().flex().flex_col().overflow_hidden().rounded_2xl().border_1().border_color(c.edge).bg(c.panel)
            .child(top)
            .child(div().id("sidebar-list").flex_1().min_h_0().overflow_y_scroll().px_2().pb_3().children(needs).child(label).child(list).children(connected))
            .children(feed)
            .child(footer)
    }

    /// Logo, version, the update button (only when one is out), light/dark and settings.
    fn render_sidebar_footer(&self, rail: bool, settings_tip: &str, c: Colors, cx: &mut Context<Self>) -> Div {
        let mono = crate::fonts::mono(cx);
        let theme_id = self.state.read(cx).theme.clone();
        let def = theme::theme_by_id(&theme_id);
        let pair = theme::theme_by_id(&def.pair).name.clone();
        let dark = def.mode == "dark";
        let update = self.updates.available.as_ref().map(|a| a.version.clone());
        let installing = self.updates.is_installing();
        let install_pct = self.updates.install_percent();
        let settings_tip = settings_tip.to_string();
        let icon_button = move |id: &'static str| div().id(id).size(px(26.0)).flex().items_center().justify_center().rounded_md().text_color(c.subtle).cursor_pointer().hover(move |d| d.bg(c.hover).text_color(c.fg));
        let update_button = update.map(|version| {
            let tip = if installing {
                match install_pct {
                    Some(pct) => format!("Installing winfer {version} ({pct}%)…"),
                    None => format!("Installing winfer {version}…"),
                }
            } else {
                format!("Update to winfer {version}")
            };
            let icon: AnyElement = if installing {
                super::updates_ui::spinning_icon(IconName::LoaderCircle, 12.0, None, "sidebar-update-spin").into_any_element()
            } else {
                Icon::new(IconName::Download).size(px(12.0)).into_any_element()
            };
            let dot = div().absolute().top(px(-2.0)).right(px(-2.0)).size(px(7.0)).rounded_full().bg(c.accent).border_2().border_color(c.panel);
            let dot: AnyElement = if installing {
                dot.with_animation("sidebar-update-dot", Animation::new(Duration::from_millis(1200)).repeat().with_max_fps(30.0), |d, t| {
                    d.opacity(0.4 + 0.6 * (0.5 + 0.5 * (t * std::f32::consts::TAU).sin()))
                }).into_any_element()
            } else {
                dot.into_any_element()
            };
            div().id("sidebar-update").relative().size(px(24.0)).flex_shrink_0().flex().items_center().justify_center().rounded(px(7.0)).bg(Hsla { a: 0.14, ..c.accent }).text_color(c.accent).cursor_pointer()
                .hover(move |d| d.bg(Hsla { a: 0.24, ..c.accent }))
                .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
                .on_click(cx.listener(move |this, _, window, cx| {
                    if this.updates.is_installing() {
                        this.execute(Command::Settings(Some("updates".into())), window, cx);
                    } else {
                        this.execute(Command::ShowReleaseNotes(true), window, cx);
                    }
                }))
                .child(icon)
                .child(dot)
        });
        let theme_button = icon_button("theme-toggle")
            .tooltip(move |window, cx| Tooltip::new(format!("Switch to {pair} (more themes in Settings)")).build(window, cx))
            .on_click(cx.listener(|this, _, window, cx| {
                let next = theme::theme_by_id(&this.state.read(cx).theme).pair.clone();
                this.execute(Command::Theme(next), window, cx);
            }))
            .child(Icon::new(if dark { IconName::Sun } else { IconName::Moon }).size(px(14.0)));
        let settings_button = icon_button("settings-button")
            .tooltip(move |window, cx| Tooltip::new(settings_tip.clone()).build(window, cx))
            .on_click(cx.listener(|this, _, window, cx| this.execute(Command::Settings(None), window, cx)))
            .child(Icon::new(IconName::Settings).size(px(14.0)));
        let logo = img(LOGO.clone()).size(px(26.0)).flex_shrink_0().rounded(px(7.0)).object_fit(ObjectFit::Cover);
        if rail {
            return div().flex().flex_col().items_center().gap_1().py_2()
                .child(logo).children(update_button).child(settings_button);
        }
        div().flex().items_center().gap_2().pl_3().pr_2().py_2()
            .child(logo)
            .child(div().flex().flex_col().min_w_0()
                .child(div().text_size(px(12.0)).font_weight(FontWeight::SEMIBOLD).text_color(c.fg).child("winfer"))
                .child(div().truncate().font_family(mono).text_size(px(10.0)).text_color(c.subtle).child(env!("CARGO_PKG_VERSION"))))
            .children(update_button)
            .child(div().flex_1())
            .child(theme_button)
            .child(settings_button)
    }

    /// The collapsed sidebar: a 60 px icon rail with the same sections.
    pub(super) fn render_sidebar_rail(&mut self, c: &Colors, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let c = *c;
        let s = self.state.read(cx);
        let settings_tip = commands::keys_for(&Command::Settings(None), s).map_or("Settings".to_string(), |k| format!("Settings ({k})"));
        let review_count = self.open_reviews(cx);
        let rail_button = move |id: SharedString, icon: IconName, tip: String| div().id(id).relative().size(px(36.0)).flex().items_center().justify_center().rounded_md().text_color(c.muted).cursor_pointer()
            .hover(move |d| d.bg(c.hover).text_color(c.fg))
            .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
            .child(Icon::new(icon).size(px(15.0)));
        let mut top = div().flex().flex_col().items_center().gap_1().pt_2p5().pb_2()
            .child(rail_button("rail-search".into(), IconName::Search, "Search or jump to…".into()).on_click(cx.listener(|this, _, window, cx| this.execute(Command::Palette(None), window, cx))))
            .child(rail_button("rail-sessions".into(), IconName::Activity, "Sessions".into()).on_click(cx.listener(|this, _, window, cx| this.execute(Command::Overlay { overlay: Overlay::Progress, provider: None }, window, cx))));
        top = top.child(rail_button("rail-review".into(), IconName::Inbox, "Review".into())
            .when(review_count > 0, |d| d.child(div().absolute().top(px(2.0)).right(px(0.0)).child(Self::count_badge(review_count, c))))
            .on_click(cx.listener(|this, _, window, cx| this.execute(Command::Overlay { overlay: Overlay::Review, provider: None }, window, cx))));
        top = top.child(rail_button("rail-history".into(), IconName::Clock, "History".into()).on_click(cx.listener(|this, _, window, cx| this.execute(Command::Overlay { overlay: Overlay::History, provider: None }, window, cx))));

        let tile_button = |this: &Self, p: &Pane, cx: &mut Context<Self>| {
            let (focus, tip) = (p.id.clone(), this.state.read(cx).pane_label(p));
            div().id(SharedString::from(format!("rail-{}", p.id))).p(px(5.0)).rounded_md().cursor_pointer().hover(move |d| d.bg(c.hover))
                .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
                .on_click(cx.listener(move |this, _, window, cx| this.execute(Command::Focus(PaneRef::Id(focus.clone())), window, cx)))
                .child(Self::agent_tile(&p.provider, p.status, c))
        };
        let mut body = div().flex().flex_col().items_center().gap_1p5().py_2();
        let waiting = self.waiting_panes(cx);
        if !waiting.is_empty() {
            let mut sec = div().flex().flex_col().items_center().w(px(44.0)).py_1().rounded_lg().bg(Hsla { a: 0.06, ..c.warning }).border_1().border_color(Hsla { a: 0.16, ..c.warning });
            for p in &waiting {
                sec = sec.child(tile_button(self, p, cx));
            }
            body = body.child(sec);
        }
        let s = self.state.read(cx);
        let workspaces = s.workspaces.clone();
        let active_ws = s.active_workspace_id.clone();
        let focused = s.focused_pane_id.clone();
        let card_bg = mix_color(c.panel, c.app, 0.68);
        for ws in &workspaces {
            let active = ws.id == active_ws;
            let color = hsla(name_hue(&ws.name) / 360.0, 0.65, 0.62, 1.0);
            let (switch, name) = (ws.id.clone(), ws.name.clone());
            let ws_agents: Vec<&Pane> = ws.panes.iter().filter(|p| !p.archived && p.provider != "shell").collect();
            let mut pill = div().w(px(44.0)).flex().flex_col().items_center().gap_0p5().py_1().rounded(px(10.0))
                .bg(card_bg).border_1().border_color(c.edge)
                .child(div().id(SharedString::from(format!("rail-ws-{}", ws.id))).relative().size(px(34.0)).flex().items_center().justify_center().rounded(px(7.0)).cursor_pointer().hover(move |d| d.bg(c.hover))
                    .tooltip(move |window, cx| Tooltip::new(name.clone()).build(window, cx))
                    .on_click(cx.listener(move |this, _, window, cx| this.execute(Command::WorkspaceSwitch(switch.clone()), window, cx)))
                    .child(div().size(px(22.0)).flex().items_center().justify_center().rounded(px(6.0)).bg(Hsla { a: 0.16, ..color }).text_size(px(11.0)).font_weight(FontWeight::SEMIBOLD).text_color(color).child(monogram(&ws.name)))
                    .when(active, |d| d.child(div().absolute().top(px(3.0)).right(px(3.0)).size(px(5.0)).rounded_full().bg(color))));
            if !ws_agents.is_empty() {
                pill = pill.child(div().w(px(24.0)));
                for p in ws_agents {
                    let on = focused.as_deref() == Some(&p.id);
                    pill = pill.child(tile_button(self, p, cx).when(on, |d| d.bg(c.active)));
                }
            }
            body = body.child(pill);
        }
        let footer = self.render_sidebar_footer(true, &settings_tip, c, cx);
        div().w(px(60.0)).h_full().flex_shrink_0().flex().flex_col().overflow_hidden().rounded_2xl().border_1().border_color(c.edge).bg(c.panel)
            .child(top)
            .child(div().id("sidebar-rail-list").flex_1().min_h_0().overflow_y_scroll().child(body))
            .child(footer)
    }

    /// "Latest": what agents and you did recently in this workspace.
    fn render_activity_feed(&self, workspace: &str, c: Colors, cx: &mut Context<Self>) -> Option<Div> {
        let s = self.state.read(cx);
        let events: Vec<_> = s.activity.iter().filter(|e| e.workspace_id == workspace).collect();
        if events.is_empty() {
            return None;
        }
        let expanded = self.sidebar.feed_expanded;
        let shown = events.iter().take(if expanded { 40 } else { 4 });
        let mut rows = div().id("activity-rows").flex().flex_col().when(expanded, |d| d.max_h(px(288.0)).overflow_y_scroll());
        for e in shown {
            let at = e.at.duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_millis() as f64);
            let text = e.text.clone();
            rows = rows.child(
                div().id(SharedString::from(format!("activity-{}", e.id))).flex().items_start().gap_2().rounded_md().px_2().py_1p5().text_size(px(12.0))
                    .tooltip(move |window, cx| Tooltip::new(text.clone()).build(window, cx))
                    .child(div().mt(px(3.0)).size(px(10.0)).flex_shrink_0().rounded_full().bg(if e.provider.is_some() { c.accent } else { c.border_strong }))
                    .child(div().flex_1().text_color(c.muted).line_clamp(2).child(e.text.clone()))
                    .child(div().flex_shrink_0().text_size(px(11.0)).text_color(c.subtle).child(super::dialogs::relative_time(at))),
            );
        }
        let more = events.len() > 4;
        Some(
            div().mx_2().pt_2().pb_1().border_t_1().border_color(c.border)
                .child(div().flex().items_center().justify_between().pr_1().pl_2p5().pb_1()
                    .child(div().text_size(px(10.5)).text_color(c.subtle).child("LATEST"))
                    .when(more, |d| d.child(div().id("feed-toggle").text_size(px(11.0)).text_color(c.accent).cursor_pointer()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.sidebar.feed_expanded = !this.sidebar.feed_expanded;
                            cx.notify();
                        }))
                        .child(if expanded { "Show less" } else { "View all" }))))
                .child(rows),
        )
    }
}

pub(super) fn reveal_path(path: &str) {
    let path = winfer_backend::editors::expand_home(path);
    let _ = plugins::reveal(&path);
}

#[cfg(test)]
mod tests {
    use super::{checkout_details, monogram, move_item, name_hue};
    use serde_json::json;

    #[test]
    fn monograms_use_the_last_word() {
        assert_eq!(monogram("orbit-web"), "W");
        assert_eq!(monogram("orbit-api"), "A");
        assert_eq!(monogram("---"), "·");
    }

    #[test]
    fn hues_are_stable() {
        assert_eq!(name_hue("winfer"), name_hue("winfer"));
        assert!(name_hue("a") < 360.0);
    }

    #[test]
    fn items_move_like_the_tauri_app() {
        let mut v = vec![1, 2, 3, 4];
        move_item(&mut v, 0, 2);
        assert_eq!(v, vec![2, 3, 1, 4]);
        move_item(&mut v, 3, 0);
        assert_eq!(v, vec![4, 2, 3, 1]);
    }

    #[test]
    fn checkouts_match_the_deepest_folder() {
        let list = Some(vec![
            json!({ "info": { "path": "/r", "branch": "main", "base": "main" }, "isMain": true, "dirty": false }),
            json!({ "info": { "path": "/r/.wt/a", "branch": "winfer/a", "base": "main" }, "isMain": false, "dirty": true }),
        ]);
        assert_eq!(checkout_details("/r/.wt/a/src", Some(&list)), vec!["Branch: winfer/a", "Worktree: /r/.wt/a", "Base branch: main", "Changes: uncommitted changes"]);
        assert_eq!(checkout_details("/x", Some(&None)), vec!["Branch / worktree: unavailable"]);
        assert_eq!(checkout_details("/x", None), vec!["Branch: checking…"]);
    }

    #[test]
    fn logo_covers_full_tile_without_outer_padding() {
        let tree = resvg::usvg::Tree::from_data(include_bytes!("../../assets/logo.svg"), &resvg::usvg::Options::default()).expect("valid logo.svg");
        let mut pixmap = resvg::tiny_skia::Pixmap::new(26, 26).expect("pixmap");
        let scale = resvg::tiny_skia::Transform::from_scale(26.0 / tree.size().width(), 26.0 / tree.size().height());
        resvg::render(&tree, scale, &mut pixmap.as_mut());
        assert!(pixmap.pixels().iter().all(|p| p.alpha() == 255), "logo.svg left transparent outer pixels when rendered at 26x26");
    }
}
