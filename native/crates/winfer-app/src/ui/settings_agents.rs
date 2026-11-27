//! Settings → Agents in five tabs: Your agents (one table for every installed agent), New panes,
//! System prompt, Tools and Review & progress (mockups/settings-overhaul/agents.html).

use super::settings::plain_button;
use super::settings_kit::{PageTab, alpha, badge, mini_switch, panel, tab_count, tab_dot};
use super::*;
use crate::model::{OPTIONAL_TOOLS, optional_tool_plugin};
use gpui_kit::component::input::Textarea;
use gpui_kit::component::tooltip::Tooltip;
use serde_json::json;

const PAGE: &str = "agents";

/// Which tab a search hit on the Agents page opens.
pub(super) fn tab_for_label(label: &str) -> &'static str {
    match label {
        "New panes" | "Default agent" | "Always open new agents in their own worktree" => "panes",
        "System prompt" | "Instructions for every agent" => "prompt",
        "Agent tools" => "tools",
        "Review" | "Review & progress" | "Run tests when an agent finishes" | "Screenshot the dev server" | "Session progress" | "Estimate how far working agents are" | "Estimate progress" | "Check every" => "review",
        _ => "agents",
    }
}

/// The color of an agent's mark tile.
fn provider_tint(c: Colors, id: &str) -> Hsla {
    match id {
        "claude" => c.claude,
        "agy" => crate::theme::hex("#4f8cff"),
        "opencode" => crate::theme::hex("#f5b544"),
        "cursor" => crate::theme::hex("#a0a0ff"),
        "pi" => crate::theme::hex("#7ee787"),
        "muse" => crate::theme::hex("#fb7185"),
        "winfer" => c.accent,
        _ => c.fg,
    }
}

/// An agent's mark on a tile tinted in its color.
pub(super) fn mark_tile(c: Colors, id: &str, size: f32) -> Div {
    let tint = provider_tint(c, id);
    let neutral = tint == c.fg;
    div().size(px(size)).flex_shrink_0().flex().items_center().justify_center().rounded(px(size * 0.27))
        .bg(alpha(tint, if neutral { 0.08 } else { 0.14 }))
        .border_1().border_color(alpha(tint, if neutral { 0.14 } else { 0.26 }))
        .child(crate::ui::code_pickers::mark(c, id, size * 0.56, 1.0))
}

fn optional_tool_meta(name: &'static str) -> (&'static str, &'static str, &'static str, IconName) {
    match name {
        "browser_drag" => ("Browser", "Drag", "Drag and drop between elements", IconName::MousePointer2),
        "browser_drop" => ("Browser", "Drop", "Drop files or data onto an element", IconName::Download),
        "browser_emulate_media" => ("Browser", "Emulate media", "Dark/light scheme, reduced motion or print", IconName::Sun),
        "browser_navigate_back" => ("Browser", "Navigate back", "Go back in browser history", IconName::Undo2),
        "kb_explain" => ("Knowledge", "Explain node", "Explain a knowledge graph node", IconName::Network),
        "kb_save" => ("Knowledge", "Save answer", "Save an answer to graph memory", IconName::Brain),
        _ => ("Tool", name, "", IconName::Wrench),
    }
}

/// A heading over a tab's content with controls (a reset mark, a link) next to the title and a
/// note on the right.
fn heading(c: Colors, title: &str, marks: Vec<AnyElement>, right: Option<AnyElement>) -> Div {
    div().flex().items_center().gap_2().mx(px(2.0)).mb(px(-6.0)).text_size(px(12.0)).font_weight(FontWeight::SEMIBOLD).text_color(c.muted)
        .child(title.to_string()).children(marks).child(div().flex_1())
        .children(right.map(|r| div().flex().items_center().gap_1p5().font_weight(FontWeight::NORMAL).text_color(c.subtle).child(r)))
}

/// A small text link in the accent color.
fn link(c: Colors, id: impl Into<ElementId>, text: &'static str) -> Stateful<Div> {
    div().id(id).flex().items_center().gap_0p5().text_color(c.accent).cursor_pointer().hover(|d| d.opacity(0.8)).child(text)
}

/// A row of an option list: icon, label (with a reset mark) and hint, and a control on the right.
fn option_row(c: Colors, icon: IconName, label: &str, mark: Option<AnyElement>, hint: Option<&str>, control: AnyElement, first: bool) -> Div {
    div().flex().items_center().gap_2p5().px_3p5().py_2p5()
        .when(!first, |d| d.border_t_1().border_color(alpha(c.border, 0.6)))
        .child(Icon::new(icon).size(px(14.0)).flex_shrink_0().text_color(c.subtle))
        .child(div().flex_1().min_w_0().flex().flex_col().gap(px(1.0))
            .child(div().flex().items_center().gap_2().text_size(px(12.5)).text_color(c.fg).child(label.to_string()).children(mark))
            .when_some(hint, |d, h| d.child(div().text_size(px(11.0)).text_color(c.subtle).child(h.to_string()))))
        .child(control)
}

/// The version, update state and auto-approve of an agent, from the version check.
struct VersionInfo {
    current: String,
    latest: Option<String>,
    outdated: bool,
    updatable: bool,
    known: bool,
}

impl Root {
    fn version_info(&self, id: &str) -> VersionInfo {
        let v = self.settings.as_ref().and_then(|s| s.versions.get(id));
        VersionInfo {
            current: v.and_then(|v| v["current"].as_str()).unwrap_or_default().to_string(),
            latest: v.and_then(|v| v["latest"].as_str()).map(str::to_string),
            outdated: v.is_some_and(|v| v["outdated"] == true),
            updatable: v.is_some_and(|v| v["updatable"] == true),
            known: v.is_some(),
        }
    }

    /// The agents the table lists: installed ones with something to set, WinferCode first.
    fn listed_agents(&self, cx: &App) -> Vec<crate::model::AgentProvider> {
        let s = self.state.read(cx);
        let mut list: Vec<_> = s.providers.iter().filter(|p| p.installed && p.id != "shell" && (p.command.is_some() || p.model_arg.is_some() || !p.auto_approve_args.is_empty())).cloned().collect();
        if let Some(pos) = list.iter().position(|p| p.id == "winfer") {
            let winfer = list.remove(pos);
            list.insert(0, winfer);
        }
        list
    }

    /// Agents with a newer version winfer can install.
    fn agents_to_update(&self, cx: &App) -> Vec<String> {
        let updating = self.settings.as_ref().map(|s| s.updating.clone()).unwrap_or_default();
        self.listed_agents(cx).into_iter().map(|p| p.id.clone()).filter(|id| {
            let v = self.version_info(id);
            v.outdated && v.updatable && !updating.contains(id)
        }).collect()
    }

    fn optional_tools(&self, cx: &App) -> Vec<&'static str> {
        let active_plugins = self.enabled_plugins();
        let _ = cx;
        OPTIONAL_TOOLS.into_iter().filter(|n| optional_tool_plugin(n).is_none_or(|id| active_plugins.iter().any(|p| p["id"] == id))).collect()
    }

    /// The tab bar of the Agents page.
    pub(super) fn agents_tabs(&mut self, c: Colors, cx: &mut Context<Self>) -> Div {
        let mono = crate::fonts::mono(cx);
        let installed = self.listed_agents(cx).len();
        let updates = !self.agents_to_update(cx).is_empty();
        let tools = self.optional_tools(cx);
        let s = self.state.read(cx);
        let on = tools.iter().filter(|n| s.enabled_tools.iter().any(|t| t == *n)).count();
        let tabs = vec![
            PageTab::new("agents", "Your agents", IconName::Bot).extra(Some(div().flex().items_center().gap(px(7.0)).child(tab_count(c, &mono, installed)).when(updates, |d| d.child(tab_dot(c.accent))).into_any_element())),
            PageTab::new("panes", "New panes", IconName::LayoutGrid),
            PageTab::new("prompt", "System prompt", IconName::MessageSquareQuote),
            PageTab::new("tools", "Tools", IconName::Wrench).extra(Some(tab_count(c, &mono, format!("{on}/{}", tools.len())))),
            PageTab::new("review", "Review & progress", IconName::ListChecks),
        ];
        self.page_tab_bar(PAGE, "agents", tabs, c, cx)
    }

    pub(super) fn page_agents(&mut self, c: Colors, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let body = match self.page_tab(PAGE, "agents") {
            "panes" => self.agents_panes(c, cx),
            "prompt" => self.agents_prompt(c, window, cx),
            "tools" => self.agents_tools(c, cx),
            "review" => self.agents_review(c, window, cx),
            _ => self.agents_list(c, cx),
        };
        body.pt(px(16.0))
    }

    /// Your agents: the table, the auto-approve note and the agents to install.
    fn agents_list(&mut self, c: Colors, cx: &mut Context<Self>) -> Div {
        let s = self.state.read(cx).clone();
        let mono = crate::fonts::mono(cx);
        let ui = self.settings.as_ref().unwrap();
        let (checking, updating, install_bounds) = (ui.checking, ui.updating.clone(), ui.install_bounds.clone());
        let listed = self.listed_agents(cx);
        let to_update = self.agents_to_update(cx);
        let default_id = s.default_provider_id();

        // Count and actions.
        let summary = div().flex().items_center().gap_1().text_size(px(12.0)).text_color(c.subtle)
            .child(div().font_weight(FontWeight::MEDIUM).text_color(c.muted).child(format!("{} installed", listed.len())))
            .when(!to_update.is_empty(), |d| d.child(format!("· {} update{}", to_update.len(), if to_update.len() == 1 { "" } else { "s" })));
        let check = plain_button(c, "agents-check", if checking { "Checking…" } else { "Check for updates" })
            .flex().items_center().gap_1p5()
            .on_click(cx.listener(|this, _, _, cx| this.check_versions(cx)));
        let n_updates = to_update.len();
        let update_all = (n_updates > 0).then(|| {
            div().id("agents-update-all").h(px(28.0)).flex().items_center().gap_1p5().px_2p5().rounded_md().bg(c.accent).text_size(px(12.0)).font_weight(FontWeight::MEDIUM).text_color(c.on_accent).cursor_pointer()
                .hover(|d| d.opacity(0.9))
                .on_click(cx.listener(move |this, _, _, cx| {
                    for id in to_update.clone() {
                        this.update_provider(id, cx);
                    }
                }))
                .child(Icon::new(IconName::Download).size(px(12.0)))
                .child(if n_updates == 1 { "Update".to_string() } else { format!("Update {n_updates}") })
        });
        let top = div().flex().items_center().gap_2().mx(px(2.0)).child(summary).child(div().flex_1()).child(check).children(update_all);

        // The table: the same columns for every agent.
        let col = |w: f32| div().w(px(w)).flex_shrink_0().flex().items_center();
        let head_cell = |text: &'static str| div().text_size(px(10.5)).font_weight(FontWeight::SEMIBOLD).text_color(c.subtle).child(text);
        let mut table = panel(c).overflow_hidden().child(
            div().flex().items_center().gap_3().px_3p5().h(px(30.0)).bg(alpha(c.app, 0.35)).border_b_1().border_color(alpha(c.border, 0.6))
                .child(col(26.0))
                .child(div().flex_1().child(head_cell("AGENT")))
                .child(col(118.0).child(head_cell("VERSION")))
                .child(col(210.0).child(head_cell("START MODEL")))
                .child(col(36.0).justify_center().child(head_cell("AUTO")))
                .child(col(36.0).justify_center().child(head_cell("★"))),
        );
        for (i, p) in listed.iter().enumerate() {
            let v = self.version_info(&p.id);
            let busy = updating.contains(&p.id);
            let is_default = p.id == default_id;
            let is_winfer = p.id == "winfer";

            let name = div().flex_1().min_w_0().flex().flex_col().gap(px(1.0))
                .child(div().flex().items_center().gap_2().child(div().truncate().text_size(px(13.0)).font_weight(FontWeight::MEDIUM).text_color(c.fg).child(p.name.clone()))
                    .when(is_winfer, |d| d.child(crate::ui::code_pickers::recommended_badge(c, true))))
                .when(is_winfer, |d| d.child(
                    div().id("winfer-code-open-settings").flex().items_center().gap_0p5().text_size(px(11.0)).text_color(c.accent).cursor_pointer().hover(|d| d.opacity(0.8))
                        .tooltip(|window, cx| Tooltip::new("Open WinferCode interface, model & context settings").build(window, cx))
                        .on_click(cx.listener(|this, _, window, cx| this.open_settings_page("winfer-code", window, cx)))
                        .child("Interface, models & context")
                        .child(Icon::new(IconName::ChevronRight).size(px(11.0))),
                ));

            let update_id = p.id.clone();
            let status: Option<AnyElement> = if busy {
                Some(div().text_color(c.accent).child("Updating…").into_any_element())
            } else if v.outdated && v.updatable {
                let latest = v.latest.clone().unwrap_or_default();
                let tip = format!("Update {} to {latest}", p.name);
                Some(div().id(SharedString::from(format!("update-{}", p.id))).h(px(20.0)).flex().items_center().gap_1().px(px(7.0)).rounded_full().bg(c.accent).text_size(px(10.5)).font_weight(FontWeight::MEDIUM).text_color(c.on_accent).cursor_pointer()
                    .hover(|d| d.opacity(0.9))
                    .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
                    .on_click(cx.listener(move |this, _, _, cx| this.update_provider(update_id.clone(), cx)))
                    .child(Icon::new(IconName::Download).size(px(10.5)))
                    .child(latest)
                    .into_any_element())
            } else if v.outdated {
                v.latest.clone().map(|l| div().text_color(c.accent).child(format!("→ {l}")).into_any_element())
            } else if v.known && v.latest.is_some() {
                Some(div().flex().items_center().gap_1().text_color(c.success).child(Icon::new(IconName::Check).size(px(10.5))).child("Up to date").into_any_element())
            } else if v.updatable {
                let tip = format!("Check and update {}", p.name);
                Some(div().id(SharedString::from(format!("update-{}", p.id))).flex().items_center().gap_1().text_color(c.muted).cursor_pointer().hover(move |d| d.text_color(c.fg))
                    .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
                    .on_click(cx.listener(move |this, _, _, cx| this.update_provider(update_id.clone(), cx)))
                    .child(Icon::new(IconName::Download).size(px(10.5)))
                    .child("Update")
                    .into_any_element())
            } else if !v.known && checking {
                Some(div().text_color(c.subtle).child("Checking…").into_any_element())
            } else {
                None
            };
            let version = col(118.0).flex_col().items_start().gap(px(3.0)).text_size(px(11.0))
                .when(!v.current.is_empty(), |d| d.child(div().font_family(mono.clone()).text_color(c.muted).child(v.current.clone())))
                .children(status);

            let model: AnyElement = if p.model_arg.is_some() {
                let list = self.models.get(&p.id).cloned().unwrap_or_default();
                let mut options = vec![(String::new(), "CLI default".to_string())];
                options.extend(list.iter().map(|m| (m.id.clone(), m.label.clone())));
                let chosen = s.agent_models.get(&p.id).cloned().unwrap_or_default();
                let label = options.iter().find(|(id, _)| *id == chosen).map_or_else(|| if chosen.is_empty() { "CLI default".to_string() } else { chosen.clone() }, |(_, l)| l.clone());
                let id = p.id.clone();
                self.select_button_flex(&format!("model-{}", p.id), label, options, 240.0, s.agent_models.contains_key(&p.id), c, cx, move |this, value, cx| {
                    let id = id.clone();
                    this.update_state(cx, |s| {
                        if value.is_empty() {
                            s.agent_models.remove(&id);
                        } else {
                            s.agent_models.insert(id, value);
                        }
                    });
                })
            } else {
                div().w_full().text_center().text_size(px(11.0)).text_color(c.subtle).child("picks its own").into_any_element()
            };

            let auto: AnyElement = if p.auto_approve_args.is_empty() {
                div().text_size(px(11.0)).text_color(c.subtle).child("—").into_any_element()
            } else {
                let (id, on) = (p.id.clone(), s.auto_approve.contains(&p.id));
                let tip = if on { "Auto-approve is on — runs tools and edits without asking. Click to turn off." } else { "Auto-approve — skip permission prompts on start. Only for sandboxes or work you can undo." };
                div().id(SharedString::from(format!("auto-{id}"))).size(px(30.0)).flex().items_center().justify_center().rounded(px(7.0)).cursor_pointer()
                    .when(on, |d| d.bg(alpha(c.warning, 0.14)).text_color(c.warning))
                    .when(!on, |d| d.text_color(c.subtle).hover(move |d| d.bg(c.hover).text_color(c.fg)))
                    .tooltip(move |window, cx| Tooltip::new(tip).build(window, cx))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let id = id.clone();
                        this.update_state(cx, |s| {
                            if s.auto_approve.contains(&id) {
                                s.auto_approve.retain(|x| *x != id);
                            } else {
                                s.auto_approve.push(id);
                            }
                        });
                    }))
                    .child(Icon::new(if on { IconName::Zap } else { IconName::Shield }).size(px(15.0)))
                    .into_any_element()
            };

            let (star_id, star_name) = (p.id.clone(), p.name.clone());
            let star = div().id(SharedString::from(format!("default-star-{star_id}"))).size(px(30.0)).flex().items_center().justify_center().rounded(px(7.0))
                .when(is_default, |d| d.tooltip(|window, cx| Tooltip::new("Default agent for new panes").build(window, cx)).child(crate::ui::code_pickers::star_icon(c.accent, 15.0)))
                .when(!is_default, |d| d.text_color(c.subtle).cursor_pointer().hover(move |d| d.bg(c.hover).text_color(c.fg))
                    .tooltip(move |window, cx| Tooltip::new(format!("Set {star_name} as default agent")).build(window, cx))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let id = star_id.clone();
                        this.update_state(cx, |s| s.default_provider = if id == "winfer" { None } else { Some(id) });
                    }))
                    .child(Icon::new(IconName::Star).size(px(15.0))));

            table = table.child(
                div().flex().items_center().gap_3().px_3p5().min_h(px(54.0)).py_2()
                    .when(i > 0, |d| d.border_t_1().border_color(alpha(c.border, 0.6)))
                    .when(is_default, |d| d.bg(alpha(c.accent, 0.06)))
                    .child(mark_tile(c, &p.id, 26.0))
                    .child(name)
                    .child(version)
                    .child(col(210.0).child(model))
                    .child(col(36.0).justify_center().child(auto))
                    .child(col(36.0).justify_center().child(star)),
            );
        }

        let note = panel(c).flex_row().items_start().gap_2p5().px_3().py_2p5().text_size(px(12.0)).text_color(c.muted)
            .child(Icon::new(IconName::ShieldQuestionMark).size(px(15.0)).flex_shrink_0().text_color(c.accent))
            .child(div().flex_1().min_w_0().child(
                "Auto-approve (⚡) skips permission prompts when the agent starts: use it only in sandboxes or on work you can undo. ★ sets the agent new panes start with; the start model is what an agent's new panes begin on.",
            ));

        // Agents to install.
        let missing: Vec<_> = s.providers.iter().filter(|p| !p.installed && p.id != "shell" && p.command.is_some()).collect();
        let mut grid = div().flex().flex_wrap().mx(px(-4.0));
        for p in &missing {
            let command = p.command.clone().unwrap_or_default();
            let install: Option<String> = p.update.as_ref().and_then(|u| u.npm.as_ref().map(|n| format!("npm install -g {n}")).or_else(|| u.url.as_ref().map(|url| format!("curl -fsSL {url} | bash"))));
            let (id, name, copy) = (p.id.clone(), p.name.clone(), install.clone());
            let tile = panel(c).flex_row().items_center().gap_2p5().px_2p5().py_2()
                .child(mark_tile(c, &p.id, 24.0))
                .child(div().flex_1().min_w_0().flex().flex_col().gap(px(1.0))
                    .child(div().truncate().text_size(px(12.5)).font_weight(FontWeight::MEDIUM).text_color(c.fg).child(p.name.clone()))
                    .child(div().truncate().font_family(mono.clone()).text_size(px(10.5)).text_color(c.subtle).child(install.unwrap_or_else(|| format!("Needs `{command}` on PATH")))))
                .when_some(copy, |d, text| d.child(
                    plain_button(c, SharedString::from(format!("copy-install-{id}")), "Copy").flex().items_center().gap_1()
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
                            this.toast(format!("Copied install command for {name}"), cx);
                        })),
                ));
            grid = grid.child(div().w(relative(0.5)).p(px(4.0)).child(tile));
        }
        let plugins_tile = div().id("agents-more-plugins").flex().items_center().justify_center().gap_2().h(px(40.0)).rounded(px(9.0)).border_1().border_dashed().border_color(c.border_strong)
            .text_size(px(12.0)).text_color(c.muted).cursor_pointer().hover(move |d| d.bg(c.hover).text_color(c.fg))
            .on_click(cx.listener(|this, _, window, cx| this.open_settings_page("plugins", window, cx)))
            .child(Icon::new(IconName::Puzzle).size(px(14.0)))
            .child("More agents come from plugins")
            .child(Icon::new(IconName::ChevronRight).size(px(12.0)));
        let check_link = link(c, "agents-install-check", "Check").on_click(cx.listener(|this, _, _, cx| this.check_versions(cx)));
        let install_section = div().relative().flex().flex_col().gap_3()
            .child(canvas(move |bounds, _, _| install_bounds.set(Some(bounds)), |_, _, _, _| {}).absolute().size_full())
            .when(!missing.is_empty(), |d| d
                .child(heading(c, "Available to install", vec![badge(c.subtle, missing.len().to_string()).into_any_element()], Some(div().flex().items_center().gap_1().child("Copy the command, run it in a terminal, then").child(check_link).into_any_element())))
                .child(grid))
            .child(plugins_tile);

        div().flex().flex_col().gap_4().child(top).child(table).child(note).child(install_section)
    }

    /// New panes: the default agent, what's used when it isn't installed, and worktrees.
    fn agents_panes(&mut self, c: Colors, cx: &mut Context<Self>) -> Div {
        let s = self.state.read(cx).clone();
        let chosen = s.default_provider.clone().unwrap_or_else(|| "winfer".into());
        let resolved = s.default_provider_id();
        let mut grid = div().flex().flex_wrap().mx(px(-4.0));
        for p in &s.providers {
            let on = p.id == chosen;
            let id = p.id.clone();
            let status = match p.id.as_str() {
                "shell" => "no agent",
                _ if !p.installed => "not installed",
                "winfer" => "recommended",
                _ => "installed",
            };
            let tile = div().id(SharedString::from(format!("default-agent-{id}"))).flex().flex_col().items_center().gap_1p5().pt_3().pb_2p5().px_1p5().rounded(px(10.0)).bg(c.panel).cursor_pointer()
                .when(on, |d| d.border_2().border_color(c.accent))
                .when(!on, |d| d.border_1().border_color(c.edge).hover(move |d| d.bg(c.hover)))
                .when(!p.installed && p.id != "shell", |d| d.opacity(0.5))
                .on_click(cx.listener(move |this, _, _, cx| {
                    let id = id.clone();
                    this.update_state(cx, |s| s.default_provider = if id == "winfer" { None } else { Some(id) });
                }))
                .child(mark_tile(c, &p.id, 30.0))
                .child(div().max_w_full().truncate().text_size(px(12.0)).font_weight(FontWeight::MEDIUM).text_color(c.fg).child(p.name.clone()))
                .child(div().text_size(px(10.5)).text_color(if on && p.id == "winfer" { c.accent } else { c.subtle }).child(status));
            grid = grid.child(div().w(relative(0.2)).p(px(4.0)).child(tile));
        }
        let mark = self.reset_mark("agents.default", c, cx);
        let head = heading(c, "Default agent", mark.into_iter().collect(), Some(div().child("⌘T, splits and new windows start this agent").into_any_element()));

        // Not installed: what new panes start instead.
        let fallback_note = (chosen != resolved).then(|| {
            super::settings_kit::banner(c.warning, IconName::TriangleAlert, format!("{} isn't installed, so new panes start {} until it is.", s.provider_name(&chosen), s.provider_name(&resolved)))
        });
        let mut chain = div().flex().items_center().flex_wrap().gap_2();
        for (i, id) in ["winfer", "claude", "shell"].into_iter().enumerate() {
            if i > 0 {
                chain = chain.child(Icon::new(IconName::ArrowRight).size(px(13.0)).text_color(c.subtle));
            }
            chain = chain.child(div().flex().items_center().gap_1p5().pl_1().pr_2().py_1().rounded_full().bg(c.app).border_1().border_color(c.edge).text_size(px(12.0)).text_color(c.muted)
                .child(mark_tile(c, id, 20.0)).child(s.provider_name(id)));
        }
        let fallback = panel(c).gap_2p5().px_3p5().py_3()
            .child(div().flex().items_center().gap_2().text_size(px(12.5)).font_weight(FontWeight::MEDIUM).text_color(c.fg)
                .child(super::settings_kit::icon_tile(c.muted, IconName::CircleQuestionMark, 26.0))
                .child("When the default agent isn't installed")
                .child(div().flex_1())
                .child(div().text_size(px(11.5)).font_weight(FontWeight::NORMAL).text_color(c.subtle).child("first one installed")))
            .child(chain);

        let workspace = s.active().map(|w| (w.name.clone(), w.use_worktrees));
        let worktree = workspace.map(|(name, on)| {
            let toggle = self.toggle("use-worktrees", on, c, cx, |this, cx| this.update_state(cx, |s| {
                if let Some(w) = s.active_mut() {
                    w.use_worktrees = !w.use_worktrees;
                }
            }));
            div().flex().flex_col().gap_4()
                .child(heading(c, "This workspace", vec![], Some(div().flex().items_center().gap_1().px_1p5().rounded_full().bg(c.hover).text_size(px(11.0))
                    .child(Icon::new(IconName::FolderOpen).size(px(11.0))).child(name).into_any_element())))
                .child(panel(c).child(option_row(c, IconName::GitBranch, "Always open new agents in their own worktree", None,
                    Some("Each new agent gets its own git worktree, so agents can't overwrite each other's files. Saved for this workspace only."), toggle, true)))
        });

        div().flex().flex_col().gap_4().child(head).child(grid).children(fallback_note).child(fallback).children(worktree)
    }

    /// System prompt: the editor, always open.
    fn agents_prompt(&mut self, c: Colors, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let s = self.state.read(cx).clone();
        let mono = crate::fonts::mono(cx);
        let prompt = self.settings.as_ref().unwrap().system_prompt.clone();
        let text = prompt.read(cx).value().to_string();
        let saved = s.system_prompt_of();
        let dirty = text.trim() != saved.trim();
        let state = match s.system_prompt.as_deref() {
            None => "default",
            Some(t) if t.trim().is_empty() => "none",
            Some(_) => "custom",
        };
        let _ = window;
        // Default puts the built-in prompt back, None saves an empty one; Custom is any other text.
        let mut pick = div().flex_shrink_0().flex().gap(px(2.0)).p(px(2.0)).rounded(px(8.0)).bg(c.app).border_1().border_color(c.edge);
        for (value, label, tip) in [("default", "Default", "Use winfer's built-in prompt"), ("custom", "Custom", "Edit the text below and save"), ("none", "None", "Start agents without a system prompt")] {
            let on = value == state;
            pick = pick.child(
                div().id(SharedString::from(format!("prompt-mode-{value}"))).h(px(24.0)).flex().items_center().px_2p5().rounded(px(6.0)).text_size(px(12.0)).cursor_pointer()
                    .when(on, |d| d.bg(c.active).text_color(c.fg))
                    .when(!on, |d| d.text_color(c.muted).hover(move |d| d.text_color(c.fg)))
                    .tooltip(move |window, cx| Tooltip::new(tip).build(window, cx))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        let (stored, text) = match value {
                            "default" => (None, crate::model::DEFAULT_SYSTEM_PROMPT.to_string()),
                            "none" => (Some(String::new()), String::new()),
                            _ => return,
                        };
                        this.update_state(cx, |s| s.system_prompt = stored);
                        if let Some(ui) = this.settings.as_ref() {
                            ui.system_prompt.update(cx, |e, cx| e.set_value(text, window, cx));
                        }
                    }))
                    .child(label),
            );
        }
        let lines = if text.trim().is_empty() { 0 } else { text.lines().count() };
        let (state_badge, state_color) = match state {
            "default" => ("Default", c.subtle),
            "none" => ("None", c.warning),
            _ => ("Custom", c.accent),
        };
        let save = cx.listener(|this, _: &ClickEvent, _, cx| {
            let Some(ui) = this.settings.as_ref() else { return };
            let text = ui.system_prompt.read(cx).value().trim().to_string();
            this.update_state(cx, |s| s.system_prompt = if text == crate::model::DEFAULT_SYSTEM_PROMPT { None } else { Some(text.clone()) });
            this.toast("System prompt saved", cx);
        });
        let discard = cx.listener(|this, _: &ClickEvent, window, cx| {
            let text = this.state.read(cx).system_prompt_of();
            if let Some(ui) = this.settings.as_ref() {
                ui.system_prompt.update(cx, |e, cx| e.set_value(text, window, cx));
            }
        });
        let editor = panel(c).overflow_hidden()
            .child(div().flex().items_center().gap_2().px_3().py_2().border_b_1().border_color(c.border).text_size(px(12.0)).text_color(c.muted)
                .child(Icon::new(IconName::FileText).size(px(14.0)))
                .child("System prompt")
                .child(badge(state_color, state_badge))
                .children(self.reset_mark("agents.prompt", c, cx))
                .child(div().flex_1())
                .child(pick))
            .child(div().p_2p5().bg(c.app).font_family(mono.clone()).child(Textarea::new(&prompt).h(px(280.0)).appearance(false).text_size(px(12.5))))
            .child(div().flex().items_center().gap_2().px_3().py_2().border_t_1().border_color(c.border).font_family(mono).text_size(px(11.0)).text_color(c.subtle)
                .child(format!("{lines} line{} · {} characters", if lines == 1 { "" } else { "s" }, text.trim().chars().count()))
                .child(div().flex_1())
                .when(dirty, |d| d
                    .child(plain_button(c, "prompt-discard", "Discard").on_click(discard))
                    .child(super::orchestration::accent_button(c, "prompt-save", "Save", true).on_click(save)))
                .when(!dirty, |d| d.child(div().flex().items_center().gap_1().child(Icon::new(IconName::Check).size(px(11.0)).text_color(c.success)).child("Saved"))));
        div().flex().flex_col().gap_4()
            .child(super::settings_kit::intro(c, "Given to every agent when it starts or restarts. Changes apply to agents started after saving.", None))
            .child(editor)
    }

    /// Tools: the optional tools, by group.
    fn agents_tools(&mut self, c: Colors, cx: &mut Context<Self>) -> Div {
        let s = self.state.read(cx).clone();
        let tools = self.optional_tools(cx);
        let on_count = tools.iter().filter(|n| s.enabled_tools.iter().any(|t| t == *n)).count();
        let total = tools.len();
        let summary = panel(c).flex_row().items_center().gap_3().px_3p5().py_3()
            .child(super::settings_kit::icon_tile(c.accent, IconName::Gauge, 30.0))
            .child(div().flex_1().min_w_0().flex().flex_col().gap(px(2.0))
                .child(div().flex().items_center().gap_2().text_size(px(12.5)).font_weight(FontWeight::MEDIUM).text_color(c.fg)
                    .child(format!("{on_count} of {total} optional tools on"))
                    .children(self.reset_mark("agents.tools", c, cx)))
                .child(div().text_size(px(11.5)).text_color(c.subtle).child("Rarely used tools stay off, so they don't spend context on every request. Turn one on when your agents need it.")));
        let mut out = div().flex().flex_col().gap_4().child(summary);
        for group in ["Browser", "Knowledge"] {
            let in_group: Vec<_> = tools.iter().copied().filter(|n| optional_tool_meta(n).0 == group).collect();
            if in_group.is_empty() {
                continue;
            }
            let note = (group == "Knowledge").then(|| div().child("from the knowledge-base plugin").into_any_element());
            out = out.child(heading(c, group, vec![], note));
            let mut grid = div().flex().flex_wrap().mx(px(-4.0));
            for name in in_group {
                let on = s.enabled_tools.iter().any(|t| t == name);
                let (_, label, hint, icon) = optional_tool_meta(name);
                let tile = div().id(SharedString::from(format!("tool-{name}"))).flex().items_center().gap_2p5().px_3().py_2p5().rounded(px(9.0)).bg(c.panel).cursor_pointer()
                    .border_1().border_color(if on { alpha(c.accent, 0.6) } else { c.edge })
                    .when(!on, |d| d.hover(move |d| d.bg(c.hover)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.update_state(cx, |s| {
                            if s.enabled_tools.iter().any(|t| t == name) {
                                s.enabled_tools.retain(|t| t != name);
                            } else {
                                s.enabled_tools.push(name.to_string());
                            }
                        });
                    }))
                    .child(super::settings_kit::icon_tile(if on { c.accent } else { c.muted }, icon, 28.0))
                    .child(super::settings_kit::small_label(c, label, hint))
                    .child(mini_switch(c, on));
                grid = grid.child(div().w(relative(0.5)).p(px(4.0)).child(tile));
            }
            out = out.child(grid);
        }
        out
    }

    /// Review & progress: checks when an agent finishes, and progress estimates.
    fn agents_review(&mut self, c: Colors, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let _ = window;
        let s = self.state.read(cx).clone();
        let auto_tests = self.rest_bool("review", "autoTests", false, cx);
        let shots = self.rest_bool("review", "screenshots", true, cx);
        let progress_on = self.rest_bool("progress", "enabled", false, cx);
        let interval = s.rest.get("progress").and_then(|p| p["intervalSec"].as_u64()).unwrap_or(60);

        let card_head = |icon: IconName, title: &str, sub: AnyElement, right: Option<AnyElement>| {
            div().flex().items_center().gap_2p5().px_3p5().pt_3().pb_2p5()
                .child(super::settings_kit::icon_tile(c.accent, icon, 28.0))
                .child(div().flex_1().min_w_0().flex().flex_col().gap(px(1.0)).child(div().text_size(px(13.0)).font_weight(FontWeight::MEDIUM).text_color(c.fg).child(title.to_string())).child(sub))
                .children(right)
        };

        // When an agent finishes: two checks and what the review card then shows.
        let tests_toggle = self.toggle("review-tests", auto_tests, c, cx, move |this, cx| this.set_rest("review", "autoTests", Value::Bool(!auto_tests), cx));
        let shots_toggle = self.toggle("review-shots", shots, c, cx, move |this, cx| this.set_rest("review", "screenshots", Value::Bool(!shots), cx));
        let options = panel(c).flex_1().min_w_0().bg(c.raised)
            .child(option_row(c, IconName::FlaskConical, "Run tests when an agent finishes", self.reset_mark("review.tests", c, cx), Some("The project's tests, in the agent's folder."), tests_toggle, true))
            .child(option_row(c, IconName::Camera, "Screenshot the dev server", self.reset_mark("review.screenshots", c, cx), Some("Shows the agent's running app on its review card."), shots_toggle, false));
        let line = |icon: IconName, text: &'static str, color: Hsla| div().flex().items_center().gap_1p5().text_size(px(11.5)).text_color(c.muted).child(Icon::new(icon).size(px(12.0)).text_color(color)).child(text);
        let preview = div().w(px(230.0)).flex_shrink_0().flex().flex_col().gap_2().p_2p5().rounded(px(9.0)).bg(c.app)
            .child(div().text_size(px(10.5)).font_weight(FontWeight::SEMIBOLD).text_color(c.subtle).child("THE REVIEW CARD SHOWS"))
            .child(line(IconName::FileText, "The diff and a summary", c.subtle))
            .when(auto_tests, |d| d.child(line(IconName::CircleCheck, "Test results", c.success)))
            .when(shots, |d| d.child(line(IconName::Image, "A screenshot of the app", c.accent)));
        let finishes = panel(c)
            .child(card_head(IconName::CircleCheck, "When an agent finishes", div().text_size(px(11.5)).text_color(c.subtle).child("Checks that run on their own and land on the agent's review card.").into_any_element(), None))
            .child(div().flex().gap_3().px_3p5().pb_3p5().child(options).child(preview));

        // Progress estimates.
        let on_toggle = self.toggle("progress-on", progress_on, c, cx, move |this, cx| this.set_rest("progress", "enabled", Value::Bool(!progress_on), cx));
        let ai_link = link(c, "progress-ai-link", "AI model").on_click(cx.listener(|this, _, window, cx| this.open_settings_page("ai", window, cx)));
        let sub = div().flex().items_center().gap_1().text_size(px(11.5)).text_color(c.subtle).child("Asks the").child(ai_link).child("how far each working agent is.").into_any_element();
        let every = self.seg_control("progress-every", vec![(30u64, "30 s"), (60, "1 min"), (120, "2 min"), (300, "5 min")], interval, c, cx, |this, v, cx| this.set_rest("progress", "intervalSec", json!(v), cx));
        let progress = panel(c)
            .child(card_head(IconName::LoaderCircle, "Estimate how far working agents are", sub, Some(div().flex().items_center().gap_2().children(self.reset_mark("progress.enabled", c, cx)).child(on_toggle).into_any_element())))
            .child(div().px_3p5().pb_3p5().when(!progress_on, |d| d.opacity(0.45)).child(
                panel(c).bg(c.raised).px_3p5().py_3().gap_2()
                    .child(div().flex().items_center().gap_2().text_size(px(12.5)).text_color(c.fg).child("Check every").children(self.reset_mark("progress.interval", c, cx)).child(div().flex_1()).child(every))
                    .child(div().text_size(px(11.5)).text_color(c.subtle).child("Agents whose output hasn't changed since the last check are skipped, so idle panes cost nothing.")),
            ));

        div().flex().flex_col().gap_4().child(finishes).child(progress)
    }
}

#[cfg(test)]
mod tests {
    use super::tab_for_label;

    #[test]
    fn search_hits_open_the_right_agents_tab() {
        assert_eq!(tab_for_label("Default agent"), "panes");
        assert_eq!(tab_for_label("Instructions for every agent"), "prompt");
        assert_eq!(tab_for_label("Agent tools"), "tools");
        assert_eq!(tab_for_label("Check every"), "review");
        assert_eq!(tab_for_label("Auto-approve"), "agents");
    }
}
