//! Overhauled Context & Usage sheet ("Compact Tabbed"): persistent capacity header with
//! compaction threshold pins, and three focused tabs (Overview, Breakdown, Compaction).

use super::code_view::{CodeView, tip, tokens};
use crate::code::names::model_name;
use crate::code::protocol::{Cmd, ContextPart};
use crate::theme::{self, Colors, blend};
use gpui_kit::assets::IconName;
use gpui_kit::component::Icon;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use std::cell::Cell;
use std::rc::Rc;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum UsageTab {
    #[default]
    Overview,
    Breakdown,
    Compaction,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum UsageFilter {
    #[default]
    All,
    Fixed,
    Conversation,
}

#[derive(Default)]
pub(super) struct UsageSheetState {
    pub tab: UsageTab,
    pub filter: UsageFilter,
    pub hover_kind: Option<String>,
    /// Where the compaction slider bar was last laid out, to turn a pointer into a percentage.
    pub ctx_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    /// The compaction marker being dragged (`true` = micro-compact / trim, `false` = full compact) and its percentage.
    pub drag: Option<(bool, u64)>,
}

impl UsageSheetState {
    pub fn open(&mut self, tab: UsageTab) {
        self.tab = tab;
        self.filter = UsageFilter::All;
        self.hover_kind = None;
        self.drag = None;
    }
}

fn snap_percent(frac: f32) -> u64 {
    ((frac.clamp(0.0, 1.0) * 20.0).round() as u64 * 5).clamp(5, 95)
}

fn alpha(color: Hsla, a: f32) -> Hsla {
    Hsla { a, ..color }
}

fn ansi_hsla(slot: usize) -> Hsla {
    let b = theme::palette().ansi[slot];
    Rgba { r: b[0] as f32 / 255.0, g: b[1] as f32 / 255.0, b: b[2] as f32 / 255.0, a: 1.0 }.into()
}

/// Canonical kind key, falling back to label matching if an older payload omitted `kind`.
fn canonical_kind(p: &ContextPart) -> &'static str {
    match p.kind.as_str() {
        "setup" => "setup",
        "system" => "system",
        "tools" => "tools",
        "mcp" => "mcp",
        "instructions" => "instructions",
        "messages" => "messages",
        "replies" => "replies",
        "thinking" => "thinking",
        "toolCalls" => "toolCalls",
        "toolResults" => "toolResults",
        _ => match p.label.as_str() {
            "System prompt & tools" => "setup",
            "System prompt" => "system",
            "Tools" => "tools",
            "MCP tools" => "mcp",
            "Memory & instructions" => "instructions",
            "Your messages" => "messages",
            "Replies" => "replies",
            "Thinking" => "thinking",
            "Tool calls" => "toolCalls",
            "Tool results" => "toolResults",
            _ => "other",
        },
    }
}

fn is_fixed_part(p: &ContextPart) -> bool {
    matches!(canonical_kind(p), "setup" | "system" | "tools" | "mcp" | "instructions")
}

/// Theme-harmonized color per context category (matches active theme's UI + ANSI accents).
fn part_color(kind: &str, c: Colors) -> Hsla {
    match kind {
        "setup" | "system" => c.subtle,
        "tools" => c.accent,
        "mcp" => ansi_hsla(13),
        "instructions" => c.warning,
        "messages" => c.claude,
        "replies" => c.danger,
        "thinking" => c.success,
        "toolCalls" => ansi_hsla(14),
        "toolResults" => ansi_hsla(6),
        _ => c.muted,
    }
}

fn pct_label(part: u64, total: u64) -> String {
    if total == 0 || part == 0 {
        return "0%".to_string();
    }
    let p = (part as f64 / total as f64) * 100.0;
    if p < 1.0 { "<1%".to_string() } else { format!("{}%", p.round() as u64) }
}

fn chip(bg: Hsla, fg: Hsla, text: impl Into<SharedString>, mono: &SharedString) -> Div {
    div()
        .h(px(20.0))
        .px_2()
        .flex()
        .items_center()
        .rounded_full()
        .bg(bg)
        .font_family(mono.clone())
        .text_size(px(10.5))
        .font_weight(FontWeight::MEDIUM)
        .text_color(fg)
        .child(text.into())
}

fn section_label(c: Colors, text: impl Into<SharedString>) -> Div {
    div().text_size(px(10.5)).font_weight(FontWeight::SEMIBOLD).text_color(c.subtle).child(text.into())
}

impl CodeView {
    pub(super) fn render_usage_sheet(&mut self, c: Colors, mono: SharedString, cx: &mut Context<Self>) -> AnyElement {
        let (
            model_label,
            model_raw,
            u,
            pct,
            mut trim_at,
            mut compact_at,
            busy,
            rate,
            breakdown,
        ) = {
            let st = &self.model.read(cx).state;
            let label = if st.model.is_empty() {
                String::new()
            } else {
                model_name(&st.model, st.actual_model.as_deref())
            };
            (
                label,
                st.model.clone(),
                st.usage.clone(),
                st.context_fraction(),
                st.trim_at,
                st.compact_at,
                st.busy,
                st.tokens_per_second().or(st.last_rate),
                st.breakdown.clone(),
            )
        };
        match self.usage_sheet.drag {
            Some((true, v)) => trim_at = v,
            Some((false, v)) => compact_at = v,
            None => {}
        }

        let parts: Vec<ContextPart> = breakdown
            .as_ref()
            .and_then(|(_, p)| p.as_ref())
            .map(|list| list.iter().filter(|p| p.tokens > 0).cloned().collect())
            .unwrap_or_default();
        let parts_total: u64 = parts.iter().map(|p| p.tokens).sum::<u64>().max(1);
        let fixed_parts: Vec<&ContextPart> = parts.iter().filter(|p| is_fixed_part(p)).collect();
        let conv_parts: Vec<&ContextPart> = parts.iter().filter(|p| !is_fixed_part(p)).collect();
        let fixed_tokens: u64 = fixed_parts.iter().map(|p| p.tokens).sum();
        let conv_tokens: u64 = conv_parts.iter().map(|p| p.tokens).sum();
        let tool_results_tokens: u64 = parts.iter().find(|p| canonical_kind(p) == "toolResults").map_or(0, |p| p.tokens);

        let pct_int = (pct * 100.0).round() as u64;
        let hot = pct >= 0.80;
        let free = u.window.saturating_sub(u.context);
        let tab = self.usage_sheet.tab;
        let filter = self.usage_sheet.filter;
        let hover_kind = self.usage_sheet.hover_kind.clone();

        // 1. Header ------------------------------------------------------------------------------
        let header = div()
            .flex()
            .items_center()
            .gap_2p5()
            .px_4()
            .py_3()
            .border_b_1()
            .border_color(c.border)
            .child(
                div()
                    .size(px(26.0))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(8.0))
                    .bg(alpha(c.accent, 0.14))
                    .child(Icon::new(IconName::Gauge).size(px(14.0)).text_color(c.accent)),
            )
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .items_center()
                    .gap_2()
                    .child(div().flex_shrink_0().whitespace_nowrap().text_size(px(13.5)).font_weight(FontWeight::SEMIBOLD).text_color(c.fg).child("Context & usage"))
                    .when(!model_label.is_empty(), |d| {
                        d.child(
                            div()
                                .h(px(21.0))
                                .min_w_0()
                                .px_2()
                                .flex()
                                .items_center()
                                .gap_1p5()
                                .rounded_full()
                                .bg(c.panel)
                                .border_1()
                                .border_color(c.border)
                                .overflow_hidden()
                                .text_size(px(11.0))
                                .text_color(c.muted)
                                .child(div().size(px(6.0)).flex_shrink_0().rounded_full().bg(c.claude))
                                .child(div().min_w_0().truncate().child(model_label.clone())),
                        )
                    }),
            )
            .child(
                div()
                    .id("cw-sheet-x")
                    .size(px(26.0))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(7.0))
                    .cursor_pointer()
                    .text_color(c.subtle)
                    .hover(move |d| d.bg(c.hover).text_color(c.fg))
                    .on_click(cx.listener(|this, _, _, cx| this.set_sheet(None, cx)))
                    .child(Icon::new(IconName::X).size(px(14.0))),
            );

        // 2. Persistent Top Summary Strip --------------------------------------------------------
        let summary_bg = blend(c.raised, c.panel, 0.72);
        let top_summary = div()
            .px_4()
            .py_3()
            .bg(summary_bg)
            .border_b_1()
            .border_color(c.border)
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(
                        div()
                            .flex()
                            .items_baseline()
                            .gap_1p5()
                            .child(div().font_family(mono.clone()).text_size(px(18.0)).font_weight(FontWeight::BOLD).text_color(c.fg).child(tokens(u.context)))
                            .child(div().font_family(mono.clone()).text_size(px(12.0)).text_color(c.subtle).child(format!("/ {} · {pct_int}%", tokens(u.window)))),
                    )
                    .child(chip(
                        if hot { alpha(c.danger, 0.16) } else { alpha(c.accent, 0.15) },
                        if hot { c.danger } else { c.accent },
                        format!("{} free", tokens(free)),
                        &mono,
                    )),
            )
            .child(self.render_capacity_bar(&parts, parts_total, u.context, pct, hot, trim_at, compact_at, hover_kind.as_deref(), c, &mono));

        // 3. Tab Bar -----------------------------------------------------------------------------
        let tabs = div()
            .flex()
            .items_center()
            .gap_1()
            .px_4()
            .border_b_1()
            .border_color(c.border)
            .bg(c.raised)
            .child(self.usage_tab_btn("Overview", UsageTab::Overview, None, tab, c, &mono, cx))
            .child(self.usage_tab_btn("Breakdown", UsageTab::Breakdown, (!parts.is_empty()).then_some(parts.len()), tab, c, &mono, cx))
            .child(self.usage_tab_btn("Compaction", UsageTab::Compaction, None, tab, c, &mono, cx));

        // 4. Tab Body ----------------------------------------------------------------------------
        let body_content: AnyElement = match tab {
            UsageTab::Overview => self.usage_tab_overview(
                &u,
                &model_raw,
                pct_int,
                hot,
                free,
                rate,
                &breakdown,
                &parts,
                parts_total,
                &fixed_parts,
                &conv_parts,
                fixed_tokens,
                conv_tokens,
                hover_kind.as_deref(),
                c,
                &mono,
                cx,
            ),
            UsageTab::Breakdown => self.usage_tab_breakdown(
                u.context,
                &breakdown,
                &parts,
                parts_total,
                &fixed_parts,
                &conv_parts,
                fixed_tokens,
                conv_tokens,
                filter,
                hover_kind.as_deref(),
                c,
                &mono,
                cx,
            ),
            UsageTab::Compaction => self.usage_tab_compaction(
                u.window,
                trim_at,
                compact_at,
                tool_results_tokens,
                conv_tokens,
                !parts.is_empty(),
                c,
                &mono,
                cx,
            ),
        };

        // 5. Fixed Footer ------------------------------------------------------------------------
        let trim_str = if trim_at > 0 { format!("Trim {trim_at}%") } else { "Trim off".to_string() };
        let cmp_str = if compact_at > 0 { format!("Compact {compact_at}%") } else { "Compact off".to_string() };
        let footer_bg = blend(c.raised, c.panel, 0.65);
        let next_tab = if tab == UsageTab::Compaction { UsageTab::Overview } else { UsageTab::Compaction };
        let cta_bg = if hot { c.danger } else { c.accent };
        let cta_fg = if hot { theme::hex("#111111") } else { c.on_accent };
        let compact_btn = div()
            .id("cw-usage-compact-now")
            .h(px(30.0))
            .px_3()
            .flex()
            .items_center()
            .gap_1p5()
            .rounded(px(8.0))
            .bg(if busy { c.hover } else { cta_bg })
            .text_size(px(12.0))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(if busy { c.subtle } else { cta_fg })
            .when(!busy, |d| {
                d.cursor_pointer()
                    .hover(move |d| d.opacity(0.92))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.send(Cmd::Compact, cx);
                        this.set_sheet(None, cx);
                    }))
            })
            .when(busy, |d| d.tooltip(tip("Wait for the turn to finish.")))
            .child(Icon::new(IconName::Shrink).size(px(13.0)))
            .child("Compact now")
            .when(conv_tokens >= 500, |d| {
                d.child(div().font_family(mono.clone()).text_size(px(11.0)).font_weight(FontWeight::NORMAL).opacity(0.8).child(format!("~{}", tokens(conv_tokens))))
            });

        let footer = div()
            .px_4()
            .py_2p5()
            .border_t_1()
            .border_color(c.border)
            .bg(footer_bg)
            .flex()
            .items_center()
            .justify_between()
            .gap_3()
            .child(
                div()
                    .id("cw-usage-footer-cmp")
                    .h(px(28.0))
                    .px_2()
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .rounded(px(7.0))
                    .cursor_pointer()
                    .text_size(px(11.5))
                    .text_color(if tab == UsageTab::Compaction { c.fg } else { c.muted })
                    .when(tab == UsageTab::Compaction, |d| d.bg(c.hover))
                    .hover(move |d| d.bg(c.hover).text_color(c.fg))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.usage_sheet.tab = next_tab;
                        this.usage_sheet.hover_kind = None;
                        cx.notify();
                    }))
                    .child(Icon::new(IconName::SlidersHorizontal).size(px(12.5)))
                    .child(format!("{trim_str} · {cmp_str}"))
                    .child(Icon::new(IconName::ChevronRight).size(px(12.0))),
            )
            .child(compact_btn);

        // Modal Shell + Scrim --------------------------------------------------------------------
        let panel = div()
            .id("cw-sheet")
            .w(px(556.0))
            .max_w_full()
            .max_h_full()
            .flex()
            .flex_col()
            .rounded(px(14.0))
            .border_1()
            .border_color(c.edge)
            .bg(c.raised)
            .shadow_lg()
            .overflow_hidden()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(header)
            .child(top_summary)
            .child(tabs)
            .child(div().id("cw-sheet-body").flex_1().min_h_0().overflow_y_scroll().px_4().py_3p5().child(body_content))
            .child(footer);

        let scrim = div()
            .id("cw-sheet-scrim")
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(Hsla { a: 0.5, ..black() })
            .p_4()
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| this.set_sheet(None, cx)))
            .child(super::code_view::rise(panel, SharedString::from("sheet-in-Usage"), 0, 12.0));

        scrim
            .with_animation(SharedString::from("sheet-scrim-Usage"), Animation::new(std::time::Duration::from_millis(180)), |d, t| d.opacity(t))
            .into_any_element()
    }

    fn usage_tab_btn(
        &self,
        label: &'static str,
        target: UsageTab,
        count: Option<usize>,
        active: UsageTab,
        c: Colors,
        mono: &SharedString,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let on = active == target;
        div()
            .id(SharedString::from(format!("cw-usage-tab-{label}")))
            .relative()
            .h(px(36.0))
            .px_2p5()
            .flex()
            .items_center()
            .gap_1p5()
            .cursor_pointer()
            .text_size(px(12.0))
            .font_weight(if on { FontWeight::SEMIBOLD } else { FontWeight::MEDIUM })
            .text_color(if on { c.fg } else { c.muted })
            .when(!on, |d| d.hover(move |d| d.text_color(c.fg)))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.usage_sheet.tab = target;
                this.usage_sheet.hover_kind = None;
                cx.notify();
            }))
            .child(label)
            .when_some(count, |d, n| {
                d.child(
                    div()
                        .px_1p5()
                        .py(px(1.0))
                        .rounded_full()
                        .bg(c.hover)
                        .font_family(mono.clone())
                        .text_size(px(10.0))
                        .text_color(c.subtle)
                        .child(n.to_string()),
                )
            })
            .when(on, |d| d.child(div().absolute().left_2().right_2().bottom_0().h(px(2.0)).rounded_full().bg(c.accent)))
    }

    #[allow(clippy::too_many_arguments)]
    fn render_capacity_bar(
        &self,
        parts: &[ContextPart],
        parts_total: u64,
        context: u64,
        pct: f32,
        hot: bool,
        trim_at: u64,
        compact_at: u64,
        hover_kind: Option<&str>,
        c: Colors,
        mono: &SharedString,
    ) -> Div {
        let has_pins = trim_at > 0 || compact_at > 0;
        let fill_frac = if context == 0 { 0.0 } else { pct.clamp(0.014, 1.0) };

        let mut fill = div().h_full().w(relative(fill_frac)).flex().gap(px(1.0));
        if parts.is_empty() {
            fill = fill.child(div().h_full().w_full().bg(if hot { c.danger } else { c.accent }));
        } else {
            for p in parts {
                let kind = canonical_kind(p);
                let share = (p.tokens as f32 / parts_total as f32).clamp(0.0, 1.0);
                let dimmed = hover_kind.is_some_and(|h| h != kind);
                fill = fill.child(
                    div()
                        .h_full()
                        .w(relative(share))
                        .min_w(px(2.0))
                        .bg(part_color(kind, c))
                        .when(dimmed, |d| d.opacity(0.28)),
                );
            }
        }

        let track = div()
            .relative()
            .h(px(12.0))
            .w_full()
            .rounded_full()
            .bg(c.app)
            .border_1()
            .border_color(c.border)
            .child(div().absolute().inset_0().rounded_full().overflow_hidden().flex().child(fill))
            .when(trim_at > 0, |d| {
                d.child(div().absolute().top(px(-3.0)).bottom(px(-3.0)).left(relative((trim_at as f32 / 100.0).clamp(0.0, 1.0))).w(px(2.0)).rounded_full().bg(c.warning))
            })
            .when(compact_at > 0, |d| {
                d.child(div().absolute().top(px(-3.0)).bottom(px(-3.0)).left(relative((compact_at as f32 / 100.0).clamp(0.0, 1.0))).w(px(2.0)).rounded_full().bg(c.accent))
            });

        let mut wrap = div().flex().flex_col().gap_1();
        if has_pins {
            // Pin labels row positioned cleanly above the track, clamped so edges never clip.
            let pin_label = |pos_pct: u64, text: String, color: Hsla| {
                let f = (pos_pct as f32 / 100.0).clamp(0.06, 0.84);
                div()
                    .absolute()
                    .top_0()
                    .left(relative(f))
                    .ml(px(-24.0))
                    .font_family(mono.clone())
                    .text_size(px(9.5))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(color)
                    .whitespace_nowrap()
                    .child(text)
            };
            let labels = div()
                .relative()
                .h(px(13.0))
                .w_full()
                .when(trim_at > 0, |d| d.child(pin_label(trim_at, format!("trim {trim_at}%"), c.warning)))
                .when(compact_at > 0, |d| d.child(pin_label(compact_at, format!("compact {compact_at}%"), c.accent)));
            wrap = wrap.child(labels);
        }
        wrap.child(track)
    }

    #[allow(clippy::too_many_arguments)]
    fn usage_tab_overview(
        &self,
        u: &crate::code::protocol::Usage,
        model_raw: &str,
        pct_int: u64,
        hot: bool,
        free: u64,
        rate: Option<f32>,
        breakdown: &Option<(u64, Option<Vec<ContextPart>>)>,
        parts: &[ContextPart],
        parts_total: u64,
        fixed_parts: &[&ContextPart],
        conv_parts: &[&ContextPart],
        fixed_tokens: u64,
        conv_tokens: u64,
        hover_kind: Option<&str>,
        c: Colors,
        mono: &SharedString,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut out = div().flex().flex_col().gap_3();

        // Near-limit alert banner
        if hot {
            let reclaim_note = if conv_tokens >= 500 {
                format!(" Compacting now can reclaim up to ~{} tokens.", tokens(conv_tokens))
            } else {
                String::new()
            };
            out = out.child(
                div()
                    .flex()
                    .items_center()
                    .gap_2p5()
                    .px_3()
                    .py_2()
                    .rounded(px(9.0))
                    .bg(blend(c.panel, c.danger, 0.11))
                    .border_1()
                    .border_color(blend(c.border, c.danger, 0.35))
                    .child(Icon::new(IconName::TriangleAlert).size(px(14.0)).text_color(c.danger).flex_shrink_0())
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(12.0))
                            .text_color(c.fg)
                            .whitespace_normal()
                            .child(format!("{pct_int}% of context used — {} left.{reclaim_note}", tokens(free))),
                    ),
            );
        }

        // Two-up Fixed overhead vs Conversation cards
        if !parts.is_empty() {
            out = out.child(
                div()
                    .flex()
                    .gap_2p5()
                    .child(self.zone_card("Fixed overhead", IconName::Lock, fixed_tokens, parts_total, fixed_parts, "System, tools, MCP & memory", c, mono))
                    .child(self.zone_card("Conversation", IconName::Sparkles, conv_tokens, parts_total, conv_parts, "Messages, replies & tool output", c, mono)),
            );
        }

        // Top context consumers card
        let consumers = if u.context == 0 {
            div().rounded(px(10.0)).border_1().border_color(c.border).bg(c.panel).p_3().text_size(px(12.0)).text_color(c.subtle).child("Send a message to inspect what fills the context window.")
        } else if breakdown.is_none() {
            div().rounded(px(10.0)).border_1().border_color(c.border).bg(c.panel).p_3().text_size(px(12.0)).text_color(c.subtle).child("Reading the context…")
        } else if parts.is_empty() {
            div().rounded(px(10.0)).border_1().border_color(c.border).bg(c.panel).p_3().text_size(px(12.0)).text_color(c.subtle).child("This session's log can't be read, so there's no breakdown.")
        } else {
            let mut top: Vec<&ContextPart> = parts.iter().collect();
            top.sort_by(|a, b| b.tokens.cmp(&a.tokens));
            let top_max = top.first().map_or(1, |p| p.tokens).max(1);
            let mut list = div().flex().flex_col().gap_0p5();
            for p in top.into_iter().take(4) {
                list = list.child(self.part_row("ov", p, parts_total, top_max, hover_kind, c, mono, cx));
            }
            div()
                .rounded(px(10.0))
                .border_1()
                .border_color(c.border)
                .bg(c.panel)
                .p_3()
                .flex()
                .flex_col()
                .gap_2()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(section_label(c, "TOP CONTEXT CONSUMERS"))
                        .child(
                            div()
                                .id("cw-usage-see-all")
                                .px_1p5()
                                .py(px(2.0))
                                .rounded(px(6.0))
                                .cursor_pointer()
                                .flex()
                                .items_center()
                                .gap_1()
                                .text_size(px(11.0))
                                .text_color(c.accent)
                                .hover(move |d| d.bg(c.hover))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.usage_sheet.tab = UsageTab::Breakdown;
                                    this.usage_sheet.hover_kind = None;
                                    cx.notify();
                                }))
                                .child(format!("See all {}", parts.len()))
                                .child(Icon::new(IconName::ArrowRight).size(px(11.0))),
                        ),
                )
                .child(list)
        };
        out = out.child(consumers);

        // Session Telemetry Card
        out = out.child(self.session_telemetry_card(u, model_raw, rate, c, mono));

        out.into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn zone_card(
        &self,
        title: &'static str,
        icon: IconName,
        total_tokens: u64,
        parts_total: u64,
        group_parts: &[&ContextPart],
        desc: &'static str,
        c: Colors,
        mono: &SharedString,
    ) -> Div {
        let group_sum = total_tokens.max(1);
        let mut strip = div().h(px(4.0)).w_full().rounded_full().bg(c.border).overflow_hidden().flex().gap(px(1.0));
        for p in group_parts {
            let share = (p.tokens as f32 / group_sum as f32).clamp(0.0, 1.0);
            strip = strip.child(div().h_full().w(relative(share)).min_w(px(2.0)).bg(part_color(canonical_kind(p), c)));
        }

        div()
            .flex_1()
            .min_w_0()
            .rounded(px(10.0))
            .border_1()
            .border_color(c.border)
            .bg(c.panel)
            .p_3()
            .flex()
            .flex_col()
            .gap_1p5()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .text_size(px(11.5))
                    .text_color(c.muted)
                    .child(title)
                    .child(Icon::new(icon).size(px(11.5)).text_color(c.subtle)),
            )
            .child(
                div()
                    .flex()
                    .items_baseline()
                    .gap_1p5()
                    .child(div().font_family(mono.clone()).text_size(px(18.0)).font_weight(FontWeight::BOLD).text_color(c.fg).child(tokens(total_tokens)))
                    .child(div().font_family(mono.clone()).text_size(px(11.5)).text_color(c.subtle).child(pct_label(total_tokens, parts_total))),
            )
            .child(strip)
            .child(div().text_size(px(11.0)).text_color(c.subtle).truncate().child(desc))
    }

    fn session_telemetry_card(
        &self,
        u: &crate::code::protocol::Usage,
        model_raw: &str,
        rate: Option<f32>,
        c: Colors,
        mono: &SharedString,
    ) -> Div {
        let hit_pct = if u.input > 0 { Some(((u.cached as f64 / u.input as f64) * 100.0).min(100.0)) } else { None };
        let fresh = u.input.saturating_sub(u.cached);
        let total_io = (u.input + u.output).max(1) as f32;
        let cost_val = match u.cost {
            Some(v) => format!("${v:.2}"),
            None if model_raw.starts_with("chatgpt") => "Plan".to_string(),
            None => "—".to_string(),
        };

        let metric = |label: &'static str, value: String, color: Hsla| {
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(2.0))
                .child(div().text_size(px(11.0)).text_color(c.subtle).child(label))
                .child(div().font_family(mono.clone()).text_size(px(14.0)).font_weight(FontWeight::SEMIBOLD).text_color(color).child(value))
        };

        let cached_w = (u.cached as f32 / total_io).clamp(0.0, 1.0);
        let fresh_w = (fresh as f32 / total_io).clamp(0.0, 1.0);
        let out_w = if u.output > 0 { (u.output as f32 / total_io).clamp(0.015, 1.0) } else { 0.0 };

        let legend_item = |color: Hsla, text: String| {
            div()
                .flex()
                .items_center()
                .gap_1()
                .child(div().size(px(6.0)).rounded_full().bg(color))
                .child(text)
        };

        let out_legend = match rate {
            Some(r) if r > 0.5 => format!("{} out · {:.0} tok/s", tokens(u.output), r),
            _ => format!("{} out", tokens(u.output)),
        };

        div()
            .rounded(px(10.0))
            .border_1()
            .border_color(c.border)
            .bg(c.panel)
            .p_3()
            .flex()
            .flex_col()
            .gap_2p5()
            .child(
                div()
                    .flex()
                    .gap_2p5()
                    .pb_2p5()
                    .border_b_1()
                    .border_color(c.border)
                    .child(metric("Cache hit", hit_pct.map_or("—".to_string(), |h| format!("{h:.0}%")), if hit_pct.is_some() { c.success } else { c.fg }))
                    .child(metric("Input", tokens(u.input), c.fg))
                    .child(metric("Output", tokens(u.output), c.fg))
                    .child(metric("Cost", cost_val, c.fg)),
            )
            .child(
                div()
                    .h(px(6.0))
                    .w_full()
                    .rounded_full()
                    .bg(c.app)
                    .overflow_hidden()
                    .flex()
                    .gap(px(1.0))
                    .when(u.cached > 0, |d| d.child(div().h_full().w(relative(cached_w)).bg(c.success)))
                    .when(fresh > 0, |d| d.child(div().h_full().w(relative(fresh_w)).bg(c.accent)))
                    .when(u.output > 0, |d| d.child(div().h_full().w(relative(out_w)).bg(c.claude))),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_x_3()
                    .gap_y_1()
                    .font_family(mono.clone())
                    .text_size(px(10.5))
                    .text_color(c.muted)
                    .child(legend_item(c.success, format!("{} cached", tokens(u.cached))))
                    .child(legend_item(c.accent, format!("{} fresh", tokens(fresh))))
                    .child(legend_item(c.claude, out_legend)),
            )
    }

    #[allow(clippy::too_many_arguments)]
    fn usage_tab_breakdown(
        &self,
        context: u64,
        breakdown: &Option<(u64, Option<Vec<ContextPart>>)>,
        parts: &[ContextPart],
        parts_total: u64,
        fixed_parts: &[&ContextPart],
        conv_parts: &[&ContextPart],
        fixed_tokens: u64,
        conv_tokens: u64,
        filter: UsageFilter,
        hover_kind: Option<&str>,
        c: Colors,
        mono: &SharedString,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if context == 0 {
            return div().rounded(px(10.0)).border_1().border_color(c.border).bg(c.panel).p_3().text_size(px(12.0)).text_color(c.subtle).child("Send a message to inspect the context breakdown.").into_any_element();
        }
        if breakdown.is_none() {
            return div().rounded(px(10.0)).border_1().border_color(c.border).bg(c.panel).p_3().text_size(px(12.0)).text_color(c.subtle).child("Reading the context…").into_any_element();
        }
        if parts.is_empty() {
            return div().rounded(px(10.0)).border_1().border_color(c.border).bg(c.panel).p_3().text_size(px(12.0)).text_color(c.subtle).child("This session's log can't be read, so there's no breakdown.").into_any_element();
        }

        let filter_btn = |id: &'static str, label: String, target: UsageFilter| {
            let on = filter == target;
            div()
                .id(SharedString::from(format!("cw-usage-flt-{id}")))
                .h(px(22.0))
                .px_2()
                .flex()
                .items_center()
                .rounded(px(5.0))
                .cursor_pointer()
                .text_size(px(11.0))
                .font_weight(FontWeight::MEDIUM)
                .text_color(if on { c.fg } else { c.subtle })
                .when(on, |d| d.bg(c.active))
                .when(!on, |d| d.hover(move |d| d.text_color(c.fg)))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.usage_sheet.filter = target;
                    this.usage_sheet.hover_kind = None;
                    cx.notify();
                }))
                .child(label)
        };

        let seg_switch = div()
            .flex()
            .items_center()
            .gap(px(2.0))
            .p(px(2.0))
            .rounded(px(7.0))
            .bg(c.app)
            .border_1()
            .border_color(c.border)
            .child(filter_btn("all", format!("All ({})", parts.len()), UsageFilter::All))
            .child(filter_btn("fixed", format!("Fixed · {}", tokens(fixed_tokens)), UsageFilter::Fixed))
            .child(filter_btn("conv", format!("Conversation · {}", tokens(conv_tokens)), UsageFilter::Conversation));

        // 100% composition ribbon (solves tiny 4% bar visibility!)
        let mut ribbon = div()
            .h(px(14.0))
            .w_full()
            .rounded(px(6.0))
            .bg(c.app)
            .p(px(2.0))
            .border_1()
            .border_color(c.border)
            .flex()
            .gap(px(2.0))
            .overflow_hidden();
        for p in parts {
            let kind = canonical_kind(p);
            let kind_owned = kind.to_string();
            let share = (p.tokens as f32 / parts_total as f32).clamp(0.0, 1.0);
            let dimmed = hover_kind.is_some_and(|h| h != kind);
            let tip_text = format!("{} · {} ({})", p.label, tokens(p.tokens), pct_label(p.tokens, parts_total));
            ribbon = ribbon.child(
                div()
                    .id(SharedString::from(format!("cw-rib-{kind}")))
                    .h_full()
                    .w(relative(share))
                    .min_w(px(3.0))
                    .rounded(px(3.0))
                    .bg(part_color(kind, c))
                    .cursor_pointer()
                    .when(dimmed, |d| d.opacity(0.28))
                    .tooltip(tip(tip_text))
                    .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                        let next = if *hovered { Some(kind_owned.clone()) } else if this.usage_sheet.hover_kind.as_deref() == Some(kind_owned.as_str()) { None } else { return };
                        this.usage_sheet.hover_kind = next;
                        cx.notify();
                    })),
            );
        }

        let shown: Vec<&ContextPart> = match filter {
            UsageFilter::All => parts.iter().collect(),
            UsageFilter::Fixed => fixed_parts.to_vec(),
            UsageFilter::Conversation => conv_parts.to_vec(),
        };
        let list_max = shown.iter().map(|p| p.tokens).max().unwrap_or(1).max(1);
        let mut rows = div().flex().flex_col().gap_0p5();
        for p in shown {
            rows = rows.child(self.part_row("bd", p, parts_total, list_max, hover_kind, c, mono, cx));
        }

        div()
            .rounded(px(10.0))
            .border_1()
            .border_color(c.border)
            .bg(c.panel)
            .p_3()
            .flex()
            .flex_col()
            .gap_2p5()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(seg_switch)
                    .child(section_label(c, "ESTIMATED")),
            )
            .child(ribbon)
            .child(rows)
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn part_row(
        &self,
        prefix: &'static str,
        p: &ContextPart,
        parts_total: u64,
        bar_max: u64,
        hover_kind: Option<&str>,
        c: Colors,
        mono: &SharedString,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let kind = canonical_kind(p);
        let kind_owned = kind.to_string();
        let color = part_color(kind, c);
        let dimmed = hover_kind.is_some_and(|h| h != kind);
        let highlighted = hover_kind.is_some_and(|h| h == kind);
        let bar_frac = (p.tokens as f32 / bar_max.max(1) as f32).clamp(0.02, 1.0);

        let mut row = div()
            .id(SharedString::from(format!("cw-part-{prefix}-{kind}")))
            .px_1p5()
            .py(px(5.0))
            .mx(px(-6.0))
            .rounded(px(7.0))
            .flex()
            .flex_col()
            .gap_1()
            .when(highlighted, |d| d.bg(c.hover))
            .when(!highlighted, |d| d.hover(move |d| d.bg(c.hover)))
            .when(dimmed, |d| d.opacity(0.45))
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                let next = if *hovered { Some(kind_owned.clone()) } else if this.usage_sheet.hover_kind.as_deref() == Some(kind_owned.as_str()) { None } else { return };
                this.usage_sheet.hover_kind = next;
                cx.notify();
            }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_size(px(12.0))
                    .child(div().size(px(7.0)).flex_shrink_0().rounded(px(2.5)).bg(color))
                    .child(div().flex_1().min_w_0().truncate().text_color(c.fg).child(p.label.clone()))
                    .child(div().font_family(mono.clone()).text_size(px(11.5)).font_weight(FontWeight::MEDIUM).text_color(c.fg).child(tokens(p.tokens)))
                    .child(div().w(px(32.0)).text_right().font_family(mono.clone()).text_size(px(11.0)).text_color(c.subtle).child(pct_label(p.tokens, parts_total))),
            )
            .child(
                div()
                    .ml(px(15.0))
                    .h(px(3.5))
                    .rounded_full()
                    .bg(c.app)
                    .overflow_hidden()
                    .child(div().h_full().w(relative(bar_frac)).rounded_full().bg(color)),
            );

        if !p.sources.is_empty() {
            let mut chips = div().ml(px(15.0)).pt(px(2.0)).flex().flex_wrap().gap_1();
            for src in p.sources.iter().take(5) {
                chips = chips.child(
                    div()
                        .h(px(20.0))
                        .px_2()
                        .flex()
                        .items_center()
                        .gap_1p5()
                        .rounded(px(5.0))
                        .bg(c.app)
                        .border_1()
                        .border_color(c.border)
                        .text_size(px(10.5))
                        .child(div().text_color(c.muted).child(src.label.clone()))
                        .child(div().font_family(mono.clone()).font_weight(FontWeight::SEMIBOLD).text_color(c.fg).child(tokens(src.tokens))),
                );
            }
            row = row.child(chips);
        }

        row
    }

    /// Where on the compaction bar a pointer is, in 5% steps between 5 and 95.
    fn usage_ctx_percent(&self, at: Point<Pixels>) -> Option<u64> {
        let b = self.usage_sheet.ctx_bounds.get()?;
        let frac = (at.x - b.left()) / b.size.width;
        Some(snap_percent(frac))
    }

    fn finish_usage_compact_drag(&mut self, cx: &mut Context<Self>) {
        if let Some((is_trim, v)) = self.usage_sheet.drag.take() {
            let cmd = if is_trim {
                Cmd::SetCompaction { trim: Some(v), compact: None }
            } else {
                Cmd::SetCompaction { trim: None, compact: Some(v) }
            };
            self.send(cmd, cx);
        }
        cx.notify();
    }

    #[allow(clippy::too_many_arguments)]
    fn usage_tab_compaction(
        &self,
        window: u64,
        trim: u64,
        compact: u64,
        tool_results_tokens: u64,
        conv_tokens: u64,
        has_breakdown: bool,
        c: Colors,
        mono: &SharedString,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let overlap = trim > 0 && compact > 0 && trim >= compact;
        let pct = |v: u64| relative(v as f32 / 100.0);
        let segment = |from: u64, to: u64, bg: Option<Hsla>, label: &str, color: Hsla| {
            div()
                .absolute()
                .top_0()
                .bottom_0()
                .left(pct(from))
                .w(pct(to.saturating_sub(from)))
                .flex()
                .items_center()
                .justify_center()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_size(px(11.0))
                .text_color(color)
                .when_some(bg, |d, bg| d.bg(bg))
                .child(label.to_string())
        };

        let first = [trim, compact].into_iter().filter(|v| *v > 0).min().unwrap_or(100);
        let bounds = self.usage_sheet.ctx_bounds.clone();
        let mut inner = div()
            .relative()
            .h(px(38.0))
            .rounded(px(8.0))
            .bg(c.app)
            .border_1()
            .border_color(c.edge)
            .overflow_hidden()
            .child(canvas(move |b, _, _| bounds.set(Some(b)), |_, _, _, _| {}).absolute().size_full())
            .child(segment(0, first, None, "conversation", c.subtle));
        if trim > 0 && (compact == 0 || trim < compact) {
            inner = inner.child(segment(trim, if compact > 0 { compact } else { 100 }, Some(alpha(c.warning, 0.2)), "trims", c.warning));
        }
        if compact > 0 {
            inner = inner.child(segment(compact, 100, Some(alpha(c.accent, 0.26)), "summarizes", c.accent));
        }

        let marker = |v: u64, color: Hsla| {
            div()
                .absolute()
                .left(pct(v))
                .top(px(-26.0))
                .bottom(px(-4.0))
                .w(px(0.0))
                .child(div().absolute().left(px(-1.5)).top(px(20.0)).bottom_0().w(px(3.0)).rounded_full().bg(color))
                .child(
                    div()
                        .absolute()
                        .top_0()
                        .left(px(-22.0))
                        .w(px(44.0))
                        .h(px(18.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_full()
                        .bg(color)
                        .font_family(mono.clone())
                        .text_size(px(10.5))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(c.app)
                        .child(format!("{v}%")),
                )
        };

        let bar = div()
            .relative()
            .child(inner)
            .when(trim > 0, |d| d.child(marker(trim, c.warning)))
            .when(compact > 0, |d| d.child(marker(compact, c.accent)));

        let drag_area = div()
            .id("cw-usage-ctx-drag")
            .pt(px(26.0))
            .cursor(CursorStyle::ResizeLeftRight)
            .on_mouse_down(MouseButton::Left, cx.listener(|this, e: &MouseDownEvent, _, cx| {
                let Some(v) = this.usage_ctx_percent(e.position) else { return };
                let (t, k) = {
                    let st = &this.model.read(cx).state;
                    (st.trim_at, st.compact_at)
                };
                // The nearest marker that's on follows the pointer.
                let is_trim = match (t > 0, k > 0) {
                    (true, true) => t.abs_diff(v) <= k.abs_diff(v),
                    (true, false) => true,
                    (false, true) => false,
                    (false, false) => return,
                };
                this.usage_sheet.drag = Some((is_trim, v));
                cx.notify();
            }))
            .on_mouse_move(cx.listener(|this, e: &MouseMoveEvent, _, cx| {
                let Some((is_trim, _)) = this.usage_sheet.drag else { return };
                if e.pressed_button != Some(MouseButton::Left) {
                    return this.finish_usage_compact_drag(cx);
                }
                if let Some(v) = this.usage_ctx_percent(e.position) {
                    this.usage_sheet.drag = Some((is_trim, v));
                    cx.notify();
                }
            }))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _, cx| this.finish_usage_compact_drag(cx)))
            .on_mouse_up_out(MouseButton::Left, cx.listener(|this, _, _, cx| this.finish_usage_compact_drag(cx)))
            .child(bar);

        let tick = |v: u64| {
            let label = div()
                .absolute()
                .w(px(40.0))
                .flex()
                .font_family(mono.clone())
                .text_size(px(10.0))
                .text_color(c.subtle)
                .child(format!("{v}%"));
            let label = match v {
                0 => label.left_0(),
                100 => label.left(px(-40.0)).justify_end(),
                _ => label.left(px(-20.0)).justify_center(),
            };
            div().absolute().left(pct(v)).top(px(5.0)).w(px(0.0)).child(label)
        };
        let ticks = div().relative().h(px(20.0)).children([0, 25, 50, 75, 100].map(tick));

        let toggle_switch = |id: &'static str, on: bool, is_trim: bool, default: u64| {
            div()
                .id(SharedString::from(id))
                .w(px(32.0))
                .h(px(18.0))
                .flex_shrink_0()
                .rounded_full()
                .bg(if on { c.accent } else { c.active })
                .p(px(2.0))
                .cursor_pointer()
                .flex()
                .when(on, |d| d.justify_end())
                .on_click(cx.listener(move |this, _, _, cx| {
                    let next = if on { 0 } else { default };
                    let cmd = if is_trim {
                        Cmd::SetCompaction { trim: Some(next), compact: None }
                    } else {
                        Cmd::SetCompaction { trim: None, compact: Some(next) }
                    };
                    this.send(cmd, cx);
                }))
                .child(div().size(px(14.0)).rounded_full().bg(hsla(0.0, 0.0, 1.0, 1.0)))
        };

        let trigger_hint = |v: u64, fallback: String| -> String {
            if v > 0 && window > 0 {
                format!("{fallback} · at {}", tokens(((window as f64) * (v as f64 / 100.0)).round() as u64))
            } else {
                fallback
            }
        };

        let micro_sub = if has_breakdown {
            trigger_hint(trim, format!("Trims old tool output ({} now)", tokens(tool_results_tokens)))
        } else {
            trigger_hint(trim, "Trims old tool output and file contents".to_string())
        };
        let full_sub = if has_breakdown {
            trigger_hint(compact, format!("Summarizes chat ({} eligible)", tokens(conv_tokens)))
        } else {
            trigger_hint(compact, "Summarizes the conversation".to_string())
        };

        let legend = |id: &'static str, label: &'static str, sub: String, color: Hsla, on: bool, is_trim: bool, default: u64| {
            div()
                .flex_1()
                .min_w(px(230.0))
                .flex()
                .items_center()
                .gap_2p5()
                .px(px(11.0))
                .py(px(9.0))
                .rounded(px(8.0))
                .bg(c.app)
                .child(div().size(px(10.0)).flex_shrink_0().rounded(px(3.0)).bg(color))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(1.0))
                        .child(div().whitespace_nowrap().text_size(px(12.0)).font_weight(FontWeight::MEDIUM).text_color(c.fg).child(label))
                        .child(div().text_size(px(10.5)).text_color(c.subtle).whitespace_normal().child(sub)),
                )
                .child(toggle_switch(id, on, is_trim, default))
        };

        let l1 = legend("cw-ctx-trim", "Micro-compact", micro_sub, c.warning, trim > 0, true, 60);
        let l2 = legend("cw-ctx-full", "Full compact", full_sub, c.accent, compact > 0, false, 85);

        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .px_0p5()
                    .child(section_label(c, "CONTEXT COMPACTION"))
                    .child(div().text_size(px(11.0)).text_color(c.subtle).child("Drag the markers to adjust")),
            )
            .child(
                div()
                    .rounded(px(12.0))
                    .border_1()
                    .border_color(c.edge)
                    .bg(c.panel)
                    .px_4()
                    .pt_2()
                    .pb_3p5()
                    .child(drag_area)
                    .child(ticks)
                    .child(div().flex().justify_center().text_size(px(11.0)).text_color(c.subtle).child("How full the context window is"))
                    .child(div().flex().flex_wrap().gap_2().mt_2p5().child(l1).child(l2))
                    .when(overlap, |d| {
                        d.child(div().mt_2().text_size(px(11.5)).text_color(c.warning).child("At or below micro-compact's level, so micro-compact never runs."))
                    }),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{ContextPart, is_fixed_part, pct_label, snap_percent};

    #[test]
    fn classifies_fixed_and_conversation_parts() {
        let part = |kind: &str, label: &str| ContextPart { kind: kind.into(), label: label.into(), tokens: 100, sources: Vec::new() };
        assert!(is_fixed_part(&part("system", "System prompt")));
        assert!(is_fixed_part(&part("tools", "Tools")));
        assert!(is_fixed_part(&part("mcp", "MCP tools")));
        assert!(is_fixed_part(&part("instructions", "Memory & instructions")));
        assert!(is_fixed_part(&part("", "System prompt")));
        assert!(!is_fixed_part(&part("messages", "Your messages")));
        assert!(!is_fixed_part(&part("toolResults", "Tool results")));
        assert!(!is_fixed_part(&part("", "Tool results")));
    }

    #[test]
    fn formats_small_and_round_percentages() {
        assert_eq!(pct_label(0, 1000), "0%");
        assert_eq!(pct_label(3, 1000), "<1%");
        assert_eq!(pct_label(29, 36_000), "<1%");
        assert_eq!(pct_label(12_700, 36_000), "35%");
    }

    #[test]
    fn snaps_drag_positions_to_five_percent_steps() {
        assert_eq!(snap_percent(-0.2), 5);
        assert_eq!(snap_percent(0.0), 5);
        assert_eq!(snap_percent(0.61), 60);
        assert_eq!(snap_percent(0.84), 85);
        assert_eq!(snap_percent(1.0), 95);
    }
}
