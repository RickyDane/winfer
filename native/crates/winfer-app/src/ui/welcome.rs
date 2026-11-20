//! The empty workspace (Graphite "Welcome"): a task box that starts the default agent, the
//! installed agents to pick from, and a few next steps.

use super::commands::Command;
use super::*;
use crate::model::Overlay;
use gpui_kit::component::input::{Input, InputEvent, InputState};

impl Root {
    fn welcome_input(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Entity<InputState> {
        if let Some(input) = &self.welcome_input {
            return input.clone();
        }
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Describe a task — Enter starts an agent on it"));
        cx.subscribe_in(&input, window, |this, input, event: &InputEvent, window, cx| {
            if matches!(event, InputEvent::PressEnter { .. }) {
                let task = input.read(cx).value().trim().to_string();
                if task.is_empty() {
                    return;
                }
                input.update(cx, |i, cx| i.set_value("", window, cx));
                this.execute(Command::Spawn { provider: None, count: 1, prompt: Some(task), worktree: None, model: None }, window, cx);
            }
        })
        .detach();
        self.welcome_input = Some(input.clone());
        input
    }

    pub(super) fn render_welcome(&mut self, c: Colors, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let input = self.welcome_input(window, cx);
        let s = self.state.read(cx);
        let workspace = s.active().map(|w| w.name.clone()).unwrap_or_else(|| "this workspace".into());
        let default = s.default_provider_id();
        let default_name = s.provider_name(&default);
        let agents: Vec<(String, String, bool)> = s.providers.iter().filter(|p| p.id != "shell").map(|p| (p.id.clone(), p.name.clone(), p.installed)).collect();
        let keys = |cmd: Command| commands::keys_for(&cmd, s).unwrap_or_default();
        let (palette_keys, sheet_keys, branch_keys) = (keys(Command::Palette(None)), keys(Command::CheatSheet), keys(Command::Overlay { overlay: Overlay::Branch, provider: None }));
        let mono = crate::fonts::mono(cx);

        let task = div().w_full().flex().items_center().gap_2().h(px(46.0)).pl_3p5().pr_1p5().rounded_xl().border_1().border_color(c.border_strong).bg(c.raised)
            .child(crate::ui::code_pickers::mark(c, &default, 16.0, 1.0))
            .child(div().flex_1().min_w_0().child(Input::new(&input).appearance(false)))
            .child(div().h(px(32.0)).px_3().flex().items_center().gap_1p5().rounded_lg().bg(c.accent).text_size(px(12.5)).font_weight(FontWeight::MEDIUM).text_color(c.on_accent)
                .child(format!("Start {default_name}")).child(Icon::new(IconName::CornerDownLeft).size(px(13.0))));

        let mut picks = div().flex().flex_wrap().justify_center().gap_2();
        for (id, name, installed) in agents {
            let pick = id.clone();
            picks = picks.child(
                div().id(SharedString::from(format!("welcome-{id}"))).flex().items_center().gap_2().h(px(32.0)).px_3().rounded_lg().border_1().border_color(c.border).bg(c.panel).text_size(px(12.5))
                    .text_color(if installed { c.fg } else { c.subtle })
                    .when(installed, |d| d.cursor_pointer().hover(move |d| d.border_color(c.border_strong).bg(c.hover)))
                    .when(installed, |d| d.on_click(cx.listener(move |this, _, window, cx| this.execute(Command::Overlay { overlay: Overlay::NewPane, provider: Some(pick.clone()) }, window, cx))))
                    .child(crate::ui::code_pickers::mark(c, &id, 14.0, if installed { 1.0 } else { 0.5 }))
                    .child(name)
                    .when(!installed, |d| d.child(div().text_size(px(10.5)).child("not installed"))),
            );
        }

        let step = |id: &'static str, icon: IconName, title: &'static str, text: &'static str, keys: String| {
            div().id(id).flex_1().min_w_0().flex().flex_col().gap_1().p_3().rounded_lg().border_1().border_color(c.border).bg(c.panel).cursor_pointer()
                .hover(move |d| d.border_color(c.border_strong).bg(c.hover))
                .child(div().flex().items_center().gap_2().text_size(px(12.5)).text_color(c.fg)
                    .child(Icon::new(icon).size(px(14.0)).text_color(c.accent)).child(div().flex_1().child(title))
                    .when(!keys.is_empty(), |d| d.child(div().rounded_sm().border_1().border_color(c.border).px_1().font_family(mono.clone()).text_size(px(10.0)).text_color(c.subtle).child(keys))))
                .child(div().text_size(px(11.5)).text_color(c.subtle).child(text))
        };
        let steps = div().w_full().flex().gap_2p5()
            .child(step("welcome-branch", IconName::GitBranch, "Open a branch", "Start an agent on an existing branch or worktree.", branch_keys)
                .on_click(cx.listener(|this, _, window, cx| this.execute(Command::Overlay { overlay: Overlay::Branch, provider: None }, window, cx))))
            .child(step("welcome-palette", IconName::Search, "Commands", "Everything winfer can do, by name.", palette_keys)
                .on_click(cx.listener(|this, _, window, cx| this.execute(Command::Palette(None), window, cx))))
            .child(step("welcome-keys", IconName::Zap, "Shortcuts", "See and change every shortcut.", sheet_keys)
                .on_click(cx.listener(|this, _, window, cx| this.execute(Command::CheatSheet, window, cx))));

        div().size_full().flex().items_center().justify_center().rounded_lg().border_1().border_color(c.border).bg(c.panel).child(
            div().w(px(620.0)).max_w(relative(0.92)).flex().flex_col().items_center().gap_5()
                .child(div().size(px(44.0)).flex().items_center().justify_center().rounded_xl().bg(Hsla { a: 0.14, ..c.accent }).text_color(c.accent).child(Icon::new(IconName::Sparkles).size(px(20.0))))
                .child(div().flex().flex_col().items_center().gap_1()
                    .child(div().text_size(px(20.0)).font_weight(FontWeight::SEMIBOLD).text_color(c.fg).child(format!("What should agents do in {workspace}?")))
                    .child(div().text_size(px(13.0)).text_color(c.muted).child("One line starts an agent in its own worktree. Or pick an agent below.")))
                .child(task)
                .child(picks)
                .child(steps),
        )
    }
}
