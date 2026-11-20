//! Modal layers: command palette, shortcut sheet, agents' questions, toasts
//! (port of `features/palette`, `QuestionDialog.tsx` and `Toaster.tsx`).

use super::commands::{BINDINGS, Command, PaneRef, ViewTarget, chord_for, format_chord, keys_for};
use super::*;
use crate::model::Overlay;
use crate::model::layout::{PRESETS, Preset};
use gpui_kit::component::input::{Input, InputEvent, InputState, MoveDown, MoveUp};

pub struct Entry {
    pub title: String,
    pub keys: Option<String>,
    pub commands: Vec<Command>,
}

/// Subsequence match; lower is better. Rewards contiguous and word-start hits.
pub fn score(query: &str, text: &str) -> Option<f64> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return Some(0.0);
    }
    let t: Vec<char> = text.to_lowercase().chars().collect();
    let mut pos: i64 = -1;
    let mut total = 0.0;
    for ch in q.chars() {
        let next = (pos + 1) as usize + t.get((pos + 1) as usize..)?.iter().position(|c| *c == ch)?;
        let gap = next as i64 - pos - 1;
        let word_start = next == 0 || matches!(t[next - 1], ' ' | ':');
        if !word_start && gap > 0 {
            total += 2.0 + gap as f64 * 0.1;
        }
        pos = next as i64;
    }
    Some(total + t.len() as f64 * 0.01)
}

fn preset_name(p: Preset) -> &'static str {
    match p {
        Preset::Auto => "auto",
        Preset::MainStack => "main-stack",
        Preset::Grid => "grid",
        Preset::Columns => "columns",
        Preset::Rows => "rows",
    }
}

/// An icon for a palette entry, from its title.
fn palette_icon(title: &str) -> IconName {
    let starts = |p: &str| title.starts_with(p);
    if starts("Approve") {
        IconName::Check
    } else if starts("Go to") || starts("Focus") {
        IconName::ArrowRight
    } else if starts("New pane") || starts("New") {
        IconName::Plus
    } else if starts("Split") {
        IconName::PanelRight
    } else if starts("Close") {
        IconName::X
    } else if starts("Theme") {
        IconName::Palette
    } else if starts("Layout") {
        IconName::LayoutGrid
    } else if starts("Settings") || starts("Open settings") {
        IconName::Settings
    } else if starts("Browser") || starts("Open URL") {
        IconName::Globe
    } else if starts("Workspace") || starts("Switch") {
        IconName::FolderOpen
    } else if title.contains("model") || title.contains("Model") {
        IconName::Cpu
    } else {
        IconName::Zap
    }
}

fn palette_entries(s: &AppState, recipes: Vec<(String, String)>) -> Vec<Entry> {
    use Command as C;
    let mut plain: Vec<(String, Vec<Command>)> = Vec::new();
    let mut add = |title: &str, cmd: Command| plain.push((title.to_string(), vec![cmd]));
    let overlay = |o| C::Overlay { overlay: o, provider: None };
    let focused = || PaneRef::Focused;

    // Agents waiting for you come first.
    for pane in s.all_panes().filter(|p| !p.archived && p.provider != "shell" && p.status == crate::model::AgentStatus::NeedsInput) {
        let label = s.pane_label(pane);
        add(&format!("Approve: {label}"), C::Approve(PaneRef::Id(pane.id.clone())));
        add(&format!("Go to: {label}"), C::Focus(PaneRef::Id(pane.id.clone())));
    }
    for p in s.providers.iter().filter(|p| p.installed) {
        add(&format!("New pane: {}", p.name), C::Overlay { overlay: Overlay::NewPane, provider: Some(p.id.clone()) });
    }
    let active = s.active();
    if active.is_some_and(|w| w.use_worktrees) {
        add("New pane without worktree", C::Spawn { provider: None, count: 1, prompt: None, worktree: Some(false), model: None });
    }
    add("Split right", C::Split { dir: SplitDir::Row });
    add("Split down", C::Split { dir: SplitDir::Col });
    add("Close pane", C::Close(focused()));
    add("Change model…", overlay(Overlay::Model));
    add("Close all panes", C::CloseAll);
    if let Some(w) = active {
        for (i, id) in w.layout.as_ref().map(leaves).unwrap_or_default().iter().enumerate() {
            if let Some(pane) = w.panes.iter().find(|p| p.id == *id) {
                add(&format!("Focus Pane {}: {}", i + 1, s.provider_name(&pane.provider)), C::Focus(PaneRef::Id(id.clone())));
            }
        }
    }
    add("Next agent needing attention", C::NextAttention);
    add("Toggle zoom", C::Zoom(focused()));
    for (change, axis, delta) in [("wider", SplitDir::Row, 0.05), ("narrower", SplitDir::Row, -0.05), ("taller", SplitDir::Col, 0.05), ("shorter", SplitDir::Col, -0.05)] {
        add(&format!("Make pane {change}"), C::Resize { target: focused(), axis, delta });
    }
    use commands::Direction as D;
    for (name, dir) in [("left", D::Left), ("right", D::Right), ("up", D::Up), ("down", D::Down)] {
        add(&format!("Swap pane {name}"), C::SwapDir(dir));
    }
    add("Bigger terminal text", C::TerminalZoom(1));
    add("Smaller terminal text", C::TerminalZoom(-1));
    add("Default terminal text size", C::TerminalZoom(0));
    add("Archive pane", C::Archive { target: focused(), restore: false });
    for p in s.all_panes().filter(|p| p.archived) {
        add(&format!("Restore from archive: {}", s.pane_label(p)), C::Archive { target: PaneRef::Id(p.id.clone()), restore: true });
    }
    add("Archived sessions", C::View(ViewTarget::Set(View::Archive)));
    add("Overview of all workspaces", C::View(ViewTarget::Set(View::Overview)));
    add("Back to the workspace view", C::View(ViewTarget::Set(View::Workspace)));
    add("Session progress", overlay(Overlay::Progress));
    add("New worktree from a branch…", overlay(Overlay::Branch));
    add("History of merged features", overlay(Overlay::History));
    add("Review finished work", overlay(Overlay::Review));
    add("Show changes", C::ShowChanges(focused()));
    add(if s.focused_pane_id.is_some() { "Open in code editor" } else { "Open in code editor (project folder)" }, C::EditorOpen { target: Some(focused()), editor: None });
    add("Merge worktree into base branch", C::MergeWorktree(focused()));
    add("Discard worktree", C::DiscardWorktree(focused()));
    let toggle = if active.is_some_and(|w| w.use_worktrees) { "Disable worktrees for new agents" } else { "Enable worktrees for new agents" };
    add(toggle, C::ToggleWorktrees);
    add("Equalize panes", C::LayoutEqualize);
    for preset in PRESETS {
        add(&format!("Layout: {}", preset_name(preset)), C::LayoutPreset(Some(preset)));
    }
    for w in &s.workspaces {
        add(&format!("Workspace: {}", w.name), C::WorkspaceSwitch(w.id.clone()));
    }
    add("New workspace…", C::WorkspaceCreate);
    add("Browser: new tab", C::BrowserNewTab);
    add("Browser: close tab", C::BrowserCloseTab);
    add("Browser: back", C::BrowserHistory(-1));
    add("Browser: forward", C::BrowserHistory(1));
    add("Browser: reload", C::BrowserHistory(0));
    add("Browser: open address…", C::BrowserFocusUrl);
    add("Toggle sidebar", C::ToggleSidebar);
    add("Toggle browser", C::ToggleBrowser);
    add("Side panel: terminal", C::SidePanel { panel: SidePanel::Terminal, show: true });
    add("Side panel: Android device", C::SidePanel { panel: SidePanel::Device, show: true });
    add("Side panel: browser", C::SidePanel { panel: SidePanel::Browser, show: true });
    add("Side panel: file viewer", C::SidePanel { panel: SidePanel::Viewer, show: true });
    for t in crate::theme::THEMES.iter() {
        add(&format!("Theme: {}", t.name), C::Theme(t.id.clone()));
    }
    add("Settings", C::Settings(None));
    add("Check for updates", C::CheckUpdates);
    add("Live voice conversation", C::LiveVoice);
    add("Keyboard shortcuts", C::CheatSheet);
    add("Interrupt agent", C::Interrupt(focused()));
    add("Stop all agents", C::InterruptAll);
    add("Approve agent's request", C::Approve(focused()));
    add("Broadcast to all agents…", overlay(Overlay::Broadcast));
    add("Race mode…", overlay(Overlay::Race));
    add("A/B test an agent with and without tools…", overlay(Overlay::AbTest));
    add("Recipes…", overlay(Overlay::Recipes));
    for (id, name) in recipes {
        add(&format!("Recipe: {name}"), C::RecipeLoad(id));
    }
    plain
        .into_iter()
        .map(|(title, commands)| {
            let keys = if commands.len() == 1 { keys_for(&commands[0], s) } else { None };
            Entry { title, keys, commands }
        })
        .collect()
}

impl Root {
    pub(super) fn open_palette(&mut self, query: String, window: &mut Window, cx: &mut Context<Self>) {
        let input = cx.new(|cx| {
            let mut state = InputState::new(window, cx).placeholder("Type a command…");
            state.set_value(query.clone(), window, cx);
            state
        });
        cx.subscribe_in(&input, window, |this, _, event: &InputEvent, window, cx| match event {
            InputEvent::Change => {
                this.palette_selected = 0;
                cx.notify();
            }
            InputEvent::PressEnter { .. } => this.run_palette(this.palette_selected, window, cx),
            _ => {}
        })
        .detach();
        input.read(cx).focus_handle(cx).focus(window, cx);
        self.palette_entries = palette_entries(self.state.read(cx), self.recipe_entries(cx));
        let plugin_entries = self.plugin_palette_entries();
        self.palette_entries.extend(plugin_entries.into_iter().map(|(title, command)| Entry { title, keys: None, commands: vec![command] }));
        self.palette_input = Some(input);
        self.palette_selected = 0;
        self.update_state(cx, |s| {
            s.palette_query = Some(query);
            s.cheat_sheet_open = false;
        });
    }

    pub(super) fn close_palette(&mut self, cx: &mut Context<Self>) {
        self.palette_input = None;
        self.focused_view = None;
        self.update_state(cx, |s| s.palette_query = None);
    }

    fn palette_results(&self, cx: &App) -> Vec<usize> {
        let query = self.palette_input.as_ref().map(|i| i.read(cx).value().to_string()).unwrap_or_default();
        let mut scored: Vec<(f64, usize)> = self.palette_entries.iter().enumerate().filter_map(|(i, e)| score(&query, &e.title).map(|s| (s, i))).collect();
        scored.sort_by(|a, b| a.0.total_cmp(&b.0));
        scored.into_iter().map(|(_, i)| i).collect()
    }

    fn run_palette(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(entry) = self.palette_results(cx).get(index).copied() else { return };
        let commands = self.palette_entries[entry].commands.clone();
        self.close_palette(cx);
        for command in commands {
            self.execute(command, window, cx);
        }
    }

    pub(super) fn render_palette(&mut self, c: &Colors, cx: &mut Context<Self>) -> Option<AnyElement> {
        let input = self.palette_input.clone()?;
        let c = *c;
        let results = self.palette_results(cx);
        let selected = self.palette_selected.min(results.len().saturating_sub(1));
        let mut list = div().id("palette-list").overflow_y_scroll().p_1p5().flex().flex_col();
        if results.is_empty() {
            list = list.child(div().px_3().py_6().text_center().text_xs().text_color(c.subtle).child("No matching commands"));
        }
        for (i, entry) in results.iter().take(200).enumerate() {
            let e = &self.palette_entries[*entry];
            let waiting = e.title.starts_with("Approve: ") || e.title.starts_with("Go to: ");
            list = list.child(
                div().id(("palette-row", i)).flex().items_center().gap_2p5().rounded_lg().px_2p5().h(px(34.0)).flex_shrink_0().text_size(px(13.0)).cursor_pointer()
                    .text_color(if i == selected { c.fg } else { c.muted })
                    .when(i == selected, |d| d.bg(c.active))
                    .child(div().size(px(24.0)).flex_shrink_0().flex().items_center().justify_center().rounded_md()
                        .bg(if waiting { Hsla { a: 0.14, ..c.warning } } else { c.hover })
                        .text_color(if waiting { c.warning } else if i == selected { c.accent } else { c.subtle })
                        .child(Icon::new(palette_icon(&e.title)).size(px(13.0))))
                    .on_mouse_move(cx.listener(move |this, _, _, cx| {
                        if this.palette_selected != i {
                            this.palette_selected = i;
                            cx.notify();
                        }
                    }))
                    .on_click(cx.listener(move |this, _, window, cx| this.run_palette(i, window, cx)))
                    .child(div().flex_1().truncate().child(e.title.clone()))
                    .when_some(e.keys.clone(), |d, k| d.child(div().rounded_sm().border_1().border_color(c.border).bg(c.panel).px_1p5().py(px(1.0)).text_size(px(10.5)).text_color(c.subtle).child(k))),
            );
        }
        let total = results.len();
        Some(
            div()
                .id("palette-backdrop")
                .absolute().size_full().flex().justify_center().pt(px(110.0)).bg(hsla(0.0, 0.0, 0.0, 0.4)).occlude()
                .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| this.close_palette(cx)))
                .child(
                    div()
                        .id("palette")
                        .w(px(620.0)).max_h(px(540.0)).flex().flex_col().overflow_hidden().rounded(px(14.0)).border_1().border_color(c.border_strong).bg(c.raised).shadow_2xl()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .capture_action(cx.listener(move |this, _: &MoveDown, _, cx| {
                            this.palette_selected = (this.palette_selected + 1).min(total.saturating_sub(1));
                            cx.stop_propagation();
                            cx.notify();
                        }))
                        .capture_action(cx.listener(|this, _: &MoveUp, _, cx| {
                            this.palette_selected = this.palette_selected.saturating_sub(1);
                            cx.stop_propagation();
                            cx.notify();
                        }))
                        .child(div().flex().items_center().gap_2p5().px_4().h(px(50.0)).flex_shrink_0().border_b_1().border_color(c.border)
                            .child(Icon::new(IconName::Search).size(px(16.0)).text_color(c.subtle))
                            .child(div().flex_1().child(Input::new(&input).appearance(false).text_size(px(15.0)))))
                        .child(list.flex_1().min_h_0())
                        .child(div().flex().items_center().gap_3().px_4().h(px(34.0)).flex_shrink_0().border_t_1().border_color(c.border).text_size(px(11.0)).text_color(c.subtle)
                            .child("↑↓ choose").child("↵ run").child("esc close").child(div().flex_1()).child(format!("{total} commands"))),
                )
                .into_any_element(),
        )
    }

    pub(super) fn render_cheat_sheet(&self, c: &Colors, cx: &mut Context<Self>) -> Option<AnyElement> {
        let s = self.state.read(cx);
        if !s.cheat_sheet_open {
            return None;
        }
        let c = *c;
        let mut columns = div().flex().gap_6();
        let recording_hint = self.recording.is_some();
        for group in ["Panes", "Layout", "Workspaces", "App"] {
            let mut column = div().flex_1().flex().flex_col().gap_1().child(div().text_xs().text_color(c.subtle).mb_1().child(group));
            let mut seen: Vec<&str> = Vec::new();
            for b in BINDINGS.iter().filter(|b| b.group == group) {
                if seen.contains(&b.id.as_str()) {
                    continue;
                }
                seen.push(&b.id);
                let chord = chord_for(b, &s.shortcuts).unwrap_or_default();
                let (title, chord) = match b.digit {
                    Some(_) => (b.title.rsplit_once(' ').map_or(b.title.clone(), |(h, _)| format!("{h} 1–9")), chord.trim_end_matches(|ch: char| ch.is_ascii_digit()).to_string() + "#"),
                    None => (b.title.clone(), chord),
                };
                let (record_id, reset_id) = (b.id.clone(), b.id.clone());
                let recording = self.recording.as_ref() == Some(&b.id);
                let customized = s.shortcuts.contains_key(&b.id);
                column = column.child(
                    div().flex().items_center().gap_2().text_xs()
                        .child(div().flex_1().text_color(c.muted).truncate().child(title))
                        .when(customized, |d| {
                            d.child(
                                div().id(SharedString::from(format!("reset-{}", b.id))).text_color(c.subtle).cursor_pointer()
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        let id = reset_id.clone();
                                        this.update_state(cx, |s| {
                                            s.shortcuts.remove(&id);
                                        });
                                    }))
                                    .child(Icon::new(IconName::RotateCcw).size(px(10.0))),
                            )
                        })
                        .child(
                            div().id(SharedString::from(format!("record-{}", b.id))).rounded_sm().px_1().cursor_pointer().hover(move |d| d.bg(c.hover))
                                .text_color(if recording { c.accent } else { c.subtle })
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.recording = if this.recording.as_ref() == Some(&record_id) { None } else { Some(record_id.clone()) };
                                    cx.notify();
                                }))
                                .child(if recording { "Press keys…".to_string() } else { format_chord(&chord) }),
                        ),
                );
            }
            columns = columns.child(column);
        }
        Some(
            div().id("cheat-backdrop").absolute().size_full().flex().items_center().justify_center().bg(hsla(0.0, 0.0, 0.0, 0.4)).occlude()
                .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| this.update_state(cx, |s| s.cheat_sheet_open = false)))
                .child(
                    div().id("cheat").w(px(1040.0)).max_h(px(640.0)).overflow_y_scroll().rounded_xl().border_1().border_color(c.border_strong).bg(c.raised).p_5()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(div().text_sm().mb_3().child(if recording_hint { "Press the new shortcut (esc cancels)" } else { "Keyboard shortcuts · click one to change it" }))
                        .child(columns),
                )
                .into_any_element(),
        )
    }

    pub(super) fn reset_question_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.question_picked.clear();
        if self.questions.is_empty() {
            self.question_input = None;
            return;
        }
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Type your own answer…"));
        cx.subscribe_in(&input, window, |this, input, event: &InputEvent, window, cx| {
            if matches!(event, InputEvent::PressEnter { .. }) {
                let text = input.read(cx).value().to_string();
                this.submit_question(text, window, cx);
            }
            if matches!(event, InputEvent::Change) {
                cx.notify();
            }
        })
        .detach();
        self.question_input = Some(input);
    }

    fn submit_question(&mut self, typed: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(pending) = self.questions.front() else { return };
        let q = &pending.questions[pending.answers.len()];
        let answer = if q.multi {
            let mut picked = self.question_picked.clone();
            picked.sort_unstable();
            let mut parts: Vec<String> = picked.iter().map(|i| q.options[*i].label.clone()).collect();
            if !typed.trim().is_empty() {
                parts.push(typed.trim().to_string());
            }
            parts.join(", ")
        } else {
            typed
        };
        self.answer_question(answer, window, cx);
    }

    pub(super) fn render_question(&mut self, c: &Colors, cx: &mut Context<Self>) -> Option<AnyElement> {
        let c = *c;
        let pending = self.questions.front()?;
        let waiting = self.questions.len() - 1;
        let total = pending.questions.len();
        let index = pending.answers.len();
        let q = pending.questions.get(index)?.clone();
        let s = self.state.read(cx);
        let asker = s.pane(&pending.pane_id).map_or(String::new(), |p| {
            let ws = s.workspaces.iter().find(|w| w.id == p.workspace_id).map_or(String::new(), |w| format!(" · {}", w.name));
            format!("{}{ws}", s.provider_name(&p.provider))
        });
        let input = self.question_input.clone()?;
        let typed = input.read(cx).value().trim().to_string();
        let multi = q.multi;
        let mut body = div().id("question-body").flex().flex_col().gap_2().overflow_y_scroll().px_5().py_4().child(div().text_size(px(13.0)).text_color(c.fg).child(q.question.clone()));
        for (i, option) in q.options.iter().enumerate() {
            let picked = self.question_picked.contains(&i);
            let label = option.label.clone();
            body = body.child(
                div().id(("question-option", i)).flex().items_start().gap_2().rounded_md().border_1().px_3().py_2().text_size(px(13.0)).cursor_pointer()
                    .border_color(if picked { c.accent } else { c.border })
                    .hover(move |d| d.bg(c.hover))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if multi {
                            match this.question_picked.iter().position(|x| *x == i) {
                                Some(at) => {
                                    this.question_picked.remove(at);
                                }
                                None => this.question_picked.push(i),
                            }
                            cx.notify();
                        } else {
                            this.answer_question(label.clone(), window, cx);
                        }
                    }))
                    .child(div().w_4().flex_shrink_0().text_color(c.subtle).child(if multi { if picked { "☑".to_string() } else { "☐".to_string() } } else { format!("{}.", i + 1) }))
                    .child(
                        div().min_w_0().flex().flex_col().child(div().text_color(c.fg).child(option.label.clone()))
                            .when_some(option.description.clone(), |d, t| d.child(div().text_xs().text_color(c.subtle).child(t))),
                    ),
            );
        }
        body = body.child(Input::new(&input));
        let can_submit = if multi { !self.question_picked.is_empty() || !typed.is_empty() } else { !typed.is_empty() };
        let submit_label = if index + 1 < total { "Next" } else { "Answer" };
        Some(
            div().id("question-backdrop").absolute().size_full().flex().items_center().justify_center().bg(hsla(0.0, 0.0, 0.0, 0.4)).occlude()
                .child(
                    div().w(px(500.0)).max_h(px(620.0)).flex().flex_col().rounded_xl().border_1().border_color(c.border_strong).bg(c.raised)
                        .child(
                            div().flex().items_start().gap_3().border_b_1().border_color(c.border).px_5().py_3()
                                .child(Icon::new(IconName::CircleQuestionMark).size(px(16.0)).text_color(c.accent))
                                .child(
                                    div().flex_1().min_w_0().flex().flex_col()
                                        .child(div().text_sm().child(format!("{}{}", q.header.clone().unwrap_or_else(|| "Question".to_string()), if total > 1 { format!("  {}/{total}", index + 1) } else { String::new() })))
                                        .child(div().text_xs().text_color(c.subtle).child(format!("{asker} asks"))),
                                ),
                        )
                        .child(body)
                        .child(
                            div().flex().items_center().gap_2().border_t_1().border_color(c.border).px_5().py_3()
                                .when(waiting > 0, |d| d.child(div().text_xs().text_color(c.subtle).child(format!("{waiting} more waiting"))))
                                .child(div().flex_1())
                                .child(
                                    div().id("question-dismiss").rounded_md().px_2p5().py_1().text_xs().text_color(c.muted).cursor_pointer().hover(move |d| d.bg(c.hover).text_color(c.fg))
                                        .on_click(cx.listener(|this, _, window, cx| this.dismiss_question(window, cx)))
                                        .child("Dismiss"),
                                )
                                .when(can_submit, |d| {
                                    d.child(
                                        div().id("question-submit").rounded_md().bg(c.accent).text_color(c.on_accent).px_3().py_1().text_xs().cursor_pointer()
                                            .on_click(cx.listener(move |this, _, window, cx| {
                                                let text = this.question_input.as_ref().map(|i| i.read(cx).value().to_string()).unwrap_or_default();
                                                this.submit_question(text, window, cx);
                                            }))
                                            .child(submit_label),
                                    )
                                }),
                        ),
                )
                .into_any_element(),
        )
    }

    pub(super) fn render_toasts(&self, c: &Colors, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.toasts.is_empty() {
            return None;
        }
        let c = *c;
        let mut stack = div().absolute().bottom(px(16.0)).right(px(16.0)).flex().flex_col().gap_2().items_end();
        for (i, toast) in self.toasts.iter().enumerate() {
            let button = toast.action.clone().map(|(label, action)| {
                div().id(("toast-action", i)).rounded_md().bg(c.accent).px_2().py_0p5().text_color(c.on_accent).cursor_pointer().child(label).on_click(cx.listener(move |this, _, window, cx| {
                    this.execute(action.clone(), window, cx);
                }))
            });
            stack = stack.child(div().flex().items_center().gap_2().max_w(px(420.0)).rounded_md().border_1().border_color(c.border_strong).bg(c.raised).px_3().py_1p5().text_xs().text_color(c.fg).child(toast.text.clone()).children(button));
        }
        Some(stack.into_any_element())
    }
}

#[cfg(test)]
mod tests {
    use super::score;

    #[test]
    fn scoring_prefers_word_starts() {
        assert!(score("xyz", "Split right").is_none());
        assert_eq!(score("", "anything"), Some(0.0));
        let start = score("sr", "Split right").unwrap();
        let middle = score("pi", "Split right").unwrap();
        assert!(start < middle);
    }
}
