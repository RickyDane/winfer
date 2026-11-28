//! Home's app events ("[winfer] Task 7 · …") in its orchestrators' chats: compact one-line rows
//! instead of user bubbles, runs of 3+ folded into one summary row, task orchestrators' reports
//! as expandable cards, the app's own prompts as muted dividers, and "[silent]" answers hidden
//! (their event shows "handled"). Only the rendering changes: the orchestrator got the raw text.

use super::code_view::{DisplayRow, Env, OpenLink, alpha, enter, md_style, toggle_body, toggle_chevron};
use crate::code::home_event::{self, EventKind, HomeEvent};
use crate::code::state::Item;
use crate::theme::Colors;
use gpui_kit::assets::IconName;
use gpui_kit::component::Icon;
use gpui_kit::component::text::TextView;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

/// Runs this many events or more fold into one summary row.
const FOLD: usize = 3;

/// The events of a message Home sent, if `item` is one.
pub(super) fn events_of(item: &Item) -> Option<Vec<HomeEvent>> {
    match item {
        Item::User { text, .. } => home_event::parse_message(text).filter(|e| !e.is_empty()),
        _ => None,
    }
}

fn is_silent_row(row: &DisplayRow, items: &[Item]) -> bool {
    matches!(row, DisplayRow::Item(i) if matches!(items.get(*i), Some(Item::Text { text, streaming: false, .. }) if home_event::is_silent(text)))
}

/// Whether a turn's rows still show something once its "[silent]" texts are gone.
fn says_something(rows: &[DisplayRow], items: &[Item]) -> bool {
    rows.iter().any(|r| match r {
        DisplayRow::Item(i) => matches!(items.get(*i), Some(Item::Text { .. } | Item::Notice { .. } | Item::Handoff { .. })) && !is_silent_row(r, items),
        DisplayRow::Report { .. } | DisplayRow::Working => true,
        DisplayRow::WorkGroup { active, .. } => *active,
        DisplayRow::Events { .. } => true,
    })
}

/// Rewrites a Home chat's rows: event messages become `Events` rows (consecutive ones merged),
/// turns that only answered "[silent]" disappear into their event, other "[silent]" texts go.
pub(super) fn fold(items: &[Item], rows: Vec<DisplayRow>) -> Vec<DisplayRow> {
    // Split into turns: a user message (or nothing, at the top) and the rows after it.
    let mut turns: Vec<(Option<DisplayRow>, Vec<DisplayRow>)> = vec![(None, Vec::new())];
    for r in rows {
        let user = matches!(&r, DisplayRow::Item(i) if matches!(items.get(*i), Some(Item::User { .. })));
        if user {
            turns.push((Some(r), Vec::new()));
        } else {
            turns.last_mut().expect("a turn").1.push(r);
        }
    }
    let mut out = Vec::new();
    let mut run: Vec<(usize, bool)> = Vec::new();
    let flush = |run: &mut Vec<(usize, bool)>, out: &mut Vec<DisplayRow>| {
        if let Some((first, _)) = run.first() {
            out.push(DisplayRow::Events { key: format!("ev-{}", items[*first].key()), events: std::mem::take(run) });
        }
    };
    for (head, body) in turns {
        let event = match &head {
            Some(DisplayRow::Item(i)) => events_of(&items[*i]).map(|evs| (*i, evs.iter().all(HomeEvent::is_divider))),
            _ => None,
        };
        let had_silent = body.iter().any(|r| is_silent_row(r, items));
        let body: Vec<DisplayRow> = body.into_iter().filter(|r| !is_silent_row(r, items)).collect();
        match event {
            Some((ix, false)) => {
                let quiet = !says_something(&body, items);
                run.push((ix, quiet && had_silent));
                if !quiet {
                    flush(&mut run, &mut out);
                    out.extend(body);
                }
            }
            Some((ix, true)) => {
                flush(&mut run, &mut out);
                out.push(DisplayRow::Events { key: format!("ev-{}", items[ix].key()), events: vec![(ix, false)] });
                out.extend(body);
            }
            None => {
                flush(&mut run, &mut out);
                out.extend(head);
                out.extend(body);
            }
        }
    }
    flush(&mut run, &mut out);
    out
}

/// "2 done · 1 needs input · 1 error" for a folded run.
pub(super) fn summary(evs: &[&HomeEvent]) -> String {
    let n = |k: &[EventKind]| evs.iter().filter(|e| k.contains(&e.kind)).count();
    let parts = [
        (n(&[EventKind::Finished, EventKind::Report, EventKind::BgExit]), "done"),
        (n(&[EventKind::NeedsInput]), "needs input"),
        (n(&[EventKind::Error]), "error"),
        (n(&[EventKind::Info]), "note"),
    ];
    let mut s = format!("{} events", evs.len());
    for (count, what) in parts.into_iter().filter(|(c, _)| *c > 0) {
        let what = if what == "error" && count > 1 { "errors" } else if what == "note" && count > 1 { "notes" } else { what };
        s.push_str(&format!(" · {count} {what}"));
    }
    s
}

fn hhmm(ms: u64) -> String {
    if ms == 0 {
        return String::new();
    }
    let secs = (ms / 1000) as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    #[cfg(unix)]
    unsafe { libc::localtime_r(&secs, &mut tm) };
    #[cfg(windows)]
    unsafe { libc::localtime_s(&mut tm, &secs) };
    format!("{:02}:{:02}", tm.tm_hour, tm.tm_min)
}

fn kind_style(c: Colors, e: &HomeEvent) -> (IconName, Hsla) {
    match e.kind {
        EventKind::Finished => (IconName::Check, c.success),
        EventKind::NeedsInput => (IconName::CircleQuestionMark, c.warning),
        EventKind::Error => (IconName::CircleAlert, c.danger),
        EventKind::Report => (IconName::FileText, c.success),
        EventKind::BgExit => (IconName::SquareTerminal, if e.bg.as_ref().is_some_and(|b| b.failed()) { c.danger } else { c.accent }),
        EventKind::Opened => (IconName::LayoutDashboard, c.subtle),
        EventKind::Mention => (IconName::Users, c.subtle),
        EventKind::Info => (IconName::Info, c.subtle),
    }
}

fn badge(env: &Env, e: &HomeEvent) -> Div {
    let (icon, color) = kind_style(env.c, e);
    div().size(px(18.0)).flex_shrink_0().flex().items_center().justify_center().rounded(px(5.0)).bg(alpha(color, 0.12)).child(Icon::new(icon).size(px(11.0)).text_color(color))
}

fn chip(env: &Env, icon: Option<IconName>, text: &str) -> Div {
    let c = env.c;
    div().h(px(18.0)).px(px(6.0)).flex().flex_shrink_0().items_center().gap_1().rounded(px(5.0)).bg(c.hover).text_size(px(10.5)).text_color(c.muted).font_family(env.mono.clone())
        .children(icon.map(|i| Icon::new(i).size(px(10.0)).text_color(c.subtle)))
        .child(text.to_string())
}

fn machine_chip(text: &str) -> Div {
    let sky = hsla(199.0 / 360.0, 0.89, 0.60, 1.0);
    div().h(px(18.0)).px(px(6.0)).flex().flex_shrink_0().items_center().gap_1().rounded(px(5.0)).bg(alpha(sky, 0.12)).text_size(px(10.5)).text_color(sky)
        .child(Icon::new(IconName::Server).size(px(11.0)).text_color(sky))
        .child(text.to_string())
}

/// An accent link that opens `href` through the chat's link handling (winfer://task/7, …).
fn link(env: &Env, id: String, label: String, href: String, bold: bool) -> Stateful<Div> {
    let (c, view) = (env.c, env.view.clone());
    div().id(SharedString::from(id)).flex_shrink_0().cursor_pointer().text_color(c.accent).when(bold, |d| d.font_weight(FontWeight::SEMIBOLD)).hover(|d| d.underline())
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_click(move |_, _, cx| {
            let href = href.clone();
            let _ = view.update(cx, |_, cx| cx.emit(OpenLink(href)));
        })
        .child(label)
}

fn task_href(task: &str) -> String {
    format!("winfer://task/{}", task.trim_start_matches("Task ").trim())
}

/// One event as a line: kind badge, task link, chips, what happened, action, "handled", time.
fn event_row(env: &Env, id: &str, e: &HomeEvent, handled: bool, at: u64) -> AnyElement {
    let c = env.c;
    let mut row = div().id(SharedString::from(format!("evr-{id}"))).w_full().min_h(px(26.0)).px_2().flex().items_center().gap(px(7.0)).rounded_lg().text_size(px(12.0 * env.scale)).text_color(c.muted).hover(move |d| d.bg(c.hover))
        .child(badge(env, e));
    if let Some(t) = &e.task {
        row = row.child(link(env, format!("evt-{id}"), t.clone(), task_href(t), true));
    }
    match (e.machine(), e.place.as_deref()) {
        (Some(m), Some(p)) if p.starts_with("its orchestrator on ") => row = row.child(div().flex_shrink_0().child("orchestrator on")).child(machine_chip(m)),
        (Some(_), Some(p)) => row = row.child(machine_chip(p)),
        (None, Some(p)) => row = row.child(chip(env, Some(IconName::Terminal), p)),
        _ => {}
    }
    if let Some(a) = &e.agent {
        row = row.child(chip(env, None, a));
    }
    if let Some(b) = &e.branch {
        row = row.child(chip(env, Some(IconName::GitBranch), b));
    }
    let detail = e.detail.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim().to_string();
    let what = match e.kind {
        EventKind::Finished => "finished",
        EventKind::NeedsInput => "needs input",
        EventKind::Error => "hit an error",
        _ => "",
    };
    match e.kind {
        EventKind::BgExit => {
            if let Some(b) = &e.bg {
                let label = if b.name.is_empty() { b.cmd.lines().next().unwrap_or("").to_string() } else { format!("“{}”", b.name) };
                row = row.child(chip(env, Some(IconName::SquareTerminal), "background task")).child(div().min_w_0().truncate().child(label));
                match (b.code, b.secs) {
                    (Some(code), Some(s)) => {
                        let color = if code == 0 { c.success } else { c.danger };
                        row = row.child(div().h(px(18.0)).px(px(6.0)).flex().flex_shrink_0().items_center().rounded(px(5.0)).bg(alpha(color, 0.10)).text_color(color).text_size(px(10.5)).font_family(env.mono.clone()).child(format!("exit {code}")))
                            .child(chip(env, None, &format!("{s}s")));
                    }
                    _ => row = row.child(div().flex_shrink_0().child("stopped")),
                }
            }
        }
        EventKind::Info => row = row.child(div().min_w_0().truncate().child(e.summary())),
        _ => {
            row = row.child(div().flex_shrink_0().child(format!("{what}{}", if detail.is_empty() { "" } else { ":" })));
            if !detail.is_empty() {
                row = row.child(div().min_w_0().truncate().text_color(c.fg).child(detail));
            }
        }
    }
    row = row.child(div().flex_1());
    // Answer: the agent's pane ("api/2" → winfer://pane/api/2).
    if e.kind == EventKind::NeedsInput
        && let Some(p) = e.place.as_deref().filter(|p| p.contains('/') && e.machine().is_none())
    {
        row = row.child(link(env, format!("eva-{id}"), "Answer".into(), format!("winfer://pane/{p}"), false).text_size(px(11.0)));
    }
    if handled {
        row = row.child(div().id(SharedString::from(format!("evh-{id}"))).flex().flex_shrink_0().items_center().gap_0p5().text_size(px(10.5)).text_color(c.subtle)
            .child(Icon::new(IconName::Check).size(px(10.0)).text_color(c.subtle)).child("handled")
            .tooltip(|window, cx| Tooltip::new("The orchestrator handled it without a reply").build(window, cx)));
    }
    let raw = e.raw.clone();
    row.child(div().flex_shrink_0().pl_2().text_size(px(11.0)).text_color(c.subtle).child(hhmm(at)))
        .tooltip(move |window, cx| Tooltip::new(raw.chars().take(600).collect::<String>()).build(window, cx))
        .into_any_element()
}

/// A task orchestrator's report: "Task 7 · report", the first lines, expandable to the full markdown.
fn report_card(env: &Env, id: &str, e: &HomeEvent, handled: bool, at: u64) -> AnyElement {
    let c = env.c;
    let key = format!("evrep-{id}");
    let open = env.open(&key, false);
    let toggled = env.toggled.get(&key).copied();
    let (view, k) = (env.view.clone(), key.clone());
    let task = e.task.clone().unwrap_or_else(|| "Task".into());
    let sub = match &e.workspace {
        Some(ws) => ws.clone(),
        None => "from the other machine".into(),
    };
    let head = div().id(SharedString::from(format!("evrh-{id}"))).flex().items_center().gap_2().px_2p5().py_2().cursor_pointer().hover(move |d| d.bg(c.hover))
        .on_click(move |_, _, cx| {
            let _ = view.update(cx, |v, cx| v.toggle(&k, open, cx));
        })
        .child(badge(env, e))
        .child(link(env, format!("evt-{id}"), task.clone(), task_href(&task), true))
        .child(div().flex_shrink_0().text_color(c.fg).font_weight(FontWeight::SEMIBOLD).child("· report"))
        .child(div().min_w_0().truncate().text_size(px(11.5)).text_color(c.subtle).child(sub))
        .child(div().flex_1())
        .when(handled, |d| d.child(div().text_size(px(10.5)).text_color(c.subtle).child("handled")))
        .child(div().text_size(px(11.0)).text_color(c.subtle).child(hhmm(at)))
        .child(toggle_chevron(&key, open, toggled, c.subtle));
    let body_md = if open || matches!(toggled, Some((_, false))) {
        e.detail.clone()
    } else {
        e.detail.lines().filter(|l| !l.trim().is_empty()).take(3).collect::<Vec<_>>().join("\n")
    };
    let md = TextView::markdown(SharedString::from(format!("cw-evmd-{id}-{open}")), SharedString::from(body_md)).style(md_style(c)).selectable(open).on_link_click({
        let view = env.view.clone();
        move |href, _, _, cx| {
            let href = href.to_string();
            let _ = view.update(cx, |_, cx| cx.emit(OpenLink(href)));
        }
    });
    let body = div().pl(px(36.0)).pr_3().pb_2p5().text_size(px(12.5 * env.scale)).line_height(relative(1.55)).text_color(c.muted).child(md)
        .when(!open, |d| d.max_h(px(64.0)).overflow_hidden());
    div().w_full().flex().flex_col().rounded_xl().border_1().border_color(c.edge).bg(c.raised).overflow_hidden().text_size(px(12.5 * env.scale))
        .child(head)
        .child(if toggled.is_some() { toggle_body(&key, toggled, body) } else { body.into_any_element() })
        .into_any_element()
}

/// An app prompt ("The user opened Home …"): a muted divider; hovering shows the full text.
fn divider(env: &Env, id: &str, e: &HomeEvent) -> AnyElement {
    let c = env.c;
    let (icon, _) = kind_style(c, e);
    let raw = e.raw.clone();
    div().w_full().flex().items_center().gap_3().py_1p5().text_size(px(11.0)).text_color(c.subtle)
        .child(div().h(px(1.0)).flex_1().bg(c.border))
        .child(div().id(SharedString::from(format!("evd-{id}"))).flex().items_center().gap_1().child(Icon::new(icon).size(px(11.0)).text_color(c.subtle)).child(e.summary())
            .tooltip(move |window, cx| Tooltip::new(raw.chars().take(600).collect::<String>()).build(window, cx)))
        .child(div().h(px(1.0)).flex_1().bg(c.border))
        .into_any_element()
}

fn one(env: &Env, id: &str, e: &HomeEvent, handled: bool, at: u64) -> AnyElement {
    if e.is_divider() {
        divider(env, id, e)
    } else if e.kind == EventKind::Report {
        report_card(env, id, e, handled, at)
    } else {
        event_row(env, id, e, handled, at)
    }
}

/// An `Events` row: its events one per line, or (3 or more) folded behind a summary row.
pub(super) fn events_row(env: &Env, items: &[Item], key: &str, entries: &[(usize, bool)]) -> AnyElement {
    let c = env.c;
    let mut evs: Vec<(String, HomeEvent, bool, u64)> = Vec::new();
    for (ix, handled) in entries {
        let Some(item) = items.get(*ix) else { continue };
        let at = if let Item::User { at, .. } = item { *at } else { 0 };
        for (n, e) in events_of(item).unwrap_or_default().into_iter().enumerate() {
            evs.push((format!("{}-{n}", item.key()), e, *handled, at));
        }
    }
    let lines = |evs: &[(String, HomeEvent, bool, u64)]| evs.iter().map(|(id, e, h, at)| one(env, id, e, *h, *at)).collect::<Vec<_>>();
    let counted = evs.iter().filter(|(_, e, ..)| !e.is_divider()).count();
    let wrap = div().w_full().flex().flex_col().gap(px(3.0)).py_1();
    if counted < FOLD {
        return enter(env, key, wrap.children(lines(&evs)));
    }
    let open = env.open(key, false);
    let toggled = env.toggled.get(key).copied();
    let (view, k) = (env.view.clone(), key.to_string());
    let refs: Vec<&HomeEvent> = evs.iter().map(|(_, e, ..)| e).collect();
    let dots = div().flex().items_center().gap(px(3.0)).children(refs.iter().take(8).map(|e| div().size(px(6.0)).rounded_full().bg(kind_style(c, e).1)));
    let urgent = refs.iter().filter(|e| matches!(e.kind, EventKind::NeedsInput | EventKind::Error)).count();
    let times = (hhmm(evs.first().map_or(0, |e| e.3)), hhmm(evs.last().map_or(0, |e| e.3)));
    let span = if times.0 == times.1 { times.0 } else { format!("{}–{}", times.0, times.1) };
    let head = div().id(SharedString::from(format!("evg-{key}"))).w_full().min_h(px(26.0)).px_2().flex().items_center().gap(px(7.0)).rounded_lg().cursor_pointer().text_size(px(12.0 * env.scale)).text_color(c.muted).hover(move |d| d.bg(c.hover))
        .on_click(move |_, _, cx| {
            let _ = view.update(cx, |v, cx| v.toggle(&k, open, cx));
        })
        .child(toggle_chevron(key, open, toggled, c.subtle))
        .child(dots)
        .child(div().min_w_0().truncate().text_color(if urgent > 0 { c.fg } else { c.muted }).child(summary(&refs)))
        .child(div().flex_1())
        .child(div().flex_shrink_0().text_size(px(11.0)).text_color(c.subtle).child(span));
    let show = open || matches!(toggled, Some((_, false)));
    enter(env, key, wrap.child(head).when(show, |d| {
        let body = div().ml(px(14.0)).pl_2().border_l_1().border_color(c.border).flex().flex_col().gap(px(3.0)).children(lines(&evs));
        d.child(toggle_body(key, toggled, body))
    }))
}

#[cfg(test)]
mod tests {
    // Not `super::*`: gpui_kit's `test` attribute would shadow the std one.
    use super::{DisplayRow, HomeEvent, Item, fold, home_event, summary};

    fn user(k: &str, text: &str) -> Item {
        Item::User { key: k.into(), text: text.into(), at: 1, images: Vec::new() }
    }
    fn said(k: &str, text: &str) -> Item {
        Item::Text { key: k.into(), text: text.into(), streaming: false }
    }
    fn tool(k: &str) -> Item {
        Item::Tool { key: k.into(), name: "read_pane".into(), title: String::new(), input: serde_json::Value::Null, output: String::new(), ok: Some(true), diff: None, started: 1, ended: Some(2) }
    }
    fn rows(items: &[Item]) -> Vec<DisplayRow> {
        fold(items, super::super::code_view::build_display_rows(items, false, true))
    }

    #[test]
    fn silent_turns_fold_into_their_event() {
        let items = [
            user("u0", "hi"),
            said("t0", "hello"),
            user("e1", "[winfer] Task 1 · a/1 (c) finished."),
            tool("x1"),
            said("t1", "[silent]"),
            user("e2", "[winfer] Task 2 · a/2 (c) needs input: Go?"),
            said("t2", "Task 2 asks whether to go."),
        ];
        assert_eq!(
            rows(&items),
            vec![
                DisplayRow::Item(0),
                DisplayRow::Item(1),
                DisplayRow::Events { key: "ev-e1".into(), events: vec![(2, true), (5, false)] },
                DisplayRow::Item(6),
            ]
        );
    }

    #[test]
    fn real_messages_split_runs_and_silent_text_hides() {
        let items = [user("e1", "[winfer] x:1 (c) finished."), said("t1", "[silent]"), user("u", "next"), said("t", "[silent]")];
        assert_eq!(rows(&items), vec![DisplayRow::Events { key: "ev-e1".into(), events: vec![(0, true)] }, DisplayRow::Item(2)]);
    }

    #[test]
    fn prompts_are_dividers_of_their_own() {
        let items = [user("e1", "[winfer] a:1 (c) finished."), user("p", "[winfer] The user opened Home. Brief."), said("t", "- a")];
        assert_eq!(
            rows(&items),
            vec![DisplayRow::Events { key: "ev-e1".into(), events: vec![(0, false)] }, DisplayRow::Events { key: "ev-p".into(), events: vec![(1, false)] }, DisplayRow::Item(2)]
        );
    }

    #[test]
    fn summary_counts_kinds() {
        let evs: Vec<HomeEvent> = ["[winfer] a:1 (c) finished.", "[winfer] Task 2 (w): r", "[winfer] Task 3 · a/3 (c) needs input: q", "[winfer] a:2 (c) hit an error.", "[winfer] b:1 (c) hit an error."]
            .iter()
            .map(|t| home_event::parse(t).unwrap())
            .collect();
        assert_eq!(summary(&evs.iter().collect::<Vec<_>>()), "5 events · 2 done · 1 needs input · 2 errors");
    }
}
