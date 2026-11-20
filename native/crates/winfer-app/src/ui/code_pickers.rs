//! The model, effort and mode pickers of the chat pane: popovers that open upward from their
//! buttons on the composer's bottom bar. The model picker has a backend rail, a search field,
//! favourites and a reasoning column; the effort picker is its own component (`EffortPicker`).

use super::code_view::{CodeView, rise};
use crate::code::names::{BACKENDS, ModelTier, backend_of, base_spec, fast_spec, is_fast, model_name, model_tier};
use crate::code::protocol::*;
use crate::code::state::Sheet;
use crate::theme::Colors;
use gpui_kit::assets::IconName;
use gpui_kit::component::Icon;
use gpui_kit::component::input::{Input, InputEvent, InputState, MoveDown, MoveUp};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use std::rc::Rc;
use std::time::Instant;

/// Levels offered when the model doesn't list its own.
pub const EFFORTS: [&str; 6] = ["minimal", "low", "medium", "high", "xhigh", "max"];

const FAVORITES: &str = "★";
const SHOWN: usize = 8;

fn alpha(c: Hsla, a: f32) -> Hsla {
    Hsla { a, ..c }
}

pub fn effort_label(level: &str) -> String {
    match level {
        "default" => "Default".into(),
        "xhigh" => "Extra high".into(),
        l => l.chars().next().map_or(String::new(), |c| c.to_uppercase().chain(l.chars().skip(1)).collect()),
    }
}

pub fn effort_hint(level: &str) -> &'static str {
    match level {
        "default" => "The model's own default",
        "minimal" => "Barely any reasoning, fastest",
        "low" => "Quick answers, light reasoning",
        "medium" => "Balanced",
        "high" => "Thorough reasoning",
        "xhigh" => "Very thorough, slower",
        "max" => "As much reasoning as the model can do",
        "ultra" => "Beyond max, for the hardest problems",
        _ => "",
    }
}

/// A filled star (favorites, defaults), in one colour.
pub fn star_icon(tint: Hsla, size: f32) -> AnyElement {
    svg().path("solid/star.svg").size(px(size)).flex_shrink_0().text_color(tint).into_any_element()
}

/// A level's solid glyph in one colour: rising bars for the steps, a flame, a bolt and a sparkle above.
pub fn effort_glyph(tint: Hsla, level: &str, size: f32) -> AnyElement {
    let solid = |file: &str| svg().path(SharedString::from(format!("solid/{file}.svg"))).size(px(size)).flex_shrink_0().text_color(tint).into_any_element();
    let filled = match level {
        "minimal" => 1,
        "low" => 2,
        "medium" => 3,
        "high" => 4,
        "xhigh" => 5,
        _ => 0,
    };
    match level {
        "max" => solid("flame"),
        "ultra" => solid("bolt"),
        "default" => solid("sparkle"),
        _ => div().w(px(size)).h(px(size)).flex().items_end().justify_center().gap(px(size * 0.1)).children((1..=5).map(|b| {
            div().w(px(size * 0.14)).h(px(size * (0.22 + 0.15 * b as f32))).rounded_sm().bg(if b <= filled { tint } else { alpha(tint, 0.25) })
        })).into_any_element(),
    }
}

/// The levels a model offers, "default" first.
pub fn effort_levels(offered: Option<Vec<String>>) -> Vec<String> {
    match offered {
        Some(l) if !l.is_empty() => std::iter::once("default".to_string()).chain(l).collect(),
        Some(_) => vec!["default".to_string()],
        None => std::iter::once("default").chain(EFFORTS).map(String::from).collect(),
    }
}

/// The reasoning-effort list: an icon, the level and a check on the current one.
#[derive(IntoElement)]
pub struct EffortPicker {
    c: Colors,
    levels: Vec<String>,
    current: String,
    on_pick: Rc<dyn Fn(String, &mut Window, &mut App)>,
}

impl EffortPicker {
    pub fn new(c: Colors, levels: Vec<String>, current: String, on_pick: impl Fn(String, &mut Window, &mut App) + 'static) -> Self {
        Self { c, levels, current, on_pick: Rc::new(on_pick) }
    }
}

impl RenderOnce for EffortPicker {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let c = self.c;
        // Lowest at the bottom, next to the button the popover opens from.
        div().flex().flex_col().gap_0p5().children(self.levels.into_iter().enumerate().rev().map(|(i, level)| {
            let on = level == self.current;
            let pick = self.on_pick.clone();
            let l = level.clone();
            div().id(("effort", i)).flex().items_center().gap_2p5().h(px(34.0)).px_2p5().rounded_lg().cursor_pointer().text_size(px(13.0))
                .when(on, |d| d.bg(alpha(c.accent, 0.12)).text_color(c.fg))
                .when(!on, |d| d.text_color(c.muted).hover(move |d| d.bg(c.hover).text_color(c.fg)))
                .on_click(move |_, window, cx| pick(l.clone(), window, cx))
                .child(effort_glyph(if on { c.accent } else { c.muted }, &level, 18.0))
                .child(div().flex_1().child(effort_label(&level)))
                .when(on, |d| d.child(Icon::new(IconName::Check).size(px(14.0)).text_color(c.accent)))
        }))
    }
}

/// A provider's brand mark in its colour.
pub(super) fn mark(c: Colors, backend: &str, size: f32, opacity: f32) -> AnyElement {
    let (file, color) = match backend {
        "claude" => ("claude", c.claude),
        "chatgpt" => ("openai", c.fg),
        "codex" => ("codex", c.fg),
        "agy" => ("agy", super::super::theme::hex("#4f8cff")),
        "opencode" => ("opencode", super::super::theme::hex("#f5b544")),
        "cursor" => ("cursor", super::super::theme::hex("#a0a0ff")),
        "pi" => ("pi", super::super::theme::hex("#7ee787")),
        "muse" => ("muse", super::super::theme::hex("#fb7185")),
        "winfer" => ("winfer", c.accent),
        "grok" => ("grok", c.fg),
        "shell" => return Icon::new(IconName::SquareTerminal).size(px(size)).flex_shrink_0().text_color(alpha(c.muted, opacity)).into_any_element(),
        _ => ("local", super::super::theme::hex("#7ee787")),
    };
    svg().path(SharedString::from(format!("providers/{file}.svg"))).size(px(size)).flex_shrink_0().text_color(alpha(color, opacity)).into_any_element()
}

/// A mode's solid glyph in one colour: a question, a pencil, a list, a flag and a bolt.
pub fn mode_glyph(tint: Hsla, mode: Mode, size: f32) -> AnyElement {
    let file = match mode {
        Mode::Ask => "mode-ask",
        Mode::AutoEdit => "mode-edit",
        Mode::Plan => "mode-plan",
        Mode::YoloPlan => "mode-flag",
        Mode::Yolo => "mode-yolo",
    };
    svg().path(SharedString::from(format!("solid/{file}.svg"))).size(px(size)).flex_shrink_0().text_color(tint).into_any_element()
}

/// Picker state that lives while a popover is open.
#[derive(Default)]
pub struct PickerState {
    pub search: Option<Entity<InputState>>,
    /// The rail's selection: a backend id or the favourites star.
    pub tab: Option<String>,
    pub selected: usize,
    pub show_all: bool,
    /// The specs on screen, in order, for ⌘1–9 and the arrow keys.
    pub rows: Vec<String>,
    /// A click outside just closed this one: the button's own click must not reopen it.
    pub out_close: Option<(Sheet, Instant)>,
}

impl CodeView {
    /// Opens a picker from its button, or closes it when it is the one open.
    pub(super) fn toggle_picker(&mut self, sheet: Sheet, cx: &mut Context<Self>) {
        if self.model.read(cx).state.sheet == Some(sheet) {
            self.set_sheet(None, cx);
            return;
        }
        if self.picker.out_close.is_some_and(|(s, at)| s == sheet && at.elapsed().as_millis() < 300) {
            return;
        }
        self.picker.tab = None;
        self.picker.selected = 0;
        self.picker.show_all = false;
        self.set_sheet(Some(sheet), cx);
    }

    /// Creates and focuses the search field while the model picker is open; drops it after.
    pub(super) fn ensure_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let open = self.model.read(cx).state.sheet == Some(Sheet::Model);
        if open && self.picker.search.is_none() {
            let input = cx.new(|cx| InputState::new(window, cx).placeholder("Search models…"));
            cx.subscribe_in(&input, window, |this, _, event: &InputEvent, window, cx| match event {
                InputEvent::Change => {
                    this.picker.selected = 0;
                    cx.notify();
                }
                InputEvent::PressEnter { .. } => {
                    let spec = this.picker.rows.get(this.picker.selected).cloned();
                    if let Some(spec) = spec {
                        this.pick_model(&spec, window, cx);
                    }
                }
                _ => {}
            })
            .detach();
            input.read(cx).focus_handle(cx).focus(window, cx);
            self.picker.search = Some(input);
        } else if !open && self.picker.search.is_some() {
            self.picker.search = None;
            self.focus_composer(window, cx);
        }
    }

    pub(super) fn pick_model(&mut self, spec: &str, _window: &mut Window, cx: &mut Context<Self>) {
        let spec = spec.to_string();
        self.send(Cmd::SetModel(spec), cx);
        cx.notify();
    }

    /// ⌘1–9 in the model picker.
    pub(super) fn pick_nth(&mut self, n: usize, window: &mut Window, cx: &mut Context<Self>) -> bool {
        match self.picker.rows.get(n).cloned() {
            Some(spec) => {
                self.pick_model(&spec, window, cx);
                true
            }
            None => false,
        }
    }

    pub(super) fn move_selection(&mut self, down: bool, cx: &mut Context<Self>) {
        let last = self.picker.rows.len().saturating_sub(1);
        self.picker.selected = if down { (self.picker.selected + 1).min(last) } else { self.picker.selected.saturating_sub(1) };
        cx.notify();
    }

    fn favorites(&self, cx: &App) -> Vec<String> {
        self.app.read(cx).rest.get("codeWeb").and_then(|v| v["favModels"].as_array()).map_or(Vec::new(), |a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
    }

    fn toggle_favorite(&mut self, spec: &str, cx: &mut Context<Self>) {
        let mut favs = self.favorites(cx);
        match favs.iter().position(|f| f == spec) {
            Some(at) => {
                favs.remove(at);
            }
            None => favs.push(spec.to_string()),
        }
        self.app.update(cx, |s, cx| {
            let entry = s.rest.entry("codeWeb".to_string()).or_insert_with(|| serde_json::json!({}));
            if !entry.is_object() {
                *entry = serde_json::json!({});
            }
            entry["favModels"] = serde_json::json!(favs);
            cx.notify();
        });
        cx.notify();
    }

    /// Wraps a popover so it opens upward from the button it is a child of.
    fn popover(&mut self, sheet: Sheet, c: Colors, panel: Div, cx: &mut Context<Self>) -> AnyElement {
        let panel = panel.id("cw-popover").occlude().rounded_xl().border_1().border_color(c.border_strong).bg(c.raised).shadow_lg().overflow_hidden()
            .on_mouse_down_out(cx.listener(move |this, _, _, cx| {
                this.picker.out_close = Some((sheet, Instant::now()));
                this.set_sheet(None, cx);
            }));
        deferred(anchored().anchor(Anchor::BottomLeft).offset(point(px(0.0), px(-8.0))).snap_to_window_with_margin(px(8.0)).child(rise(panel, SharedString::from(format!("pop-{sheet:?}")), 0, 8.0))).with_priority(10).into_any_element()
    }

    pub(super) fn model_popover(&mut self, c: Colors, cx: &mut Context<Self>) -> AnyElement {
        let (models, current, fav) = {
            let st = &self.model.read(cx).state;
            (st.models.clone(), st.model.clone(), self.favorites(cx))
        };
        let query = self.picker.search.as_ref().map(|i| i.read(cx).value().trim().to_lowercase()).unwrap_or_default();
        let listed: Vec<&ModelInfo> = models.iter().filter(|m| !is_fast(&m.spec)).collect();
        let backends: Vec<(&str, &str)> = BACKENDS.into_iter().filter(|(id, _)| listed.iter().any(|m| backend_of(&m.spec).map(|b| b.0) == Some(*id))).collect();
        let tab = self.picker.tab.clone().unwrap_or_else(|| backend_of(&current).map(|b| b.0.to_string()).or_else(|| backends.first().map(|b| b.0.to_string())).unwrap_or_else(|| FAVORITES.to_string()));
        let mut rows: Vec<&ModelInfo> = listed.iter().copied().filter(|m| {
            if !query.is_empty() {
                m.label.to_lowercase().contains(&query) || m.spec.to_lowercase().contains(&query)
            } else if tab == FAVORITES {
                fav.contains(&m.spec)
            } else {
                backend_of(&m.spec).map(|b| b.0) == Some(tab.as_str())
            }
        }).collect();
        let total = rows.len();
        let capped = !self.picker.show_all && query.is_empty() && total > SHOWN;
        if capped {
            rows.truncate(SHOWN);
        }
        self.picker.rows = rows.iter().map(|m| m.spec.clone()).collect();
        let selected_row = self.picker.selected.min(rows.len().saturating_sub(1));

        let mut rail = div().w(px(54.0)).flex_shrink_0().flex().flex_col().items_center().gap_1p5().py_2p5().border_r_1().border_color(c.border);
        let mut tabs: Vec<String> = vec![FAVORITES.to_string()];
        tabs.extend(backends.iter().map(|(id, _)| id.to_string()));
        for (i, id) in tabs.into_iter().enumerate() {
            let on = tab == id && query.is_empty();
            let pick = id.clone();
            rail = rail.child(div().id(("pk-tab", i)).relative().size(px(34.0)).flex().items_center().justify_center().rounded_lg().cursor_pointer()
                .when(on, |d| d.bg(c.hover)).hover(move |d| d.bg(c.hover))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.picker.tab = Some(pick.clone());
                    this.picker.selected = 0;
                    this.picker.show_all = false;
                    cx.notify();
                }))
                .when(on, |d| d.child(div().absolute().right(px(-10.0)).top(px(8.0)).w(px(2.0)).h(px(18.0)).rounded_full().bg(c.accent)))
                .child(if id == FAVORITES { star_icon(if on { c.warning } else { alpha(c.warning, 0.65) }, 17.0) } else { mark(c, &id, 18.0, if on { 1.0 } else { 0.6 }) }));
        }

        let mut list = div().id("pk-list").flex_1().min_h_0().overflow_y_scroll().p_2().flex().flex_col().gap_0p5();
        if rows.is_empty() {
            let text = if !query.is_empty() { "No model matches" } else if tab == FAVORITES { "Star a model to keep it here" } else { "No models listed yet. They appear once the session has started." };
            list = list.child(div().px_3().py_4().text_size(px(12.0)).text_color(c.subtle).child(text));
        }
        for (i, info) in rows.iter().enumerate() {
            let spec = info.spec.clone();
            let name = if info.label.is_empty() { model_name(&spec, None) } else { info.label.clone() };
            let on = base_spec(&current) == spec;
            let twin = fast_spec(&spec, &models);
            let on_fast = twin.as_deref() == Some(current.as_str());
            let is_fav = fav.contains(&spec);
            let backend = backend_of(&spec).map_or("", |b| b.0);
            let tier = model_tier(&spec, &info.detail);
            let group = SharedString::from(format!("pk-row-{i}"));
            let (pick, star, zap) = (spec.clone(), spec.clone(), twin.clone().map(|t| if on_fast { spec.clone() } else { t }));
            list = list.child(div().id(("pk-row", i)).group(group.clone()).h(px(36.0)).flex().items_center().gap_2p5().px_2p5().rounded_lg().cursor_pointer().text_size(px(13.0)).text_color(c.fg)
                .when(i == selected_row, |d| d.bg(c.hover))
                .hover(move |d| d.bg(c.hover))
                .on_click(cx.listener(move |this, _, window, cx| this.pick_model(&pick, window, cx)))
                .child(mark(c, backend, 16.0, 1.0))
                .child(div().min_w_0().truncate().child(name))
                .when(on || on_fast, |d| d.child(Icon::new(IconName::Check).size(px(13.0)).text_color(c.muted)))
                .when(matches!(tier, ModelTier::Fast), |d| d.child(div().text_size(px(10.0)).px_1p5().rounded_full().bg(alpha(c.warning, 0.14)).text_color(c.warning).child("light")))
                .child(div().flex_1())
                .when_some(zap, |d, z| d.child(div().id(("pk-fast", i)).size(px(24.0)).flex().items_center().justify_center().rounded_md().hover(move |d| d.bg(c.active))
                    .tooltip(super::code_view::tip("Fast mode"))
                    .on_click(cx.listener(move |this, _, window, cx| { cx.stop_propagation(); this.pick_model(&z, window, cx); }))
                    .child(Icon::new(IconName::Zap).size(px(13.0)).text_color(if on_fast { c.warning } else { c.subtle }))))
                .child(div().id(("pk-star", i)).size(px(24.0)).flex().items_center().justify_center().rounded_md().hover(move |d| d.bg(c.active))
                    .when(!is_fav, |d| d.opacity(0.0).group_hover(group.clone(), |d| d.opacity(1.0)))
                    .on_click(cx.listener(move |this, _, _, cx| { cx.stop_propagation(); this.toggle_favorite(&star, cx); }))
                    .child(star_icon(if is_fav { c.warning } else { c.subtle }, 13.0)))
                .when(i < 9, |d| d.child(div().min_w(px(30.0)).h(px(20.0)).flex().items_center().justify_center().rounded_md().bg(c.hover).text_size(px(11.0)).text_color(c.subtle).child(format!("⌘{}", i + 1)))));
        }
        if total > SHOWN && query.is_empty() {
            let all = self.picker.show_all;
            list = list.child(div().id("pk-more").px_2p5().py_1p5().cursor_pointer().text_size(px(12.0)).text_color(c.subtle).hover(move |d| d.text_color(c.fg))
                .on_click(cx.listener(|this, _, _, cx| { this.picker.show_all = !this.picker.show_all; cx.notify(); }))
                .child(if all { "Show less".to_string() } else { format!("Show {} more", total - SHOWN) }));
        }

        let search = div().flex().items_center().gap_2().h(px(44.0)).px_3().border_b_1().border_color(c.border)
            .child(Icon::new(IconName::Search).size(px(15.0)).text_color(c.subtle))
            .child(div().flex_1().when_some(self.picker.search.clone(), |d, input| d.child(Input::new(&input).appearance(false))));

        let st = &self.model.read(cx).state;
        let offered = models.iter().find(|i| base_spec(&i.spec) == base_spec(&st.model)).and_then(|i| i.efforts.clone());
        let levels = effort_levels(offered);
        let effort = if st.effort.is_empty() { "default".to_string() } else { st.effort.clone() };
        let model_label = if st.model.is_empty() { String::new() } else { model_name(&st.model, st.actual_model.as_deref()) };
        let view = cx.entity();
        let reasoning = div().w(px(196.0)).flex_shrink_0().flex().flex_col().border_l_1().border_color(c.border)
            .child(div().flex().items_center().justify_between().h(px(44.0)).px_3().text_size(px(12.0)).text_color(c.subtle).child("Reasoning").child(div().truncate().child(model_label)))
            .child(div().id("pk-effort").flex_1().min_h_0().overflow_y_scroll().p_2().child(EffortPicker::new(c, levels, effort, move |level, _, cx| {
                let _ = view.update(cx, |this, cx| {
                    this.send(Cmd::SetEffort(if level == "default" { None } else { Some(level) }), cx);
                    this.set_sheet(None, cx);
                });
            })));

        let panel = div().w(px(580.0)).h(px(372.0)).flex()
            .on_action(cx.listener(|this, _: &MoveUp, _, cx| this.move_selection(false, cx)))
            .on_action(cx.listener(|this, _: &MoveDown, _, cx| this.move_selection(true, cx)))
            .child(rail)
            .child(div().flex_1().min_w_0().flex().flex_col().child(search).child(list))
            .child(reasoning);
        self.popover(Sheet::Model, c, panel, cx)
    }

    pub(super) fn effort_popover(&mut self, c: Colors, cx: &mut Context<Self>) -> AnyElement {
        let st = &self.model.read(cx).state;
        let offered = st.models.iter().find(|i| base_spec(&i.spec) == base_spec(&st.model)).and_then(|i| i.efforts.clone());
        let levels = effort_levels(offered);
        let effort = if st.effort.is_empty() { "default".to_string() } else { st.effort.clone() };
        let model_label = if st.model.is_empty() { String::new() } else { model_name(&st.model, st.actual_model.as_deref()) };
        let hint = effort_hint(&effort);
        let view = cx.entity();
        let panel = div().w(px(260.0)).flex().flex_col()
            .child(div().flex().items_center().justify_between().h(px(40.0)).px_3().border_b_1().border_color(c.border).text_size(px(12.0)).text_color(c.subtle).child("Reasoning").child(div().truncate().child(model_label)))
            .child(div().p_2().child(EffortPicker::new(c, levels, effort, move |level, _, cx| {
                let _ = view.update(cx, |this, cx| {
                    this.send(Cmd::SetEffort(if level == "default" { None } else { Some(level) }), cx);
                    this.set_sheet(None, cx);
                });
            })))
            .child(div().px_3().pb_2p5().pt_0p5().text_size(px(11.5)).text_color(c.subtle).child(hint));
        self.popover(Sheet::Effort, c, panel, cx)
    }

    pub(super) fn mode_popover(&mut self, c: Colors, cx: &mut Context<Self>) -> AnyElement {
        let current = self.model.read(cx).state.mode;
        let mut list = div().p_2().flex().flex_col().gap_0p5();
        for (i, mode) in Mode::ALL.into_iter().enumerate() {
            let on = mode == current;
            list = list.child(div().id(("pk-mode", i)).flex().items_center().gap_2p5().px_2p5().py_1p5().rounded_lg().cursor_pointer()
                .when(on, |d| d.bg(alpha(c.accent, 0.12)))
                .when(!on, |d| d.hover(move |d| d.bg(c.hover)))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.send(Cmd::SetMode(mode), cx);
                    this.set_sheet(None, cx);
                }))
                .child(mode_glyph(if on { c.accent } else { c.muted }, mode, 18.0))
                .child(div().flex_1().flex().flex_col().child(div().text_size(px(13.0)).text_color(c.fg).child(mode.label())).child(div().text_size(px(11.5)).text_color(c.subtle).child(mode.hint())))
                .when(on, |d| d.child(Icon::new(IconName::Check).size(px(14.0)).text_color(c.accent))));
        }
        let panel = div().w(px(330.0)).flex().flex_col().child(list).child(div().px_3().pb_2().text_size(px(11.0)).text_color(c.subtle).child("⇧⇥ cycles the mode"));
        self.popover(Sheet::Mode, c, panel, cx)
    }
}
