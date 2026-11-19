//! A pane's header (port of `PaneFrame.tsx`, `GlassPaneHeader.tsx`, `PaneMenu.tsx`, the context
//! meter in `UsageMeter.tsx` and `TaskCostChip.tsx`): title and rename, progress, context use,
//! cost, branch, model, dev server link, the "…" menu and the pane buttons.

use super::commands::{Command, PaneRef};
use super::menu::MenuItem;
use super::orchestration::{format_cost, format_tokens};
use super::*;
use crate::model::Overlay;
use gpui_kit::component::input::Input;
use gpui_kit::component::tooltip::Tooltip;
use serde_json::Value;
use winfer_backend::{context_breakdown, usage as usage_backend, usage_history};

/// Colour per part of the context (`src/features/usage/contextParts.ts`).
const PART_COLORS: [(&str, &str); 10] = [
    ("setup", "#8a94a8"), ("system", "#8a94a8"), ("tools", "#5b8def"), ("mcp", "#9b7bea"), ("instructions", "#c79a3e"),
    ("messages", "#e5735a"), ("replies", "#d867a8"), ("thinking", "#a3b85a"), ("toolCalls", "#6dbf67"), ("toolResults", "#3fb5a3"),
];

#[derive(Default)]
pub(super) struct PaneMeters {
    /// How full each pane's context window is (`Context` in usage.ts).
    pub context: HashMap<String, Value>,
    /// What a pane's agent says it spent this session (winfer code).
    pub spend: HashMap<String, Value>,
    /// Logged spend by worktree folder and by agent session.
    pub by_dir: HashMap<String, Value>,
    pub by_session: HashMap<String, Value>,
    /// The context card's breakdown and the reading it belongs to.
    breakdown: HashMap<String, (f64, Option<Vec<Value>>)>,
    hovered: Option<String>,
    seconds: u64,
}

/// "84k", "1.2M".
pub fn short_tokens(n: f64) -> String {
    if n >= 1_000_000.0 {
        format!("{:.1}M", n / 1_000_000.0)
    } else if n >= 1000.0 {
        format!("{}k", (n / 1000.0).round())
    } else {
        format!("{n}")
    }
}

/// Context colour: fine until it gets full enough for the agent to start compacting.
fn context_color(used: f64, c: &Colors) -> Hsla {
    if used >= 85.0 {
        c.danger
    } else if used >= 60.0 {
        c.warning
    } else {
        c.accent
    }
}

/// "Opus 5.5: $1.20 · 1.4M tokens" lines, for a tooltip.
pub fn cost_breakdown(usage: &Value, place: &str) -> String {
    let mut lines = vec![format!("At API prices, {} model calls {place}:", usage["calls"])];
    for m in usage["models"].as_array().into_iter().flatten() {
        let tokens: f64 = ["input", "output", "cacheRead", "cacheWrite"].iter().filter_map(|k| m[*k].as_f64()).sum();
        let cost = if m["priced"] == false { "no known price".to_string() } else { format_cost(m["cost"].as_f64().unwrap_or(0.0)) };
        lines.push(format!("{}: {cost} · {} tokens", m["model"].as_str().unwrap_or_default(), format_tokens(tokens)));
    }
    lines.join("\n")
}

fn norm_dir(dir: &str) -> String {
    let d = dir.replace('\\', "/");
    if d.len() > 1 { d.trim_end_matches('/').to_string() } else { d }
}

impl Root {
    /// Context and spend from a `usage://report` (the status line, winfer code).
    pub(super) fn note_pane_report(&mut self, payload: &Value) {
        let Some(pane) = payload["paneId"].as_str() else { return };
        if payload["report"]["context"].is_object() {
            self.meters.context.insert(pane.to_string(), payload["report"]["context"].clone());
        }
        if payload["report"]["spend"].is_object() {
            self.meters.spend.insert(pane.to_string(), payload["report"]["spend"].clone());
        }
    }

    /// Every second: Codex and OpenCode context, read from their logs; pane costs while shown.
    pub(super) fn tick_meters(&mut self, cx: &mut Context<Self>) {
        self.meters.seconds += 1;
        let t = self.meters.seconds;
        let s = self.state.read(cx);
        let panes: Vec<Pane> = s.all_panes().cloned().collect();
        let show_cost = s.show_pane_cost;
        for pane in &panes {
            let poll = match pane.provider.as_str() {
                "codex" => t == 3 || (t % 30 == 0 && pane.status == AgentStatus::Working),
                "opencode" => t == 3 || t % 20 == 0,
                _ => false,
            };
            if poll {
                self.refresh_context(pane.clone(), cx);
            }
            if show_cost && pane.provider != "winfer" && !self.meters.spend.contains_key(&pane.id) && (t == 2 || (t % 30 == 0 && pane.status == AgentStatus::Working)) {
                self.refresh_cost(pane, cx);
            }
        }
    }

    /// A pane's agent changed status: its numbers likely moved.
    pub(super) fn on_meter_status(&mut self, pane: &str, cx: &mut Context<Self>) {
        let Some(pane) = self.state.read(cx).pane(pane).cloned() else { return };
        if pane.provider == "codex" {
            let p = pane.clone();
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(Duration::from_millis(1500)).await;
                let _ = this.update(cx, |this, cx| this.refresh_context(p, cx));
            })
            .detach();
        }
        if self.state.read(cx).show_pane_cost {
            self.refresh_cost(&pane, cx);
        }
    }

    fn refresh_context(&mut self, pane: Pane, cx: &mut Context<Self>) {
        let id = pane.id.clone();
        cx.spawn(async move |this, cx| {
            let found = match pane.provider.as_str() {
                "codex" => tauri::async_runtime::spawn(usage_backend::usage_codex_context(pane.session_id.clone(), pane.cwd.clone())).await.ok().flatten(),
                _ => tauri::async_runtime::spawn(usage_backend::usage_opencode_context(pane.cwd.clone())).await.ok().flatten(),
            };
            let Some(context) = found.and_then(|c| serde_json::to_value(c).ok()) else { return };
            let _ = this.update(cx, |this, cx| {
                this.meters.context.insert(id, context);
                cx.notify();
            });
        })
        .detach();
    }

    fn refresh_cost(&mut self, pane: &Pane, cx: &mut Context<Self>) {
        let dir = pane.worktree.as_ref().map(|t| norm_dir(&t.path));
        let session = pane.session_id.clone().filter(|_| dir.is_none());
        if dir.is_none() && session.is_none() {
            return;
        }
        cx.spawn(async move |this, cx| {
            let app = backend::app().clone();
            let found = match (&dir, &session) {
                (Some(d), _) => tauri::async_runtime::spawn(usage_history::usage_dirs(app, vec![d.clone()])).await,
                (_, Some(s)) => tauri::async_runtime::spawn(usage_history::usage_sessions(app, vec![s.clone()])).await,
                _ => return,
            };
            let Some(usage) = found.ok().and_then(Result::ok).and_then(|v| serde_json::to_value(v).ok()).and_then(|v| v.as_array()?.first().cloned()) else { return };
            let _ = this.update(cx, |this, cx| {
                match (dir, session) {
                    (Some(d), _) => this.meters.by_dir.insert(d, usage),
                    (_, Some(s)) => this.meters.by_session.insert(s, usage),
                    _ => None,
                };
                cx.notify();
            });
        })
        .detach();
    }

    fn load_breakdown(&mut self, pane: &Pane, cx: &mut Context<Self>) {
        let Some(context) = self.meters.context.get(&pane.id).cloned() else { return };
        let at = context["at"].as_f64().unwrap_or(0.0);
        if self.meters.breakdown.get(&pane.id).is_some_and(|(a, _)| *a == at) {
            return;
        }
        let Some(source) = context["source"].as_str().map(str::to_string) else {
            self.meters.breakdown.insert(pane.id.clone(), (at, None));
            return;
        };
        let provider = context["provider"].as_str().unwrap_or(&pane.provider).to_string();
        let tokens = context["tokens"].as_u64().unwrap_or(0);
        let id = pane.id.clone();
        cx.spawn(async move |this, cx| {
            let parts = tauri::async_runtime::spawn(context_breakdown::usage_context_breakdown(provider, source, tokens)).await.ok().flatten();
            let parts = parts.and_then(|p| serde_json::to_value(p).ok()).and_then(|v| v.as_array().cloned());
            let _ = this.update(cx, |this, cx| {
                this.meters.breakdown.insert(id, (at, parts));
                cx.notify();
            });
        })
        .detach();
    }

    fn context_meter(&self, pane: &Pane, c: Colors, cx: &mut Context<Self>) -> Option<AnyElement> {
        let context = self.meters.context.get(&pane.id)?;
        let used = context["usedPercent"].as_f64().unwrap_or(0.0).clamp(0.0, 100.0);
        let color = context_color(used, &c);
        let p = pane.clone();
        let id = pane.id.clone();
        let mono = crate::fonts::mono(cx);
        let bar = div().size(px(14.0)).rounded_full().border_2().border_color(Hsla { a: 0.15, ..c.fg }).child(div().size_full().rounded_full().border_2().border_color(color).opacity((used.max(3.0) / 100.0) as f32));
        Some(
            div().id(SharedString::from(format!("context-{}", pane.id))).flex().flex_shrink_0().items_center().gap_1()
                .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                    this.meters.hovered = hovered.then(|| id.clone());
                    if *hovered {
                        this.load_breakdown(&p, cx);
                    }
                    cx.notify();
                }))
                .child(bar)
                .child(div().font_family(mono).text_size(px(10.0)).text_color(if used >= 60.0 { color } else { c.subtle }).child(format!("{}%", used.round())))
                .when(self.meters.hovered.as_deref() == Some(&pane.id), |d| d.child(self.context_card(pane, c)))
                .into_any_element(),
        )
    }

    /// The card under the context meter: what fills the window.
    fn context_card(&self, pane: &Pane, c: Colors) -> Div {
        let context = &self.meters.context[&pane.id];
        let (tokens, window) = (context["tokens"].as_f64().unwrap_or(0.0), context["window"].as_f64().unwrap_or(0.0));
        let used = context["usedPercent"].as_f64().unwrap_or(0.0).clamp(0.0, 100.0);
        let free = (window - tokens).max(0.0);
        let pct = |n: f64| if window > 0.0 { n * 100.0 / window } else { 0.0 };
        let at = context["at"].as_f64().unwrap_or(0.0);
        let parts = self.meters.breakdown.get(&pane.id).filter(|(a, _)| *a == at).map(|(_, p)| p.clone());
        let color_of = |kind: &str| PART_COLORS.iter().find(|(k, _)| *k == kind).map_or(c.subtle, |(_, hex)| theme::hex(hex));
        let row = |color: Option<Hsla>, label: String, n: f64, nested: bool, muted: bool| {
            let p = pct(n);
            div().flex().items_center().gap_2().text_size(px(if nested { 10.0 } else { 11.0 })).when(nested, |d| d.pl_4())
                .child(match color {
                    Some(col) if !nested => div().size_2().flex_shrink_0().rounded_sm().bg(col),
                    _ => div().w_2().flex_shrink_0().text_center().text_color(c.subtle).child(if nested { "·" } else { "" }),
                })
                .child(div().flex_1().truncate().text_color(if muted { c.subtle } else { c.muted }).child(label))
                .child(div().text_color(c.fg).child(short_tokens(n)))
                .child(div().w(px(40.0)).text_right().text_color(c.subtle).child(if p < 1.0 && n > 0.0 { "<1%".to_string() } else { format!("{}%", p.round()) }))
        };
        let mut bar = div().flex().h_2().overflow_hidden().rounded_full().bg(c.hover);
        let mut body = div().flex().flex_col().gap_1();
        match &parts {
            Some(Some(list)) if !list.is_empty() => {
                for part in list {
                    let n = part["tokens"].as_f64().unwrap_or(0.0);
                    let col = color_of(part["kind"].as_str().unwrap_or_default());
                    bar = bar.child(div().h_full().bg(col).w(relative((pct(n) / 100.0) as f32)));
                    body = body.child(row(Some(col), part["label"].as_str().unwrap_or_default().to_string(), n, false, false));
                    for source in part["sources"].as_array().into_iter().flatten() {
                        body = body.child(row(None, source["label"].as_str().unwrap_or_default().to_string(), source["tokens"].as_f64().unwrap_or(0.0), true, false));
                    }
                }
                body = body.child(row(Some(c.hover), "Free space".into(), free, false, true)).child(div().pt_1().text_size(px(10.0)).text_color(c.subtle).child("Estimated from the session's conversation."));
            }
            Some(_) => {
                bar = bar.child(div().h_full().bg(context_color(used, &c)).w(relative((used / 100.0) as f32)));
                body = body.child(div().text_size(px(11.0)).text_color(c.subtle).child(format!("{} tokens free. No breakdown for this session.", short_tokens(free))));
            }
            None => {
                bar = bar.child(div().h_full().bg(context_color(used, &c)).w(relative((used / 100.0) as f32)));
                body = body.child(div().text_size(px(11.0)).text_color(c.subtle).child("Reading the session…"));
            }
        }
        div().absolute().top(px(22.0)).right_0().w(px(300.0)).flex().flex_col().gap_2p5().rounded_lg().border_1().border_color(c.border_strong).bg(c.raised).p_3()
            .child(div().flex().items_baseline().gap_2().text_xs()
                .child(div().flex_1().text_color(c.fg).child("Context"))
                .child(div().text_size(px(11.0)).text_color(c.muted).child(format!("{} / {} · {}%", short_tokens(tokens), short_tokens(window), used.round()))))
            .child(bar)
            .child(body)
    }

    /// A pane's cost in its header, when turned on in Settings → Usage.
    fn pane_cost(&self, pane: &Pane, c: Colors, cx: &App) -> Option<AnyElement> {
        if !self.state.read(cx).show_pane_cost {
            return None;
        }
        let mono = crate::fonts::mono(cx);
        let chip = |id: String, text: String, sub: String, tip: String| {
            div().id(SharedString::from(id)).flex().flex_shrink_0().items_center().gap_1().font_family(mono.clone()).text_size(px(11.0)).text_color(c.muted)
                .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
                .child(Icon::new(IconName::Coins).size(px(11.0)).text_color(c.subtle))
                .child(text)
                .child(div().text_color(c.subtle).child(sub))
                .into_any_element()
        };
        if let Some(spend) = self.meters.spend.get(&pane.id) {
            let (input, output, cached) = (spend["input"].as_f64().unwrap_or(0.0), spend["output"].as_f64().unwrap_or(0.0), spend["cached"].as_f64().unwrap_or(0.0));
            let cost = spend["cost"].as_f64();
            if input + output == 0.0 && cost.unwrap_or(0.0) == 0.0 {
                return None;
            }
            let estimated = spend["estimated"] == true;
            let mut tip = vec![
                format!("This session{}", if cost.is_some() { ", at API prices:" } else { ":" }),
                format!("{} in ({} cached) · {} out", format_tokens(input), format_tokens(cached), format_tokens(output)),
                cost.map_or("No price reported (billed to your plan)".to_string(), |v| format!("Cost: {} (your plan may cover it)", format_cost(v))),
            ];
            if estimated {
                tip.push(format!("Estimated at {}'s API prices; other models and service tiers may differ.", spend["model"].as_str().unwrap_or_default()));
            }
            let text = cost.map_or("—".to_string(), |v| format!("{}{}", if estimated { "≈" } else { "" }, format_cost(v)));
            return Some(chip(format!("cost-{}", pane.id), text, format!("· {}", format_tokens(input + output)), tip.join("\n")));
        }
        if pane.provider == "winfer" {
            return None;
        }
        let (usage, place) = match (&pane.worktree, &pane.session_id) {
            (Some(t), _) => (self.meters.by_dir.get(&norm_dir(&t.path)), "in this worktree"),
            (None, Some(s)) => (self.meters.by_session.get(s), "in this session"),
            _ => return None,
        };
        let usage = usage.filter(|u| u["calls"].as_u64().unwrap_or(0) > 0)?;
        let cost = usage["cost"].as_f64().unwrap_or(0.0);
        let text = if usage["priced"] == true || cost > 0.0 { format_cost(cost) } else { "—".into() };
        Some(chip(format!("cost-{}", pane.id), text, format!("· {}", format_tokens(usage["tokens"].as_f64().unwrap_or(0.0))), cost_breakdown(usage, place)))
    }

    /// The "…" menu's actions.
    fn header_menu(&self, pane: &Pane) -> Vec<MenuItem> {
        let target = PaneRef::Id(pane.id.clone());
        let rename = pane.id.clone();
        let mut items = vec![MenuItem::run("Rename…", move |this, window, cx| this.start_rename(&rename, window, cx))];
        if let Some(wt) = &pane.worktree {
            let path = wt.path.clone();
            items.push(MenuItem::command("Show changes", Command::ShowChanges(target.clone())));
            items.push(MenuItem::command(format!("Merge into {}", wt.base), Command::MergeWorktree(target.clone())));
            items.push(MenuItem::run(if cfg!(target_os = "macos") { "Reveal worktree in Finder" } else { "Show worktree in Explorer" }, move |_, _, _| sidebar::reveal_path(&path)));
            items.push(MenuItem::command("Discard worktree", Command::DiscardWorktree(target.clone())).danger());
        }
        items.push(MenuItem::command("Open in code editor", Command::EditorOpen { target: Some(target.clone()), editor: None }));
        items.push(MenuItem::commands("Change model…", vec![Command::Focus(target.clone()), Command::Overlay { overlay: Overlay::Model, provider: None }]));
        if pane.archived {
            items.push(MenuItem::command("Restore from archive", Command::Archive { target: target.clone(), restore: true }));
        } else {
            items.push(MenuItem::command("Zoom", Command::Zoom(target.clone())));
            items.push(MenuItem::command("Archive", Command::Archive { target: target.clone(), restore: false }));
        }
        items.push(MenuItem::command("Close", Command::Close(target)));
        items
    }

    /// The pane's header (index, status, agent, title, progress and the buttons) and, for agents,
    /// its status line under the terminal (workspace, branch, model, cost and context).
    pub(super) fn render_header(&self, pane: &Pane, index: usize, zoomed: bool, c: &Colors, cx: &mut Context<Self>) -> (AnyElement, Option<AnyElement>) {
        let c = *c;
        let s = self.state.read(cx);
        let label = s.pane_label(pane);
        let name = s.provider_name(&pane.provider);
        let workspace_name = (s.view() == View::Overview).then(|| s.workspaces.iter().find(|w| w.id == pane.workspace_id).map(|w| w.name.clone())).flatten();
        let keys = |cmd: Command| commands::keys_for(&cmd, s).map_or(String::new(), |k| format!(" ({k})"));
        let (zoom_keys, split_keys, close_keys) = (keys(Command::Zoom(PaneRef::Focused)), keys(Command::Split { dir: crate::model::layout::SplitDir::Row }), keys(Command::Close(PaneRef::Focused)));
        let focused = s.focused_pane_id.as_deref() == Some(&pane.id);
        let progress = (pane.provider != "shell").then(|| s.progress.get(&pane.id).filter(|p| p.percent.is_some()).cloned()).flatten();
        let mono = crate::fonts::mono(cx);
        let id = pane.id.clone();
        let target = PaneRef::Id(id.clone());
        let status = pane.status;
        let renaming = self.sidebar.renaming.as_ref().filter(|(r, _)| *r == pane.id).map(|(_, i)| i.clone());
        let title_tip = format!("{}\nDouble-click to rename", if label == name { name.clone() } else { format!("{name}: {label}") });
        let rename_id = id.clone();
        let title = match renaming {
            Some(input) => div().flex_1().child(Input::new(&input)).into_any_element(),
            None => div().id(SharedString::from(format!("title-{id}"))).flex_1().min_w_0().truncate()
                .text_size(px(13.5)).font_weight(FontWeight::SEMIBOLD).text_color(if focused { c.fg } else { c.muted })
                .tooltip(move |window, cx| Tooltip::new(title_tip.clone()).build(window, cx))
                .on_click(cx.listener(move |this, e: &ClickEvent, window, cx| {
                    if e.click_count() == 2 {
                        this.start_rename(&rename_id, window, cx);
                    }
                }))
                .child(label.clone())
                .into_any_element(),
        };
        let button = move |key: String, icon: IconName, tip: String| {
            div().id(SharedString::from(key)).rounded_sm().p_1().text_color(c.subtle).cursor_pointer()
                .hover(move |d| d.bg(c.hover).text_color(c.fg))
                .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(Icon::new(icon).size(px(13.0)))
        };
        let context = self.context_meter(pane, c, cx);
        let cost = self.pane_cost(pane, c, cx);
        let branch = pane.worktree.as_ref().map(|t| (t.branch.trim_start_matches("winfer/").to_string(), t.path.clone()));
        let (t1, t2, t3, t4, t5, t6, t7) = (target.clone(), target.clone(), target.clone(), target.clone(), target.clone(), target.clone(), target.clone());
        let menu_pane = pane.clone();
        let branch_el = branch.map(|(b, path)| {
            div().id(SharedString::from(format!("branch-{id}"))).flex().max_w(px(160.0)).items_center().gap_1().rounded_sm().px_1p5().py_0p5().font_family(mono.clone()).text_size(px(11.0)).text_color(c.subtle).cursor_pointer()
                .hover(move |d| d.bg(c.hover).text_color(c.fg))
                .tooltip(move |window, cx| Tooltip::new(format!("{path}\nClick to show changes")).build(window, cx))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(move |this, _, window, cx| this.execute(Command::ShowChanges(t1.clone()), window, cx)))
                .child(Icon::new(IconName::GitBranch).size(px(11.0)))
                .child(div().truncate().child(b))
        });
        let model_el = pane.model.clone().map(|m| {
            div().id(SharedString::from(format!("model-{id}"))).max_w(px(112.0)).truncate().rounded_sm().px_1p5().py_0p5().font_family(mono.clone()).text_size(px(11.0)).text_color(c.subtle).cursor_pointer()
                .hover(move |d| d.bg(c.hover).text_color(c.fg))
                .tooltip(|window, cx| Tooltip::new("Model · click to change").build(window, cx))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.execute(Command::Focus(t2.clone()), window, cx);
                    this.execute(Command::Overlay { overlay: Overlay::Model, provider: None }, window, cx);
                }))
                .child(m)
        });
        let url_el = pane.dev_url.clone().map(|u| {
            let open = u.clone();
            div().id(SharedString::from(format!("url-{id}"))).flex().items_center().gap_1().rounded_sm().px_1p5().py_0p5().font_family(mono.clone()).text_size(px(11.0)).text_color(c.accent).cursor_pointer()
                .hover(move |d| d.bg(Hsla { a: 0.15, ..c.accent }))
                .tooltip(move |window, cx| Tooltip::new(format!("Open {u} in the browser panel")).build(window, cx))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(move |this, _, window, cx| this.execute(Command::BrowserOpen { url: open.clone(), new_tab: false }, window, cx)))
                .child("Launch")
                .child(Icon::new(IconName::ExternalLink).size(px(10.0)))
        });
        let menu_button = button(format!("menu-{id}"), IconName::Ellipsis, "More".into()).on_click(cx.listener(move |this, e: &ClickEvent, _, cx| {
            let items = this.header_menu(&menu_pane);
            let at = point(e.position().x - px(240.0), e.position().y + px(10.0));
            this.open_menu(at, None, items, cx);
        }));
        let workspace = workspace_name.clone();
        let restore_tip = format!("Put back into {}", workspace.clone().unwrap_or_else(|| "its workspace".into()));
        let actions: Vec<AnyElement> = if pane.archived {
            vec![div().id(SharedString::from(format!("restore-{id}"))).flex().items_center().gap_1().rounded_sm().px_1p5().py_0p5().text_size(px(11.0)).text_color(c.muted).cursor_pointer()
                .hover(move |d| d.bg(c.hover).text_color(c.fg))
                .tooltip(move |window, cx| Tooltip::new(restore_tip.clone()).build(window, cx))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(move |this, _, window, cx| this.execute(Command::Archive { target: t3.clone(), restore: true }, window, cx)))
                .child(Icon::new(IconName::ArchiveRestore).size(px(12.0)))
                .child("Restore")
                .into_any_element()]
        } else {
            let note_pane = id.clone();
            let has_note = self.state.read(cx).pane_notes.contains_key(&id);
            vec![
                button(format!("note-{id}"), IconName::StickyNote, if has_note { "Edit pane note".into() } else { "Add pane note".into() })
                    .when(has_note, |d| d.text_color(c.accent))
                    .on_click(cx.listener(move |this, e: &ClickEvent, window, cx| this.toggle_pane_note(&note_pane, e.position(), window, cx))).into_any_element(),
                button(format!("archive-{id}"), IconName::Archive, "Archive: keep it running, out of the way".into())
                    .on_click(cx.listener(move |this, _, window, cx| this.execute(Command::Archive { target: t4.clone(), restore: false }, window, cx))).into_any_element(),
                button(format!("zoom-{id}"), if zoomed { IconName::Minimize2 } else { IconName::Maximize2 }, format!("{}{zoom_keys}", if zoomed { "Restore" } else { "Zoom" }))
                    .on_click(cx.listener(move |this, _, window, cx| this.execute(Command::Zoom(t5.clone()), window, cx))).into_any_element(),
                button(format!("split-{id}"), IconName::Plus, format!("Split right{split_keys}"))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.execute(Command::Focus(t6.clone()), window, cx);
                        this.execute(Command::Split { dir: crate::model::layout::SplitDir::Row }, window, cx);
                    })).into_any_element(),
            ]
        };
        let close = button(format!("close-{id}"), IconName::X, format!("Close{close_keys}")).on_click(cx.listener(move |this, _, window, cx| this.execute(Command::Close(t7.clone()), window, cx)));
        let drag_id = id.clone();
        let archived = pane.archived;
        let bar = progress.as_ref().map(|p| progress::progress_bar(Some(p), status, &c).absolute().bottom_0().left_0().right_0().h(px(2.0)));
        let percent = progress.as_ref().and_then(|p| p.percent).filter(|_| status != AgentStatus::Idle);
        let base = div().id(SharedString::from(format!("header-{id}"))).relative().flex_shrink_0()
            .when(!archived, |d| d.on_mouse_down(MouseButton::Left, cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                this.drag = Some(Drag::Pane { id: drag_id.clone(), at: e.position });
                cx.notify();
            })))
            .children(bar);
        // One slim row: index, status, agent, title, progress, then the buttons. Needing input or
        // an error tints it.
        let tint = match status {
            AgentStatus::NeedsInput => Some(Hsla { a: 0.08, ..c.warning }),
            AgentStatus::Error => Some(Hsla { a: 0.08, ..c.danger }),
            _ => None,
        };
        let header = base.h(px(34.0)).flex().items_center().gap_2().pl_2p5().pr_1p5().border_b_1().border_color(c.border).bg(c.raised)
            .when_some(tint, |d, t| d.bg(t))
            .child(div().flex_shrink_0().font_family(mono.clone()).text_size(px(10.5)).text_color(if focused { c.accent } else { c.subtle }).child(index.to_string()))
            .child(div().size(px(7.0)).flex_shrink_0().rounded_full().bg(status_color(status, &c)))
            .child(crate::ui::code_pickers::mark(c, &pane.provider, 13.0, 1.0))
            .children(workspace_name.map(|w| div().max_w(px(112.0)).flex_shrink_0().truncate().rounded_sm().bg(c.hover).px_1p5().text_size(px(10.5)).text_color(c.muted).child(w)))
            .child(title)
            .when_some(percent, |d, n| d.child(div().flex().flex_shrink_0().items_center().gap_1p5().font_family(mono.clone()).text_size(px(11.0)).text_color(c.muted)
                .child(div().w(px(32.0)).h(px(3.0)).rounded_full().bg(c.active).child(div().h_full().rounded_full().w(relative(n as f32 / 100.0)).bg(if status == AgentStatus::NeedsInput { c.warning } else if n >= 100 { c.success } else { c.accent })))
                .child(format!("{n}%"))))
            .children(url_el)
            .child(div().flex().flex_shrink_0().items_center().when(!focused, |d| d.opacity(0.55)).child(menu_button).children(actions).child(close));
        // The status line: the pane's facts, out of the header so the title has room.
        let agent = pane.provider != "shell";
        let line = (agent && (branch_el.is_some() || model_el.is_some() || cost.is_some() || context.is_some())).then(|| {
            div().h(px(26.0)).flex_shrink_0().flex().items_center().gap_2().overflow_hidden().px_2().border_t_1().border_color(c.border)
                .font_family(mono.clone()).text_size(px(10.5)).text_color(c.subtle)
                .children(branch_el)
                .children(context)
                .children(cost)
                .child(div().flex_1())
                .children(model_el)
                .into_any_element()
        });
        let _ = name;
        (header.into_any_element(), line)
    }
}

#[cfg(test)]
mod tests {
    use super::{cost_breakdown, short_tokens};
    use serde_json::json;

    #[test]
    fn short_token_counts() {
        assert_eq!(short_tokens(950.0), "950");
        assert_eq!(short_tokens(84_200.0), "84k");
        assert_eq!(short_tokens(1_240_000.0), "1.2M");
    }

    #[test]
    fn cost_tooltips_list_models() {
        let usage = json!({ "calls": 3, "models": [{ "model": "opus", "priced": true, "cost": 1.2, "input": 1000, "output": 400, "cacheRead": 0, "cacheWrite": 0 }, { "model": "x", "priced": false, "cost": 0, "input": 10, "output": 0, "cacheRead": 0, "cacheWrite": 0 }] });
        assert_eq!(cost_breakdown(&usage, "in this worktree"), "At API prices, 3 model calls in this worktree:\nopus: $1.20 · 1.4K tokens\nx: no known price · 10 tokens");
    }
}

/// ⌘F: the scrollback search box of a pane.
pub(super) struct SearchBox {
    pane: String,
    input: Entity<gpui_kit::component::input::InputState>,
    result: Option<(usize, usize)>,
}

impl Root {
    pub(super) fn sync_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        use gpui_kit::component::input::{InputEvent, InputState};
        let wanted = self.state.read(cx).search_pane_id.clone();
        if self.search.as_ref().map(|s| &s.pane) == wanted.as_ref() {
            return;
        }
        if let Some(old) = self.search.take()
            && let Some(session) = SESSIONS.get(&old.pane)
        {
            session.search("", true);
        }
        self.focused_view = None;
        let Some(pane) = wanted else { return };
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Search scrollback"));
        cx.subscribe_in(&input, window, |this, input, event: &InputEvent, _, cx| {
            let Some(search) = this.search.as_mut() else { return };
            let query = input.read(cx).value().to_string();
            let Some(session) = SESSIONS.get(&search.pane) else { return };
            match event {
                InputEvent::Change => {
                    session.search("", true);
                    search.result = session.search(&query, true);
                }
                InputEvent::PressEnter { shift, .. } => search.result = session.search(&query, !shift),
                _ => return,
            }
            cx.notify();
        })
        .detach();
        input.update(cx, |i, cx| i.focus(window, cx));
        self.search = Some(SearchBox { pane, input, result: None });
    }

    fn step_search(&mut self, forward: bool, cx: &mut Context<Self>) {
        let Some(search) = self.search.as_mut() else { return };
        let query = search.input.read(cx).value().to_string();
        if let Some(session) = SESSIONS.get(&search.pane) {
            search.result = session.search(&query, forward);
        }
        cx.notify();
    }

    pub(super) fn close_search(&mut self, cx: &mut Context<Self>) -> bool {
        if self.state.read(cx).search_pane_id.is_none() {
            return false;
        }
        self.update_state(cx, |s| s.search_pane_id = None);
        true
    }

    pub(super) fn render_search(&self, pane: &str, c: Colors, cx: &mut Context<Self>) -> Option<AnyElement> {
        let search = self.search.as_ref().filter(|s| s.pane == pane)?;
        let mono = crate::fonts::mono(cx);
        let has_query = !search.input.read(cx).value().is_empty();
        let count = match (has_query, search.result) {
            (true, Some((i, n))) if n > 0 => format!("{}/{n}", i + 1),
            (true, Some(_)) => "0/0".into(),
            _ => String::new(),
        };
        let button = |id: &'static str, icon: IconName, tip: &'static str| {
            div().id(id).rounded_sm().p_0p5().text_color(c.subtle).cursor_pointer().hover(move |d| d.text_color(c.fg))
                .tooltip(move |window, cx| Tooltip::new(tip).build(window, cx))
                .child(Icon::new(icon).size(px(13.0)))
        };
        Some(
            div().absolute().top(px(36.0)).right(px(12.0)).flex().items_center().gap_1().rounded_md().border_1().border_color(c.border_strong).bg(c.raised).px_2().py_1()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(div().w(px(176.0)).child(Input::new(&search.input).appearance(false)))
                .child(div().min_w(px(40.0)).text_right().font_family(mono).text_size(px(11.0)).text_color(c.subtle).child(count))
                .child(button("search-prev", IconName::ChevronUp, "Previous (Shift+Enter)").on_click(cx.listener(|this, _, _, cx| this.step_search(false, cx))))
                .child(button("search-next", IconName::ChevronDown, "Next (Enter)").on_click(cx.listener(|this, _, _, cx| this.step_search(true, cx))))
                .child(button("search-close", IconName::X, "Close (Esc)").on_click(cx.listener(|this, _, _, cx| {
                    this.close_search(cx);
                })))
                .into_any_element(),
        )
    }
}
