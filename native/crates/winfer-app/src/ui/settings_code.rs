//! Settings → WinferCode in six tabs: Interface, Models, Permissions, Efficiency, Local models and
//! MCP servers (the "Visual + tabs" design in mockups/winfer-code-settings/d-visual-tabs.html).
//! The settings themselves load and save in `settings_harness.rs`.

use super::settings::{plain_button, row};
use super::settings_kit::{alpha, badge, checkbox, intro, kbd, mini_switch, panel, radio, section, small_label};
use super::settings_harness::{BACKENDS, WINFER_PROVIDER, digits, effort_label, hint, parse_efforts, parse_servers};
use super::*;
use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea};
use gpui_kit::component::slider::{Slider, SliderEvent, SliderState, SliderValue};
use gpui_kit::component::tooltip::Tooltip;
use serde_json::{Value, json};
use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;
use winfer_backend::harness;

#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub(super) enum CodeTab {
    #[default]
    Interface,
    Models,
    Permissions,
    Efficiency,
    Local,
    Mcp,
}

const TABS: [(CodeTab, &str, IconName); 6] = [
    (CodeTab::Interface, "Interface", IconName::LayoutPanelLeft),
    (CodeTab::Models, "Models", IconName::Cpu),
    (CodeTab::Permissions, "Permissions", IconName::ShieldCheck),
    (CodeTab::Efficiency, "Efficiency", IconName::Feather),
    (CodeTab::Local, "Local models", IconName::Server),
    (CodeTab::Mcp, "MCP servers", IconName::Plug),
];

/// The tab a setting found by the settings search is on.
pub(super) fn tab_for_label(label: &str) -> CodeTab {
    match label {
        "Model" | "Reasoning effort" | "Main model" | "Subagents" => CodeTab::Models,
        l if l.starts_with("Subagent model") => CodeTab::Models,
        "Permission mode" | "Auto-approve in panes" | "Always-allowed commands" | "Resume after usage limits" => CodeTab::Permissions,
        "Lean setup" | "Claude Code" | "Codex" | "Lean Claude Code" | "Lean Codex" | "Lean Codex CLI panes" | "Short Codex prompt" | "Codex CLI panes too" | "Skills" | "Skills in lean sessions" | "Micro-compact at" | "Full compact at" | "Context compaction" => CodeTab::Efficiency,
        l if l.starts_with("Selected skills") => CodeTab::Efficiency,
        "Local models" | "Use local models" | "Host / Port" | "API path" | "API key" | "Context window" | "Thinking levels" | "Send level to chat template" | "Web search" | "Connection" => CodeTab::Local,
        l if l.starts_with("Thinking budget") => CodeTab::Local,
        "MCP servers" | "Tools" => CodeTab::Mcp,
        _ => CodeTab::Interface,
    }
}

/// A tab by its name in lower case without spaces ("local", "mcp", …), for the dev layer.
#[cfg(debug_assertions)]
pub(super) fn tab_named(name: &str) -> CodeTab {
    TABS.iter().find(|(_, label, _)| label.to_lowercase().replace(' ', "").starts_with(&name.to_lowercase())).map_or(CodeTab::Interface, |t| t.0)
}

/// Which skills the picker lists.
#[derive(Clone, PartialEq, Default)]
enum SkillFilter {
    #[default]
    All,
    Picked,
    Source(String),
}

/// The page's own state: its tab and what its controls are doing.
#[derive(Default)]
pub(super) struct CodeUi {
    pub(super) tab: CodeTab,
    text_size: Option<Entity<SliderState>>,
    skill_query: Option<Entity<InputState>>,
    skills_open: bool,
    skill_filter: SkillFilter,
    skill_hl: usize,
    skill_scroll: ScrollHandle,
    local_scroll: ScrollHandle,
    local_query: Option<Entity<InputState>>,
    logos: super::model_logos::ModelLogos,
    /// Where the context bar was last laid out, to turn a pointer into a percentage.
    ctx_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    /// The compaction marker being dragged and where it is (saved on release).
    drag: Option<(&'static str, u64)>,
    mcp_json: bool,
    new_server: Option<(Entity<InputState>, Entity<InputState>)>,
    new_level: Option<Entity<InputState>>,
}

/// The web interface's display options: key, label, line under it, default, icon and group.
const WEB_OPTS: [(&str, &str, &str, bool, IconName, &str); 10] = [
    ("showContext", "Context meter", "Click it for the breakdown", true, IconName::Gauge, "Status line"),
    ("showCost", "Session cost", "When the backend reports one", true, IconName::Coins, "Status line"),
    ("showRate", "Tokens per second", "Next to the working indicator", true, IconName::Zap, "Status line"),
    ("showPlan", "Plan above the composer", "Todos and running subagents", true, IconName::ListChecks, "Transcript"),
    ("expandThinking", "Show thinking", "Expanded, not collapsed", false, IconName::Brain, "Transcript"),
    ("expandShell", "Live shell output", "Open while a command runs", false, IconName::SquareTerminal, "Transcript"),
    ("compactTools", "Compact tool calls", "One live row, click to expand", true, IconName::Rows3, "Transcript"),
    ("showBgNotes", "Background task results", "One row per finished task; off hides them", true, IconName::Info, "Transcript"),
    ("compact", "Compact layout", "Tighter spacing", false, IconName::FoldVertical, "Transcript"),
    ("sendAnimation", "Animated send button", "Its nebula drifts while working", true, IconName::Sparkles, "Transcript"),
];

#[derive(Clone, Copy, PartialEq)]
enum Cap {
    Allow,
    Asks,
    Block,
}

/// Permission modes from safest to freest: id, label, line under it, what it does, and whether
/// reading files, editing files and running commands are allowed, ask first or are blocked.
const MODES: [(&str, &str, &str, &str, [Cap; 3]); 5] = [
    ("plan", "Plan", "read-only", "Plans without changing anything; read-only commands run, others ask.", [Cap::Allow, Cap::Block, Cap::Asks]),
    ("yolo-plan", "Yolo plan", "read-only, no asking", "Plans without changing anything; its tools run without approval.", [Cap::Allow, Cap::Block, Cap::Allow]),
    ("ask", "Ask", "asks first", "Asks before edits, commands and network access.", [Cap::Allow, Cap::Asks, Cap::Asks]),
    ("auto-edit", "Auto-edit", "commands ask", "File edits go through; commands still ask.", [Cap::Allow, Cap::Allow, Cap::Asks]),
    ("yolo", "Yolo", "nothing asks", "Nothing asks before editing files or running commands. Use it where mistakes are cheap.", [Cap::Allow, Cap::Allow, Cap::Allow]),
];

const EFFORTS: [(&str, &str); 7] = [("", "Default"), ("minimal", "Min"), ("low", "Low"), ("medium", "Med"), ("high", "High"), ("xhigh", "XHigh"), ("max", "Max")];

/// The chips the skill field shows while the list is closed.
const MAX_SKILL_CHIPS: usize = 6;

/// How risky a permission mode is, as a color.
fn mode_color(c: Colors, mode: &str, auto_approve: bool) -> Hsla {
    match mode {
        _ if auto_approve => c.danger,
        "auto-edit" => c.warning,
        "yolo" => c.danger,
        _ => c.success,
    }
}

/// The backend a model spec runs on (`chatgpt`, `claude`, `codex`, `local`, …).
fn backend_of(spec: &str) -> String {
    match spec.split_once('/') {
        Some((b, _)) if BACKENDS.iter().any(|(id, _)| *id == b) => b.to_string(),
        _ => harness::backend::parse_spec(spec).0.id().to_string(),
    }
}

/// The context window from a model's detail line ("Local · http://… · 100k context · winfer tools").
fn context_of(detail: &str) -> Option<String> {
    detail.split(" · ").find(|p| p.contains("context")).map(|p| p.trim().to_string())
}

fn backend_label(id: &str) -> &str {
    BACKENDS.iter().find(|(b, _)| *b == id).map_or(id, |(_, l)| *l)
}

/// A rough count of the tokens a skill's name and description add to every request.
pub(super) fn skill_tokens(name: &str, description: &str) -> u64 {
    ((name.len() + description.len()) as f64 / 4.0).ceil() as u64 + 12
}

#[derive(Clone)]
struct SkillItem {
    id: String,
    name: String,
    description: String,
    source: String,
    tokens: u64,
}

fn skill_items(list: &[Value]) -> Vec<SkillItem> {
    let mut items: Vec<SkillItem> = list
        .iter()
        .map(|s| {
            let (name, description) = (s["name"].as_str().unwrap_or_default().to_string(), s["description"].as_str().unwrap_or_default().to_string());
            SkillItem { id: s["id"].as_str().unwrap_or_default().to_string(), tokens: skill_tokens(&name, &description), source: s["source"].as_str().unwrap_or("user").to_string(), name, description }
        })
        .collect();
    items.sort_by_key(|s| (source_rank(&s.source), s.source.clone()));
    items
}

/// Your own skills first, then plugins, then other agents' folders.
fn source_rank(source: &str) -> u8 {
    match source {
        "user" => 0,
        "codex" | "agents" => 2,
        _ => 1,
    }
}

/// A source's group heading and where its skills live.
fn source_label(source: &str) -> (String, &'static str) {
    match source {
        "user" => ("Your skills".into(), "~/.claude/skills"),
        "codex" => ("Codex".into(), "~/.codex/skills"),
        "agents" => ("Agents".into(), "~/.agents/skills"),
        other => (other.to_string(), "plugin"),
    }
}

fn skill_matches(s: &SkillItem, query: &str) -> bool {
    query.is_empty() || [&s.name, &s.description, &s.source].iter().any(|t| t.to_lowercase().contains(query))
}

/// The skills the picker lists for a filter and search, in list order.
fn visible_skills(items: &[SkillItem], picked: &[String], filter: &SkillFilter, query: &str) -> Vec<SkillItem> {
    let q = query.trim().to_lowercase();
    items
        .iter()
        .filter(|s| match filter {
            SkillFilter::All => true,
            SkillFilter::Picked => picked.contains(&s.id),
            SkillFilter::Source(src) => &s.source == src,
        })
        .filter(|s| skill_matches(s, &q))
        .cloned()
        .collect()
}

/// The web interface as it will look: updates with every display option and the text size.
struct Preview {
    scale: f32,
    context: bool,
    cost: bool,
    rate: bool,
    plan: bool,
    thinking: bool,
    shell: bool,
    compact_tools: bool,
    compact: bool,
    model: String,
    mode: String,
}

fn web_preview(c: Colors, mono: SharedString, p: Preview) -> Div {
    let fs = 11.0 * p.scale;
    let gap = if p.compact { fs * 0.35 } else { fs * 0.7 };
    let tool = |text: &str, diff: bool, running: bool, more: bool| {
        div().flex().items_center().gap(px(fs * 0.5)).px(px(fs * 0.6)).py(px(fs * 0.32)).rounded(px(fs * 0.5)).bg(c.raised).border_1().border_color(c.edge).overflow_hidden().whitespace_nowrap()
            .font_family(mono.clone()).text_size(px(fs * 0.88)).text_color(c.muted)
            .child(div().size(px(fs * 0.5)).flex_shrink_0().rounded_full().bg(if running { c.warning } else { c.success }))
            .child(text.to_string())
            .when(diff, |d| d.child(div().text_color(c.success).child("+4")).child(div().text_color(c.danger).child("−2")))
            .when(more, |d| d.child(div().flex_1()).child(div().text_color(c.subtle).child("›")))
    };
    let mut items: Vec<Div> = vec![div().flex().justify_end().child(div().max_w(relative(0.8)).px(px(fs * 0.7)).py(px(fs * 0.4)).rounded(px(fs * 0.8)).bg(alpha(c.accent, 0.18)).text_color(c.fg).child("fix the flaky upload test"))];
    items.push(if p.thinking {
        div().pl(px(fs * 0.7)).py(px(fs * 0.3)).border_l_2().border_color(c.border_strong).italic().text_color(c.subtle).child("The retry resolves before the mock server answers, so the second attempt races the first…")
    } else {
        div().italic().text_color(c.subtle).child("Thought for 4s ›")
    });
    if p.compact_tools {
        items.push(tool("3 tool calls · Edit src/upload.ts", true, false, true));
    } else {
        items.push(tool("Read src/upload.ts", false, false, false));
        items.push(tool("Grep \"retry\" src/", false, false, false));
        items.push(tool("Edit src/upload.ts", true, false, false));
    }
    items.push(if p.shell {
        div().flex().flex_col().px(px(fs * 0.7)).py(px(fs * 0.45)).rounded(px(fs * 0.5)).bg(if c.light { hsla(0.0, 0.0, 0.0, 0.04) } else { hsla(0.0, 0.0, 0.0, 0.35) }).font_family(mono.clone()).text_size(px(fs * 0.86)).text_color(c.muted)
            .child("$ pnpm test upload").child(div().text_color(c.success).child("✓ retries after timeout (212 ms)")).child(div().text_color(c.success).child("✓ 12 passed"))
    } else {
        tool("$ pnpm test upload", false, true, true)
    });
    if p.plan {
        items.push(div().flex().flex_col().gap(px(fs * 0.15)).px(px(fs * 0.7)).py(px(fs * 0.4)).rounded(px(fs * 0.5)).bg(c.raised).border_1().border_color(c.edge).text_size(px(fs * 0.92)).text_color(c.subtle)
            .child(div().line_through().child("☑ Find the race")).child(div().line_through().child("☑ Await the mock server")).child(div().text_color(c.fg).child("◐ Run the upload tests")));
    }
    items.push(div().flex().gap_1().text_size(px(fs * 0.9)).text_color(c.accent).child("✻ Working…").child(div().text_color(c.subtle).child(if p.rate { "14s · 86 tok/s" } else { "14s" })));
    let status = div().flex().items_center().gap(px(fs)).px(px(fs * 0.9)).pt(px(fs * 0.4)).pb(px(fs * 0.55)).font_family(mono.clone()).text_size(px(fs * 0.85)).text_color(c.subtle).whitespace_nowrap()
        .child(format!("{} · {}", p.model, p.mode))
        .child(div().flex_1())
        .when(p.context, |d| d.child(div().flex().items_center().gap(px(fs * 0.4))
            .child(div().w(px(fs * 3.5)).h(px(fs * 0.45)).rounded_full().bg(c.active).child(div().h_full().w(relative(0.42)).rounded_full().bg(c.accent)))
            .child("42%")))
        .when(p.cost, |d| d.child("$0.18"));
    // The screen's content is positioned absolutely, so it never makes the preview taller than the
    // option list beside it: what doesn't fit scrolls off the top, like a real transcript.
    div().relative().flex_1().min_h_0().rounded(px(7.0)).bg(c.app).overflow_hidden().child(
        div().absolute().top_0().left_0().size_full().flex().flex_col()
            .child(div().flex_1().min_h_0().overflow_hidden().flex().flex_col().justify_end().gap(px(gap)).px(px(fs * 0.9)).pt(px(fs * 0.9)).pb(px(fs * 0.4)).text_size(px(fs)).children(items))
            .child(div().mx(px(fs * 0.9)).mb(px(fs * 0.5)).px(px(fs * 0.7)).py(px(fs * 0.55)).rounded(px(fs * 0.6)).bg(c.raised).border_1().border_color(c.edge).text_size(px(fs)).text_color(c.subtle).child("Ask WinferCode…"))
            .child(status),
    )
}

impl Root {
    /// The tab bar under the page header.
    pub(super) fn code_tabs(&mut self, c: Colors, cx: &mut Context<Self>) -> Div {
        let tab = self.harness_page.ui.tab;
        let st = self.harness_page.settings.clone().unwrap_or(Value::Null);
        let approve_on = self.state.read(cx).auto_approve.iter().any(|p| p == WINFER_PROVIDER);
        let mode_dot = mode_color(c, st["mode"].as_str().unwrap_or("ask"), approve_on);
        let local_on = st["local"]["enabled"] == true;
        let servers = st["mcpServers"].as_object().map_or(0, |m| m.len());
        let mono = crate::fonts::mono(cx);
        let mut bar = div().flex_shrink_0().flex().items_end().gap_0p5().pl(px(26.0)).pr_8().border_b_1().border_color(c.border);
        for (t, label, icon) in TABS {
            let on = t == tab;
            let extra: Option<AnyElement> = match t {
                CodeTab::Permissions => Some(div().size(px(6.0)).rounded_full().bg(mode_dot).into_any_element()),
                CodeTab::Local if local_on => Some(div().size(px(6.0)).rounded_full().bg(c.success).into_any_element()),
                CodeTab::Mcp if servers > 0 => Some(div().px_1p5().rounded_full().bg(c.hover).font_family(mono.clone()).text_size(px(10.0)).text_color(c.subtle).child(servers.to_string()).into_any_element()),
                _ => None,
            };
            bar = bar.child(
                div().id(SharedString::from(format!("code-tab-{label}"))).relative().h(px(38.0)).flex().items_center().gap(px(7.0)).px(px(9.0)).text_size(px(12.5)).cursor_pointer()
                    .text_color(if on { c.fg } else { c.muted })
                    .hover(move |d| d.text_color(c.fg))
                    .on_click(cx.listener(move |this, _, _, cx| this.open_code_tab(t, cx)))
                    .child(Icon::new(icon).size(px(14.0)).when(on, |i| i.text_color(c.accent)))
                    .child(label)
                    .children(extra)
                    .when(on, |d| d.child(div().absolute().left(px(7.0)).right(px(7.0)).bottom(px(-1.0)).h(px(2.0)).rounded_full().bg(c.accent))),
            );
        }
        bar
    }

    pub(super) fn open_code_tab(&mut self, tab: CodeTab, cx: &mut Context<Self>) {
        self.harness_page.ui.tab = tab;
        self.harness_page.ui.skills_open = false;
        self.scroll_settings_top();
        cx.notify();
    }

    pub(super) fn page_harness(&mut self, c: Colors, window: &mut Window, cx: &mut Context<Self>) -> Div {
        self.load_harness(window, cx);
        let body = match self.harness_page.ui.tab {
            CodeTab::Interface => self.code_interface(c, window, cx),
            CodeTab::Models => self.code_models(c, cx),
            CodeTab::Permissions => self.code_permissions(c, cx),
            CodeTab::Efficiency => self.code_efficiency(c, window, cx),
            CodeTab::Local => self.code_local(c, window, cx),
            CodeTab::Mcp => self.code_mcp(c, window, cx),
        };
        let error = self.harness_page.result.as_ref().and_then(|r| r.as_ref().err()).map(|e| div().px_1().text_xs().text_color(c.danger).child(e.clone()));
        body.pt_4().children(error)
    }

    /* Interface */

    fn code_interface(&mut self, c: Colors, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let web = self.code_web("web", true, cx);
        let mono = crate::fonts::mono(cx);
        let style_card = |id: &'static str, title: &'static str, sub: &'static str, preview: Div, on: bool, cx: &mut Context<Self>| {
            div().id(id).flex_1().min_w_0().flex().flex_col().gap(px(9.0)).rounded(px(10.0)).bg(c.panel).cursor_pointer()
                .when(on, |d| d.border_2().border_color(c.accent).p(px(9.0)))
                .when(!on, |d| d.border_1().border_color(c.edge).p(px(10.0)).hover(move |d| d.border_color(c.border_strong)))
                .on_click(cx.listener(move |this, _, _, cx| this.set_rest("codeWeb", "web", json!(id == "style-web"), cx)))
                .child(preview)
                .child(div().flex().items_center().gap_2()
                    .child(div().flex_shrink_0().text_size(px(13.0)).font_weight(FontWeight::MEDIUM).text_color(c.fg).child(title))
                    .child(div().flex_1().min_w_0().truncate().text_size(px(12.0)).text_color(c.subtle).child(sub))
                    .child(radio(c, on)))
        };
        let bar = |w: f32, h: f32, color: Hsla| div().h(px(h)).w(relative(w)).rounded_full().bg(color);
        let web_mini = div().h(px(74.0)).rounded(px(7.0)).bg(c.app).px(px(9.0)).py(px(8.0)).flex().flex_col().gap(px(5.0)).overflow_hidden()
            .child(div().flex().justify_end().child(bar(0.5, 7.0, alpha(c.accent, 0.25))))
            .child(div().h(px(14.0)).flex().items_center().gap_1().px(px(5.0)).rounded(px(4.0)).bg(c.raised).border_1().border_color(c.edge)
                .child(div().size(px(5.0)).rounded_full().bg(c.success)).child(div().flex_1().h(px(3.0)).rounded_full().bg(c.active)))
            .child(bar(0.7, 4.0, alpha(c.danger, 0.4)))
            .child(bar(0.85, 4.0, alpha(c.success, 0.4)))
            .child(bar(0.75, 7.0, c.active));
        let line = |parts: Vec<(String, Hsla)>| div().flex().gap_1().whitespace_nowrap().children(parts.into_iter().map(|(t, col)| div().text_color(col).child(t)));
        let tui_mini = div().h(px(74.0)).rounded(px(7.0)).bg(c.app).px(px(9.0)).py(px(8.0)).flex().flex_col().gap(px(3.0)).overflow_hidden().font_family(mono.clone()).text_size(px(8.5)).text_color(c.subtle)
            .child(line(vec![("❯".into(), c.accent), ("fix the flaky upload test".into(), c.subtle)]))
            .child(line(vec![("●".into(), c.success), ("Edit src/upload.ts".into(), c.subtle), ("+4".into(), c.success), ("−2".into(), c.danger)]))
            .child(line(vec![("●".into(), c.success), ("Bash pnpm test upload".into(), c.subtle)]))
            .child(line(vec![("  ⎿ 12 passed".into(), c.subtle)]));
        let styles = div().flex().gap_3()
            .child(style_card("style-web", "Native", "Timeline, inline diffs, approvals", web_mini, web, cx))
            .child(style_card("style-tui", "Terminal (TUI)", "The classic terminal UI", tui_mini, !web, cx));

        // The display options, and a preview beside them that follows every change.
        let mut list = panel(c).flex_1().min_w_0().pb_1();
        let mut group = "";
        let mut on_of: HashMap<&str, bool> = HashMap::new();
        for (key, label, sub, default, icon, g) in WEB_OPTS {
            if g != group {
                group = g;
                list = list.child(div().px(px(14.0)).pt(px(8.0)).pb(px(4.0)).text_size(px(10.5)).font_weight(FontWeight::SEMIBOLD).text_color(c.subtle).child(g.to_uppercase()));
            }
            let on = self.code_web(key, default, cx);
            on_of.insert(key, on);
            list = list.child(
                div().id(SharedString::from(format!("web-opt-{key}"))).flex().items_center().gap_2p5().px(px(14.0)).py(px(7.0)).cursor_pointer()
                    .hover(move |d| d.bg(c.hover))
                    .on_click(cx.listener(move |this, _, _, cx| this.set_rest("codeWeb", key, json!(!on), cx)))
                    .child(Icon::new(icon).size(px(14.0)).text_color(if on { c.accent } else { c.subtle }))
                    .child(div().flex_1().min_w_0().flex().flex_col()
                        .child(div().text_size(px(12.5)).text_color(c.fg).child(label))
                        .child(div().text_size(px(11.0)).text_color(c.subtle).child(sub)))
                    .child(mini_switch(c, on)),
            );
        }
        let scale = self.state.read(cx).rest.get("codeWeb").and_then(|v| v["textScale"].as_f64()).unwrap_or(1.0);
        let st = self.harness_page.settings.clone().unwrap_or(Value::Null);
        let model = st["model"].as_str().filter(|m| !m.is_empty()).map_or("automatic".to_string(), |m| m.rsplit('/').next().unwrap_or(m).to_string());
        let opt = |k: &str| on_of.get(k).copied().unwrap_or(false);
        let preview = Preview {
            scale: scale as f32,
            context: opt("showContext"),
            cost: opt("showCost"),
            rate: opt("showRate"),
            plan: opt("showPlan"),
            thinking: opt("expandThinking"),
            shell: opt("expandShell"),
            compact_tools: opt("compactTools"),
            compact: opt("compact"),
            model,
            mode: st["mode"].as_str().unwrap_or("ask").to_string(),
        };
        let live = panel(c).w(px(336.0)).flex_shrink_0().p_2()
            .child(div().flex().items_center().px(px(2.0)).pb(px(7.0)).text_size(px(10.5)).font_weight(FontWeight::SEMIBOLD).text_color(c.subtle)
                .child("PREVIEW").child(div().flex_1()).child(div().font_family(mono.clone()).font_weight(FontWeight::NORMAL).child(format!("{}%", (scale * 100.0).round()))))
            .child(web_preview(c, mono.clone(), preview));
        let options = div().flex().gap(px(14.0)).when(!web, |d| d.opacity(0.45)).child(list).child(live);

        let slider = self.text_size_slider(scale, window, cx);
        let text_size = panel(c).flex_row().items_center().gap_3().px(px(14.0)).py(px(9.0)).when(!web, |d| d.opacity(0.45))
            .child(div().flex_shrink_0().text_size(px(12.5)).font_weight(FontWeight::MEDIUM).text_color(c.fg).child("Text size"))
            .child(div().text_size(px(10.0)).text_color(c.subtle).child("A"))
            .child(div().flex_1().child(Slider::new(&slider)))
            .child(div().text_size(px(15.0)).text_color(c.subtle).child("A"))
            .child(div().w(px(40.0)).flex().justify_end().font_family(mono.clone()).text_size(px(12.0)).text_color(c.muted).child(format!("{}%", (scale * 100.0).round())))
            .child(plain_button(c, "text-reset", "Reset").when(scale == 1.0, |d| d.opacity(0.4)).on_click(cx.listener(|this, _, _, cx| this.set_rest("codeWeb", "textScale", json!(1.0), cx))))
            .child(div().flex().gap_1().child(kbd(c, &mono, if cfg!(target_os = "macos") { "⌘ +" } else { "Ctrl +" })).child(kbd(c, &mono, if cfg!(target_os = "macos") { "⌘ −" } else { "Ctrl −" })));
        let sidebar = self.harness_toggle("planSidebar", st["planSidebar"] == true, c, cx);
        div().flex().flex_col().gap_4()
            .child(intro(c, "How new WinferCode panes look. Open panes keep the style they started with.", None))
            .child(styles)
            .child(section(c, "Native interface", Some(if web { "Preview updates as you change things".into() } else { "Only for panes that use the native interface".into() })))
            .child(options)
            .child(text_size)
            .child(panel(c).child(row(c, "Plan sidebar", Some("Both styles: the plan sits on the right when the pane is wide enough. /sidebar toggles it in a session.".into()), sidebar)))
    }

    /// The text size slider, kept in step with ⌘+ / ⌘− in the panes.
    fn text_size_slider(&mut self, scale: f64, window: &mut Window, cx: &mut Context<Self>) -> Entity<SliderState> {
        if let Some(s) = self.harness_page.ui.text_size.clone() {
            if let SliderValue::Single(v) = s.read(cx).value()
                && (v as f64 - scale).abs() > 0.001
            {
                s.update(cx, |s, cx| s.set_value(scale as f32, window, cx));
            }
            return s;
        }
        let s = cx.new(|_| SliderState::new().min(0.8).max(1.5).step(0.05).default_value(scale as f32));
        cx.subscribe(&s, |this, _, e: &SliderEvent, cx| {
            let (SliderEvent::Change(SliderValue::Single(v)) | SliderEvent::Release(SliderValue::Single(v))) = e else { return };
            let v = ((*v as f64) * 20.0).round() / 20.0;
            let now = this.state.read(cx).rest.get("codeWeb").and_then(|w| w["textScale"].as_f64()).unwrap_or(1.0);
            if (now - v).abs() > 0.001 {
                this.set_rest("codeWeb", "textScale", json!(v), cx);
            }
        })
        .detach();
        self.harness_page.ui.text_size = Some(s.clone());
        s
    }

    /* Models */

    fn code_models(&mut self, c: Colors, cx: &mut Context<Self>) -> Div {
        let st = self.harness_page.settings.clone().unwrap_or(Value::Null);
        let loading = self.harness_page.models.is_none();
        let list = self.harness_page.models.clone().unwrap_or_default();
        let mono = crate::fonts::mono(cx);
        let model = st["model"].as_str().unwrap_or_default().to_string();
        let mut options = vec![(String::new(), "Automatic".to_string())];
        for (id, label) in BACKENDS {
            options.extend(list.iter().filter(|m| m["spec"].as_str().is_some_and(|s| s.starts_with(&format!("{id}/")))).map(|m| (m["spec"].as_str().unwrap_or_default().to_string(), format!("{label} · {}", m["label"].as_str().unwrap_or_default()))));
        }
        if !model.is_empty() && !options.iter().any(|o| o.0 == model) {
            options.push((model.clone(), model.clone()));
        }
        let current = options.iter().find(|o| o.0 == model).map_or(model.clone(), |o| o.1.clone());
        let current = if loading && !model.is_empty() { model.clone() } else { current };
        let select = self.select_button_flex("harness-model", current, options, 320.0, false, c, cx, |this, v, cx| this.save_harness("model", json!(v), cx));

        let effort = st["effort"].as_str().unwrap_or_default().to_string();
        let mut pills = div().flex().gap(px(2.0)).p(px(2.0)).rounded(px(8.0)).bg(c.app).border_1().border_color(c.edge);
        for (value, label) in EFFORTS {
            let on = effort == value;
            pills = pills.child(
                div().id(SharedString::from(format!("effort-{label}"))).flex_1().h(px(24.0)).flex().items_center().justify_center().rounded(px(6.0)).text_size(px(11.0)).cursor_pointer()
                    .when(on, |d| d.bg(c.accent).text_color(c.on_accent))
                    .when(!on, |d| d.text_color(c.muted).hover(move |d| d.bg(c.hover).text_color(c.fg)))
                    .on_click(cx.listener(move |this, _, _, cx| this.save_harness("effort", if value.is_empty() { Value::Null } else { json!(value) }, cx)))
                    .child(label),
            );
        }
        let now = if model.is_empty() { self.harness_page.auto_backend.clone().unwrap_or_else(|| "chatgpt".into()) } else { backend_of(&model) };
        let resolved = if model.is_empty() { format!("Automatic → {}", backend_label(&now)) } else { format!("Runs on {}", backend_label(&now)) };
        let main = div().w(px(324.0)).flex_shrink_0().flex().flex_col().gap_2p5().p(px(14.0)).rounded(px(12.0)).bg(c.panel).border_1().border_color(c.accent)
            .child(div().flex().items_center().gap_2().text_size(px(13.0)).font_weight(FontWeight::SEMIBOLD).text_color(c.fg)
                .child(div().size(px(26.0)).flex().items_center().justify_center().rounded(px(7.0)).bg(alpha(c.accent, 0.15)).text_color(c.accent).child(Icon::new(IconName::Sparkles).size(px(14.0))))
                .child("Main model"))
            .child(select)
            .child(div().text_size(px(11.0)).text_color(c.subtle).mb(px(-4.0)).child("Reasoning effort"))
            .child(pills)
            .child(div().flex().items_center().gap_1p5().px(px(9.0)).py(px(7.0)).rounded(px(7.0)).bg(c.app).text_size(px(11.5)).text_color(c.muted)
                .child(Icon::new(IconName::CircleCheck).size(px(12.0)).text_color(c.success)).child(resolved));

        // One row per backend: what its subagents run on. The backend the main model runs on is lit.
        let levels = |all: &[&str], empty: &str| std::iter::once((String::new(), empty.to_string())).chain(all.iter().map(|e| (e.to_string(), e.to_string()))).collect::<Vec<_>>();
        // Rows are 50px with 8px between; the main card's line meets the trunk at the middle. The
        // path from the main card to the backend in use is drawn in the accent color.
        let (row_h, gap) = (50.0, 8.0);
        let middle = (BACKENDS.len() as f32 * row_h + (BACKENDS.len() - 1) as f32 * gap) / 2.0;
        let lit_at = BACKENDS.iter().position(|(id, _)| *id == now).map(|i| i as f32 * (row_h + gap) + row_h / 2.0);
        let mut subs = div().relative().flex_1().min_w_0().flex().flex_col().gap_2().pl(px(40.0))
            .child(div().absolute().left(px(20.0)).top(px(row_h / 2.0)).bottom(px(row_h / 2.0)).w(px(1.5)).bg(c.border_strong))
            .child(div().absolute().left_0().top(px(middle)).w(px(21.5)).h(px(1.5)).bg(if lit_at.is_some() { c.accent } else { c.border_strong }))
            .children(lit_at.map(|y| div().absolute().left(px(20.0)).top(px(y.min(middle))).h(px((y - middle).abs() + 1.5)).w(px(1.5)).bg(c.accent)));
        for (id, label) in BACKENDS {
            let cur = st["subagents"][id].clone();
            let prefix = format!("{id}/");
            let mut opts = vec![(String::new(), "Same as main model".to_string())];
            opts.extend(list.iter().filter_map(|m| {
                let spec = m["spec"].as_str()?;
                (spec.starts_with(&prefix) && !spec.ends_with("-fast")).then(|| (spec[prefix.len()..].to_string(), m["label"].as_str().unwrap_or_default().to_string()))
            }));
            let sub_model = cur["model"].as_str().unwrap_or_default().to_string();
            if !sub_model.is_empty() && !opts.iter().any(|o| o.0 == sub_model) {
                opts.push((sub_model.clone(), sub_model.clone()));
            }
            let shown = opts.iter().find(|o| o.0 == sub_model).map_or(sub_model.clone(), |o| o.1.clone());
            let keep = cur["effort"].clone();
            let model_select = self.select_button_flex(&format!("sub-{id}"), shown, opts, 240.0, false, c, cx, move |this, v, cx| {
                let Some(st) = this.harness_page.settings.as_ref() else { return };
                let mut next = st["subagents"].as_object().cloned().unwrap_or_default();
                if v.is_empty() { next.remove(id); } else { next.insert(id.to_string(), json!({ "model": v, "effort": keep })); }
                this.save_harness("subagents", Value::Object(next), cx);
            });
            let effort_select = (!sub_model.is_empty() && id != "claude").then(|| {
                let (m, e) = (sub_model.clone(), cur["effort"].as_str().unwrap_or_default().to_string());
                self.select_button_flex(&format!("sub-effort-{id}"), if e.is_empty() { "Effort".into() } else { e }, levels(&["minimal", "low", "medium", "high", "xhigh"], "Default effort"), 160.0, false, c, cx, move |this, v, cx| {
                    let Some(st) = this.harness_page.settings.as_ref() else { return };
                    let mut next = st["subagents"].as_object().cloned().unwrap_or_default();
                    next.insert(id.to_string(), json!({ "model": m, "effort": if v.is_empty() { Value::Null } else { json!(v) } }));
                    this.save_harness("subagents", Value::Object(next), cx);
                })
            });
            let lit = id == now;
            let (tile_bg, tile_fg, tile): (Hsla, Hsla, AnyElement) = match id {
                "chatgpt" => (alpha(crate::theme::hex("#10a37f"), 0.15), crate::theme::hex("#10a37f"), "GP".into_any_element()),
                "claude" => (alpha(c.claude, 0.15), c.claude, "C".into_any_element()),
                "codex" => (c.hover, c.muted, "CX".into_any_element()),
                _ => (alpha(c.accent, 0.15), c.accent, Icon::new(IconName::Server).size(px(13.0)).into_any_element()),
            };
            subs = subs.child(
                div().relative().h(px(50.0)).flex().items_center().gap_2().px(px(10.0)).rounded(px(10.0)).bg(c.panel).border_1().border_color(if lit { alpha(c.accent, 0.6) } else { c.edge })
                    .child(div().absolute().left(px(-21.0)).top(px(23.0)).w(px(20.0)).h(px(1.5)).bg(if lit { c.accent } else { c.border_strong }))
                    .child(div().size(px(26.0)).flex_shrink_0().flex().items_center().justify_center().rounded(px(7.0)).bg(tile_bg).text_color(tile_fg).text_size(px(11.0)).font_weight(FontWeight::BOLD).child(tile))
                    .child(div().w(px(78.0)).flex_shrink_0().flex().flex_col()
                        .child(div().truncate().text_size(px(12.0)).font_weight(FontWeight::MEDIUM).text_color(c.fg).child(label.replace(" account", "")))
                        .child(div().text_size(px(10.5)).text_color(if lit { c.accent } else { c.subtle }).child(if lit { "active backend" } else { "subagents" })))
                    .child(div().flex_1().min_w_0().child(model_select))
                    .children(effort_select.map(|e| div().w(px(82.0)).flex_shrink_0().child(e))),
            );
        }
        let explain = |icon: IconName, title: &str, text: &str| div().flex_1().min_w_0().flex().gap_2p5().p(px(12.0)).rounded(px(9.0)).bg(c.panel).border_1().border_color(c.edge).text_size(px(12.0)).text_color(c.muted)
            .child(Icon::new(icon).size(px(15.0)).flex_shrink_0().text_color(c.accent))
            .child(div().flex_1().min_w_0().flex().flex_col().gap_0p5().child(div().text_color(c.fg).font_weight(FontWeight::MEDIUM).child(title.to_string())).child(text.to_string()));
        div().flex().flex_col().gap_4()
            .child(intro(c, "New sessions start with these.", Some(div().flex().gap_1().child(kbd(c, &mono, "/model")).child(kbd(c, &mono, "/effort")).child(kbd(c, &mono, "/subagent-model")).into_any_element())))
            .child(div().flex().items_center().child(main).child(subs))
            .child(div().flex().gap_3()
                .child(explain(IconName::Sparkles, "The main model", "Plans the work, writes the important changes and checks what subagents bring back."))
                .child(explain(IconName::GitFork, "Subagents", "Explore the code, look things up and make small fixes, so a cheaper model can do them. One per backend.")))
    }

    /* Permissions */

    fn code_permissions(&mut self, c: Colors, cx: &mut Context<Self>) -> Div {
        let st = self.harness_page.settings.clone().unwrap_or(Value::Null);
        let mono = crate::fonts::mono(cx);
        let mode = st["mode"].as_str().unwrap_or("ask").to_string();
        let approve_on = self.state.read(cx).auto_approve.iter().any(|p| p == WINFER_PROVIDER);
        let (_, _, _, what, caps) = MODES.iter().find(|m| m.0 == mode).copied().unwrap_or(MODES[2]);

        let segments = div().absolute().left(relative(0.1)).right(relative(0.1)).top(px(13.0)).h(px(4.0)).flex().rounded_full().overflow_hidden().opacity(0.55)
            .children([c.success, c.success, c.warning, c.danger].map(|col| div().flex_1().h_full().bg(col)));
        let mut track = div().relative().flex().child(segments);
        for (id, label, sub, _, _) in MODES {
            let on = id == mode;
            let knob = div().size(px(30.0)).flex().items_center().justify_center().rounded_full().when(on, |d| d.bg(alpha(c.accent, 0.22)))
                .child(div().size(px(22.0)).rounded_full().border_2().when(on, |d| d.bg(c.accent).border_color(c.panel)).when(!on, |d| d.bg(c.panel).border_color(c.border_strong)));
            track = track.child(
                div().id(SharedString::from(format!("mode-{id}"))).flex_1().flex().flex_col().items_center().gap_1().cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| this.save_harness("mode", json!(id), cx)))
                    .child(knob)
                    .child(div().text_size(px(12.5)).text_color(if on { c.fg } else { c.muted }).when(on, |d| d.font_weight(FontWeight::MEDIUM)).child(label))
                    .child(div().text_size(px(10.5)).text_color(c.subtle).child(sub)),
            );
        }
        let cap_box = |icon: IconName, title: &str, cap: Cap| {
            let (i, text, col) = match cap {
                Cap::Allow => (IconName::Check, "Allowed", c.success),
                Cap::Asks => (IconName::Hand, "Asks first", c.warning),
                Cap::Block => (IconName::Lock, "Blocked", c.subtle),
            };
            div().flex_1().flex().items_center().gap_2p5().px_3().py_2p5().rounded(px(9.0)).bg(c.app)
                .child(div().size(px(28.0)).flex().items_center().justify_center().rounded(px(7.0)).bg(c.hover).text_color(c.muted).child(Icon::new(icon).size(px(14.0))))
                .child(div().flex().flex_col()
                    .child(div().text_size(px(12.5)).font_weight(FontWeight::MEDIUM).text_color(c.fg).child(title.to_string()))
                    .child(div().flex().items_center().gap_1().text_size(px(11.5)).text_color(col).child(Icon::new(i).size(px(12.0))).child(text)))
        };
        let scale = div().flex().flex_col().p(px(16.0)).pt(px(18.0)).rounded(px(12.0)).bg(c.panel).border_1().border_color(c.edge).when(approve_on, |d| d.opacity(0.4))
            .child(track)
            .child(div().flex().gap_2().mt_4()
                .child(cap_box(IconName::FileSearch, "Read files", caps[0]))
                .child(cap_box(IconName::FilePenLine, "Edit files", caps[1]))
                .child(cap_box(IconName::Terminal, "Run commands", caps[2])))
            .child(div().flex().items_center().gap_2().mt_3().text_size(px(12.0)).text_color(c.muted).child(Icon::new(IconName::Info).size(px(13.0)).text_color(c.accent)).child(what));

        let approve = self.toggle("harness-approve", approve_on, c, cx, move |this, cx| this.update_state(cx, |s| {
            if approve_on { s.auto_approve.retain(|p| p != WINFER_PROVIDER); } else { s.auto_approve.push(WINFER_PROVIDER.into()); }
        }));
        // Missing from older configs: on by default.
        let resume = self.harness_toggle("autoResume", st["autoResume"] != false, c, cx);
        let commands = self.harness_page.commands.clone().expect("loaded");
        let mut tags = div().w(px(300.0)).min_h(px(32.0)).flex().flex_wrap().items_center().gap_1p5().p_1().rounded_md().bg(c.raised).border_1().border_color(c.edge);
        for (i, t) in self.allowed_commands().into_iter().enumerate() {
            tags = tags.child(
                div().h(px(22.0)).flex().items_center().gap_0p5().pl_2().pr(px(3.0)).rounded(px(5.0)).bg(c.hover).font_family(mono.clone()).text_size(px(11.5)).text_color(c.fg).child(t)
                    .child(div().id(SharedString::from(format!("cmd-x-{i}"))).size(px(16.0)).flex().items_center().justify_center().rounded_sm().cursor_pointer().text_color(c.subtle)
                        .hover(move |d| d.bg(c.active).text_color(c.fg))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let mut list = this.allowed_commands();
                            if i < list.len() {
                                list.remove(i);
                            }
                            this.save_harness("allowedCommands", json!(list), cx);
                        }))
                        .child(Icon::new(IconName::X).size(px(11.0)))),
            );
        }
        tags = tags.child(div().flex_1().min_w(px(70.0)).child(Input::new(&commands).appearance(false).text_size(px(11.5))));
        div().flex().flex_col().gap_4()
            .child(intro(c, "What new sessions may do without asking.", Some(div().flex().items_center().gap_1p5().child(kbd(c, &mono, "⇧ Tab")).child("cycles it in a session").into_any_element())))
            .when(approve_on, |d| d.child(div().flex().items_center().gap_2p5().px_3().py_2p5().rounded(px(9.0)).bg(alpha(c.warning, 0.12)).text_size(px(12.5)).text_color(c.warning)
                .child(Icon::new(IconName::TriangleAlert).size(px(15.0)))
                .child("Auto-approve is on: WinferCode panes start with --yolo and ignore the mode below.")))
            .child(scale)
            .child(panel(c)
                .child(row(c, "Auto-approve in panes", Some("Start WinferCode panes with --yolo, whatever the mode above. Applies to panes started after the change.".into()), approve))
                .child(div().border_t_1().border_color(c.border).child(row(c, "Always-allowed commands", Some("Commands starting with these never ask. ChatGPT account only; Claude Code and Codex use their own rules.".into()), tags)))
                .child(div().border_t_1().border_color(c.border).child(row(c, "Resume after usage limits", Some("When every account is out of usage (e.g. the 5-hour limit), a Claude Code or Codex session waits for the reset, then continues and sends the messages queued meanwhile. Esc stops waiting.".into()), resume))))
    }

    /* Efficiency */

    fn code_efficiency(&mut self, c: Colors, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let st = self.harness_page.settings.clone().unwrap_or(Value::Null);
        let (bridges, lean_codex) = (st["leanBridges"] == true, st["leanCodex"] == true);
        let lean_card = |title: &'static str, toggle: AnyElement, on: bool, saving: &str, lean: f32, line: &str, extra: Vec<Div>| {
            let tip = hint(title).unwrap_or_default();
            panel(c).flex_1().min_w_0().gap_2p5().p(px(14.0)).rounded(px(12.0))
                .child(div().flex().items_center().gap_2()
                    .child(div().text_size(px(13.0)).font_weight(FontWeight::MEDIUM).text_color(c.fg).child(title))
                    .child(div().id(SharedString::from(format!("lean-tip-{title}"))).child(Icon::new(IconName::Info).size(px(13.0)).text_color(c.subtle))
                        .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx)))
                    .child(div().flex_1())
                    .child(toggle))
                .child(div().flex().items_end().gap_1().when(!on, |d| d.opacity(0.3))
                    .child(div().text_size(px(28.0)).font_weight(FontWeight::SEMIBOLD).text_color(c.success).child(saving.to_string()))
                    .child(div().pb(px(4.0)).text_size(px(11.5)).text_color(c.subtle).child("input tokens · same pass rate")))
                .child(div().flex().flex_col().gap_1().when(!on, |d| d.opacity(0.3))
                    .child(div().h(px(6.0)).w_full().rounded_full().bg(c.active))
                    .child(div().flex().items_center().gap_2().child(div().h(px(6.0)).w(relative(lean)).rounded_full().bg(c.success)).child(div().text_size(px(10.5)).text_color(c.subtle).child("lean"))))
                .child(div().text_size(px(12.0)).text_color(c.subtle).child(line.to_string()))
                .children(extra)
        };
        let t_claude = self.harness_toggle("leanBridges", bridges, c, cx);
        // Lean Codex includes the short system prompt and Codex CLI panes (they follow it).
        let t_codex = self.toggle("harness-leanCodex", lean_codex, c, cx, move |this, cx| {
            if let Some(st) = this.harness_page.settings.as_mut() {
                st["leanCodexPrompt"] = json!(!lean_codex);
            }
            this.save_harness("leanCodex", json!(!lean_codex), cx);
        });
        let leans = div().flex().gap_3()
            .child(lean_card("Claude Code", t_claude, bridges, "−71%", 0.29, "Short prompt, coding tools only. Project CLAUDE.md and skills still load.", Vec::new()))
            .child(lean_card("Codex", t_codex, lean_codex, "−52%", 0.48, "Short system prompt; no apps, plugins or bundled skills; tool results capped at 4k tokens. Codex CLI panes too.", Vec::new()));
        let mut page = div().flex().flex_col().gap_4()
            .child(section(c, "Lean setup", Some("Fewer tools and skills per request · new sessions".into())))
            .child(leans);
        if bridges || lean_codex {
            page = page.child(self.skills_card(c, &st, window, cx));
        }
        page.child(section(c, "Context compaction", Some("Drag the markers · /compaction".into()))).child(self.compaction_card(c, &st, cx))
    }

    /// Which skills lean sessions load: all of them, or the picked ones in a compact picker that
    /// stays one field however many skills there are.
    fn skills_card(&mut self, c: Colors, st: &Value, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let all = st["leanAllSkills"] == true;
        let items = skill_items(self.harness_page.skills.as_deref().unwrap_or_default());
        let all_tokens: u64 = items.iter().map(|s| s.tokens).sum();
        let mode_seg = self.seg_control("skills-mode", vec![(false, "Selected"), (true, "All")], all, c, cx, |this, v, cx| this.save_harness("leanAllSkills", json!(v), cx));
        let note = if self.harness_page.skills.is_none() {
            "Loading your skills…".to_string()
        } else if items.is_empty() {
            "No skills found in ~/.claude/skills, your Claude Code plugins, ~/.codex/skills or ~/.agents/skills.".to_string()
        } else if all {
            format!("All {} load; their descriptions add ~{} tokens to every request.", items.len(), all_tokens)
        } else {
            "Only the ones you pick load. Built-in and project skills always come along.".into()
        };
        let head = div().flex().items_center().gap_6().child(small_label(c, "Skills in lean sessions", note)).child(mode_seg);
        let mut card = panel(c).px_4().py_3();
        card = card.child(head);
        if !all && !items.is_empty() {
            card = card.child(self.skill_picker(c, st, items, all_tokens, window, cx));
        }
        card
    }

    fn picked_skills(&self) -> Vec<String> {
        self.harness_page.settings.as_ref().and_then(|s| s["leanSkills"].as_array()).map_or(Vec::new(), |a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
    }

    fn toggle_skill(&mut self, id: &str, cx: &mut Context<Self>) {
        let mut picked = self.picked_skills();
        match picked.iter().position(|p| p == id) {
            Some(i) => { picked.remove(i); }
            None => picked.push(id.to_string()),
        }
        self.save_harness("leanSkills", json!(picked), cx);
    }

    fn skill_query_text(&self, cx: &App) -> String {
        self.harness_page.ui.skill_query.as_ref().map_or(String::new(), |q| q.read(cx).value().to_string())
    }

    fn visible_now(&self, cx: &App) -> Vec<SkillItem> {
        let items = skill_items(self.harness_page.skills.as_deref().unwrap_or_default());
        visible_skills(&items, &self.picked_skills(), &self.harness_page.ui.skill_filter, &self.skill_query_text(cx))
    }

    /// Closes the skill list if it's open (Esc); true if it was.
    pub(super) fn close_skill_list(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let open = self.harness_page.ui.skills_open;
        if open {
            self.set_skills_open(false, window, cx);
        }
        open
    }

    fn set_skills_open(&mut self, open: bool, window: &mut Window, cx: &mut Context<Self>) {
        let ui = &mut self.harness_page.ui;
        ui.skills_open = open;
        ui.skill_hl = 0;
        if !open {
            ui.skill_filter = SkillFilter::All;
            if let Some(q) = ui.skill_query.clone() {
                q.update(cx, |s, cx| s.set_value("", window, cx));
            }
        }
        cx.notify();
    }

    fn skill_picker(&mut self, c: Colors, _st: &Value, items: Vec<SkillItem>, all_tokens: u64, window: &mut Window, cx: &mut Context<Self>) -> Stateful<Div> {
        let query = match self.harness_page.ui.skill_query.clone() {
            Some(q) => q,
            None => {
                let q = cx.new(|cx| InputState::new(window, cx).placeholder("Add a skill…"));
                cx.subscribe_in(&q, window, |this, _, e: &InputEvent, window, cx| match e {
                    InputEvent::Focus => {
                        if !this.harness_page.ui.skills_open {
                            this.set_skills_open(true, window, cx);
                        }
                    }
                    InputEvent::Change => {
                        this.harness_page.ui.skills_open = true;
                        this.harness_page.ui.skill_hl = 0;
                        cx.notify();
                    }
                    // Enter is handled with the arrow keys, below: after an arrow key the field doesn't report it.
                    InputEvent::PressEnter { .. } | InputEvent::Blur => {}
                })
                .detach();
                self.harness_page.ui.skill_query = Some(q.clone());
                q
            }
        };
        let mono = crate::fonts::mono(cx);
        let picked = self.picked_skills();
        let q_text = query.read(cx).value().to_string();
        let ui = &self.harness_page.ui;
        let (open, filter, hl) = (ui.skills_open, ui.skill_filter.clone(), ui.skill_hl);
        let by_id: HashMap<&str, &SkillItem> = items.iter().map(|s| (s.id.as_str(), s)).collect();
        let picked_items: Vec<&SkillItem> = picked.iter().filter_map(|id| by_id.get(id.as_str()).copied()).collect();
        let picked_tokens: u64 = picked_items.iter().map(|s| s.tokens).sum();

        // The field: picked skills as chips, then the search.
        let shown = if open { picked_items.len() } else { picked_items.len().min(MAX_SKILL_CHIPS) };
        let mut field = div().id("skill-field").flex().flex_wrap().items_center().gap(px(5.0)).min_h(px(34.0)).p(px(4.0)).pl(px(5.0)).rounded(px(8.0)).bg(c.raised).border_1()
            .border_color(if open { alpha(c.accent, 0.55) } else { c.edge }).cursor_text()
            .on_click(cx.listener(|this, _, window, cx| {
                if let Some(q) = this.harness_page.ui.skill_query.clone() {
                    q.update(cx, |s, cx| s.focus(window, cx));
                }
                if !this.harness_page.ui.skills_open {
                    this.set_skills_open(true, window, cx);
                }
            }));
        for s in picked_items.iter().take(shown) {
            let id = s.id.clone();
            field = field.child(
                div().h(px(24.0)).flex().items_center().gap(px(1.0)).pl_2().pr(px(2.0)).rounded(px(6.0)).bg(alpha(c.accent, 0.16)).text_size(px(11.5)).text_color(c.fg).whitespace_nowrap()
                    .when(s.source != "user", |d| d.child(div().text_color(c.subtle).child(format!("{}:", s.source))))
                    .child(s.name.clone())
                    .child(div().id(SharedString::from(format!("skill-x-{id}"))).ml_0p5().size(px(18.0)).flex().items_center().justify_center().rounded(px(4.0)).text_color(c.subtle).cursor_pointer()
                        .hover(move |d| d.bg(c.active).text_color(c.fg))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.toggle_skill(&id, cx);
                        }))
                        .child(Icon::new(IconName::X).size(px(11.0)))),
            );
        }
        if picked_items.len() > shown {
            field = field.child(div().id("skill-more").h(px(24.0)).flex().items_center().px_2().rounded(px(6.0)).bg(c.hover).text_size(px(11.5)).text_color(c.muted).cursor_pointer()
                .hover(move |d| d.text_color(c.fg))
                .on_click(cx.listener(|this, _, window, cx| {
                    cx.stop_propagation();
                    this.set_skills_open(true, window, cx);
                    this.harness_page.ui.skill_filter = SkillFilter::Picked;
                }))
                .child(format!("+{} more", picked_items.len() - shown)));
        }
        let placeholder_note = picked_items.is_empty() && q_text.is_empty() && !open;
        field = field
            .child(div().flex_1().min_w(px(120.0)).child(Input::new(&query).appearance(false).text_size(px(12.0))))
            .when(placeholder_note, |d| d.child(div().text_size(px(11.5)).text_color(c.subtle).child("No skills picked")))
            .child(div().id("skill-caret").size(px(24.0)).flex().items_center().justify_center().rounded(px(6.0)).text_color(c.subtle).cursor_pointer()
                .hover(move |d| d.bg(c.hover).text_color(c.fg))
                .on_click(cx.listener(move |this, _, window, cx| {
                    cx.stop_propagation();
                    this.set_skills_open(!open, window, cx);
                }))
                .child(Icon::new(if open { IconName::ChevronUp } else { IconName::ChevronDown }).size(px(14.0))));

        let mut picker = div().id("skill-picker").mt_2p5().flex().flex_col()
            .on_mouse_down_out(cx.listener(|this, _: &MouseDownEvent, window, cx| {
                if this.harness_page.ui.skills_open {
                    this.set_skills_open(false, window, cx);
                }
            }))
            // ↑ ↓ move through the list, ↵ picks, Esc closes it, ⌫ in an empty search drops the last skill.
            .capture_key_down(cx.listener(|this, e: &KeyDownEvent, window, cx| {
                let key = e.keystroke.key.as_str();
                match key {
                    "enter" if this.harness_page.ui.skills_open => {
                        let visible = this.visible_now(cx);
                        if let Some(s) = visible.get(this.harness_page.ui.skill_hl) {
                            let id = s.id.clone();
                            this.toggle_skill(&id, cx);
                        }
                        cx.stop_propagation();
                    }
                    "down" | "up" if this.harness_page.ui.skills_open => {
                        let n = this.visible_now(cx).len().max(1);
                        let hl = this.harness_page.ui.skill_hl;
                        this.harness_page.ui.skill_hl = if key == "down" { (hl + 1) % n } else { (hl + n - 1) % n };
                        this.scroll_skill_into_view(cx);
                        cx.stop_propagation();
                        cx.notify();
                    }
                    "escape" if this.harness_page.ui.skills_open => {
                        this.set_skills_open(false, window, cx);
                        cx.stop_propagation();
                    }
                    "backspace" if this.skill_query_text(cx).is_empty() => {
                        if let Some(last) = this.picked_skills().last().cloned() {
                            this.toggle_skill(&last, cx);
                            cx.stop_propagation();
                        }
                    }
                    _ => {}
                }
            }))
            .child(field);

        if open {
            let visible = visible_skills(&items, &picked, &filter, &q_text);
            let mut sources: Vec<String> = Vec::new();
            for s in &items {
                if !sources.contains(&s.source) {
                    sources.push(s.source.clone());
                }
            }
            let mut groups = div().id("skill-groups").flex().gap(px(3.0)).p(px(6.0)).border_b_1().border_color(c.border).overflow_x_scroll();
            let filters: Vec<(SkillFilter, String, usize)> = [(SkillFilter::All, "All".to_string(), items.len()), (SkillFilter::Picked, "Picked".to_string(), picked_items.len())]
                .into_iter()
                .chain(sources.iter().map(|src| (SkillFilter::Source(src.clone()), source_label(src).0, items.iter().filter(|s| &s.source == src).count())))
                .collect();
            for (i, (f, label, n)) in filters.into_iter().enumerate() {
                let on = f == filter;
                groups = groups.child(
                    div().id(("skill-filter", i)).flex_shrink_0().h(px(22.0)).flex().items_center().gap_1().px_2().rounded(px(6.0)).text_size(px(11.0)).cursor_pointer().whitespace_nowrap()
                        .when(on, |d| d.bg(c.active).text_color(c.fg))
                        .when(!on, |d| d.text_color(c.muted).hover(move |d| d.bg(c.hover).text_color(c.fg)))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.harness_page.ui.skill_filter = f.clone();
                            this.harness_page.ui.skill_hl = 0;
                            if let Some(q) = this.harness_page.ui.skill_query.clone() {
                                q.update(cx, |s, cx| s.focus(window, cx));
                            }
                            cx.notify();
                        }))
                        .child(label)
                        .child(div().font_family(mono.clone()).text_size(px(10.0)).text_color(c.subtle).child(n.to_string())),
                );
            }
            let q_lower = q_text.trim().to_lowercase();
            let mut list = div().id("skill-list").max_h(px(224.0)).overflow_y_scroll().track_scroll(&self.harness_page.ui.skill_scroll).flex().flex_col().pb_1();
            if visible.is_empty() {
                list = list.child(div().p_4().flex().justify_center().text_size(px(12.0)).text_color(c.subtle).child(if items.is_empty() { "No skills found.".to_string() } else { format!("No skill matches “{}”.", q_text.trim()) }));
            }
            for src in &sources {
                let rows: Vec<&SkillItem> = visible.iter().filter(|s| &s.source == src).collect();
                if rows.is_empty() {
                    continue;
                }
                let ids: Vec<String> = rows.iter().map(|s| s.id.clone()).collect();
                let all_on = ids.iter().all(|id| picked.contains(id));
                let (label, path) = source_label(src);
                list = list.child(
                    div().flex().items_center().gap_2().px(px(10.0)).pt(px(7.0)).pb(px(3.0)).text_size(px(10.5)).font_weight(FontWeight::SEMIBOLD).text_color(c.subtle)
                        .child(label.to_uppercase())
                        .child(div().font_family(mono.clone()).font_weight(FontWeight::NORMAL).text_size(px(10.0)).child(path))
                        .child(div().flex_1())
                        .child(div().id(SharedString::from(format!("skill-all-{src}"))).text_size(px(11.0)).font_weight(FontWeight::MEDIUM).text_color(c.accent).cursor_pointer()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let mut next = this.picked_skills();
                                if all_on {
                                    next.retain(|p| !ids.contains(p));
                                } else {
                                    for id in &ids {
                                        if !next.contains(id) {
                                            next.push(id.clone());
                                        }
                                    }
                                }
                                this.save_harness("leanSkills", json!(next), cx);
                            }))
                            .child(if all_on { "Unpick all" } else { "Pick all" })),
                );
                for s in rows {
                    let on = picked.contains(&s.id);
                    let is_hl = visible.iter().position(|v| v.id == s.id) == Some(hl);
                    let id = s.id.clone();
                    // The part of the name the search matched, in the accent color.
                    let name: Div = match (!q_lower.is_empty()).then(|| s.name.to_lowercase().find(&q_lower)).flatten() {
                        Some(at) if s.name.is_char_boundary(at) && s.name.is_char_boundary(at + q_lower.len()) => div().flex()
                            .child(s.name[..at].to_string())
                            .child(div().text_color(c.accent).font_weight(FontWeight::SEMIBOLD).child(s.name[at..at + q_lower.len()].to_string()))
                            .child(s.name[at + q_lower.len()..].to_string()),
                        _ => div().child(s.name.clone()),
                    };
                    let desc = s.description.clone();
                    list = list.child(
                        div().id(SharedString::from(format!("skill-row-{id}"))).h(px(28.0)).flex_shrink_0().flex().items_center().gap_2p5().px(px(10.0)).text_size(px(12.0)).text_color(c.fg).cursor_pointer()
                            .when(is_hl, |d| d.bg(c.hover))
                            .hover(move |d| d.bg(c.hover))
                            .tooltip(move |window, cx| Tooltip::new(desc.clone()).build(window, cx))
                            .on_click(cx.listener(move |this, _, _, cx| this.toggle_skill(&id, cx)))
                            .child(checkbox(c, on))
                            .child(div().w(px(136.0)).flex_shrink_0().overflow_hidden().whitespace_nowrap().child(name))
                            .child(div().flex_1().min_w_0().truncate().text_size(px(11.5)).text_color(c.subtle).child(s.description.clone()))
                            .child(div().w(px(44.0)).flex_shrink_0().flex().justify_end().font_family(mono.clone()).text_size(px(10.5)).text_color(c.subtle).child(s.tokens.to_string())),
                    );
                }
            }
            let keys = div().flex().gap(px(14.0)).px(px(10.0)).py(px(6.0)).border_t_1().border_color(c.border).text_size(px(11.0)).text_color(c.subtle)
                .children([("↑ ↓", "move"), ("↵", "pick"), ("⌫", "remove last"), ("esc", "close")].map(|(k, what)| div().flex().items_center().gap_1().child(kbd(c, &mono, k)).child(what)));
            picker = picker.child(div().mt_1p5().flex().flex_col().rounded(px(8.0)).bg(c.app).border_1().border_color(c.edge).overflow_hidden().child(groups).child(list).child(keys));
        }

        // How much the picked skills cost, against loading them all.
        let share = if all_tokens == 0 { 0.0 } else { picked_tokens as f32 / all_tokens as f32 };
        let foot = div().flex().items_center().gap_2p5().mt_2().text_size(px(11.5)).text_color(c.subtle)
            .child(format!("{} of {} picked", picked_items.len(), items.len()))
            .child(div().w(px(160.0)).h(px(5.0)).rounded_full().bg(c.active).overflow_hidden().child(div().h_full().w(relative(share)).rounded_full().bg(c.success)))
            .child(div().font_family(mono.clone()).child(format!("~{picked_tokens} of ~{all_tokens} tokens per request")))
            .child(div().flex_1())
            .when(!picked_items.is_empty(), |d| d.child(div().id("skills-clear").px_1p5().py_0p5().rounded(px(5.0)).text_color(c.muted).cursor_pointer()
                .hover(move |d| d.bg(c.hover).text_color(c.fg))
                .on_click(cx.listener(|this, _, _, cx| this.save_harness("leanSkills", json!([]), cx)))
                .child("Clear")));
        picker.child(foot)
    }

    /// Scrolls the skill list so the highlighted row is in view.
    fn scroll_skill_into_view(&mut self, cx: &App) {
        let visible = self.visible_now(cx);
        let Some(target) = visible.get(self.harness_page.ui.skill_hl) else { return };
        // The list's children: one heading per source, then its rows.
        let mut ix = 0;
        let mut last_source: Option<&str> = None;
        let mut sources: Vec<&str> = Vec::new();
        for s in &visible {
            if !sources.contains(&s.source.as_str()) {
                sources.push(&s.source);
            }
        }
        for src in sources {
            for s in visible.iter().filter(|s| s.source == src) {
                if last_source != Some(src) {
                    ix += 1;
                    last_source = Some(src);
                }
                if s.id == target.id {
                    self.harness_page.ui.skill_scroll.scroll_to_item(ix);
                    return;
                }
                ix += 1;
            }
        }
    }

    /// The context window as a bar, with the two compaction levels as markers to drag.
    fn compaction_card(&mut self, c: Colors, st: &Value, cx: &mut Context<Self>) -> Div {
        let mono = crate::fonts::mono(cx);
        let (mut trim, mut compact) = (st["trimAt"].as_u64().unwrap_or(0), st["compactAt"].as_u64().unwrap_or(0));
        match self.harness_page.ui.drag {
            Some(("trimAt", v)) => trim = v,
            Some((_, v)) => compact = v,
            None => {}
        }
        let overlap = trim > 0 && compact > 0 && trim >= compact;
        let pct = |v: u64| relative(v as f32 / 100.0);
        let segment = |from: u64, to: u64, bg: Option<Hsla>, label: &str, color: Hsla| {
            div().absolute().top_0().bottom_0().left(pct(from)).w(pct(to.saturating_sub(from))).flex().items_center().justify_center().overflow_hidden().whitespace_nowrap().text_size(px(11.0)).text_color(color)
                .when_some(bg, |d, bg| d.bg(bg)).child(label.to_string())
        };
        let first = [trim, compact].into_iter().filter(|v| *v > 0).min().unwrap_or(100);
        let bounds = self.harness_page.ui.ctx_bounds.clone();
        let mut inner = div().relative().h(px(38.0)).rounded(px(8.0)).bg(c.app).border_1().border_color(c.edge).overflow_hidden()
            .child(canvas(move |b, _, _| bounds.set(Some(b)), |_, _, _, _| {}).absolute().size_full())
            .child(segment(0, first, None, "conversation", c.subtle));
        if trim > 0 && (compact == 0 || trim < compact) {
            inner = inner.child(segment(trim, if compact > 0 { compact } else { 100 }, Some(alpha(c.warning, 0.2)), "trims", c.warning));
        }
        if compact > 0 {
            inner = inner.child(segment(compact, 100, Some(alpha(c.accent, 0.26)), "summarizes", c.accent));
        }
        let marker = |v: u64, color: Hsla| {
            div().absolute().left(pct(v)).top(px(-26.0)).bottom(px(-4.0)).w(px(0.0))
                .child(div().absolute().left(px(-1.5)).top(px(20.0)).bottom_0().w(px(3.0)).rounded_full().bg(color))
                .child(div().absolute().top_0().left(px(-22.0)).w(px(44.0)).h(px(18.0)).flex().items_center().justify_center().rounded_full().bg(color).font_family(mono.clone()).text_size(px(10.5)).font_weight(FontWeight::MEDIUM).text_color(c.app).child(format!("{v}%")))
        };
        let bar = div().relative().child(inner).when(trim > 0, |d| d.child(marker(trim, c.warning))).when(compact > 0, |d| d.child(marker(compact, c.accent)));
        let drag_area = div().id("ctx-drag").pt(px(26.0)).cursor(CursorStyle::ResizeLeftRight)
            .on_mouse_down(MouseButton::Left, cx.listener(|this, e: &MouseDownEvent, _, cx| {
                let Some(v) = this.ctx_percent(e.position) else { return };
                let st = this.harness_page.settings.clone().unwrap_or(Value::Null);
                let (t, k) = (st["trimAt"].as_u64().unwrap_or(0), st["compactAt"].as_u64().unwrap_or(0));
                // The nearest marker that's on follows the pointer.
                let key = match (t > 0, k > 0) {
                    (true, true) => if t.abs_diff(v) <= k.abs_diff(v) { "trimAt" } else { "compactAt" },
                    (true, false) => "trimAt",
                    (false, true) => "compactAt",
                    (false, false) => return,
                };
                this.harness_page.ui.drag = Some((key, v));
                cx.notify();
            }))
            .on_mouse_move(cx.listener(|this, e: &MouseMoveEvent, _, cx| {
                let Some((key, _)) = this.harness_page.ui.drag else { return };
                if e.pressed_button != Some(MouseButton::Left) {
                    return this.finish_ctx_drag(cx);
                }
                if let Some(v) = this.ctx_percent(e.position) {
                    this.harness_page.ui.drag = Some((key, v));
                    cx.notify();
                }
            }))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _, cx| this.finish_ctx_drag(cx)))
            .on_mouse_up_out(MouseButton::Left, cx.listener(|this, _, _, cx| this.finish_ctx_drag(cx)))
            .child(bar);
        let tick = |v: u64| {
            let label = div().absolute().w(px(40.0)).flex().font_family(mono.clone()).text_size(px(10.0)).text_color(c.subtle).child(format!("{v}%"));
            let label = match v {
                0 => label.left_0(),
                100 => label.left(px(-40.0)).justify_end(),
                _ => label.left(px(-20.0)).justify_center(),
            };
            div().absolute().left(pct(v)).top(px(5.0)).w(px(0.0)).child(label)
        };
        let ticks = div().relative().h(px(20.0)).children([0, 25, 50, 75, 100].map(tick));
        let legend = |id: &'static str, label: &str, sub: &str, color: Hsla, on: bool, default: u64, this: &Self, cx: &mut Context<Self>| {
            let toggle = this.toggle(id, on, c, cx, move |this, cx| {
                let key = if id == "ctx-trim" { "trimAt" } else { "compactAt" };
                this.save_harness(key, json!(if on { 0 } else { default }), cx);
            });
            div().flex_1().flex().items_center().gap_2p5().px(px(11.0)).py(px(9.0)).rounded(px(8.0)).bg(c.app)
                .child(div().size(px(10.0)).flex_shrink_0().rounded(px(3.0)).bg(color))
                .child(small_label(c, label.to_string(), sub.to_string()))
                .child(toggle)
        };
        let l1 = legend("ctx-trim", "Micro-compact", "Trims old tool output and file contents", c.warning, trim > 0, 60, self, cx);
        let l2 = legend("ctx-full", "Full compact", "Summarizes the conversation", c.accent, compact > 0, 85, self, cx);
        panel(c).px_4().pt_2().pb_3p5().rounded(px(12.0))
            .child(drag_area)
            .child(ticks)
            .child(div().flex().justify_center().text_size(px(11.0)).text_color(c.subtle).child("How full the context window is"))
            .child(div().flex().gap_2p5().mt_2p5().child(l1).child(l2))
            .when(overlap, |d| d.child(div().mt_2().text_xs().text_color(c.warning).child(hint("overlap").unwrap_or_default())))
    }

    /// Where on the context bar a pointer is, in 5% steps between 5 and 95.
    fn ctx_percent(&self, at: Point<Pixels>) -> Option<u64> {
        let b = self.harness_page.ui.ctx_bounds.get()?;
        let frac = ((at.x - b.left()) / b.size.width).clamp(0.0, 1.0);
        Some(((frac * 20.0).round() as u64 * 5).clamp(5, 95))
    }

    fn finish_ctx_drag(&mut self, cx: &mut Context<Self>) {
        if let Some((key, v)) = self.harness_page.ui.drag.take() {
            self.save_harness(key, json!(v), cx);
        }
        cx.notify();
    }

    /* Local models */

    fn code_local(&mut self, c: Colors, window: &mut Window, cx: &mut Context<Self>) -> Div {
        self.local_fields(window, cx);
        let st = self.harness_page.settings.clone().unwrap_or(Value::Null);
        let enabled = st["local"]["enabled"] == true;
        let mono = crate::fonts::mono(cx);
        let local_models: Vec<(String, String)> = self.harness_page.models.iter().flatten()
            .filter_map(|m| m["spec"].as_str().filter(|s| s.starts_with("local/")).map(|s| (s.to_string(), m["detail"].as_str().unwrap_or_default().to_string())))
            .collect();
        let toggle = self.toggle("local-enabled", enabled, c, cx, move |this, cx| this.commit_local(Some(("enabled", json!(!enabled))), None, cx));
        let hero = div().flex().items_center().gap(px(14.0)).px_4().py(px(10.0)).rounded(px(12.0)).border_1().border_color(c.edge).bg(alpha(c.accent, 0.06))
            .child(div().size(px(38.0)).flex_shrink_0().flex().items_center().justify_center().rounded(px(10.0)).bg(alpha(c.accent, 0.15)).text_color(c.accent).child(Icon::new(IconName::Server).size(px(19.0))))
            .child(small_label(c, "Local models", "Run WinferCode on vLLM, LM Studio, Ollama, llama.cpp or oMLX with winfer's own tools. Needs the Responses API (/v1/responses); its models show up as local/<model>."))
            .when(enabled && !local_models.is_empty(), |d| d.child(badge(c.success, format!("{} model{}", local_models.len(), if local_models.len() == 1 { "" } else { "s" }))))
            .child(toggle);
        let page = div().flex().flex_col().gap_4().child(hero);
        if !enabled {
            return page;
        }
        let draft = self.local_draft(None, cx);
        let efforts: Vec<String> = draft["efforts"].as_array().map_or(Vec::new(), |a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect());
        for level in efforts.iter().filter(|e| *e != "none") {
            if !self.harness_page.local.as_ref().is_some_and(|i| i.budgets.contains_key(level)) {
                let value = draft["thinkingBudgets"][level].as_u64().map_or(String::new(), |n| n.to_string());
                let input = cx.new(|cx| InputState::new(window, cx).placeholder("No cap").default_value(value));
                cx.subscribe_in(&input, window, |this, _, e: &InputEvent, window, cx| {
                    if matches!(e, InputEvent::Blur | InputEvent::PressEnter { .. }) {
                        this.commit_local(None, Some(window), cx);
                    }
                })
                .detach();
                if let Some(i) = self.harness_page.local.as_mut() {
                    i.budgets.insert(level.clone(), input);
                }
            }
        }
        let new_level = match self.harness_page.ui.new_level.clone() {
            Some(i) => i,
            None => {
                let i = cx.new(|cx| InputState::new(window, cx).placeholder("+ level"));
                cx.subscribe_in(&i, window, |this, input, e: &InputEvent, window, cx| {
                    if matches!(e, InputEvent::PressEnter { .. }) {
                        let typed = parse_efforts(&input.read(cx).value());
                        if typed.is_empty() {
                            return;
                        }
                        input.update(cx, |s, cx| s.set_value("", window, cx));
                        this.set_local_efforts(|list| list.extend(typed), window, cx);
                    }
                })
                .detach();
                self.harness_page.ui.new_level = Some(i.clone());
                i
            }
        };
        let inp = self.harness_page.local.as_ref().expect("local fields");
        let (host, port, path, key, context) = (inp.host.clone(), inp.port.clone(), inp.path.clone(), inp.key.clone(), inp.context.clone());
        let budgets: HashMap<String, Entity<InputState>> = inp.budgets.clone();

        let (testing, result) = (self.harness_page.local_testing, self.harness_page.local_test.clone());
        let conn = div().flex_1().min_w_0().h(px(34.0)).flex().items_center().rounded(px(8.0)).bg(c.raised).border_1().border_color(c.edge).overflow_hidden().font_family(mono.clone()).text_size(px(12.5))
            .child(div().pl(px(10.0)).text_color(c.subtle).child("http://"))
            .child(div().flex_1().min_w(px(120.0)).child(Input::new(&host).appearance(false).text_size(px(12.5))))
            .child(div().flex_shrink_0().text_color(c.subtle).child(":"))
            .child(div().w(px(52.0)).flex_shrink_0().child(Input::new(&port).appearance(false).text_size(px(12.5))))
            .child(div().w(px(56.0)).flex_shrink_0().child(Input::new(&path).appearance(false).text_size(px(12.5))));
        let test = plain_button(c, "local-test", if testing { "Testing…" } else { "Test" }).h(px(34.0))
            .on_click(cx.listener(move |this, _, _, cx| if !testing { this.test_local(cx) }));
        let mut server = panel(c)
            .child(div().flex().items_center().gap_2().px_4().py_3().child(conn).child(div().w(px(120.0)).flex_shrink_0().child(Input::new(&key))).child(test))
            // Only a test's result shows under the address.
            .children(result.as_ref().map(|r| {
                let (text, color) = match r { Ok(t) => (t.clone(), c.success), Err(t) => (t.clone(), c.danger) };
                div().px_4().pb_2p5().mt(px(-4.0)).text_xs().font_family(mono.clone()).text_color(color).child(text)
            }));
        // A search over the found models, which can be many.
        let query = match self.harness_page.ui.local_query.clone() {
            Some(q) => q,
            None => {
                let q = cx.new(|cx| InputState::new(window, cx).placeholder("Search models…"));
                cx.subscribe(&q, |this, _, e: &InputEvent, cx| {
                    if matches!(e, InputEvent::Change) {
                        this.harness_page.ui.local_scroll.set_offset(point(px(0.0), px(0.0)));
                        cx.notify();
                    }
                })
                .detach();
                self.harness_page.ui.local_query = Some(q.clone());
                q
            }
        };
        let q_text = query.read(cx).value().trim().to_lowercase();
        let total = local_models.len();
        let local_models: Vec<(String, String)> = local_models.into_iter().filter(|(spec, _)| q_text.is_empty() || spec.to_lowercase().contains(&q_text)).collect();
        if total > 0 {
            server = server.child(div().flex().items_center().gap_2().h(px(38.0)).px_4().border_t_1().border_color(c.border).text_color(c.subtle)
                .child(Icon::new(IconName::Search).size(px(13.0)))
                .child(div().flex_1().min_w_0().child(Input::new(&query).appearance(false).text_size(px(12.5))))
                .child(div().flex_shrink_0().font_family(mono.clone()).text_size(px(11.0)).child(if q_text.is_empty() { format!("{total} models") } else { format!("{} of {total}", local_models.len()) })));
            if local_models.is_empty() {
                server = server.child(div().h(px(44.0)).flex().items_center().justify_center().border_t_1().border_color(c.border).text_size(px(12.0)).text_color(c.subtle).child(format!("No model matches “{q_text}”.")));
            }
        }
        // The found models: up to four rows, then the list scrolls.
        const ROW_H: f32 = 44.0;
        let scroll = self.harness_page.ui.local_scroll.clone();
        // The wheel scrolls this list only, not the page around it; scrolling redraws the edge shadows.
        let mut found = div().id("local-models-list").max_h(px(ROW_H * 4.0)).overflow_y_scroll().track_scroll(&scroll).flex().flex_col()
            .on_scroll(|_: &ScrollEvent, _, cx| cx.stop_propagation());
        for (i, (spec, detail)) in local_models.iter().enumerate() {
            let s = spec.clone();
            // The maker's logo (Qwen, Gemma, …) where the family is known; else a dot.
            let mark = match super::model_logos::maker_of(spec).and_then(|slug| self.model_logo(slug, c.fg, cx)) {
                Some(image) => div().size(px(16.0)).flex_shrink_0().child(img(image).size_full()).into_any_element(),
                None => div().size(px(16.0)).flex_shrink_0().flex().items_center().justify_center().child(div().size(px(7.0)).rounded_full().bg(c.success)).into_any_element(),
            };
            found = found.child(div().h(px(ROW_H)).flex_shrink_0().flex().items_center().gap_2p5().px_4().border_t_1().border_color(c.border).text_size(px(12.5))
                .child(mark)
                .child(div().flex_1().min_w_0().truncate().font_family(mono.clone()).text_color(c.fg).child(spec.clone()))
                .children(context_of(detail).map(|ctx| badge(c.muted, ctx)))
                .child(div().id(("use-local", i)).px_2().py_1().rounded_md().text_xs().text_color(c.muted).cursor_pointer().hover(move |d| d.bg(c.hover).text_color(c.fg))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.save_harness("model", json!(s), cx);
                        this.open_code_tab(CodeTab::Models, cx);
                    }))
                    .child("Use as main")));
        }
        if !local_models.is_empty() {
            // Fades at the edges where more rows are scrolled out of view, like compacted tool calls.
            let fade = canvas(|_, _, _| {}, move |bounds, _, window, _| crate::ui::code_view::paint_scroll_edge_fades_with(bounds, &scroll, c.panel, crate::ui::code_view::EdgeFade { height: 64.0, max_share: 0.4, alphas: [1.0, 0.94, 0.74, 0.38, 0.0] }, window)).absolute().inset_0();
            server = server.child(div().relative().child(found).child(fade));
        }

        // Thinking levels as cards, each with its budget and how it compares.
        let max_budget = efforts.iter().filter_map(|l| budgets.get(l)).map(|i| digits(&i.read(cx).value())).max().unwrap_or(0).max(1);
        let mut levels = div().flex().gap_2().px_4().pt(px(14.0)).pb(px(14.0));
        for (n, level) in efforts.iter().enumerate() {
            let lv = level.clone();
            let remove = div().id(("level-x", n)).size(px(18.0)).flex().items_center().justify_center().rounded(px(4.0)).text_color(c.subtle).cursor_pointer()
                .hover(move |d| d.bg(c.hover).text_color(c.fg))
                .on_click(cx.listener(move |this, _, window, cx| {
                    let lv = lv.clone();
                    this.set_local_efforts(move |list| list.retain(|l| *l != lv), window, cx);
                }))
                .child(Icon::new(IconName::X).size(px(11.0)));
            let head = div().flex().items_center().justify_between().text_size(px(12.0)).font_weight(FontWeight::MEDIUM).text_color(c.fg).child(effort_label(level)).child(remove);
            let card = div().flex_1().min_w_0().flex().flex_col().gap(px(7.0)).p(px(10.0)).rounded(px(9.0)).bg(c.app).child(head);
            let card = match budgets.get(level) {
                Some(input) if level != "none" => {
                    let v = digits(&input.read(cx).value());
                    card.child(Input::new(input).text_size(px(11.5)))
                        .child(div().h(px(4.0)).rounded_full().bg(c.active).overflow_hidden().child(div().h_full().w(relative(if v > 0 { v as f32 / max_budget as f32 } else { 1.0 })).bg(if v > 0 { c.accent } else { alpha(c.accent, 0.25) })))
                        .child(div().text_size(px(10.5)).text_color(c.subtle).child(if v > 0 { format!("{v} tokens") } else { "no cap".into() }))
                }
                _ => card.child(div().flex_1().text_size(px(10.5)).text_color(c.subtle).child("Thinking off")),
            };
            levels = levels.child(card);
        }
        levels = levels.child(div().w(px(96.0)).flex_shrink_0().flex().items_center().p_1p5().rounded(px(9.0)).border_1().border_dashed().border_color(c.border_strong).child(Input::new(&new_level).appearance(false).text_size(px(11.5))));
        let template = draft["templateEffort"] == true;
        let template_toggle = self.toggle("local-template", template, c, cx, move |this, cx| this.commit_local(Some(("templateEffort", json!(!template))), None, cx));
        let web = draft["webSearch"] == true;
        let web_toggle = self.toggle("local-web", web, c, cx, move |this, cx| this.commit_local(Some(("webSearch", json!(!web))), None, cx));
        page.child(server)
            .child(section(c, "Thinking levels", Some("As the chat template names them · llama.cpp templates override these".into())))
            .child(panel(c).child(levels).child(div().border_t_1().border_color(c.border).child(row(c, "Send level to chat template", Some("As chat_template_kwargs.reasoning_effort (llama.cpp, oMLX, MLX). Turn off if the server rejects it. Budgets go as thinking_budget (oMLX).".into()), template_toggle))))
            .child(panel(c)
                .child(row(c, "Context window", Some("For the context meter and compaction. Empty: what the server reports (else 32k).".into()), div().w(px(112.0)).child(Input::new(&context))))
                .child(div().border_t_1().border_color(c.border).child(row(c, "Web search", Some("Gives the models a web_search tool when the server searches the web (oMLX).".into()), web_toggle))))
    }

    /// A model maker's logo; starts fetching it the first time (None until it's there).
    fn model_logo(&mut self, slug: &'static str, color: Hsla, cx: &mut Context<Self>) -> Option<Arc<Image>> {
        let (image, fetch) = self.harness_page.ui.logos.get(slug, color);
        if fetch {
            cx.spawn(async move |this, cx| {
                let svg = backend::background(move || super::model_logos::fetch(slug)).await.unwrap_or_else(|e| Err(e.to_string()));
                let _ = this.update(cx, |this, cx| {
                    // A network error shows the dot until the next launch, which tries again.
                    this.harness_page.ui.logos.loaded(slug, svg.ok().flatten());
                    cx.notify();
                });
            })
            .detach();
        }
        image
    }

    /// Changes the thinking levels (through their field, which the saved settings read).
    fn set_local_efforts(&mut self, change: impl FnOnce(&mut Vec<String>), window: &mut Window, cx: &mut Context<Self>) {
        let Some(field) = self.harness_page.local.as_ref().map(|i| i.efforts.clone()) else { return };
        let mut list = parse_efforts(&field.read(cx).value());
        change(&mut list);
        let mut seen = Vec::new();
        list.retain(|l| if seen.contains(l) { false } else { seen.push(l.clone()); true });
        field.update(cx, |s, cx| s.set_value(list.join(", "), window, cx));
        self.commit_local(None, Some(window), cx);
    }

    /* MCP servers */

    fn code_mcp(&mut self, c: Colors, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let st = self.harness_page.settings.clone().unwrap_or(Value::Null);
        let servers = st["mcpServers"].as_object().cloned().unwrap_or_default();
        let json_view = self.harness_page.ui.mcp_json;
        let mono = crate::fonts::mono(cx);
        let view = self.seg_control("mcp-view", vec![(false, "List"), (true, "JSON")], json_view, c, cx, |this, v, cx| {
            this.harness_page.ui.mcp_json = v;
            cx.notify();
        });
        let page = div().flex().flex_col().gap_3().child(intro(c, "Extra MCP servers for every backend.", Some(view.into_any_element())));
        if json_view {
            let mcp = self.harness_page.mcp.clone().expect("loaded");
            let check = match parse_servers(&mcp.read(cx).value()) {
                Ok(v) => div().text_color(c.success).child(format!("✓ Valid · {} server{}", v.as_object().map_or(0, |m| m.len()), if v.as_object().map_or(0, |m| m.len()) == 1 { "" } else { "s" })),
                Err(e) => div().text_color(c.danger).child(e),
            };
            return page.child(panel(c).p_3().gap_1p5()
                .child(Textarea::new(&mcp).font_family(mono.clone()).text_size(px(12.0)))
                .child(div().flex().items_center().gap_2().text_xs().child(check).child(div().flex_1()).child(div().text_color(c.subtle).child("Saved when you leave the field."))));
        }
        let card = |icon: IconName, tint: bool| div().flex().items_center().gap_3().px(px(14.0)).py(px(11.0)).rounded(px(10.0)).bg(c.panel).border_1().border_color(c.edge)
            .child(div().size(px(32.0)).flex_shrink_0().flex().items_center().justify_center().rounded(px(8.0)).bg(if tint { alpha(c.accent, 0.15) } else { c.hover }).text_color(if tint { c.accent } else { c.muted }).child(Icon::new(icon).size(px(15.0))));
        let mut list = div().flex().flex_col().gap_2().child(card(IconName::SquareTerminal, true)
            .child(small_label(c, "winfer · built in", "Panes, the browser, secrets and notifications. Added automatically in panes."))
            .child(Icon::new(IconName::Lock).size(px(13.0)).text_color(c.subtle)));
        for (i, (name, cfg)) in servers.iter().enumerate() {
            let line = match cfg["url"].as_str() {
                Some(url) => url.to_string(),
                None => std::iter::once(cfg["command"].as_str().unwrap_or_default().to_string()).chain(cfg["args"].as_array().into_iter().flatten().filter_map(|a| a.as_str().map(str::to_string))).collect::<Vec<_>>().join(" "),
            };
            let env = cfg["env"].as_object().map_or(0, |e| e.len());
            let lower = name.to_lowercase();
            let icon = if lower.contains("github") { IconName::Github } else if ["sql", "postgres", "db", "mongo", "redis"].iter().any(|k| lower.contains(k)) { IconName::Database } else { IconName::Plug };
            let n = name.clone();
            let icon_button = |id: (&'static str, usize), icon: IconName| div().id(id).size(px(28.0)).flex().items_center().justify_center().rounded_md().text_color(c.subtle).cursor_pointer().hover(move |d| d.bg(c.hover).text_color(c.fg)).child(Icon::new(icon).size(px(13.0)));
            list = list.child(card(icon, false)
                .child(div().flex_1().min_w_0().flex().flex_col().gap_0p5()
                    .child(div().flex().items_center().gap_2().text_size(px(12.5)).font_weight(FontWeight::MEDIUM).text_color(c.fg).child(name.clone())
                        .when(env > 0, |d| d.child(badge(c.muted, format!("{env} env var{}", if env == 1 { "" } else { "s" })))))
                    .child(div().truncate().font_family(mono.clone()).text_size(px(11.5)).text_color(c.subtle).child(line)))
                .child(icon_button(("mcp-edit", i), IconName::Pencil).on_click(cx.listener(|this, _, _, cx| {
                    this.harness_page.ui.mcp_json = true;
                    cx.notify();
                })))
                .child(icon_button(("mcp-remove", i), IconName::Trash).on_click(cx.listener(move |this, _, window, cx| {
                    let Some(st) = this.harness_page.settings.as_ref() else { return };
                    let mut next = st["mcpServers"].as_object().cloned().unwrap_or_default();
                    next.remove(&n);
                    this.save_mcp_servers(Value::Object(next), window, cx);
                }))));
        }
        let form = self.harness_page.ui.new_server.clone();
        list = list.child(match form {
            Some((name, command)) => div().flex().items_center().gap_2().p_3().rounded(px(10.0)).bg(c.panel).border_1().border_color(c.accent)
                .child(div().w(px(140.0)).child(Input::new(&name)))
                .child(div().flex_1().min_w_0().child(Input::new(&command)))
                .child(plain_button(c, "mcp-add", "Add").on_click(cx.listener(|this, _, window, cx| this.add_mcp_server(window, cx))))
                .child(plain_button(c, "mcp-cancel", "Cancel").on_click(cx.listener(|this, _, _, cx| {
                    this.harness_page.ui.new_server = None;
                    cx.notify();
                })))
                .into_any_element(),
            None => div().id("mcp-new").h(px(42.0)).flex().items_center().justify_center().gap_1p5().rounded(px(10.0)).border_1().border_dashed().border_color(c.border_strong).text_size(px(12.5)).text_color(c.muted).cursor_pointer()
                .hover(move |d| d.bg(c.hover).text_color(c.fg))
                .on_click(cx.listener(|this, _, window, cx| {
                    let name = cx.new(|cx| InputState::new(window, cx).placeholder("Name"));
                    let command = cx.new(|cx| InputState::new(window, cx).placeholder("Command, e.g. npx -y @modelcontextprotocol/server-github"));
                    name.update(cx, |s, cx| s.focus(window, cx));
                    this.harness_page.ui.new_server = Some((name, command));
                    cx.notify();
                }))
                .child(Icon::new(IconName::Plus).size(px(13.0)))
                .child("Add server")
                .child(div().ml_1p5().text_color(c.subtle).child("a name and the command that starts it"))
                .into_any_element(),
        });
        let _ = window;
        page.child(list)
    }

    fn add_mcp_server(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((name, command)) = self.harness_page.ui.new_server.clone() else { return };
        let (n, line) = (name.read(cx).value().trim().to_string(), command.read(cx).value().trim().to_string());
        let mut parts = line.split_whitespace().map(str::to_string);
        let (Some(program), false) = (parts.next(), n.is_empty()) else {
            self.harness_page.result = Some(Err("MCP servers: give the server a name and a command.".into()));
            return cx.notify();
        };
        let args: Vec<String> = parts.collect();
        let mut next = self.harness_page.settings.as_ref().and_then(|s| s["mcpServers"].as_object().cloned()).unwrap_or_default();
        next.insert(n, json!({ "command": program, "args": args }));
        self.harness_page.ui.new_server = None;
        self.save_mcp_servers(Value::Object(next), window, cx);
    }

    /// Saves the servers and shows them in the JSON field too.
    fn save_mcp_servers(&mut self, servers: Value, window: &mut Window, cx: &mut Context<Self>) {
        let text = if servers.as_object().is_some_and(|m| !m.is_empty()) { serde_json::to_string_pretty(&servers).unwrap_or_default() } else { String::new() };
        if let Some(mcp) = self.harness_page.mcp.clone() {
            mcp.update(cx, |s, cx| s.set_value(text, window, cx));
        }
        self.save_harness("mcpServers", servers, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::{CodeTab, SkillFilter, SkillItem, backend_of, skill_items, skill_tokens, tab_for_label, visible_skills};
    use serde_json::json;

    fn item(id: &str, source: &str, description: &str) -> SkillItem {
        SkillItem { id: id.into(), name: id.rsplit(':').next().unwrap().into(), description: description.into(), source: source.into(), tokens: 10 }
    }

    #[test]
    fn search_hits_open_the_right_tab() {
        assert_eq!(tab_for_label("Use TUI style"), CodeTab::Interface);
        assert_eq!(tab_for_label("Subagent model · Codex"), CodeTab::Models);
        assert_eq!(tab_for_label("Always-allowed commands"), CodeTab::Permissions);
        assert_eq!(tab_for_label("Resume after usage limits"), CodeTab::Permissions);
        assert_eq!(tab_for_label("Full compact at"), CodeTab::Efficiency);
        assert_eq!(tab_for_label("Lean Codex CLI panes"), CodeTab::Efficiency);
        assert_eq!(tab_for_label("Thinking budget · Low"), CodeTab::Local);
        assert_eq!(tab_for_label("Host / Port"), CodeTab::Local);
        assert_eq!(tab_for_label("MCP servers"), CodeTab::Mcp);
    }

    #[test]
    fn skill_filters_and_search() {
        let items = vec![item("dataviz", "user", "Charts"), item("superpowers:review", "superpowers", "Code review"), item("sql-review", "user", "Reviews SQL")];
        let picked = vec!["dataviz".to_string()];
        let ids = |v: Vec<SkillItem>| v.into_iter().map(|s| s.id).collect::<Vec<_>>();
        assert_eq!(ids(visible_skills(&items, &picked, &SkillFilter::All, "")).len(), 3);
        assert_eq!(ids(visible_skills(&items, &picked, &SkillFilter::Picked, "")), ["dataviz"]);
        assert_eq!(ids(visible_skills(&items, &picked, &SkillFilter::Source("superpowers".into()), "")), ["superpowers:review"]);
        assert_eq!(ids(visible_skills(&items, &picked, &SkillFilter::All, " REV")), ["superpowers:review", "sql-review"]);
        assert!(skill_tokens("pdf", "Read, fill, merge and create PDFs") > 12);
    }

    #[test]
    fn skills_list_mine_first() {
        let list = vec![json!({ "id": "codex:a", "name": "a", "source": "codex" }), json!({ "id": "p:b", "name": "b", "source": "p" }), json!({ "id": "c", "name": "c", "source": "user" })];
        assert_eq!(skill_items(&list).iter().map(|s| s.source.as_str()).collect::<Vec<_>>(), ["user", "p", "codex"]);
    }

    #[test]
    fn local_model_pill_shows_the_context() {
        assert_eq!(super::context_of("Local · http://127.0.0.1:8000/v1 · 100k context · winfer tools").as_deref(), Some("100k context"));
        assert_eq!(super::context_of("Local · winfer tools"), None);
    }

    #[test]
    fn backends_of_model_specs() {
        assert_eq!(backend_of("local/devstral"), "local");
        assert_eq!(backend_of("claude/claude-opus-5-5"), "claude");
        assert_eq!(backend_of("claude-sonnet-5"), "claude");
    }
}
