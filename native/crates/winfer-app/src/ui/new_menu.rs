//! The title bar's New button: a provider picker for opening a pane (port of
//! `features/panes/NewPaneMenu.tsx`). The star marks (and sets) the default agent, the chevron
//! lists a provider's models, and each row shows how much of its usage limit is left.

use super::*;
use crate::ui::code_pickers::mark;

/// Which provider's model list is open, while the menu is.
#[derive(Default)]
pub(super) struct NewMenu {
    pub open: bool,
    pub expanded: Option<String>,
}

/// The mark that stands for a provider.
fn backend_key(id: &str) -> &str {
    id
}

impl Root {
    pub(super) fn toggle_new_menu(&mut self, cx: &mut Context<Self>) {
        self.new_menu.open = !self.new_menu.open;
        self.new_menu.expanded = None;
        if self.new_menu.open {
            self.load_all_models(cx);
        }
        cx.notify();
    }

    /// "86%", or "0% · resets Mo. 21:16" once it's nearly out.
    fn usage_hint(&self, provider: &str) -> Option<(String, bool)> {
        let now = crate::code::state::now_ms() as f64;
        let pooled = usage::pooled(provider, &self.usage.snapshots, &self.usage.accounts, now);
        let (left, resets_at) = usage::overall_summary(pooled.as_ref().map(|p| &p.0), now)?;
        let left = left.round();
        let text = match resets_at.filter(|r| *r > now) {
            Some(r) if left < 10.0 => format!("{left:.0}% · resets {}", usage::short_when(r, now)),
            _ => format!("{left:.0}%"),
        };
        Some((text, left < 30.0))
    }

    pub(super) fn render_new_menu(&self, c: &Colors, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.new_menu.open {
            return None;
        }
        let c = *c;
        let s = self.state.read(cx);
        let default_id = s.default_provider_id();
        let mut list = div().flex().flex_col().gap(px(2.0));
        let mut providers: Vec<_> = s.providers.iter().collect();
        if let Some(pos) = providers.iter().position(|p| p.id == "winfer")
            && pos > 0
        {
            let winfer = providers.remove(pos);
            providers.insert(0, winfer);
        }
        let has_others = providers.len() > 1;
        for p in providers {
            let id = p.id.clone();
            let is_winfer = id == "winfer";
            let expanded = self.new_menu.expanded.as_deref() == Some(id.as_str());
            let installed = p.installed;
            let open_id = id.clone();
            let mut row = div().id(SharedString::from(format!("new-{id}"))).flex_1().min_w_0().flex().items_center().gap_2().rounded_md().px_2().py_1p5().text_size(px(13.0))
                .when(installed, |d| d.text_color(c.fg).cursor_pointer().hover(move |d| d.bg(c.hover)))
                .when(!installed, |d| d.text_color(c.subtle))
                .child(mark(c, backend_key(&id), 15.0, if installed { 1.0 } else { 0.4 }))
                .child(
                    div().flex_1().min_w_0().flex().items_center().gap_1p5()
                        .child(div().truncate().child(p.name.clone()))
                        .when(is_winfer && installed, |d| d.child(crate::ui::code_pickers::recommended_badge(c, true))),
                );
            if !installed {
                row = row.child(div().flex_shrink_0().text_size(px(11.0)).child("not installed"));
            } else {
                if let Some(model) = s.agent_models.get(&id) {
                    row = row.child(div().flex_shrink_0().font_family(crate::fonts::mono(cx)).text_size(px(10.0)).text_color(c.subtle).child(model.clone()));
                }
                if let Some((text, low)) = self.usage_hint(&id) {
                    row = row.child(div().flex_shrink_0().whitespace_nowrap().font_family(crate::fonts::mono(cx)).text_size(px(10.0)).text_color(if low { c.warning } else { c.subtle }).child(text));
                }
                row = row.on_click(cx.listener(move |this, _, _, cx| {
                    this.new_menu = Default::default();
                    this.add_pane(Some(open_id.clone()), AddPane::default(), cx);
                }));
            }
            let mut line = div().flex().items_center().child(row);
            if installed {
                let star_id = id.clone();
                let is_default = id == default_id;
                line = line.child(
                    div().id(SharedString::from(format!("new-star-{id}"))).rounded_sm().p_1().cursor_pointer().hover(move |d| d.bg(c.hover))
                        .tooltip(crate::ui::code_view::tip(if is_default { "Default agent for new panes" } else { "Make this the default agent for new panes" }))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let id = star_id.clone();
                            this.update_state(cx, |s| s.default_provider = if id == "winfer" { None } else { Some(id) });
                        }))
                        .child(crate::ui::code_pickers::star_icon(if is_default { c.accent } else { Hsla { a: 0.5, ..c.subtle } }, 12.0)),
                );
                if p.model_arg.is_some() && self.models.get(&id).is_none_or(|m| !m.is_empty()) {
                    let chevron_id = id.clone();
                    line = line.child(
                        div().id(SharedString::from(format!("new-models-{id}"))).rounded_sm().p_1().cursor_pointer().text_color(c.subtle).hover(move |d| d.bg(c.hover).text_color(c.fg))
                            .tooltip(crate::ui::code_view::tip("Choose a model"))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.new_menu.expanded = if this.new_menu.expanded.as_deref() == Some(chevron_id.as_str()) { None } else { Some(chevron_id.clone()) };
                                this.load_all_models(cx);
                                cx.notify();
                            }))
                            .child(Icon::new(if expanded { IconName::ChevronDown } else { IconName::ChevronRight }).size(px(12.0))),
                    );
                }
            }
            let mut block = div().flex().flex_col().child(line);
            if expanded {
                let mut models = div().id(SharedString::from(format!("new-list-{id}"))).ml_6().pl_1().max_h(px(192.0)).overflow_y_scroll().flex().flex_col();
                match self.models.get(&id) {
                    None => models = models.child(div().px_2().py_1().text_xs().text_color(c.subtle).child("Loading models…")),
                    Some(list) => {
                        for (i, m) in list.iter().enumerate() {
                            let (model_id, provider_id) = (m.id.clone(), id.clone());
                            models = models.child(
                                div().id(SharedString::from(format!("new-model-{id}-{i}"))).truncate().rounded_sm().px_2().py_1().text_xs().text_color(c.muted).cursor_pointer().hover(move |d| d.bg(c.hover).text_color(c.fg))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.new_menu = Default::default();
                                        this.add_pane(Some(provider_id.clone()), AddPane { model: Some(model_id.clone()), ..AddPane::default() }, cx);
                                    }))
                                    .child(m.label.clone()),
                            );
                        }
                    }
                }
                block = block.child(div().mb_1().child(models));
            }
            list = list.child(block);
            if is_winfer && has_others {
                list = list.child(div().h(px(1.0)).mx_1p5().my_0p5().bg(Hsla { a: 0.55, ..c.border }));
            }
        }
        Some(
            div().id("new-menu-backdrop").absolute().size_full().occlude()
                .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                    this.new_menu = Default::default();
                    cx.notify();
                }))
                .child(
                    div().id("new-menu").absolute().top(px(44.0)).right(px(44.0)).w(px(296.0)).max_h(relative(0.8)).overflow_hidden().rounded_lg().border_1().border_color(c.edge).bg(c.raised).p_1().shadow_lg()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(list),
                )
                .into_any_element(),
        )
    }
}
