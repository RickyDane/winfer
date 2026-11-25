//! Building blocks shared by the tabbed settings pages (the "visual + tabs" design of
//! mockups/settings-overhaul/): the tab bar under the page header, small controls and tiles.

use super::*;

/// One tab of a page: its id (what [`Root::page_tab`] returns), label, icon and an optional dot or
/// count after the label.
pub(super) struct PageTab {
    pub id: &'static str,
    pub label: &'static str,
    pub icon: IconName,
    pub extra: Option<AnyElement>,
}

impl PageTab {
    pub fn new(id: &'static str, label: &'static str, icon: IconName) -> Self {
        Self { id, label, icon, extra: None }
    }

    pub fn extra(mut self, extra: Option<AnyElement>) -> Self {
        self.extra = extra;
        self
    }
}

/// A small dot after a tab's label (updates waiting, a warning…).
pub(super) fn tab_dot(color: Hsla) -> AnyElement {
    div().size(px(6.0)).rounded_full().bg(color).into_any_element()
}

/// A count after a tab's label.
pub(super) fn tab_count(c: Colors, mono: &SharedString, n: impl ToString) -> AnyElement {
    div().px_1p5().rounded_full().bg(c.hover).font_family(mono.clone()).text_size(px(10.0)).text_color(c.subtle).child(n.to_string()).into_any_element()
}

impl Root {
    /// The open tab of a tabbed page (`default` until one is picked).
    pub(super) fn page_tab(&self, page: &'static str, default: &'static str) -> &'static str {
        self.settings.as_ref().and_then(|s| s.page_tabs.get(page).copied()).unwrap_or(default)
    }

    /// Opens a tab of a page and scrolls to its top.
    pub(super) fn set_page_tab(&mut self, page: &'static str, tab: &'static str, cx: &mut Context<Self>) {
        if let Some(s) = self.settings.as_mut() {
            s.page_tabs.insert(page, tab);
        }
        self.scroll_settings_top();
        cx.notify();
    }

    /// The tab bar under a page's header.
    pub(super) fn page_tab_bar(&mut self, page: &'static str, default: &'static str, tabs: Vec<PageTab>, c: Colors, cx: &mut Context<Self>) -> Div {
        let current = self.page_tab(page, default);
        let mut bar = div().flex_shrink_0().flex().items_end().gap_0p5().pl(px(26.0)).pr_8().border_b_1().border_color(c.border);
        for t in tabs {
            let on = t.id == current;
            let id = t.id;
            bar = bar.child(
                div().id(SharedString::from(format!("{page}-tab-{id}"))).relative().h(px(38.0)).flex().items_center().gap(px(7.0)).px(px(9.0)).text_size(px(12.5)).cursor_pointer()
                    .text_color(if on { c.fg } else { c.muted })
                    .hover(move |d| d.text_color(c.fg))
                    .on_click(cx.listener(move |this, _, _, cx| this.set_page_tab(page, id, cx)))
                    .child(Icon::new(t.icon).size(px(14.0)).when(on, |i| i.text_color(c.accent)))
                    .child(t.label)
                    .children(t.extra)
                    .when(on, |d| d.child(div().absolute().left(px(7.0)).right(px(7.0)).bottom(px(-1.0)).h(px(2.0)).rounded_full().bg(c.accent))),
            );
        }
        bar
    }

    /// The tab bar of a page that has tabs (None for pages without).
    pub(super) fn settings_page_tabs(&mut self, page: &str, c: Colors, cx: &mut Context<Self>) -> Option<Div> {
        match page {
            "winfer-code" => Some(self.code_tabs(c, cx)),
            "agents" => Some(self.agents_tabs(c, cx)),
            "voice" => Some(self.voice_tabs(c, cx)),
            "usage" => {
                self.swap_machine_usage();
                let tabs = self.usage_tabs(c, cx);
                self.swap_machine_usage();
                Some(tabs)
            }
            "data" => Some(self.data_tabs(c, cx)),
            "diagnostics" => Some(self.diagnostics_tabs(c, cx)),
            "plugins" => Some(self.plugins_tabs(c, cx)),
            "skills" => Some(self.skills_tabs(c, cx)),
            _ => None,
        }
    }

    /// Opens the tab of `page` that holds the search hit `label`.
    pub(super) fn open_tab_for_hit(&mut self, page: &str, label: &str) {
        let tab: Option<(&'static str, &'static str)> = match page {
            "voice" => Some(("voice", super::settings_more::voice_tab_for(label))),
            "agents" => Some(("agents", super::settings_agents::tab_for_label(label))),
            "winfer-code" => {
                self.harness_page.ui.tab = super::settings_code::tab_for_label(label);
                None
            }
            "usage" => Some(("usage", match label {
                "Spend" | "On panes" | "Show task cost on panes" => "spend",
                "Tools" => "tools",
                _ => "limits",
            })),
            "plugins" => Some(("plugins", match label {
                "Options" | "Hide names on plugin tabs" | "Your own plugins" => "options",
                "Discover plugins" => "discover",
                _ => "installed",
            })),
            "skills" => Some(("skills", if label == "Discover skills" { "discover" } else { "installed" })),
            "data" => Some(("data", if label.starts_with("Import") { "import" } else { "export" })),
            "diagnostics" => Some(("diagnostics", match label {
                "Logs" | "Log folder" | "Sizes" | "Verbose logging" | "Clear logs" => "logs",
                "Latest crash" => "crashes",
                _ => "overview",
            })),
            _ => None,
        };
        if let (Some((page, tab)), Some(s)) = (tab, self.settings.as_mut()) {
            s.page_tabs.insert(page, tab);
        }
    }

    /// Two to five choices side by side.
    pub(super) fn seg_control<T: Clone + PartialEq + 'static>(&self, id: &str, options: Vec<(T, &'static str)>, current: T, c: Colors, cx: &mut Context<Self>, pick: impl Fn(&mut Self, T, &mut Context<Self>) + Clone + 'static) -> Div {
        let mut seg = div().flex_shrink_0().flex().gap(px(2.0)).p(px(2.0)).rounded(px(8.0)).bg(c.app).border_1().border_color(c.edge);
        for (i, (value, label)) in options.into_iter().enumerate() {
            let on = value == current;
            let pick = pick.clone();
            seg = seg.child(
                div().id(SharedString::from(format!("{id}-{i}"))).h(px(24.0)).flex().items_center().gap_1p5().px_2p5().rounded(px(6.0)).text_size(px(12.0)).cursor_pointer()
                    .when(on, |d| d.bg(c.active).text_color(c.fg))
                    .when(!on, |d| d.text_color(c.muted).hover(move |d| d.text_color(c.fg)))
                    .on_click(cx.listener(move |this, _, _, cx| pick(this, value.clone(), cx)))
                    .child(label),
            );
        }
        seg
    }
}

pub(super) fn alpha(color: Hsla, a: f32) -> Hsla {
    Hsla { a, ..color }
}

pub(super) fn mini_switch(c: Colors, on: bool) -> Div {
    div().w(px(28.0)).h(px(16.0)).flex_shrink_0().flex().rounded_full().p(px(2.0)).bg(if on { c.accent } else { c.active }).when(on, |d| d.justify_end()).child(div().size(px(12.0)).rounded_full().bg(hsla(0.0, 0.0, 1.0, 1.0)))
}

pub(super) fn radio(c: Colors, on: bool) -> Div {
    let d = div().size(px(14.0)).flex_shrink_0().rounded_full().flex().items_center().justify_center();
    if on { d.bg(c.accent).child(div().size(px(5.0)).rounded_full().bg(hsla(0.0, 0.0, 1.0, 1.0))) } else { d.border_1().border_color(c.border_strong) }
}

pub(super) fn checkbox(c: Colors, on: bool) -> Div {
    let d = div().size(px(14.0)).flex_shrink_0().rounded(px(4.0)).flex().items_center().justify_center();
    if on { d.bg(c.accent).child(Icon::new(IconName::Check).size(px(10.0)).text_color(c.on_accent)) } else { d.border_1().border_color(c.border_strong) }
}

/// A rounded card without a header.
pub(super) fn panel(c: Colors) -> Div {
    div().flex().flex_col().rounded(px(10.0)).border_1().border_color(c.edge).bg(c.panel)
}

pub(super) fn kbd(c: Colors, mono: &SharedString, text: &str) -> Div {
    div().px_1p5().rounded_sm().bg(c.hover).font_family(mono.clone()).text_size(px(10.5)).text_color(c.muted).child(text.to_string())
}

/// A line above a tab's content, with shortcuts or a control on the right.
pub(super) fn intro(c: Colors, text: &str, right: Option<AnyElement>) -> Div {
    div().flex().items_center().gap_2p5().mx(px(2.0)).text_size(px(12.0)).text_color(c.subtle).child(text.to_string()).child(div().flex_1()).children(right)
}

/// A small heading over a group of cards, with a note on the right.
pub(super) fn section(c: Colors, title: &str, right: Option<String>) -> Div {
    div().flex().items_center().gap_2().mx(px(2.0)).mb(px(-6.0)).text_size(px(12.0)).font_weight(FontWeight::SEMIBOLD).text_color(c.muted).child(title.to_string()).child(div().flex_1())
        .when_some(right, |d, r| d.child(div().font_weight(FontWeight::NORMAL).text_color(c.subtle).child(r)))
}

pub(super) fn badge(color: Hsla, text: impl Into<SharedString>) -> Div {
    div().h(px(18.0)).flex().items_center().gap_1().px_1p5().rounded_full().bg(alpha(color, 0.15)).text_size(px(10.5)).font_weight(FontWeight::MEDIUM).text_color(color).child(text.into())
}

pub(super) fn small_label(c: Colors, title: impl Into<SharedString>, sub: impl Into<SharedString>) -> Div {
    div().flex_1().min_w_0().flex().flex_col().gap_0p5().child(div().text_size(px(12.5)).font_weight(FontWeight::MEDIUM).text_color(c.fg).child(title.into())).child(div().text_size(px(11.5)).text_color(c.subtle).child(sub.into()))
}

/// A tinted square with an icon (26px; tiles and list rows).
pub(super) fn icon_tile(color: Hsla, icon: IconName, size: f32) -> Div {
    div().size(px(size)).flex_shrink_0().flex().items_center().justify_center().rounded(px(size * 0.27)).bg(alpha(color, 0.14)).text_color(color).child(Icon::new(icon).size(px(size * 0.54)))
}

/// A tinted note with an icon: info (accent), a warning or an error.
pub(super) fn banner(color: Hsla, icon: IconName, text: impl Into<SharedString>) -> Div {
    div().flex().items_center().gap_2p5().px_3().py_2p5().rounded(px(9.0)).bg(alpha(color, 0.12)).text_size(px(12.5)).text_color(color)
        .child(Icon::new(icon).size(px(15.0)).flex_shrink_0())
        .child(div().flex_1().min_w_0().child(text.into()))
}

/// A thin meter: `share` of the bar (0–1) in `color`.
pub(super) fn meter(c: Colors, share: f32, color: Hsla, height: f32) -> Div {
    div().relative().h(px(height)).rounded_full().bg(c.active).overflow_hidden()
        .child(div().absolute().top_0().bottom_0().left_0().w(relative(share.clamp(0.0, 1.0))).rounded_full().bg(color))
}

/// A centered icon and text for an empty list.
pub(super) fn empty_state(c: Colors, icon: IconName, text: impl Into<SharedString>) -> Div {
    div().flex().flex_col().items_center().gap_1p5().px_4().py(px(26.0)).text_size(px(12.0)).text_color(c.subtle).child(Icon::new(icon).size(px(22.0))).child(div().text_center().child(text.into()))
}
