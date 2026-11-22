//! Panels that open over the chat: model, effort, mode, sessions, usage, tasks and the rest
//! (port of `features/code/Sheets.tsx`).

use super::code_view::{CodeView, elapsed, rise, tokens};
use crate::code::names::{BACKENDS, ModelTier, backend_of, base_spec, fast_spec, is_fast, model_name, model_tier};
use crate::code::protocol::*;
use crate::code::state::{Sheet, now_ms};
use crate::theme::Colors;
use gpui_kit::assets::IconName;
use gpui_kit::component::Icon;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

const EFFORTS: [&str; 6] = ["minimal", "low", "medium", "high", "xhigh", "max"];

fn effort_label(level: &str) -> String {
    match level {
        "default" => "Default".into(),
        "xhigh" => "Extra high".into(),
        l => l.chars().next().map_or(String::new(), |c| c.to_uppercase().chain(l.chars().skip(1)).collect()),
    }
}

fn effort_hint(level: &str) -> &'static str {
    match level {
        "default" => "The model's own default",
        "minimal" => "Barely any reasoning, fastest",
        "low" => "Quick answers, light reasoning",
        "medium" => "Balanced",
        "high" => "Thorough reasoning",
        "xhigh" => "Very thorough, slower",
        "max" => "As much reasoning as the model can do",
        _ => "",
    }
}

fn title_of(sheet: Sheet) -> (&'static str, IconName) {
    match sheet {
        Sheet::TaskLog => ("Task output", IconName::SquareTerminal),
        Sheet::Usage => ("Usage", IconName::Coins),
        Sheet::Status => ("Status", IconName::Info),
        Sheet::Help => ("Keys & commands", IconName::CircleQuestionMark),
        Sheet::Mcp => ("MCP servers", IconName::Plug),
        Sheet::Login => ("Sign in", IconName::KeyRound),
        Sheet::Agents => ("Subagents & tasks", IconName::Bot),
        Sheet::Sessions => ("Resume a session", IconName::Clock),
        Sheet::Btw => ("By the way", IconName::MessageCircleQuestionMark),
        Sheet::Compaction => ("Context compaction", IconName::Shrink),
        Sheet::Model => ("Model", IconName::Cpu),
        Sheet::Effort => ("Reasoning effort", IconName::Zap),
        Sheet::Mode => ("Permission mode", IconName::Shield),
        Sheet::SubagentModel => ("Subagent model", IconName::Workflow),
    }
}

fn list_item(c: Colors, id: impl Into<ElementId>, selected: bool) -> Stateful<Div> {
    div().id(id).w_full().flex().items_center().gap_2p5().px_3().py_2().rounded_xl().border_1().cursor_pointer().text_size(px(12.5)).text_color(c.fg)
        .when(selected, |d| d.bg(Hsla { a: 0.12, ..c.accent }))
        .when(!selected, |d| d.hover(move |d| d.bg(c.hover)))
}

fn dim(c: Colors, text: impl Into<SharedString>) -> Div {
    div().text_size(px(11.5)).text_color(c.subtle).child(text.into())
}

impl CodeView {
    pub(super) fn render_sheet(&mut self, sheet: Sheet, c: Colors, mono: SharedString, _window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let (title, icon) = title_of(sheet);
        let body = match sheet {
            Sheet::Model => self.sheet_models(false, c, cx),
            Sheet::SubagentModel => self.sheet_models(true, c, cx),
            Sheet::Effort => self.sheet_effort(c, cx),
            Sheet::Mode => self.sheet_mode(c, cx),
            Sheet::Sessions => self.sheet_sessions(c, cx),
            Sheet::Usage => self.sheet_usage(c, cx),
            Sheet::Status => self.sheet_status(c, mono.clone(), cx),
            Sheet::Help => self.sheet_help(c, mono.clone(), cx),
            Sheet::Mcp => self.sheet_mcp(c, cx),
            Sheet::Login => self.sheet_login(c),
            Sheet::Agents => self.sheet_agents(c, cx),
            Sheet::TaskLog => self.sheet_task_log(c, mono.clone(), cx),
            Sheet::Btw => self.sheet_btw(c, cx),
            Sheet::Compaction => self.sheet_compaction(c, cx),
        };
        if matches!(sheet, Sheet::Model | Sheet::SubagentModel | Sheet::Effort | Sheet::Mode) {
            let st = &self.model.read(cx).state;
            let subtitle = match sheet {
                Sheet::Effort => effort_label(if st.effort.is_empty() { "default" } else { &st.effort }),
                Sheet::Mode => st.mode.label().to_string(),
                Sheet::SubagentModel => st.sub_model.clone().unwrap_or_else(|| "same as main".into()),
                _ => if st.model.is_empty() { String::new() } else { model_name(&st.model, st.actual_model.as_deref()) },
            };
            return self.bottom_sheet(title, icon, subtitle, body, c, cx);
        }
        let wide = matches!(sheet, Sheet::Help | Sheet::Usage | Sheet::TaskLog | Sheet::Model | Sheet::SubagentModel);
        let scrim = div().id("cw-sheet-scrim").absolute().top_0().left_0().size_full().flex().items_center().justify_center().bg(Hsla { a: 0.5, ..black() }).p_4()
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| this.set_sheet(None, cx)))
            .child(
                rise(div().id("cw-sheet").w(px(if wide { 560.0 } else { 440.0 })).max_w_full().max_h_full().flex().flex_col().rounded_xl().border_1().border_color(c.edge).bg(c.raised).shadow_lg()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(div().flex().items_center().gap_2().px_4().py_3().child(Icon::new(icon).size(px(14.0)).text_color(c.accent)).child(div().flex_1().text_size(px(13.0)).text_color(c.fg).child(title))
                        .child(div().id("cw-sheet-x").p_0p5().rounded_md().cursor_pointer().text_color(c.subtle).hover(move |d| d.bg(c.hover).text_color(c.fg)).on_click(cx.listener(|this, _, _, cx| this.set_sheet(None, cx))).child(Icon::new(IconName::X).size(px(14.0)))))
                    .child(div().id("cw-sheet-body").flex_1().min_h_0().overflow_y_scroll().p_3().child(body)), SharedString::from(format!("sheet-in-{title}")), 0, 12.0),
            );
        scrim.with_animation(SharedString::from(format!("sheet-scrim-{title}")), Animation::new(std::time::Duration::from_millis(180)), |d, t| d.opacity(t)).into_any_element()
    }

    /// A picker that slides up from the bottom of the pane, over the composer.
    fn bottom_sheet(&mut self, title: &'static str, icon: IconName, subtitle: String, body: AnyElement, c: Colors, cx: &mut Context<Self>) -> AnyElement {
        let panel = div().id("cw-sheet").w_full().max_w(px(super::code_view::COLUMN)).max_h(relative(0.78)).flex().flex_col().rounded_t_2xl().border_1().border_b_0().border_color(c.edge).bg(c.raised).shadow_lg()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(div().w_full().flex().justify_center().pt_2().child(div().w(px(36.0)).h(px(4.0)).rounded_full().bg(c.border_strong)))
            .child(div().flex().items_center().gap_2p5().px_5().pt_2().pb_3()
                .child(div().size(px(26.0)).flex().items_center().justify_center().rounded_lg().bg(Hsla { a: 0.14, ..c.accent }).child(Icon::new(icon).size(px(14.0)).text_color(c.accent)))
                .child(div().flex_1().flex().flex_col().child(div().text_size(px(14.0)).text_color(c.fg).child(title)).when(!subtitle.is_empty(), |d| d.child(div().text_size(px(11.5)).text_color(c.subtle).child(format!("Now: {subtitle}")))))
                .child(div().id("cw-sheet-x").size(px(26.0)).flex().items_center().justify_center().rounded_lg().cursor_pointer().text_color(c.subtle).hover(move |d| d.bg(c.hover).text_color(c.fg)).on_click(cx.listener(|this, _, _, cx| this.set_sheet(None, cx))).child(Icon::new(IconName::X).size(px(14.0)))))
            .child(div().id("cw-sheet-body").flex_1().min_h_0().overflow_y_scroll().px_4().pb_4().child(body));
        let scrim = div().id("cw-sheet-scrim").absolute().top_0().left_0().size_full().flex().flex_col().justify_end().items_center().px_2().bg(Hsla { a: 0.45, ..black() })
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| this.set_sheet(None, cx)))
            .child(rise(panel, SharedString::from(format!("drawer-{title}")), 0, 36.0));
        scrim.with_animation(SharedString::from(format!("drawer-scrim-{title}")), Animation::new(std::time::Duration::from_millis(200)), |d, t| d.opacity(t)).into_any_element()
    }

    fn sheet_models(&mut self, sub: bool, c: Colors, cx: &mut Context<Self>) -> AnyElement {
        let m = self.model.read(cx);
        let st = &m.state;
        let current = if sub { st.sub_model.clone().unwrap_or_default() } else { st.model.clone() };
        let models = st.models.clone();
        let mut out = div().flex().flex_col().gap_3();
        if sub {
            let main = st.sub_model.is_none();
            out = out.child(list_item(c, "sub-main", main).on_click(cx.listener(|this, _, _, cx| {
                this.send(Cmd::SetSubagent { model: None, effort: None }, cx);
                this.set_sheet(None, cx);
            })).child(div().flex_1().child("Same as the main model")).when(main, |d| d.child(Icon::new(IconName::Check).size(px(13.0)).text_color(c.accent))));
        }
        if models.is_empty() {
            return out.child(dim(c, "No models listed yet. They appear once the session has started.")).into_any_element();
        }
        for (backend, label) in BACKENDS {
            let list: Vec<&ModelInfo> = models.iter().filter(|m| backend_of(&m.spec).map(|b| b.0) == Some(backend) && !is_fast(&m.spec)).collect();
            if list.is_empty() {
                continue;
            }
            let mut group = div().flex().flex_col().gap_1().child(div().px_1().pt_1().text_size(px(10.5)).text_color(c.subtle).child(label.to_uppercase()));
            for info in list {
                let spec = info.spec.clone();
                let tail = spec.split_once('/').map_or(spec.clone(), |(_, t)| t.to_string());
                let selected = if sub { current == tail } else { base_spec(&current) == spec };
                let tier = model_tier(&spec, &info.detail);
                let tier_label = match tier { ModelTier::Fast => "fast", ModelTier::Frontier => "frontier", ModelTier::Balanced => "" };
                let twin = fast_spec(&spec, &models);
                let on_fast = !sub && twin.as_deref() == Some(current.as_str());
                let spec_pick = spec.clone();
                let name = if info.label.is_empty() { model_name(&spec, None) } else { info.label.clone() };
                let mut row = list_item(c, SharedString::from(format!("m-{spec}")), selected || on_fast)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if sub {
                            let tail = spec_pick.split_once('/').map_or(spec_pick.clone(), |(_, t)| t.to_string());
                            this.send(Cmd::SetSubagent { model: Some(tail), effort: None }, cx);
                        } else {
                            this.send(Cmd::SetModel(spec_pick.clone()), cx);
                        }
                        this.set_sheet(None, cx);
                    }))
                    .child(div().size(px(28.0)).flex_shrink_0().flex().items_center().justify_center().rounded_lg().bg(if selected || on_fast { Hsla { a: 0.18, ..c.accent } } else { c.hover })
                        .child(Icon::new(match tier { ModelTier::Fast => IconName::Zap, ModelTier::Frontier => IconName::Sparkles, ModelTier::Balanced => IconName::Cpu }).size(px(14.0)).text_color(if selected || on_fast { c.accent } else { c.muted })))
                    .child(div().flex().flex_col().flex_1().min_w_0().child(div().truncate().text_color(c.fg).child(name)).child(div().truncate().text_size(px(11.0)).text_color(c.subtle).child(info.detail.clone())));
                if !tier_label.is_empty() {
                    let tint = if matches!(tier, ModelTier::Frontier) { c.accent } else { c.warning };
                    row = row.child(div().text_size(px(10.0)).px_1p5().py_0p5().rounded_full().bg(Hsla { a: 0.14, ..tint }).text_color(tint).child(tier_label));
                }
                if let (Some(fast), false) = (twin, sub) {
                    let spec_fast = if on_fast { spec.clone() } else { fast.clone() };
                    row = row.child(div().id(SharedString::from(format!("fast-{spec}"))).px_1p5().h(px(20.0)).flex().items_center().gap_0p5().rounded_md().border_1().text_size(px(10.5))
                        .text_color(if on_fast { c.warning } else { c.subtle })
                        .on_click(cx.listener(move |this, _, _, cx| { cx.stop_propagation(); this.send(Cmd::SetModel(spec_fast.clone()), cx); this.set_sheet(None, cx); }))
                        .child(Icon::new(IconName::Zap).size(px(10.0))).child("Fast"));
                }
                if selected || on_fast {
                    row = row.child(Icon::new(IconName::Check).size(px(13.0)).text_color(c.accent));
                }
                group = group.child(row);
            }
            out = out.child(group);
        }
        if sub {
            let effort = st.sub_effort.clone().unwrap_or_default();
            if st.sub_model.is_some() {
                let mut row = div().flex().flex_wrap().gap_1().px_2().pt_1().child(dim(c, "Effort"));
                for level in std::iter::once("default").chain(EFFORTS) {
                    let on = (level == "default" && effort.is_empty()) || effort == level;
                    let (model, level_s) = (st.sub_model.clone(), level.to_string());
                    row = row.child(div().id(SharedString::from(format!("se-{level}"))).px_2().h(px(22.0)).flex().items_center().rounded_md().border_1().text_size(px(11.0)).cursor_pointer()
                        .text_color(if on { c.fg } else { c.muted })
                        .on_click(cx.listener(move |this, _, _, cx| this.send(Cmd::SetSubagent { model: model.clone(), effort: if level_s == "default" { None } else { Some(level_s.clone()) } }, cx)))
                        .child(effort_label(level)));
                }
                out = out.child(row);
            }
        }
        out.into_any_element()
    }

    fn sheet_effort(&mut self, c: Colors, cx: &mut Context<Self>) -> AnyElement {
        let m = self.model.read(cx);
        let st = &m.state;
        let offered = super::code_pickers::match_model(&st.models, &st.model).and_then(|i| i.efforts.clone());
        let levels: Vec<String> = match offered {
            Some(l) if !l.is_empty() => std::iter::once("default".to_string()).chain(l).collect(),
            Some(_) => vec!["default".to_string()],
            None => std::iter::once("default").chain(EFFORTS).map(String::from).collect(),
        };
        let current = if st.effort.is_empty() { "default".to_string() } else { st.effort.clone() };
        let mut strip = div().flex().flex_wrap().gap_2();
        for level in &levels {
            let on = current == *level;
            let rank = EFFORTS.iter().position(|e| e == level).map(|p| p + 1);
            let pick = level.clone();
            let bars = div().flex().items_end().gap(px(2.5)).h(px(18.0)).children((1..=EFFORTS.len()).map(|b| {
                let filled = rank.is_some_and(|r| b <= r);
                div().w(px(3.5)).h(px(5.0 + b as f32 * 2.2)).rounded_full().bg(if filled { if on { c.accent } else { c.muted } } else { c.border_strong })
            }));
            let glyph: AnyElement = if rank.is_some() { bars.into_any_element() } else { div().h(px(18.0)).flex().items_center().child(Icon::new(IconName::Sparkles).size(px(15.0)).text_color(if on { c.accent } else { c.muted })).into_any_element() };
            strip = strip.child(div().id(SharedString::from(format!("e-{level}"))).flex_1().min_w(px(88.0)).flex().flex_col().items_center().gap_2().py_3().rounded_xl().border_1().cursor_pointer().text_size(px(12.0))
                .when(on, |d| d.bg(Hsla { a: 0.12, ..c.accent }).text_color(c.fg))
                .when(!on, |d| d.text_color(c.muted).hover(move |d| d.bg(c.hover).text_color(c.fg)))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.send(Cmd::SetEffort(if pick == "default" { None } else { Some(pick.clone()) }), cx);
                    this.set_sheet(None, cx);
                }))
                .child(glyph)
                .child(effort_label(level)));
        }
        div().flex().flex_col().gap_3().child(strip).child(div().flex().items_center().gap_2().px_1().text_size(px(12.0)).text_color(c.muted).child(Icon::new(IconName::Info).size(px(13.0)).text_color(c.subtle)).child(effort_hint(&current))).into_any_element()
    }

    fn sheet_mode(&mut self, c: Colors, cx: &mut Context<Self>) -> AnyElement {
        let current = self.model.read(cx).state.mode;
        let mut out = div().flex().flex_col().gap_1();
        for mode in Mode::ALL {
            out = out.child(list_item(c, SharedString::from(format!("mode-{}", mode.id())), mode == current)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.send(Cmd::SetMode(mode), cx);
                    this.set_sheet(None, cx);
                }))
                .child(div().flex().flex_col().flex_1().child(mode.label()).child(dim(c, mode.hint())))
                .when(mode == current, |d| d.child(Icon::new(IconName::Check).size(px(13.0)).text_color(c.accent))));
        }
        out.child(dim(c, "Shift+Tab cycles the mode.")).into_any_element()
    }

    fn sheet_sessions(&mut self, c: Colors, cx: &mut Context<Self>) -> AnyElement {
        let sessions = self.model.read(cx).state.sessions.clone();
        let Some(list) = sessions else { return dim(c, "Loading…").into_any_element() };
        if list.is_empty() {
            return dim(c, "No past sessions in this folder.").into_any_element();
        }
        let now = now_ms() as f64 / 1000.0;
        let mut out = div().flex().flex_col().gap_0p5();
        for s in list {
            let id = s.id.clone();
            let age = elapsed(((now - s.updated).max(0.0) * 1000.0) as u64);
            out = out.child(list_item(c, SharedString::from(format!("s-{}", s.id)), false)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.send(Cmd::Resume(id.clone()), cx);
                    this.set_sheet(None, cx);
                }))
                .child(div().flex().flex_col().flex_1().min_w_0().child(div().truncate().child(if s.title.is_empty() { "Untitled".to_string() } else { s.title.clone() }))
                    .child(dim(c, format!("{} · {} messages · {age} ago", model_name(&s.model, None), s.messages)))));
        }
        out.into_any_element()
    }

    fn sheet_usage(&mut self, c: Colors, cx: &mut Context<Self>) -> AnyElement {
        let m = self.model.read(cx);
        let st = &m.state;
        let u = &st.usage;
        let pct = st.context_fraction();
        let mut out = div().flex().flex_col().gap_3()
            .child(div().flex().flex_col().gap_1()
                .child(div().flex().justify_between().text_size(px(12.5)).text_color(c.fg).child("Context").child(format!("{} / {} · {}%", tokens(u.context), tokens(u.window), (pct * 100.0).round())))
                .child(div().h(px(4.0)).rounded_full().bg(c.border).child(div().h_full().w(relative(pct)).rounded_full().bg(if pct > 0.85 { c.danger } else { c.accent }))));
        let mut grid = div().flex().flex_wrap().gap_x_4().gap_y_1();
        for (label, value) in [("Input", tokens(u.input)), ("Cached", tokens(u.cached)), ("Output", tokens(u.output)), ("Cost", u.cost.map_or("—".to_string(), |c| format!("${c:.2}")))] {
            grid = grid.child(div().flex().flex_col().child(dim(c, label)).child(div().text_size(px(13.0)).text_color(c.fg).child(value)));
        }
        out = out.child(grid);
        match &st.breakdown {
            None => out = out.child(dim(c, "Reading the context…")),
            Some((_, None)) => out = out.child(dim(c, "This session's log can't be read, so there's no breakdown.")),
            Some((total, Some(parts))) => {
                let total = (*total).max(1);
                let mut list = div().flex().flex_col().gap_1p5().child(dim(c, "What fills the context (estimated)"));
                for p in parts {
                    let share = p.tokens as f32 / total as f32;
                    list = list.child(div().flex().flex_col().gap_0p5()
                        .child(div().flex().justify_between().text_size(px(12.0)).text_color(c.fg).child(p.label.clone()).child(format!("{} · {}%", tokens(p.tokens), (share * 100.0).round())))
                        .child(div().h(px(3.0)).rounded_full().bg(c.border).child(div().h_full().w(relative(share.clamp(0.0, 1.0))).rounded_full().bg(c.accent)))
                        .children(p.sources.iter().take(4).map(|s| div().flex().justify_between().pl_2().text_size(px(11.0)).text_color(c.subtle).child(s.label.clone()).child(tokens(s.tokens)))));
                }
                out = out.child(list);
            }
        }
        out.into_any_element()
    }

    fn sheet_status(&mut self, c: Colors, mono: SharedString, cx: &mut Context<Self>) -> AnyElement {
        let m = self.model.read(cx);
        let st = &m.state;
        let rows = [
            ("Session", st.session.clone().unwrap_or_else(|| "—".into())),
            ("Model", if st.model.is_empty() { "—".into() } else { format!("{} ({})", model_name(&st.model, st.actual_model.as_deref()), st.model) }),
            ("Mode", st.mode.label().to_string()),
            ("Effort", if st.effort.is_empty() { "default".into() } else { st.effort.clone() }),
            ("Directory", st.cwd.clone().unwrap_or_else(|| "—".into())),
            ("Branch", st.branch.clone().unwrap_or_else(|| "—".into())),
            ("Subagents", st.sub_model.clone().map_or("Same as the main model".to_string(), |m| m)),
        ];
        let mut out = div().flex().flex_col().gap_1p5();
        for (k, v) in rows {
            out = out.child(div().flex().gap_3().text_size(px(12.5)).child(div().w(px(84.0)).flex_shrink_0().text_color(c.subtle).child(k)).child(div().flex_1().min_w_0().font_family(mono.clone()).text_color(c.fg).whitespace_normal().child(v)));
        }
        out.into_any_element()
    }

    fn sheet_help(&mut self, c: Colors, mono: SharedString, cx: &mut Context<Self>) -> AnyElement {
        let keys = [("enter", "send (steers the running turn)"), ("shift+enter", "new line"), ("esc", "interrupt / close a panel"), ("shift+tab", "cycle permission mode"), ("y · a · n", "allow · always allow · deny an approval (empty composer)"), ("/", "commands")];
        let mut out = div().flex().flex_col().gap_3();
        let mut k = div().flex().flex_col().gap_1();
        for (key, d) in keys {
            k = k.child(div().flex().gap_3().text_size(px(12.0)).child(div().w(px(90.0)).flex_shrink_0().font_family(mono.clone()).text_color(c.fg).child(key)).child(div().text_color(c.muted).child(d)));
        }
        out = out.child(k);
        let mut cmds = div().flex().flex_col();
        for (cmd, d) in COMMANDS {
            cmds = cmds.child(list_item(c, SharedString::from(format!("h-{cmd}")), false)
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.set_sheet(None, cx);
                    this.run_from_sheet(cmd, window, cx);
                }))
                .child(div().w(px(130.0)).flex_shrink_0().font_family(mono.clone()).child(cmd)).child(div().flex_1().min_w_0().text_size(px(11.5)).text_color(c.subtle).child(d)));
        }
        out.child(cmds).into_any_element()
    }

    fn sheet_mcp(&mut self, c: Colors, cx: &mut Context<Self>) -> AnyElement {
        let mcp = self.model.read(cx).state.mcp.clone();
        if mcp.is_empty() {
            return dim(c, "No MCP servers. Add them under \"mcpServers\" in WinferCode's config.json (inside winfer, its own server is added automatically).").into_any_element();
        }
        div().flex().flex_col().gap_1().children(mcp.into_iter().map(|m| div().flex().items_center().gap_1p5().px_2().py_1().text_size(px(12.5)).text_color(c.fg).child(Icon::new(IconName::Activity).size(px(12.0)).text_color(c.success)).child(m))).into_any_element()
    }

    fn sheet_login(&mut self, c: Colors) -> AnyElement {
        let mut out = div().flex().flex_col().gap_2();
        for (k, v) in [("ChatGPT", "winfer → Settings → AI → Sign in with ChatGPT"), ("Claude Code", "run `claude` once and `/login`"), ("Codex", "run `codex login`")] {
            out = out.child(div().flex().gap_3().text_size(px(12.5)).child(div().w(px(96.0)).flex_shrink_0().text_color(c.fg).child(k)).child(div().text_color(c.muted).child(v)));
        }
        out.into_any_element()
    }

    fn sheet_agents(&mut self, c: Colors, cx: &mut Context<Self>) -> AnyElement {
        let tasks = self.model.read(cx).state.tasks.clone();
        if tasks.is_empty() {
            return dim(c, "No subagents or background shells in this session.").into_any_element();
        }
        let now = now_ms();
        let mut out = div().flex().flex_col().gap_1p5();
        for t in tasks.into_iter().rev() {
            let running = t.ev.status.as_deref().is_none_or(|s| s == "running");
            let (icon, color) = match t.ev.status.as_deref() {
                Some("done") => (IconName::CircleCheck, c.success),
                Some("failed") => (IconName::CircleX, c.danger),
                Some("stopped") => (IconName::CircleSlash, c.subtle),
                _ => (IconName::LoaderCircle, c.accent),
            };
            let title = t.ev.title.clone().or(t.ev.description.clone()).unwrap_or_else(|| t.ev.id.clone());
            let id = t.ev.id.clone();
            let (id_stop, id_log) = (id.clone(), id.clone());
            let has_log = t.ev.output_file.is_some() || !t.recent_lines.is_empty();
            let mut meta = vec![if t.ev.agent.is_some() { "subagent".to_string() } else { "shell".to_string() }];
            if let Some(a) = &t.ev.agent { meta.push(a.clone()); }
            if let Some(m) = &t.ev.model { meta.push(model_name(m, None)); }
            if let Some(n) = t.ev.tools { meta.push(format!("{n} tools")); }
            meta.push(elapsed(now.saturating_sub(t.started_at)));
            out = out.child(div().flex().flex_col().gap_0p5().rounded_md().border_1().border_color(c.edge).px_2().py_1p5()
                .child(div().flex().items_center().gap_1p5().child(Icon::new(icon).size(px(13.0)).text_color(color)).child(div().flex_1().min_w_0().truncate().text_size(px(12.5)).text_color(c.fg).child(title))
                    .when(has_log, |d| d.child(div().id(SharedString::from(format!("log-{id_log}"))).px_1p5().rounded_md().cursor_pointer().text_size(px(11.0)).text_color(c.muted).hover(move |d| d.bg(c.hover)).on_click(cx.listener(move |this, _, _, cx| {
                        let id = id_log.clone();
                        this.model.update(cx, |m, cx| { m.state.open_task_log(&id); m.flush(cx); cx.notify(); });
                    })).child("Output")))
                    .when(running, |d| d.child(div().id(SharedString::from(format!("stop-{id_stop}"))).px_1p5().rounded_md().cursor_pointer().text_size(px(11.0)).text_color(c.danger).hover(move |d| d.bg(c.hover)).on_click(cx.listener(move |this, _, _, cx| this.send(Cmd::StopTask(id_stop.clone()), cx))).child("Stop"))))
                .child(dim(c, meta.join(" · ")))
                .when_some(t.recent_lines.last().cloned(), |d, l| d.child(div().truncate().text_size(px(11.0)).text_color(c.subtle).child(l))));
        }
        out.into_any_element()
    }

    fn sheet_task_log(&mut self, c: Colors, mono: SharedString, cx: &mut Context<Self>) -> AnyElement {
        let st = &self.model.read(cx).state;
        match &st.task_log {
            None => dim(c, "Loading…").into_any_element(),
            Some((_, None, _)) => dim(c, "No output yet.").into_any_element(),
            Some((_, Some(text), file)) => div().flex().flex_col().gap_1()
                .when_some(file.clone(), |d, f| d.child(dim(c, f)))
                .child(div().rounded_md().border_1().border_color(c.edge).bg(c.app).p_2().font_family(mono).text_size(px(11.5)).text_color(c.muted).whitespace_normal().child(text.clone())).into_any_element(),
        }
    }

    fn sheet_btw(&mut self, c: Colors, cx: &mut Context<Self>) -> AnyElement {
        let btw = self.model.read(cx).state.btw.clone();
        if btw.is_empty() {
            return dim(c, "Ask a side question with /btw <question>. It sees this session but doesn't interrupt the turn.").into_any_element();
        }
        let mut out = div().flex().flex_col().gap_2();
        for b in btw {
            out = out.child(div().flex().flex_col().gap_1().child(div().text_size(px(12.5)).text_color(c.fg).child(b.question.clone()))
                .child(div().text_size(px(12.5)).text_color(if b.error.is_some() { c.danger } else { c.muted }).whitespace_normal().child(match (&b.answer, &b.error) {
                    (Some(a), _) => a.clone(),
                    (_, Some(e)) => e.clone(),
                    _ => "Thinking…".to_string(),
                })));
        }
        out.into_any_element()
    }

    fn sheet_compaction(&mut self, c: Colors, cx: &mut Context<Self>) -> AnyElement {
        let st = &self.model.read(cx).state;
        let (trim, compact) = (st.trim_at, st.compact_at);
        let mut out = div().flex().flex_col().gap_3().child(dim(c, "When the context fills up, WinferCode first trims old tool output (micro-compact), then summarizes the conversation (full compact)."));
        for (label, current, is_trim) in [("Micro-compact at", trim, true), ("Full compact at", compact, false)] {
            let mut row = div().flex().flex_wrap().gap_1();
            for p in std::iter::once(0u64).chain((30..=95).step_by(5)) {
                let on = current == p;
                row = row.child(div().id(SharedString::from(format!("cmp-{is_trim}-{p}"))).px_1p5().h(px(22.0)).flex().items_center().rounded_md().border_1().text_size(px(11.0)).cursor_pointer()
                    .text_color(if on { c.fg } else { c.muted })
                    .on_click(cx.listener(move |this, _, _, cx| this.send(if is_trim { Cmd::SetCompaction { trim: Some(p), compact: None } } else { Cmd::SetCompaction { trim: None, compact: Some(p) } }, cx)))
                    .child(if p == 0 { "Off".to_string() } else { format!("{p}%") }));
            }
            out = out.child(div().flex().flex_col().gap_1().child(div().text_size(px(12.5)).text_color(c.fg).child(label)).child(row));
        }
        out.into_any_element()
    }
}
