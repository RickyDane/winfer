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
use winfer_backend::{usage as usage_backend, usage_history};

#[derive(Clone, Debug)]
pub(super) struct BgTask {
    pub id: String,
    pub title: String,
    pub description: Option<String>,
    pub cmd: String,
    pub status: String,
    pub log: Option<String>,
    pub started_at: u64,
}

#[derive(Clone)]
struct HeaderBgItem {
    id: String,
    title: String,
    description: Option<String>,
    agent: Option<String>,
    model: Option<String>,
    last: Option<String>,
    recent_lines: Vec<String>,
    started_at: u64,
    log_file: Option<String>,
    from_code: bool,
}

fn pane_header_cursor(archived: bool, dragging: bool) -> Option<CursorStyle> {
    (!archived).then_some(if dragging { CursorStyle::ClosedHand } else { CursorStyle::OpenHand })
}

fn bg_clock(ms: u64) -> String {
    let s = ms / 1000;
    let h = s / 3600;
    let m = (s % 3600) / 60;
    if h > 0 { format!("{h}:{m:02}:{:02}", s % 60) } else { format!("{m}:{:02}", s % 60) }
}

#[derive(Default)]
pub(super) struct PaneMeters {
    /// How full each pane's context window is (`Context` in usage.ts).
    pub context: HashMap<String, Value>,
    /// What a pane's agent says it spent this session (winfer code).
    pub spend: HashMap<String, Value>,
    /// Logged spend by worktree folder and by agent session.
    pub by_dir: HashMap<String, Value>,
    pub by_session: HashMap<String, Value>,
    seconds: u64,
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

/// A ring filling clockwise with the used share of the context window and its percent (no sign) inside.
pub(crate) fn context_ring(fraction: f64, color: Hsla, text: Hsla, size: f32, mono: SharedString, c: Colors) -> Div {
    let fraction = fraction.clamp(0.0, 1.0) as f32;
    let track = c.border_strong;
    let stroke = (size / 10.0).max(2.0);
    let arcs = canvas(|_, _, _| {}, move |bounds, _, window, _| {
        let center = bounds.center();
        let radius = (bounds.size.width.min(bounds.size.height) - px(stroke)) / 2.0;
        // The track (a full circle), then the used arc from 12 o'clock, clockwise.
        let mut arc = |to: f32, color: Hsla| {
            let steps = ((to * 64.0).ceil() as usize).max(2);
            let mut path = PathBuilder::stroke(px(stroke));
            for i in 0..=steps {
                let angle = to * i as f32 / steps as f32 * std::f32::consts::TAU - std::f32::consts::FRAC_PI_2;
                let p = point(center.x + radius * angle.cos(), center.y + radius * angle.sin());
                if i == 0 { path.move_to(p) } else { path.line_to(p) }
            }
            if let Ok(path) = path.build() {
                window.paint_path(path, color);
            }
        };
        arc(1.0, track);
        if fraction > 0.0 {
            arc(fraction, color);
        }
    }).absolute().size_full();
    // The number spans the ring's full width and is centered by text alignment (on its exact shaped
    // width) rather than as a flex item, whose box width gets rounded to whole pixels and shifts the
    // glyph off the ring's center.
    div().relative().size(px(size)).flex_shrink_0()
        .child(arcs)
        .child(div().absolute().inset_0().flex().flex_col().justify_center().text_center().font_family(mono).text_size(px(size / 3.0)).font_weight(FontWeight::SEMIBOLD).text_color(text).child(format!("{}", (fraction * 100.0).round())))
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
        let mut items = vec![MenuItem::run("Rename…", move |this, window, cx| {
            this.start_rename(&rename, window, cx);
            this.sidebar.rename_in_header = true;
        })];
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
            items.push(MenuItem::commands("Split right", vec![Command::Focus(target.clone()), Command::Split { dir: crate::model::layout::SplitDir::Row }]));
            let note = pane.id.clone();
            items.push(MenuItem::run("Pane note…", move |this, window, cx| {
                let at = window.mouse_position();
                this.toggle_pane_note(&note, at, window, cx)
            }));
            items.push(MenuItem::command("Archive", Command::Archive { target: target.clone(), restore: false }));
        }
        items.push(MenuItem::command("Close", Command::Close(target)));
        items
    }

    /// The Single Spatial Island pane header: one floating frosted capsule inset 8px from the top,
    /// left, and right edges of the pane. Left: provider badge + live status dot and a two-line
    /// identity/metadata stack. Right: live bg tasks, dev server launch, unified telemetry pill,
    /// divider, and action icons.
    pub(super) fn render_header(&self, pane: &Pane, width: f32, zoomed: bool, c: &Colors, cx: &mut Context<Self>) -> AnyElement {
        let c = *c;
        // Collapse by pane width: <340 secondary actions go to the "…" menu, <230 the title and the
        // rest of the actions too, <150 nothing more: avatar, "…" and close stay.
        let (compact, narrow) = (width < 340.0, width < 230.0);
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
        let renaming = self.sidebar.renaming.as_ref().filter(|(r, _)| *r == pane.id && self.sidebar.rename_in_header).map(|(_, i)| i.clone());
        let title_tip = format!("{}\nDouble-click to rename", if label == name { name.clone() } else { format!("{name}: {label}") });
        let rename_id = id.clone();
        let title = match renaming {
            Some(input) => div().flex_1().min_w_0().on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation()).child(Input::new(&input)).into_any_element(),
            None => div().id(SharedString::from(format!("title-{id}"))).flex_1().min_w_0().truncate()
                .text_size(px(13.0)).font_weight(if focused { FontWeight::SEMIBOLD } else { FontWeight::MEDIUM }).text_color(if focused { c.fg } else { c.muted })
                .tooltip(move |window, cx| Tooltip::new(title_tip.clone()).build(window, cx))
                .on_click(cx.listener(move |this, e: &ClickEvent, window, cx| {
                    if e.click_count() == 2 {
                        this.start_rename(&rename_id, window, cx);
                        this.sidebar.rename_in_header = true;
                    }
                }))
                .child(super::title_swap::title("head", &id, &label))
                .into_any_element(),
        };
        let button = move |key: String, icon: IconName, tip: String| {
            div().id(SharedString::from(key)).size(px(24.0)).flex().items_center().justify_center().rounded_md().text_color(c.subtle).cursor_pointer()
                .hover(move |d| d.bg(c.hover).text_color(c.fg))
                .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(Icon::new(icon).size(px(13.0)))
        };
        let pill_bg = if c.light { Hsla { a: 0.055, ..c.fg } } else { Hsla { a: 0.07, ..c.fg } };
        let cost = if compact { None } else { self.pane_cost(pane, c, cx) };
        let branch = pane.worktree.as_ref().map(|t| (t.branch.trim_start_matches("winfer/").to_string(), t.path.clone()));
        let (t1, t2, t3, t4, t5, t6, t7) = (target.clone(), target.clone(), target.clone(), target.clone(), target.clone(), target.clone(), target.clone());
        let menu_pane = pane.clone();
        let branch_el = match branch {
            Some((b, path)) => {
                let teal = crate::theme::hex(if c.light { "#0f766e" } else { "#5eead4" });
                div().id(SharedString::from(format!("branch-{id}"))).flex().max_w(px(176.0)).items_center().gap_1().rounded_full().bg(Hsla { a: 0.11, ..teal }).px_1p5().py(px(0.5)).font_family(mono.clone()).text_size(px(10.5)).text_color(teal).cursor_pointer()
                    .hover(move |d| d.bg(Hsla { a: 0.20, ..teal }))
                    .tooltip(move |window, cx| Tooltip::new(format!("{path}\nClick to show changes")).build(window, cx))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |this, _, window, cx| this.execute(Command::ShowChanges(t1.clone()), window, cx)))
                    .child(Icon::new(IconName::FolderGit2).size(px(11.0)).flex_shrink_0())
                    .child(div().min_w_0().truncate().child(b))
                    .into_any_element()
            }
            None => div().id(SharedString::from(format!("shared-{id}"))).flex().flex_shrink_0().items_center().gap_1().text_size(px(11.0)).text_color(c.subtle)
                .tooltip(|window, cx| Tooltip::new("Works in the project folder, not its own worktree").build(window, cx))
                .child(Icon::new(IconName::GitBranch).size(px(11.0)).flex_shrink_0())
                .child("shared folder")
                .into_any_element(),
        };
        let model_el = pane.model.clone().map(|m| {
            div().flex().min_w_0().items_center().gap_1p5()
                .child(div().text_size(px(10.0)).text_color(Hsla { a: 0.45, ..c.subtle }).child("·"))
                .child(
                    div().id(SharedString::from(format!("model-{id}"))).max_w(px(120.0)).truncate().text_size(px(11.0)).text_color(c.muted).cursor_pointer()
                        .hover(move |d| d.text_color(c.fg))
                        .tooltip(|window, cx| Tooltip::new("Model · click to change").build(window, cx))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.execute(Command::Focus(t2.clone()), window, cx);
                            this.execute(Command::Overlay { overlay: Overlay::Model, provider: None }, window, cx);
                        }))
                        .child(m),
                )
        });
        let url_el = pane.dev_url.clone().map(|u| {
            let open = u.clone();
            div().id(SharedString::from(format!("url-{id}"))).flex().flex_shrink_0().items_center().gap_1().h(px(24.0)).rounded_full().border_1().border_color(Hsla { a: 0.28, ..c.accent }).bg(Hsla { a: 0.14, ..c.accent }).px_2().text_size(px(11.0)).font_weight(FontWeight::MEDIUM).text_color(c.accent).cursor_pointer()
                .hover(move |d| d.bg(Hsla { a: 0.22, ..c.accent }))
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
            vec![div().id(SharedString::from(format!("restore-{id}"))).flex().items_center().gap_1().h(px(24.0)).rounded_full().border_1().border_color(c.border).bg(pill_bg).px_2p5().text_size(px(11.0)).font_weight(FontWeight::MEDIUM).text_color(c.fg).cursor_pointer()
                .hover(move |d| d.bg(c.hover))
                .tooltip(move |window, cx| Tooltip::new(restore_tip.clone()).build(window, cx))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(move |this, _, window, cx| this.execute(Command::Archive { target: t3.clone(), restore: true }, window, cx)))
                .child(Icon::new(IconName::ArchiveRestore).size(px(11.5)))
                .child("Restore")
                .into_any_element()]
        } else {
            let note_pane = id.clone();
            let has_note = self.state.read(cx).pane_notes.contains_key(&id);
            let mut v = vec![
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
            ];
            // The last two go first, then archive and the note: the order the width asks for.
            if narrow { v.drain(2..); }
            if compact { v.drain(..v.len().min(2)); }
            v
        };
        let close = div().id(SharedString::from(format!("close-{id}"))).size(px(24.0)).flex().items_center().justify_center().rounded_md().text_color(c.subtle).cursor_pointer()
            .hover(move |d| d.bg(Hsla { a: 0.14, ..c.danger }).text_color(c.danger))
            .tooltip(move |window, cx| Tooltip::new(format!("Close{close_keys}")).build(window, cx))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(move |this, _, window, cx| this.execute(Command::Close(t7.clone()), window, cx)))
            .child(Icon::new(IconName::X).size(px(13.0)));
        let drag_id = id.clone();
        let archived = pane.archived;
        let workspace_chip = workspace_name.map(|w| {
            div().max_w(px(120.0)).flex_shrink_0().truncate().rounded_full().bg(pill_bg).px_1p5().py(px(0.5)).text_size(px(10.5)).font_weight(FontWeight::MEDIUM).text_color(c.fg).child(w)
        });
        let bar = progress.as_ref().map(|p| {
            // Inset past the rounded corners (radius 12) so the track sits inside the header instead of poking out of it.
            div().absolute().left(px(14.0)).right(px(14.0)).bottom(px(3.0)).h(px(2.0))
                .child(progress::progress_bar(Some(p), status, &c).h_full().bg(Hsla { a: if c.light { 0.08 } else { 0.10 }, ..c.fg }))
        });
        let fill = match status {
            AgentStatus::NeedsInput => crate::theme::blend(c.raised, c.warning, if c.light { 0.14 } else { 0.13 }),
            AgentStatus::Error => crate::theme::blend(c.raised, c.danger, if c.light { 0.12 } else { 0.13 }),
            _ => c.raised,
        };
        let badge_tint = match pane.provider.as_str() {
            "claude" => c.claude,
            "agy" => crate::theme::hex("#4f8cff"),
            "opencode" => crate::theme::hex("#f5b544"),
            "cursor" => crate::theme::hex("#8b5cf6"),
            "pi" | "local" => crate::theme::hex("#22c55e"),
            "muse" => crate::theme::hex("#fb7185"),
            "winfer" => c.accent,
            _ => c.muted,
        };
        let meta_row = match status {
            AgentStatus::NeedsInput => div().flex().min_w_0().items_center().gap_1p5().overflow_hidden().text_size(px(11.0))
                .children(workspace_chip)
                .child(div().min_w_0().truncate().font_family(mono.clone()).text_size(px(10.5)).text_color(c.warning).child("Waiting for your input")),
            AgentStatus::Error => div().flex().min_w_0().items_center().gap_1p5().overflow_hidden().text_size(px(11.0))
                .children(workspace_chip)
                .child(div().min_w_0().truncate().font_family(mono.clone()).text_size(px(10.5)).text_color(c.danger).child("The process stopped with an error")),
            _ => div().flex().min_w_0().items_center().gap_1p5().overflow_hidden().text_size(px(11.0)).text_color(c.subtle)
                .children(workspace_chip)
                .child(branch_el)
                .children(model_el),
        };
        let telemetry = cost.map(|cost| div().flex().flex_shrink_0().items_center().h(px(24.0)).px_2().rounded_full().bg(pill_bg).child(cost));
        let island_shadow = if c.light {
            vec![
                BoxShadow { color: hsla(0.0, 0.0, 0.0, 0.10), offset: point(px(0.0), px(6.0)), blur_radius: px(16.0), spread_radius: px(-6.0), inset: false },
                BoxShadow { color: hsla(0.0, 0.0, 0.0, 0.06), offset: point(px(0.0), px(2.0)), blur_radius: px(5.0), spread_radius: px(-1.0), inset: false },
                BoxShadow { color: hsla(0.0, 0.0, 0.0, 0.04), offset: point(px(0.0), px(1.0)), blur_radius: px(2.0), spread_radius: px(0.0), inset: false },
            ]
        } else {
            vec![
                BoxShadow { color: hsla(0.0, 0.0, 0.0, 0.50), offset: point(px(0.0), px(8.0)), blur_radius: px(22.0), spread_radius: px(-6.0), inset: false },
                BoxShadow { color: hsla(0.0, 0.0, 0.0, 0.30), offset: point(px(0.0), px(2.0)), blur_radius: px(6.0), spread_radius: px(-1.0), inset: false },
                BoxShadow { color: hsla(0.0, 0.0, 0.0, 0.20), offset: point(px(0.0), px(1.0)), blur_radius: px(2.0), spread_radius: px(0.0), inset: false },
            ]
        };
        let bg_chip = self.render_bg_chip(&id, c, cx);
        let group_id = SharedString::from(format!("hdr-grp-{id}"));
        let dragging = matches!(&self.drag, Some(Drag::Pane { id: drag_id, .. }) if *drag_id == id);
        div()
            .id(SharedString::from(format!("header-{id}")))
            .group(group_id.clone())
            .relative()
            .flex_shrink_0()
            .mx_2()
            .mt_2()
            .mb_0()
            .flex()
            .items_center()
            .gap_2p5()
            .px_2()
            .py(px(7.0))
            .rounded_xl()
            .bg(fill)
            .border_1()
            .border_color(c.edge)
            .shadow(island_shadow)
            .when_some(pane_header_cursor(archived, dragging), |d, cursor| d.cursor(cursor))
            .when(!archived, |d| {
                d.on_mouse_down(MouseButton::Left, cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                    gpui_kit::base::GlobalState::suppress_text_selection(cx);
                    this.drag = Some(Drag::Pane { id: drag_id.clone(), start: e.position, at: e.position, active: false });
                    cx.notify();
                }))
            })
            .when(archived, |d| {
                d.on_mouse_down(MouseButton::Left, |_, _, cx| {
                    gpui_kit::base::GlobalState::suppress_text_selection(cx);
                })
            })
            .children(bar)
            // Left: Provider avatar + live status dot
            .child(
                div().relative().size(px(30.0)).flex_shrink_0().flex().items_center().justify_center().rounded_lg()
                    .bg(Hsla { a: if c.light { 0.14 } else { 0.20 }, ..badge_tint })
                    .child(crate::ui::code_pickers::mark(c, &pane.provider, 14.0, 1.0))
                    .child(
                        div().absolute().right(px(-3.0)).bottom(px(-3.0)).rounded_full().bg(fill).p(px(2.0))
                            .child(div().size(px(6.0)).rounded_full().bg(status_color(status, &c))),
                    ),
            )
            // Center: Identity two-line stack (#index + title, then branch/model/alert)
            .child(
                div().flex_1().min_w_0().flex().flex_col()
                    .when(narrow, |d| d.invisible().w(px(0.0)))
                    .child(
                        div().flex().min_w_0().items_center().gap_1p5().line_height(px(16.0))
                            .child(title),
                    )
                    .child(meta_row.line_height(px(14.0))),
            )
            // Right: Live bg tasks, dev launch, telemetry pill, divider, actions
            .child(
                div().flex().flex_shrink_0().items_center().gap_1p5()
                    .children(bg_chip)
                    .children(url_el)
                    .children(telemetry)
                    .when(!narrow, |d| d.child(div().w(px(1.0)).h(px(16.0)).bg(c.border)))
                    .child(
                        div().flex().flex_shrink_0().items_center().gap(px(1.0))
                            .when(!focused, |d| d.opacity(0.48).group_hover(group_id, |d| d.opacity(1.0)))
                            .child(menu_button)
                            .children(actions)
                            .child(close),
                    ),
            )
            .into_any_element()
    }

    /// Updates MCP `bg` task tracking for a pane from `"bg://task"` backend events.
    pub(super) fn on_bg_task(&mut self, payload: &Value, cx: &mut Context<Self>) {
        let pane_id = payload["paneId"].as_str().unwrap_or_default().to_string();
        let id = payload["id"].as_str().unwrap_or_default().to_string();
        let status = payload["status"].as_str().unwrap_or_default().to_string();
        if pane_id.is_empty() || id.is_empty() {
            return;
        }
        let list = self.bg_tasks.entry(pane_id.clone()).or_default();
        if let Some(existing) = list.iter_mut().find(|t| t.id == id) {
            existing.status = status.clone();
            if let Some(t) = payload["title"].as_str().filter(|s| !s.is_empty()) {
                existing.title = t.to_string();
            }
            if let Some(d) = payload["description"].as_str().filter(|s| !s.is_empty()) {
                existing.description = Some(d.to_string());
            }
            if let Some(l) = payload["log"].as_str().filter(|s| !s.is_empty()) {
                existing.log = Some(l.to_string());
            }
        } else if status == "running" {
            list.push(BgTask {
                id,
                title: payload["title"].as_str().unwrap_or("background task").to_string(),
                description: payload["description"].as_str().filter(|s| !s.is_empty()).map(str::to_string),
                cmd: payload["cmd"].as_str().unwrap_or_default().to_string(),
                status,
                log: payload["log"].as_str().filter(|s| !s.is_empty()).map(str::to_string),
                started_at: crate::code::state::now_ms(),
            });
        }
        list.retain(|t| t.status == "running");
        if self.bg_popover.as_ref().is_some_and(|(p, _)| *p == pane_id) && self.running_bg_items(&pane_id, cx).is_empty() {
            self.bg_popover = None;
        }
        cx.notify();
    }

    pub(super) fn close_bg_popover(&mut self, cx: &mut Context<Self>) -> bool {
        if self.bg_popover.take().is_some() {
            cx.notify();
            true
        } else {
            false
        }
    }

    /// Running subagents and background shells for a pane (from both `winfer code` and winfer's MCP `bg` tool).
    fn running_bg_items(&self, pane_id: &str, cx: &App) -> Vec<HeaderBgItem> {
        let mut out = Vec::new();
        let mut seen = std::collections::HashSet::new();
        if let Some(model) = self.code_models.get(pane_id) {
            let st = &model.read(cx).state;
            for t in &st.tasks {
                if t.ev.status.as_deref() != Some("running") {
                    continue;
                }
                seen.insert(t.ev.id.clone());
                let title = t.ev.title.clone().or_else(|| t.ev.description.clone()).unwrap_or_else(|| t.ev.id.clone());
                out.push(HeaderBgItem {
                    id: t.ev.id.clone(),
                    title,
                    description: t.ev.description.clone(),
                    agent: t.ev.agent.clone(),
                    model: t.ev.model.clone(),
                    last: t.ev.last.clone(),
                    recent_lines: t.recent_lines.clone(),
                    started_at: t.started_at,
                    log_file: t.ev.output_file.clone(),
                    from_code: true,
                });
            }
        }
        if let Some(tasks) = self.bg_tasks.get(pane_id) {
            for t in tasks {
                if t.status != "running" || !seen.insert(t.id.clone()) {
                    continue;
                }
                out.push(HeaderBgItem {
                    id: t.id.clone(),
                    title: t.title.clone(),
                    description: t.description.clone(),
                    agent: None,
                    model: None,
                    last: (!t.cmd.is_empty()).then(|| t.cmd.clone()),
                    recent_lines: Vec::new(),
                    started_at: t.started_at,
                    log_file: t.log.clone(),
                    from_code: false,
                });
            }
        }
        out
    }

    /// Compact pill on the pane header showing running subagents and background shells.
    fn render_bg_chip(&self, pane_id: &str, c: Colors, cx: &mut Context<Self>) -> Option<AnyElement> {
        let items = self.running_bg_items(pane_id, cx);
        if items.is_empty() {
            return None;
        }
        let agents = items.iter().filter(|t| t.agent.is_some()).count();
        let shells = items.len() - agents;
        let open = self.bg_popover.as_ref().is_some_and(|(p, _)| p == pane_id);
        let mut parts = Vec::new();
        if agents > 0 {
            parts.push(format!("{agents} subagent{}", if agents == 1 { "" } else { "s" }));
        }
        if shells > 0 {
            parts.push(format!("{shells} background shell{}", if shells == 1 { "" } else { "s" }));
        }
        let tip = format!("{} running\nClick for details", parts.join(" · "));
        let pid = pane_id.to_string();
        let mono = crate::fonts::mono(cx);
        let mut chip = div()
            .id(SharedString::from(format!("bg-chip-{pane_id}")))
            .flex()
            .flex_shrink_0()
            .items_center()
            .gap_1p5()
            .h(px(24.0))
            .pl(px(4.0))
            .pr_2()
            .rounded_full()
            .border_1()
            .border_color(Hsla { a: if open { 0.36 } else { 0.26 }, ..c.accent })
            .bg(Hsla { a: if open { 0.22 } else { 0.13 }, ..c.accent })
            .text_size(px(11.0))
            .font_weight(FontWeight::MEDIUM)
            .text_color(if open { c.fg } else { c.muted })
            .cursor_pointer()
            .hover(move |d| d.bg(Hsla { a: 0.17, ..c.accent }).text_color(c.fg))
            .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(move |this, e: &ClickEvent, _, cx| {
                if this.bg_popover.as_ref().is_some_and(|(p, _)| *p == pid) {
                    this.bg_popover = None;
                } else {
                    this.bg_popover = Some((pid.clone(), e.position()));
                }
                cx.notify();
            }))
            .child(
                div().relative().size(px(16.0)).flex_shrink_0().flex().items_center().justify_center().rounded_full().bg(Hsla { a: 0.18, ..c.accent })
                    .child(Icon::new(if agents > 0 { IconName::Bot } else { IconName::SquareTerminal }).size(px(10.0)).text_color(c.accent))
                    .child(div().absolute().top(px(-1.0)).right(px(-1.0)).size(px(4.5)).rounded_full().bg(c.accent)),
            );
        if agents > 0 {
            chip = chip.child(div().flex().items_center().gap_1()
                .child(div().font_family(mono.clone()).text_color(c.fg).child(agents.to_string()))
                .child(if agents == 1 { "agent" } else { "agents" }));
        }
        if agents > 0 && shells > 0 {
            chip = chip.child(div().size(px(3.0)).rounded_full().bg(Hsla { a: 0.6, ..c.subtle }));
        }
        if shells > 0 {
            chip = chip.child(div().flex().items_center().gap_1()
                .child(div().font_family(mono).text_color(c.fg).child(shells.to_string()))
                .child(if shells == 1 { "shell" } else { "shells" }));
        }
        chip = chip.child(Icon::new(if open { IconName::ChevronUp } else { IconName::ChevronDown }).size(px(11.0)).text_color(c.subtle));
        Some(chip.into_any_element())
    }

    /// Anchored popover listing all running subagents & background shells for a pane, with log viewer and stop actions.
    pub(super) fn render_bg_popover(&mut self, c: &Colors, window: &Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (pane_id, anchor) = self.bg_popover.clone()?;
        let items = self.running_bg_items(&pane_id, cx);
        if items.is_empty() {
            self.bg_popover = None;
            return None;
        }
        let c = *c;
        let mono = crate::fonts::mono(cx);
        let now = crate::code::state::now_ms();
        let agents = items.iter().filter(|t| t.agent.is_some()).count();
        let shells = items.len() - agents;
        let win_w = window.viewport_size().width / px(1.0);
        let width = 380.0_f32.min((win_w - 16.0).max(240.0));
        let left = (anchor.x / px(1.0) - width + 40.0).clamp(8.0, (win_w - width - 8.0).max(8.0));
        let top = (anchor.y / px(1.0) + 14.0).max(40.0);

        let mut head = div().flex().items_center().gap_2().px_3().py_2()
            .child(div().flex_1().text_size(px(12.0)).font_weight(FontWeight::SEMIBOLD).text_color(c.fg).child("In the background"));
        if agents > 0 {
            head = head.child(div().flex().items_center().gap_1().rounded_full().bg(Hsla { a: 0.14, ..c.accent }).px_2().py(px(1.0)).text_size(px(10.5)).font_family(mono.clone()).text_color(c.accent)
                .child(Icon::new(IconName::Bot).size(px(10.5)))
                .child(agents.to_string()));
        }
        if shells > 0 {
            head = head.child(div().flex().items_center().gap_1().rounded_full().bg(Hsla { a: 0.14, ..c.success }).px_2().py(px(1.0)).text_size(px(10.5)).font_family(mono.clone()).text_color(c.success)
                .child(Icon::new(IconName::SquareTerminal).size(px(10.5)))
                .child(shells.to_string()));
        }

        let mut rows = div().id("bg-pop-list").flex().flex_col().gap_1().p_1p5().max_h(px(320.0)).overflow_y_scroll();
        for (i, item) in items.into_iter().enumerate() {
            let is_agent = item.agent.is_some();
            let kind_label = match (&item.agent, &item.model) {
                (Some(a), Some(m)) => format!("{a} · {}", crate::code::names::model_name(m, None)),
                (Some(a), None) => a.clone(),
                (None, _) => "shell".to_string(),
            };
            let elapsed_str = bg_clock(now.saturating_sub(item.started_at));
            let sub_line = if is_agent {
                item.recent_lines.last().cloned().or_else(|| item.last.clone()).unwrap_or_else(|| "starting…".into())
            } else {
                item.description.clone().or_else(|| item.last.clone()).unwrap_or_else(|| "running…".into())
            };
            let (open_pid, open_id, open_log, open_code) = (pane_id.clone(), item.id.clone(), item.log_file.clone(), item.from_code);
            let (stop_pid, stop_id, stop_code) = (pane_id.clone(), item.id.clone(), item.from_code);
            let tip = item.description.clone().map(|d| format!("{d}\nClick to view output")).unwrap_or_else(|| "Click to view output".into());

            rows = rows.child(
                div().id(("bg-row", i)).flex().flex_col().gap_1().rounded_lg().px_2p5().py_2().cursor_pointer().hover(move |d| d.bg(c.hover))
                    .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.bg_popover = None;
                        if open_code {
                            if let Some(model) = this.code_models.get(&open_pid) {
                                let id = open_id.clone();
                                model.update(cx, |m, cx| {
                                    m.state.open_task_log(&id);
                                    m.flush(cx);
                                    cx.notify();
                                });
                            }
                        } else if let Some(path) = &open_log {
                            this.viewer.update(cx, |v, cx| v.open(path.clone(), None, false, cx));
                            this.update_state(cx, |s| {
                                s.browser_open = true;
                                s.side_panel = Some(SidePanel::Viewer);
                            });
                        }
                        cx.notify();
                    }))
                    .child(
                        div().flex().items_center().gap_2()
                            .child(div().size(px(20.0)).flex_shrink_0().flex().items_center().justify_center().rounded_md().bg(Hsla { a: 0.14, ..if is_agent { c.accent } else { c.success } })
                                .child(Icon::new(if is_agent { IconName::Bot } else { IconName::SquareTerminal }).size(px(11.0)).text_color(if is_agent { c.accent } else { c.success })))
                            .child(div().flex_1().min_w_0().truncate().text_size(px(12.5)).font_weight(FontWeight::MEDIUM).text_color(c.fg).child(item.title))
                            .child(div().flex_shrink_0().rounded_sm().bg(c.hover).px_1p5().py(px(1.0)).text_size(px(10.0)).text_color(c.subtle).child(kind_label))
                            .child(div().flex_shrink_0().font_family(mono.clone()).text_size(px(10.5)).text_color(c.subtle).child(elapsed_str))
                            .child(
                                div().id(("bg-stop", i)).flex_shrink_0().flex().items_center().gap_1().h(px(22.0)).px_2().rounded_md().bg(Hsla { a: 0.10, ..c.danger }).text_size(px(10.5)).font_weight(FontWeight::MEDIUM).text_color(c.danger).cursor_pointer()
                                    .hover(move |d| d.bg(Hsla { a: 0.22, ..c.danger }))
                                    .tooltip(|window, cx| Tooltip::new("Stop this background task").build(window, cx))
                                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        if stop_code {
                                            if let Some(model) = this.code_models.get(&stop_pid) {
                                                let id = stop_id.clone();
                                                model.update(cx, |m, cx| m.send(crate::code::protocol::Cmd::StopTask(id), cx));
                                            }
                                        } else {
                                            let _ = winfer_backend::harness::bg::stop(&stop_id);
                                            if let Some(list) = this.bg_tasks.get_mut(&stop_pid) {
                                                list.retain(|t| t.id != stop_id);
                                            }
                                            if this.running_bg_items(&stop_pid, cx).is_empty() {
                                                this.bg_popover = None;
                                            }
                                        }
                                        cx.notify();
                                    }))
                                    .child(Icon::new(IconName::Square).size(px(9.0)))
                                    .child("Stop"),
                            ),
                    )
                    .child(div().pl(px(28.0)).truncate().font_family(mono.clone()).text_size(px(11.0)).text_color(c.subtle).child(sub_line)),
            );
        }

        Some(
            div().id("bg-popover-backdrop").absolute().top_0().left_0().size_full()
                .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                    this.bg_popover = None;
                    cx.notify();
                }))
                .child(
                    div().id("bg-popover").absolute().left(px(left)).top(px(top)).w(px(width)).overflow_hidden().rounded_xl().bg(c.raised).shadow_lg().occlude()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(head)
                        .child(rows),
                )
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{cost_breakdown, pane_header_cursor};
    use gpui_kit::CursorStyle;
    use serde_json::json;

    #[test]
    fn cost_tooltips_list_models() {
        let usage = json!({ "calls": 3, "models": [{ "model": "opus", "priced": true, "cost": 1.2, "input": 1000, "output": 400, "cacheRead": 0, "cacheWrite": 0 }, { "model": "x", "priced": false, "cost": 0, "input": 10, "output": 0, "cacheRead": 0, "cacheWrite": 0 }] });
        assert_eq!(cost_breakdown(&usage, "in this worktree"), "At API prices, 3 model calls in this worktree:\nopus: $1.20 · 1.4K tokens\nx: no known price · 10 tokens");
    }

    #[test]
    fn header_cursor_shows_open_and_closed_hand() {
        assert_eq!(pane_header_cursor(false, false), Some(CursorStyle::OpenHand));
        assert_eq!(pane_header_cursor(false, true), Some(CursorStyle::ClosedHand));
        assert_eq!(pane_header_cursor(true, false), None);
        assert_eq!(pane_header_cursor(true, true), None);
    }
}

#[cfg(test)]
mod drag_selection_tests {
    use gpui_kit::base::{GlobalState, TextSelection};
    use gpui_kit::component::text::TextView;
    use gpui_kit::prelude::FluentBuilder;
    use gpui_kit::{
        AppContext, Context, CursorStyle, InteractiveElement, IntoElement, Modifiers, MouseButton,
        MouseDownEvent, MouseUpEvent, ParentElement, Render, SharedString, Styled, TestAppContext,
        Window, div, point, px,
    };

    struct TwoPaneHarness {
        dragging: bool,
    }

    impl Render for TwoPaneHarness {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let dragging = self.dragging;
            div()
                .relative()
                .size_full()
                .flex()
                .on_mouse_up(
                    MouseButton::Left,
                    cx.listener(|this, _: &MouseUpEvent, _, cx| {
                        this.dragging = false;
                        cx.notify();
                    }),
                )
                .child(
                    div().w(px(300.0)).h_full().flex().flex_col().child(
                        div()
                            .id("header-p1")
                            .h(px(40.0))
                            .cursor(if dragging { CursorStyle::ClosedHand } else { CursorStyle::OpenHand })
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _: &MouseDownEvent, _, cx| {
                                    GlobalState::suppress_text_selection(cx);
                                    this.dragging = true;
                                    cx.notify();
                                }),
                            )
                            .child("Pane 1"),
                    ),
                )
                .child(
                    div().w(px(300.0)).h_full().flex().flex_col().child(
                        TextView::markdown(
                            SharedString::from("pane-2-body"),
                            SharedString::from("Selectable text in the second pane"),
                        )
                        .selectable(true),
                    ),
                )
                .when(dragging, |d| {
                    d.child(
                        div()
                            .id("drag-shield")
                            .absolute()
                            .top_0()
                            .left_0()
                            .size_full()
                            .occlude()
                            .cursor(CursorStyle::ClosedHand)
                            .on_mouse_up(
                                MouseButton::Left,
                                cx.listener(|this, _: &MouseUpEvent, _, cx| {
                                    this.dragging = false;
                                    cx.notify();
                                }),
                            ),
                    )
                })
        }
    }

    #[gpui_kit::test]
    fn dragging_pane_header_does_not_select_other_pane_text(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (_, vcx) = cx.add_window_view(|window, cx| {
            let view = cx.new(|_| TwoPaneHarness { dragging: false });
            gpui_kit::component::Root::new(view, window, cx)
        });
        vcx.run_until_parked();
        vcx.update(|window, cx| {
            let _ = window.draw(cx);
        });

        // Sanity check: dragging directly inside pane 2 selects text.
        vcx.simulate_mouse_down(point(px(305.0), px(12.0)), MouseButton::Left, Modifiers::default());
        vcx.simulate_mouse_move(point(px(480.0), px(12.0)), Some(MouseButton::Left), Modifiers::default());
        vcx.simulate_mouse_up(point(px(480.0), px(12.0)), MouseButton::Left, Modifiers::default());
        vcx.run_until_parked();
        assert!(vcx.update(|window, cx| TextSelection::has_selection(window, cx)));

        // Dragging from pane 1's header across pane 2 must NOT select text in pane 2.
        vcx.simulate_mouse_down(point(px(100.0), px(20.0)), MouseButton::Left, Modifiers::default());
        vcx.simulate_mouse_move(point(px(320.0), px(12.0)), Some(MouseButton::Left), Modifiers::default());
        vcx.simulate_mouse_move(point(px(480.0), px(12.0)), Some(MouseButton::Left), Modifiers::default());
        vcx.simulate_mouse_up(point(px(480.0), px(12.0)), MouseButton::Left, Modifiers::default());
        vcx.run_until_parked();
        assert!(!vcx.update(|window, cx| TextSelection::has_selection(window, cx)));
        assert!(vcx.update(|window, cx| TextSelection::selected_text(window, cx)).is_empty());
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
            div().absolute().top(px(36.0)).right(px(12.0)).flex().items_center().gap_1().rounded_md().border_1().border_color(c.edge).bg(c.raised).px_2().py_1()
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
