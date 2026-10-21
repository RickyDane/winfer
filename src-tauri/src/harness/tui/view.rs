//! Drawing: transcript, welcome screen, plan, activity line, prompt or approval, status bar
//! (with running subagents), completions and pickers.
//!
//! Style: minimal silver on black, a nebula gradient (blue → violet → pink) for accents,
//! moon phases for progress, and motion that is always a pure function of time.

use std::time::{Duration, Instant};

use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block as Frame_, BorderType, Borders, Clear, Paragraph};
use unicode_width::UnicodeWidthStr;

use super::app::App;
use super::transcript::{Block, NoticeKind, Status, diff_lines, truncate};
use super::{anim, theme};
use crate::harness::event::Mode;

const LOGO: [&str; 2] = ["█ █ █ █ █▀▄ █▀▀ █▀▀ █▀▄", "▀▄▀▄▀ █ █ █ █▀  ██▄ █▀▄"];

/// Left margin of the transcript.
const MARGIN: u16 = 2;

pub fn draw(f: &mut Frame, app: &mut App) {
    app.poll_shell_output();
    let area = f.area();
    let now = Instant::now();
    let t = now.duration_since(app.epoch);
    let width = area.width as usize;
    app.refresh_agents();

    // Bottom-up layout.
    let agents = agents_line(app, width, t);
    let status_h = 1 + u16::from(agents.is_some());
    let (bottom_lines, cursor) = match app.approvals.front() {
        None => composer_lines(app, width, t),
        Some(a) if !a.questions.is_empty() => (question_lines(app, width, t), None),
        Some(_) => (approval_lines(app, width, t), None),
    };
    let bottom_h = (bottom_lines.len() as u16).min(area.height.saturating_sub(4));
    let activity_h = 1u16;
    // The plan goes in a right sidebar when there's room (and it's on); else above the prompt.
    let sidebar_w = sidebar_width(app, area.width);
    let todo_lines = if sidebar_w.is_some() { Vec::new() } else { todo_lines(app, width, t) };
    let todo_h = (todo_lines.len() as u16).min(area.height / 3);
    let queue_lines: Vec<Line> = app
        .queue
        .iter()
        .take(3)
        .map(|m| {
            Line::from(vec![
                Span::styled("  ↳ ", theme::fg(theme::VIOLET())),
                Span::styled(truncate(m.text.lines().next().unwrap_or(""), width.saturating_sub(16)), theme::muted()),
                Span::styled("  queued", theme::dim()),
            ])
        })
        .collect();
    let queue_h = queue_lines.len() as u16;
    let fixed = status_h + bottom_h + activity_h + todo_h + queue_h;
    let main_h = area.height.saturating_sub(fixed);

    let main = Rect { x: area.x, y: area.y, width: area.width, height: main_h };
    let mut y = main.y + main_h;
    let todo_area = Rect { x: area.x, y, width: area.width, height: todo_h };
    // Clicking the plan (below its spacer row) opens or collapses it.
    app.plan_rows = (todo_h > 1).then_some((y + 1, y + todo_h));
    y += todo_h;
    let queue_area = Rect { x: area.x, y, width: area.width, height: queue_h };
    y += queue_h;
    let activity_area = Rect { x: area.x, y, width: area.width, height: activity_h };
    y += activity_h;
    let bottom_area = Rect { x: area.x, y, width: area.width, height: bottom_h };
    y += bottom_h;
    let status_area = Rect { x: area.x, y, width: area.width, height: status_h };

    let (content, side) = match sidebar_w {
        Some(w) => (Rect { width: main.width - w, ..main }, Some(Rect { x: main.x + main.width - w, width: w, ..main })),
        None => (main, None),
    };
    if app.items.is_empty() {
        app.view = Default::default();
        welcome(f, app, content, t);
    } else {
        transcript(f, app, content, now);
    }
    if let Some(side) = side {
        sidebar(f, app, side, t);
    }
    f.render_widget(Paragraph::new(todo_lines), todo_area);
    f.render_widget(Paragraph::new(queue_lines), queue_area);
    f.render_widget(Paragraph::new(activity(app, width, t)), activity_area);
    f.render_widget(Paragraph::new(bottom_lines), bottom_area);
    if let Some((cx, cy)) = cursor {
        if app.picker.is_none() && !app.btw_showing() {
            f.set_cursor_position(Position { x: bottom_area.x + cx, y: bottom_area.y + cy });
        }
    }
    let mut status_lines = Vec::from_iter(agents);
    status_lines.push(status(app, width));
    f.render_widget(Paragraph::new(status_lines), status_area);
    completion(f, app, main);
    picker(f, app, area, t);
    btw(f, app, area, t);
}

/// Tool calls and thinking are "steps": consecutive steps sit together as one timeline.
fn is_step(b: &Block) -> bool {
    matches!(b, Block::Tool { .. } | Block::Reasoning { .. })
}

/// The rendered transcript up to the first item that is still changing, kept between frames
/// so a long conversation isn't re-rendered (and copied) on every animation tick.
#[derive(Default)]
pub struct TranscriptCache {
    width: usize,
    expanded: bool,
    /// Version of each cached item (see `Item::version`).
    versions: Vec<u64>,
    lines: std::sync::Arc<Vec<Line<'static>>>,
    rows: Vec<usize>,
    prev_step: Option<bool>,
}

/// Appends an item's lines (with a blank line between blocks, none between consecutive steps).
fn push_item(lines: &mut Vec<Line<'static>>, rows: &mut Vec<usize>, prev_step: &mut Option<bool>, base: usize, item_lines: Vec<Line<'static>>, step: bool) {
    if item_lines.is_empty() {
        rows.push(usize::MAX);
        return;
    }
    if prev_step.is_some_and(|p| !(p && step)) {
        lines.push(Line::default());
    }
    *prev_step = Some(step);
    rows.push(base + lines.len());
    lines.extend(item_lines);
}

fn transcript(f: &mut Frame, app: &mut App, area: Rect, now: Instant) {
    let width = (area.width.saturating_sub(MARGIN * 2) as usize).max(10);
    let expanded = app.expanded;
    let epoch = app.epoch;
    let n = app.items.len();
    // Items before the first one still animating are settled and can be cached.
    let settled = app.items.iter().position(|i| i.animated()).unwrap_or(n);
    let cache = &mut app.tcache;
    let reusable = cache.width == width
        && cache.expanded == expanded
        && cache.versions.len() <= settled
        && cache.versions.iter().zip(&app.items).all(|(v, i)| *v == i.version);
    if !reusable {
        *cache = TranscriptCache { width, expanded, ..Default::default() };
    }
    if cache.versions.len() < settled {
        let head = std::sync::Arc::make_mut(&mut cache.lines);
        for item in &mut app.items[cache.versions.len()..settled] {
            let item_lines = item.lines(width, expanded, now, epoch);
            push_item(head, &mut cache.rows, &mut cache.prev_step, 0, item_lines, is_step(&item.block));
            cache.versions.push(item.version);
        }
    }
    // The tail is drawn fresh.
    let head_len = cache.lines.len();
    let mut rows = cache.rows.clone();
    let mut prev_step = cache.prev_step;
    let mut tail: Vec<Line<'static>> = Vec::new();
    for item in &mut app.items[settled..] {
        let item_lines = item.lines(width, expanded, now, epoch);
        push_item(&mut tail, &mut rows, &mut prev_step, head_len, item_lines, is_step(&item.block));
    }
    let lines = super::selection::Lines { head: cache.lines.clone(), tail };

    let h = area.height as usize;
    let total = lines.len();
    // A clicked card keeps its header on the same row: it opens downward, pushing what follows
    // past the bottom. Only a card that itself runs past the bottom lifts, just enough to show it.
    if let Some((idx, row)) = app.anchor.take() {
        if let Some(&r) = rows.get(idx).filter(|r| **r != usize::MAX && h > 0) {
            let card_end = rows[idx + 1..].iter().copied().find(|&r| r != usize::MAX).unwrap_or(total);
            let end = (r + h - row.min(h - 1)).max(card_end.min(r + h));
            app.scroll = total.saturating_sub(end);
            app.hold = app.scroll;
        }
    }
    app.hold = app.hold.min(app.scroll);
    let max_scroll = total.saturating_sub(h).max(app.hold.min(total.saturating_sub(1)));
    app.scroll = app.scroll.min(max_scroll);
    let end = total - app.scroll;
    let start = end.saturating_sub(h);
    let visible: Vec<Line> = (start..end).map(|i| lines[i].clone()).collect();
    // Bottom-align short transcripts so the conversation sits by the prompt.
    let pad = h.saturating_sub(visible.len()) as u16;
    let shown = visible.len();
    let inner = Rect { x: area.x + MARGIN, y: area.y + pad, width: area.width.saturating_sub(MARGIN), height: area.height - pad };
    f.render_widget(Paragraph::new(visible), inner);
    // Highlight the mouse selection.
    if let Some(sel) = &app.selection {
        let buf = f.buffer_mut();
        for r in 0..shown {
            let Some((from, to)) = sel.cols(start + r) else { continue };
            let lw = lines[start + r].width();
            // Highlight up to the end of each line's text.
            let to = if to == usize::MAX { lw.max(1) } else { to.min(lw.max(from + 1)) };
            for c in from..to.min(inner.width as usize) {
                if let Some(cell) = buf.cell_mut((inner.x + c as u16, inner.y + r as u16)) {
                    cell.set_bg(theme::SELECTION());
                }
            }
        }
    }
    app.item_rows = rows;
    app.view = super::selection::View { lines, top: area.y, bottom: area.y + area.height, x: inner.x, y: inner.y, start, shown };
    if app.scroll > 0 {
        let line = Line::from(vec![
            Span::styled(" ↓ ", theme::fg(theme::VIOLET())),
            Span::styled(format!("{} more", app.scroll), theme::fg(theme::MUTED())),
            Span::styled("  ctrl+l ", theme::fg(theme::DIM())),
        ]);
        let w = line.width() as u16;
        let r = Rect { x: area.x + area.width.saturating_sub(w + 2), y: area.y + area.height.saturating_sub(1), width: w, height: 1 };
        f.render_widget(Clear, r);
        f.render_widget(Paragraph::new(line), r);
    }
}

fn welcome(f: &mut Frame, app: &App, area: Rect, t: Duration) {
    let accent = theme::backend(app.kind);
    let mut lines: Vec<Line> = Vec::new();
    for row in LOGO {
        lines.push(Line::from(anim::nebula_text(row, t, Style::default())).centered());
    }
    lines.push(Line::default());
    lines.push(Line::from(vec![Span::styled("winfer code", theme::fg(theme::MUTED())), Span::styled(format!("  v{}", env!("CARGO_PKG_VERSION")), theme::fg(theme::DIM()))]).centered());
    lines.push(Line::default());
    lines.push(Line::default());
    let model = if app.actual_model.is_empty() { app.model.clone() } else { app.actual_model.clone() };
    let mut who = vec![Span::styled("● ", theme::fg(accent)), Span::styled(app.kind.label(), theme::fg(theme::FG()))];
    if !model.is_empty() {
        who.push(Span::styled(format!("  {model}"), theme::muted()));
    }
    if let Some(e) = &app.config.effort {
        who.push(Span::styled(format!(" · {e}"), theme::dim()));
    }
    who.push(Span::styled(" · ", theme::dim()));
    who.push(Span::styled(app.config.mode.label().to_string(), theme::fg(mode_color(app.config.mode).unwrap_or(theme::DIM()))));
    lines.push(Line::from(who).centered());
    let dir = crate::harness::tools::short_path(&app.cwd.to_string_lossy());
    let mut whereat = vec![Span::styled(dir, theme::dim())];
    if let Some(b) = &app.branch {
        whereat.push(Span::styled(format!("  ⎇ {b}"), theme::dim()));
    }
    lines.push(Line::from(whereat).centered());
    lines.push(Line::default());
    lines.push(Line::default());
    // Key hints, wrapped onto as many centered rows as the width needs.
    let mut row: Vec<Span<'static>> = Vec::new();
    for (k, d) in [("/", "commands"), ("@", "files"), ("!", "shell"), ("ctrl+t", "model"), ("⇧tab", "mode")] {
        let hint = theme::key_hint(k, d);
        let w: usize = row.iter().chain(hint.iter()).map(|s| s.content.width()).sum::<usize>() + 4;
        if !row.is_empty() && w + 4 > area.width as usize {
            lines.push(Line::from(std::mem::take(&mut row)).centered());
        }
        if !row.is_empty() {
            row.push(Span::raw("    "));
        }
        row.extend(hint);
    }
    lines.push(Line::from(row).centered());

    // Everything drifts up and fades in on launch.
    let k = (t.as_secs_f32() / 0.9).min(1.0);
    anim::fade(&mut lines, k);
    let h = lines.len() as u16;
    let rise = ((1.0 - anim::ease_out(k)) * 2.0).round() as u16;
    let y = area.y + area.height.saturating_sub(h) / 2 + rise;
    let text_rows = y..y + h;

    // Starfield behind, kept off the text rows.
    let buf = f.buffer_mut();
    for (x, sy, g, c) in anim::stars(area.width, area.height, t) {
        let yy = area.y + sy;
        if text_rows.contains(&yy) {
            continue;
        }
        if let Some(cell) = buf.cell_mut((area.x + x, yy)) {
            cell.set_symbol(g).set_fg(c);
        }
    }
    let room = (area.y + area.height).saturating_sub(y);
    f.render_widget(Paragraph::new(lines), Rect { x: area.x, y, width: area.width, height: h.min(room) });
}

fn mode_color(mode: Mode) -> Option<ratatui::style::Color> {
    match mode {
        Mode::Ask => None,
        Mode::AutoEdit => Some(theme::GREEN()),
        Mode::Plan => Some(theme::BLUE()),
        Mode::YoloPlan => Some(theme::VIOLET()),
        Mode::Yolo => Some(theme::RED()),
    }
}

/// How long a finished plan stays up before it's cleared away.
const PLAN_LINGER: Duration = Duration::from_secs(4);

/// Whether there's a plan to show: some of it is still open, or it was just finished.
fn plan_visible(app: &App) -> bool {
    !app.todos.is_empty()
        && (app.todos.iter().any(|t| t.status != "completed") || app.plan_done.is_some_and(|d| d.elapsed() < PLAN_LINGER))
}

/// Terminals at least this wide get the plan as a sidebar.
const SIDEBAR_MIN_WIDTH: u16 = 120;

/// The sidebar's width, when it's shown.
fn sidebar_width(app: &App, width: u16) -> Option<u16> {
    (app.config.plan_sidebar && plan_visible(app) && width >= SIDEBAR_MIN_WIDTH).then(|| (width * 3 / 10).clamp(32, 46))
}

/// The plan on the right: a progress bar, then every task, wrapped, with the current one bright.
fn sidebar(f: &mut Frame, app: &App, area: Rect, t: Duration) {
    // A hairline separates it from the conversation.
    let buf = f.buffer_mut();
    for y in area.y..area.y + area.height {
        if let Some(cell) = buf.cell_mut((area.x, y)) {
            cell.set_symbol("│").set_fg(theme::FAINT());
        }
    }
    let inner = Rect { x: area.x + 2, width: area.width.saturating_sub(3), ..area };
    let w = inner.width as usize;
    let done = app.todos.iter().filter(|t| t.status == "completed").count();
    let total = app.todos.len();
    let count = format!("{done}/{total}");
    let mut lines = vec![
        Line::default(),
        with_right(vec![Span::styled("plan", theme::bold(theme::MUTED()))], vec![Span::styled(count, theme::dim())], w),
    ];
    let filled = (done * w).div_ceil(total.max(1)).min(w);
    lines.push(Line::from(
        (0..w).map(|i| Span::styled("━", theme::fg(if i < filled { theme::nebula(i as f32 / (w as f32 * 2.0) - t.as_secs_f32() * 0.08) } else { theme::FAINT() }))).collect::<Vec<_>>(),
    ));
    lines.push(Line::default());
    let head = lines.len();
    let mut current = None;
    for todo in &app.todos {
        let (mark, mark_style, text_style) = match todo.status.as_str() {
            "completed" => ("●", theme::fg(theme::DIM()), theme::dim()),
            "in_progress" => (anim::dots_spin(t), anim::spin_style(), theme::bold(theme::FG())),
            _ => ("○", theme::fg(theme::DIM()), theme::muted()),
        };
        if todo.status == "in_progress" && current.is_none() {
            current = Some(lines.len());
        }
        let first = vec![Span::styled(format!("{mark} "), mark_style)];
        lines.extend(super::markdown::wrap(vec![Span::styled(todo.content.clone(), text_style)], w, first, vec![Span::raw("  ")]));
        lines.push(Line::default());
    }
    // Long plans scroll to keep the current task in view.
    let room = area.height as usize;
    if lines.len() > room {
        let anchor = current.unwrap_or(head);
        let skip = anchor.saturating_sub(head + 2).min(lines.len() - room);
        if skip > 0 {
            lines.drain(head..head + skip);
            lines.insert(head, Line::from(Span::styled("↑ more", theme::dim())));
        }
        if lines.len() > room {
            lines.truncate(room - 1);
            lines.push(Line::from(Span::styled("↓ more", theme::dim())));
        }
    }
    f.render_widget(Paragraph::new(lines), inner);
}

/// The plan above the prompt (when there's no room for the sidebar). Collapsed, one line:
/// "▸ plan ━━━━━━──── 2/4 · ◐ Build and run the test suite"; open (click or ctrl+x), every step.
fn todo_lines(app: &App, width: usize, t: Duration) -> Vec<Line<'static>> {
    if !plan_visible(app) {
        return Vec::new();
    }
    let done = app.todos.iter().filter(|t| t.status == "completed").count();
    let total = app.todos.len();
    let bar_w = if width >= 70 { 12 } else { 6 };
    let filled = (done * bar_w).div_ceil(total.max(1)).min(bar_w);
    let chevron = if app.plan_open { "▾ " } else { "▸ " };
    let mut spans = vec![Span::raw(pad_str()), Span::styled(chevron, theme::fg(theme::VIOLET())), Span::styled("plan ", theme::dim())];
    for i in 0..bar_w {
        let c = if i < filled { theme::nebula(i as f32 / 40.0 - t.as_secs_f32() * 0.08) } else { theme::FAINT() };
        spans.push(Span::styled("━", theme::fg(c)));
    }
    spans.push(Span::styled(format!(" {done}/{total}"), theme::dim()));
    let step_style = |status: &str| match status {
        "completed" => ("●".to_string(), theme::fg(theme::DIM()), theme::dim()),
        "in_progress" => (anim::dots_spin(t).to_string(), anim::spin_style(), theme::bold(theme::FG())),
        _ => ("○".to_string(), theme::fg(theme::DIM()), theme::muted()),
    };
    if app.plan_open {
        let mut out = vec![Line::default(), Line::from(spans)];
        let indent = " ".repeat(MARGIN as usize + 2);
        for todo in &app.todos {
            let (mark, mark_style, text_style) = step_style(&todo.status);
            let first = vec![Span::raw(indent.clone()), Span::styled(format!("{mark} "), mark_style)];
            let rest = vec![Span::raw(format!("{indent}  "))];
            out.extend(super::markdown::wrap(vec![Span::styled(todo.content.clone(), text_style)], width.saturating_sub(MARGIN as usize), first, rest));
        }
        return out;
    }
    let current = app.todos.iter().find(|t| t.status == "in_progress");
    let next = app.todos.iter().find(|t| t.status == "pending");
    let (mark, mark_style, text, text_style) = match (current, next) {
        (Some(c), _) => {
            let (m, ms, ts) = step_style("in_progress");
            (m, ms, c.content.clone(), ts)
        }
        (None, Some(n)) => ("○".into(), theme::fg(theme::DIM()), format!("next: {}", n.content), theme::muted()),
        (None, None) => ("●".into(), theme::fg(theme::GREEN()), "all done".into(), theme::muted()),
    };
    spans.push(Span::styled(" · ", theme::dim()));
    spans.push(Span::styled(format!("{mark} "), mark_style));
    let used: usize = spans.iter().map(|s| s.content.width()).sum();
    spans.push(Span::styled(truncate(&text, width.saturating_sub(used + MARGIN as usize).max(8)), text_style));
    vec![Line::default(), Line::from(spans)]
}

/// Spans right-aligned after `left` in a line `width` wide (dropped if they don't fit).
fn with_right(mut left: Vec<Span<'static>>, right: Vec<Span<'static>>, width: usize) -> Line<'static> {
    let lw: usize = left.iter().map(|s| s.content.width()).sum();
    let rw: usize = right.iter().map(|s| s.content.width()).sum();
    if lw + rw + 2 <= width {
        left.push(Span::raw(" ".repeat(width - lw - rw)));
        left.extend(right);
    }
    Line::from(left)
}

fn activity(app: &App, width: usize, t: Duration) -> Line<'static> {
    let pad = " ".repeat(MARGIN as usize);
    if !app.approvals.is_empty() {
        let k = anim::pulse(t, 1400);
        return Line::from(vec![
            Span::raw(pad),
            Span::styled("◆ ", theme::fg(theme::mix(theme::YELLOW(), theme::FG(), k * 0.6))),
            Span::styled(
                if app.approvals[0].questions.is_empty() { "waiting for your approval" } else { "waiting for your answer" },
                theme::fg(theme::YELLOW()),
            ),
            if app.approvals.len() > 1 { Span::styled(format!("  +{} more", app.approvals.len() - 1), theme::dim()) } else { Span::raw("") },
        ]);
    }
    if app.busy {
        // The working status sits on the prompt's rule (see `working_rule`); this row stays
        // empty so it doesn't read as part of the last action above.
        return Line::default();
    }
    if let Some((text, at, kind)) = &app.toast {
        let age = at.elapsed().as_millis() as f32;
        if age < 4000.0 {
            let k_in = (age / 250.0).min(1.0);
            let fade = ((age - 2500.0) / 1500.0).clamp(0.0, 1.0);
            let (mark, base) = match kind {
                NoticeKind::Error => ("✗ ", theme::RED()),
                NoticeKind::Success => ("✓ ", theme::GREEN()),
                NoticeKind::Info => ("· ", theme::FG()),
            };
            let color = theme::mix(theme::VOID(), theme::mix(base, theme::FAINT(), fade), anim::ease_out(k_in));
            // Slides in from the right.
            let shift = ((1.0 - anim::ease_out(k_in)) * 6.0) as usize;
            return Line::from(vec![Span::raw(" ".repeat(MARGIN as usize + shift)), Span::styled(mark, theme::fg(color)), Span::styled(truncate(text, width.saturating_sub(8)), theme::fg(color))]);
        }
    }
    if app.quit_armed.is_some_and(|q| q.elapsed() < Duration::from_millis(1500)) {
        return Line::from(vec![Span::raw(pad), Span::styled("press ctrl+c again to exit", theme::fg(theme::YELLOW()))]);
    }
    if !app.composer.is_empty() || app.items.is_empty() {
        return Line::default();
    }
    let mut hint = Vec::new();
    for (k, d) in [("/", "commands"), ("@", "files"), ("!", "shell"), ("⇧tab", "mode")] {
        hint.push(Span::styled(k.to_string(), theme::dim()));
        hint.push(Span::styled(format!(" {d}   "), theme::dim()));
    }
    with_right(vec![], hint, width)
}

fn pad_str() -> String {
    " ".repeat(MARGIN as usize)
}

/// The rule above the prompt while the agent works, with the status set into it:
/// "── ✹ Warping…  1m 14s · ↑ 180k (95% cached) · ↓ 25.6k ─────────────── esc to stop ──". A comet runs along it.
/// Between turns, while subagents or shells still run in the background, it reads
/// "── ✹ 2 background agents running…  3m 02s ──── ctrl+g agents ──".
fn working_rule(app: &App, width: usize, t: Duration) -> Line<'static> {
    let mut left = vec![Span::styled(format!("{} ", anim::star_spin(t)), theme::bold(theme::nebula(t.as_secs_f32() * 0.3)))];
    let right = if app.busy {
        left.extend(anim::nebula_shimmer(&format!("{}…", app.verb), t, theme::FG()));
        let mut info = format!("  {}", anim::elapsed(app.turn_started.elapsed()));
        if app.usage.turn_input > 0 {
            info.push_str(&format!(" · ↑ {}", anim::tokens(app.usage.turn_input)));
            let pct = app.usage.turn_cached.min(app.usage.turn_input) * 100 / app.usage.turn_input;
            if pct > 0 {
                info.push_str(&format!(" ({pct}% cached)"));
            }
        }
        if app.usage.turn > 0 {
            info.push_str(&format!(" · ↓ {}", anim::tokens(app.usage.turn)));
        }
        left.push(Span::styled(info, theme::muted()));
        theme::key_hint("esc", "to stop").to_vec()
    } else {
        let (agents, shells) = (app.running_subagents(), app.running_shells());
        let plural = |n: usize, what: &str| format!("{n} background {what}{}", if n == 1 { "" } else { "s" });
        let label = match (agents, shells) {
            (0, s) => plural(s, "shell"),
            (a, 0) => plural(a, "agent"),
            (a, s) => format!("{} · {}", plural(a, "agent"), plural(s, "shell")),
        };
        left.extend(anim::nebula_shimmer(&format!("{label} running…"), t, theme::FG()));
        // The user's turn is still going: its clock keeps running.
        let since = if app.turn_open { Some(app.turn_started) } else { app.tasks.iter().filter(|t| t.status == Status::Running).map(|t| t.started).min() };
        if let Some(since) = since {
            left.push(Span::styled(format!("  {}", anim::elapsed(since.elapsed())), theme::muted()));
        }
        let mut keys = theme::key_hint("esc", "to stop").to_vec();
        if agents > 0 {
            keys.push(Span::raw(" "));
            keys.extend(theme::key_hint("ctrl+g", "agents"));
        }
        if shells > 0 {
            keys.push(Span::raw(" "));
            keys.extend(theme::key_hint("ctrl+b", "tasks"));
        }
        keys
    };
    let w = |s: &[Span]| s.iter().map(|s| s.content.width()).sum::<usize>();
    let rule = anim::comet(width, t, true);
    let lead = MARGIN as usize;
    let (lw, rw) = (w(&left), w(&right));
    // Room for "──␠left␠" … "␠right␠──"; drop the right part, then the label, when narrow.
    let show_right = rw > 0 && lead + lw + 2 + rw + 2 + lead + 4 <= width;
    if lead + lw + 2 + lead > width {
        return Line::from(rule);
    }
    let mut out: Vec<Span<'static>> = rule[..lead].to_vec();
    out.push(Span::raw(" "));
    out.extend(left);
    out.push(Span::raw(" "));
    let mid_end = if show_right { width - lead - rw - 2 } else { width };
    out.extend(rule[(lead + lw + 2).min(mid_end)..mid_end].iter().cloned());
    if show_right {
        out.push(Span::raw(" "));
        out.extend(right);
        out.push(Span::raw(" "));
        out.extend(rule[width - lead..].iter().cloned());
    }
    Line::from(out)
}

/// The prompt: a hairline (a comet runs along it while working) and the input rows.
fn composer_lines(app: &App, width: usize, t: Duration) -> (Vec<Line<'static>>, Option<(u16, u16)>) {
    let indent = MARGIN as usize + 2;
    let inner = width.saturating_sub(indent + MARGIN as usize).max(4);
    let (rows, (cr, cc)) = app.composer.layout(inner);
    let max_rows = 10;
    let first = cr.saturating_sub(max_rows - 1);
    let shown: Vec<&String> = rows.iter().skip(first).take(max_rows).collect();
    let mut out = vec![if app.working() { working_rule(app, width, t) } else { Line::from(anim::comet(width, t, false)) }];
    let prompt = if app.busy { theme::fg(theme::DIM()) } else { theme::bold(theme::nebula(0.45)) };
    let lead = |i: usize| if i == 0 && first == 0 { Span::styled(format!("{}❯ ", pad_str()), prompt) } else { Span::raw(" ".repeat(indent)) };
    if app.composer.is_empty() {
        let hint = if app.busy { "queue a follow-up…" } else { "ask anything…" };
        out.push(Line::from(vec![lead(0), Span::styled(truncate(hint, inner), theme::fg(theme::DIM()))]));
    } else {
        for (i, row) in shown.iter().enumerate() {
            let style = if row.starts_with('/') && i == 0 && first == 0 { theme::bold(theme::VIOLET()) } else { theme::fg(theme::FG()) };
            let mut spans = vec![lead(i)];
            spans.extend(highlight_tags(row, style));
            out.push(Line::from(spans));
        }
    }
    out.push(Line::default());
    let cursor = (indent as u16 + cc as u16, 1 + (cr - first) as u16);
    (out, Some(cursor))
}

/// Colors `[Pasted text #1 …]`/`[Image #1]` tags and `@mentions` in the prompt.
fn highlight_tags(row: &str, style: Style) -> Vec<Span<'static>> {
    let mut out = Vec::new();
    let mut rest = row;
    while !rest.is_empty() {
        let tag = rest.find("[Pasted text #").into_iter().chain(rest.find("[Image #")).min();
        let at = rest.find('@').filter(|&i| i == 0 || rest.as_bytes()[i - 1] == b' ');
        let next = [tag, at].into_iter().flatten().min();
        let Some(i) = next else {
            out.push(Span::styled(rest.to_string(), style));
            break;
        };
        if i > 0 {
            out.push(Span::styled(rest[..i].to_string(), style));
        }
        let end = if Some(i) == tag { rest[i..].find(']').map(|e| i + e + 1).unwrap_or(rest.len()) } else { rest[i..].find(' ').map(|e| i + e).unwrap_or(rest.len()) };
        out.push(Span::styled(rest[i..end].to_string(), theme::fg(theme::BLUE())));
        rest = &rest[end..];
    }
    out
}

fn approval_lines(app: &App, width: usize, t: Duration) -> Vec<Line<'static>> {
    let a = app.approvals.front().unwrap();
    let k = anim::pulse(t, 1600);
    let amber = theme::mix(theme::mix(theme::YELLOW(), theme::FAINT(), 0.5), theme::YELLOW(), k);
    let pad = pad_str();
    let body = " ".repeat(MARGIN as usize + 2);
    let inner = width.saturating_sub(MARGIN as usize * 2 + 2);
    let mut out = vec![Line::from(Span::styled("─".repeat(width), theme::fg(theme::mix(theme::FAINT(), theme::YELLOW(), 0.2 + 0.35 * k))))];
    out.push(Line::from(vec![
        Span::raw(pad),
        Span::styled("◆ ", theme::bold(amber)),
        Span::styled("Allow ", theme::bold(theme::FG())),
        Span::styled(a.tool.clone(), theme::bold(theme::YELLOW())),
        Span::styled("?", theme::bold(theme::FG())),
    ]));
    for line in a.title.lines().take(6) {
        out.push(Line::from(vec![Span::raw(body.clone()), Span::styled(truncate(line, inner), theme::fg(theme::FG()))]));
    }
    if !a.detail.is_empty() && a.detail != a.title {
        out.push(Line::from(vec![Span::raw(body.clone()), Span::styled(truncate(&a.detail, inner), theme::dim())]));
    }
    if let Some(d) = &a.diff {
        let lines = diff_lines(d, inner.saturating_sub(3));
        let max = if app.expanded { 40 } else { 12 };
        let total = lines.len();
        for l in lines.into_iter().take(max) {
            let mut spans = vec![Span::raw(body.clone()), Span::styled("│  ", theme::fg(theme::FAINT()))];
            spans.extend(l.spans);
            out.push(Line::from(spans));
        }
        if total > max {
            out.push(Line::from(vec![Span::raw(body.clone()), Span::styled(format!("╰  {} more lines · ctrl+o", total - max), theme::dim())]));
        }
    }
    out.push(Line::default());
    let options = ["yes", "yes, don't ask again this session", "no, tell it what to do"];
    let keys = ["y", "a", "n"];
    let mut spans = vec![Span::raw(body.clone())];
    for (i, (o, key)) in options.iter().zip(keys).enumerate() {
        let selected = i == app.approval_choice;
        if selected {
            spans.push(Span::styled("❯ ", theme::bold(theme::nebula(t.as_secs_f32() * 0.3))));
            spans.push(Span::styled(o.to_string(), theme::bold(theme::FG())));
        } else {
            spans.push(Span::styled(format!("{key} "), theme::fg(theme::DIM())));
            spans.push(Span::styled(o.to_string(), theme::muted()));
        }
        spans.push(Span::raw("     "));
    }
    let w: usize = spans.iter().map(|s| s.content.width()).sum();
    if w > width {
        // Narrow terminals: one option per line.
        for (i, (o, key)) in options.iter().zip(keys).enumerate() {
            let selected = i == app.approval_choice;
            out.push(Line::from(vec![
                Span::raw(body.clone()),
                Span::styled(if selected { "❯ ".to_string() } else { format!("{key} ") }, if selected { theme::bold(theme::VIOLET()) } else { theme::fg(theme::DIM()) }),
                Span::styled(o.to_string(), if selected { theme::bold(theme::FG()) } else { theme::muted() }),
            ]));
        }
    } else {
        out.push(Line::from(spans));
    }
    out.push(Line::default());
    out
}

/// The agent's questions, one at a time: its options, then a row to type an answer.
fn question_lines(app: &App, width: usize, t: Duration) -> Vec<Line<'static>> {
    let a = app.approvals.front().unwrap();
    let i = app.answering.index.min(a.questions.len() - 1);
    let q = &a.questions[i];
    let k = anim::pulse(t, 1600);
    let amber = theme::mix(theme::mix(theme::YELLOW(), theme::FAINT(), 0.5), theme::YELLOW(), k);
    let body = " ".repeat(MARGIN as usize + 2);
    let inner = width.saturating_sub(MARGIN as usize * 2 + 2);
    let mut out = vec![Line::from(Span::styled("─".repeat(width), theme::fg(theme::mix(theme::FAINT(), theme::YELLOW(), 0.2 + 0.35 * k))))];
    let mut head = vec![Span::raw(pad_str()), Span::styled("◆ ", theme::bold(amber))];
    head.push(Span::styled(if q.header.is_empty() { "Question".to_string() } else { q.header.clone() }, theme::bold(theme::YELLOW())));
    if a.questions.len() > 1 {
        head.push(Span::styled(format!("  {}/{}", i + 1, a.questions.len()), theme::dim()));
    }
    out.push(Line::from(head));
    let indent = vec![Span::raw(body.clone())];
    out.extend(super::markdown::wrap(vec![Span::styled(q.question.clone(), theme::bold(theme::FG()))], width.saturating_sub(MARGIN as usize), indent.clone(), indent));
    out.push(Line::default());
    let row = |selected: bool, mark: String, label: String, label_style: Style, extra: Vec<Span<'static>>| {
        let mut spans = vec![
            Span::raw(body.clone()),
            Span::styled(if selected { "❯ " } else { "  " }, theme::bold(theme::nebula(t.as_secs_f32() * 0.3))),
            Span::styled(mark, if selected { theme::fg(theme::FG()) } else { theme::fg(theme::DIM()) }),
            Span::styled(label, label_style),
        ];
        spans.extend(extra);
        spans
    };
    for (n, (label, description)) in q.options.iter().enumerate() {
        let selected = n == app.approval_choice;
        let mark = if q.multi {
            if app.answering.picked.contains(&n) { "[x] ".to_string() } else { "[ ] ".to_string() }
        } else {
            format!("{}. ", n + 1)
        };
        let used = 2 + mark.width() + label.width();
        let extra = if description.is_empty() || used + 5 > inner {
            Vec::new()
        } else {
            vec![Span::styled(format!("  {}", truncate(description, inner - used - 2)), theme::dim())]
        };
        let style = if selected { theme::bold(theme::FG()) } else { theme::muted() };
        out.push(Line::from(row(selected, mark, label.clone(), style, extra)));
    }
    let typing = app.approval_choice >= q.options.len();
    let other = &app.answering.other;
    let (label, style) = if other.is_empty() {
        ("Type your own answer".to_string(), if typing { theme::fg(theme::MUTED()) } else { theme::dim() })
    } else {
        (truncate(other, inner.saturating_sub(8)), if typing { theme::bold(theme::FG()) } else { theme::muted() })
    };
    let caret = if typing { vec![Span::styled("▏", theme::bold(theme::VIOLET()))] } else { Vec::new() };
    out.push(Line::from(row(typing, "✎  ".into(), label, style, caret)));
    out.push(Line::default());
    let hint = if q.multi { "↑↓ move · space tick · enter confirm · esc decline" } else { "↑↓ move · enter choose · esc decline" };
    out.push(Line::from(vec![Span::raw(body), Span::styled(hint, theme::dim())]));
    out.push(Line::default());
    out
}

fn status(app: &App, width: usize) -> Line<'static> {
    let accent = theme::backend(app.kind);
    let model = if !app.actual_model.is_empty() { app.actual_model.clone() } else if !app.model.is_empty() { app.model.clone() } else { "default".into() };
    let dot = if app.alive { theme::fg(accent) } else { theme::fg(theme::DIM()) };
    let mut left = vec![
        Span::raw(pad_str()),
        Span::styled(if app.alive { "● " } else { "○ " }, dot),
        Span::styled(app.kind.label().to_string(), theme::fg(theme::MUTED())),
        Span::styled(format!("  {model}"), theme::dim()),
    ];
    if let Some(e) = &app.config.effort {
        left.push(Span::styled(format!(" · {e}"), theme::dim()));
    }
    left.push(Span::styled(" · ", theme::dim()));
    let mode = app.config.mode.label().to_string();
    match mode_color(app.config.mode) {
        Some(c) => left.push(Span::styled(mode, theme::bold(c))),
        None => left.push(Span::styled(mode, theme::dim())),
    }
    let mut right = Vec::new();
    let shells = app.running_shells();
    if shells > 0 {
        right.push(Span::styled(format!("⇢ {shells} bg  "), theme::fg(theme::PINK())));
    }
    let pct = app.ctx_shown.value();
    if app.usage.window > 0 && pct > 0.0 {
        let color = if pct > 0.85 { theme::RED() } else if pct > 0.6 { theme::YELLOW() } else { theme::MUTED() };
        right.push(Span::styled(format!("{} ", theme::moon(pct)), theme::fg(color)));
        let label = if pct < 0.01 { "<1%".to_string() } else { format!("{:.0}%", pct * 100.0) };
        right.push(Span::styled(label, theme::fg(color)));
        right.push(Span::styled(" ctx", theme::dim()));
    }
    if let Some(c) = app.usage.cost {
        right.push(Span::styled(format!("   ${c:.2}"), theme::dim()));
    }
    let dir = crate::harness::tools::short_path(&app.cwd.to_string_lossy());
    let dir = dir.rsplit('/').next().unwrap_or(&dir).to_string();
    right.push(Span::styled(format!("   {dir}"), theme::dim()));
    if let Some(b) = &app.branch {
        right.push(Span::styled(format!(" ⎇ {b}"), theme::dim()));
    }
    right.push(Span::raw(pad_str()));
    let lw: usize = left.iter().map(|s| s.content.width()).sum();
    let rw: usize = right.iter().map(|s| s.content.width()).sum();
    if lw + rw + 1 > width {
        // Narrow: drop the directory and branch first.
        right.retain(|s| !s.content.contains('⎇') && !s.content.starts_with("   ") || s.content.contains('$'));
    }
    let rw: usize = right.iter().map(|s| s.content.width()).sum();
    let mut spans = left;
    spans.push(Span::raw(" ".repeat(width.saturating_sub(lw + rw))));
    spans.extend(right);
    Line::from(spans)
}

/// The running subagents, one after another on the line above the status bar:
/// "◐ 2 agents   ◓ Find auth code · 3 tools · Grep  1m 02s   ◑ Review tests  12s   ctrl+g".
/// None when no agent runs.
fn agents_line(app: &App, width: usize, t: Duration) -> Option<Line<'static>> {
    let running: Vec<_> = app.subagents().into_iter().filter(|a| a.status == super::transcript::Status::Running).collect();
    if running.is_empty() {
        return None;
    }
    let n = running.len();
    let now = Instant::now();
    let mut spans = vec![
        Span::raw(pad_str()),
        Span::styled(format!("{} ", anim::moon_spin(t)), theme::bold(theme::nebula(t.as_secs_f32() * 0.3))),
        Span::styled(format!("{n} agent{}", if n == 1 { "" } else { "s" }), theme::bold(theme::VIOLET())),
    ];
    let hint = [Span::styled("   ctrl+g", theme::muted()), Span::styled(" list", theme::dim()), Span::raw(pad_str())];
    let hint_w: usize = hint.iter().map(|s| s.content.width()).sum();
    let mut hidden = 0;
    for (i, a) in running.iter().enumerate() {
        let title = if a.title.trim().is_empty() { "subagent" } else { a.title.trim() };
        let took = anim::elapsed(now.duration_since(a.started));
        let mut info = String::new();
        if a.steps > 0 {
            info.push_str(&format!(" · {} tool{}", a.steps, if a.steps == 1 { "" } else { "s" }));
        }
        if !a.last.is_empty() {
            info.push_str(&format!(" · {}", a.last.split_whitespace().take(3).collect::<Vec<_>>().join(" ")));
        }
        // Each agent's moon is a little out of phase with the others.
        let phase = t + Duration::from_millis(i as u64 * 190);
        let sep = if i == 0 { " · " } else { "   " };
        let mut entry = vec![
            Span::styled(sep.to_string(), theme::dim()),
            Span::styled(if i == 0 { String::new() } else { format!("{} ", anim::moon_spin(phase)) }, theme::fg(theme::nebula(0.2 + i as f32 * 0.17))),
            Span::styled(truncate(title, 28), theme::fg(theme::FG())),
            Span::styled(truncate(&info, 30), theme::dim()),
            Span::styled(format!("  {took}"), theme::muted()),
        ];
        let used: usize = spans.iter().map(|s| s.content.width()).sum();
        let ew: usize = entry.iter().map(|s| s.content.width()).sum();
        let more = if i + 1 < n { 10 } else { 0 };
        if used + ew + hint_w + more > width {
            // Keep the first agent even on narrow screens, without its details.
            if i == 0 {
                entry.truncate(3);
                let room = width.saturating_sub(used + 6);
                entry[2] = Span::styled(truncate(title, room), theme::fg(theme::FG()));
                spans.extend(entry);
                hidden = n - 1;
            } else {
                hidden = n - i;
            }
            break;
        }
        spans.extend(entry);
    }
    if hidden > 0 {
        spans.push(Span::styled(format!("   +{hidden} more"), theme::muted()));
    }
    Some(with_right(spans, hint.to_vec(), width))
}

fn completion(f: &mut Frame, app: &App, main: Rect) {
    let Some(c) = &app.completion else { return };
    let h = (c.items.len() as u16 + 1).min(main.height);
    let w = main.width.saturating_sub(MARGIN * 2).min(76);
    let area = Rect { x: main.x, y: main.y + main.height.saturating_sub(h), width: main.width, height: h };
    f.render_widget(Clear, area);
    let iw = w as usize;
    let label_w = c.items.iter().map(|i| i.label.width()).max().unwrap_or(0).min(iw.saturating_sub(6));
    let mut lines = vec![Line::default()];
    for (i, it) in c.items.iter().enumerate().take(h.saturating_sub(1) as usize) {
        let sel = i == c.selected;
        let label = truncate(&it.label, label_w);
        let gap = " ".repeat(label_w.saturating_sub(label.width()) + 3);
        let detail = truncate(&it.detail, iw.saturating_sub(label_w + 6));
        lines.push(Line::from(vec![
            Span::raw(pad_str()),
            Span::styled(if sel { "❯ " } else { "  " }, theme::bold(theme::nebula(0.45))),
            Span::styled(label, if sel { theme::bold(theme::FG()) } else { theme::muted() }),
            Span::raw(gap),
            Span::styled(detail, if sel { theme::muted() } else { theme::dim() }),
        ]));
    }
    f.render_widget(Paragraph::new(lines), area);
}

fn picker(f: &mut Frame, app: &App, full: Rect, t: Duration) {
    let Some(p) = &app.picker else { return };
    let k = app.overlay.value();
    let visible = p.visible();
    let w = full.width.saturating_sub(4).min(70);
    let groups = if p.filter.is_empty() { visible.iter().map(|&i| &p.items[i].group).filter(|g| !g.is_empty()).collect::<std::collections::HashSet<_>>().len() } else { 0 };
    let rows = (full.height.saturating_sub(10)).min(18).min((visible.len() + groups * 2).max(4) as u16) as usize;
    let h_full = (rows as u16 + 6).min(full.height);
    let h = ((h_full as f32) * (0.7 + 0.3 * k)).round() as u16;
    let x = full.x + (full.width.saturating_sub(w)) / 2;
    let y = full.y + (full.height.saturating_sub(h_full)) / 3 + ((1.0 - k) * 2.0).round() as u16;
    let area = Rect { x, y, width: w, height: h.max(3) };
    dim_behind(f, full, k);
    f.render_widget(Clear, area);
    let block = Frame_::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(theme::fg(theme::FAINT()))
        .title(Line::from(vec![Span::raw(" "), Span::styled(p.title.clone(), theme::bold(theme::FG())), Span::raw(" ")]))
        .title_bottom({
            let mut keys = vec![
                Span::styled(" ↑↓", theme::muted()),
                Span::styled(" select   ", theme::dim()),
                Span::styled("enter", theme::muted()),
                Span::styled(if p.kind.is_live() { " jump   " } else { " choose   " }, theme::dim()),
            ];
            if p.kind.is_live() {
                keys.extend([Span::styled("ctrl+x", theme::muted()), Span::styled(" stop   ", theme::dim())]);
            }
            keys.extend([Span::styled("esc", theme::muted()), Span::styled(" close ", theme::dim())]);
            Line::from(keys).right_aligned()
        });
    let inner = block.inner(area);
    f.render_widget(block, area);
    nebula_border(f, area, t, k);
    let inner = Rect { x: inner.x + 1, y: inner.y + 1, width: inner.width.saturating_sub(2), height: inner.height.saturating_sub(1) };
    if inner.height == 0 {
        return;
    }
    let iw = inner.width as usize;
    let mut lines = vec![
        Line::from(vec![
            Span::styled(" ⌕ ", theme::fg(theme::VIOLET())),
            if p.filter.is_empty() { Span::styled("type to filter", theme::fg(theme::DIM())) } else { Span::styled(p.filter.clone(), theme::bold(theme::FG())) },
            if p.loading { Span::styled(format!("   {} loading models", anim::dots_spin(t)), theme::dim()) } else { Span::raw("") },
        ]),
        Line::default(),
    ];
    let room = inner.height.saturating_sub(lines.len() as u16) as usize;
    // Keep the selection in view.
    let mut entries: Vec<(Option<String>, usize)> = Vec::new();
    let mut last_group = String::new();
    for &i in &visible {
        let g = &p.items[i].group;
        if p.filter.is_empty() && !g.is_empty() && *g != last_group {
            if !last_group.is_empty() {
                entries.push((Some(String::new()), usize::MAX));
            }
            entries.push((Some(g.clone()), usize::MAX));
            last_group = g.clone();
        }
        entries.push((None, i));
    }
    let sel_item = visible.get(p.selected).copied();
    let sel_row = entries.iter().position(|e| e.1 == sel_item.unwrap_or(usize::MAX) && e.0.is_none()).unwrap_or(0);
    let start = sel_row.saturating_sub(room.saturating_sub(1));
    let label_w = visible.iter().map(|&i| p.items[i].label.width()).max().unwrap_or(0).min(iw / 2);
    for (group, i) in entries.iter().skip(start).take(room) {
        if let Some(g) = group {
            lines.push(Line::from(Span::styled(format!("   {g}"), theme::dim())));
            continue;
        }
        let it = &p.items[*i];
        let sel = Some(*i) == sel_item;
        let label = truncate(&it.label, label_w);
        let gap = " ".repeat(label_w.saturating_sub(label.width()));
        let detail = truncate(&it.detail, iw.saturating_sub(label_w + 9));
        lines.push(Line::from(vec![
            Span::styled(if sel { " ❯ " } else { "   " }, theme::bold(theme::nebula(0.45 + t.as_secs_f32() * 0.1))),
            Span::styled(label, if sel { theme::bold(theme::FG()) } else { theme::muted() }),
            Span::raw(gap),
            Span::styled(if it.current { "  ●  " } else { "     " }, theme::fg(theme::GREEN())),
            Span::styled(detail, if sel { theme::muted() } else { theme::dim() }),
        ]));
    }
    if visible.is_empty() {
        lines.push(Line::from(Span::styled("   no matches", theme::dim())));
    }
    anim::fade(&mut lines, k);
    f.render_widget(Paragraph::new(lines), inner);
}

/// Dims everything behind a modal.
fn dim_behind(f: &mut Frame, full: Rect, k: f32) {
    let buf = f.buffer_mut();
    for yy in full.y..full.y + full.height {
        for xx in full.x..full.x + full.width {
            if let Some(cell) = buf.cell_mut((xx, yy)) {
                let base = if matches!(cell.fg, ratatui::style::Color::Rgb(..)) { cell.fg } else { theme::FG() };
                cell.set_fg(theme::mix(base, theme::VOID(), 0.75 * k));
                if matches!(cell.bg, ratatui::style::Color::Rgb(..)) {
                    cell.set_bg(theme::mix(cell.bg, theme::VOID(), 0.75 * k));
                }
            }
        }
    }
}

/// The `/btw` modal: the side question and its answer (or a spinner while it's thought about).
fn btw(f: &mut Frame, app: &mut App, full: Rect, t: Duration) {
    if !app.btw_showing() {
        return;
    }
    let k = app.overlay.value();
    let Some(b) = &mut app.btw else { return };
    let w = full.width.saturating_sub(4).min(90);
    let iw = w.saturating_sub(4) as usize;
    let mut body: Vec<Line<'static>> = super::markdown::wrap(
        vec![Span::styled(b.question.clone(), theme::bold(theme::FG()))],
        iw,
        vec![Span::styled("❯ ", theme::fg(theme::VIOLET()))],
        vec![Span::raw("  ")],
    );
    body.push(Line::default());
    match &b.answer {
        None => body.push(Line::from(vec![
            Span::styled(anim::moon_spin(t), anim::spin_style()),
            Span::styled(format!(" thinking {}", anim::elapsed(b.started.elapsed())), theme::dim()),
        ])),
        Some(Ok(a)) => body.extend(super::markdown::render(a, iw)),
        Some(Err(e)) => body.extend(super::markdown::wrap(vec![Span::styled(e.clone(), theme::fg(theme::RED()))], iw, vec![Span::styled("✗ ", theme::fg(theme::RED()))], vec![Span::raw("  ")])),
    }
    let room = full.height.saturating_sub(8).max(3);
    let rows = (body.len() as u16).min(room);
    let max_scroll = (body.len() as u16).saturating_sub(rows);
    b.scroll = b.scroll.min(max_scroll);
    let scroll = b.scroll;
    let done = b.answer.is_some();
    let h_full = (rows + 4).min(full.height);
    let h = ((h_full as f32) * (0.7 + 0.3 * k)).round() as u16;
    let x = full.x + (full.width.saturating_sub(w)) / 2;
    let y = full.y + (full.height.saturating_sub(h_full)) / 3 + ((1.0 - k) * 2.0).round() as u16;
    let area = Rect { x, y, width: w, height: h.max(3) };
    dim_behind(f, full, k);
    f.render_widget(Clear, area);
    let mut keys = Vec::new();
    if max_scroll > 0 {
        keys.extend([Span::styled(" ↑↓", theme::muted()), Span::styled(format!(" scroll {}/{}   ", scroll, max_scroll), theme::dim())]);
    }
    if done {
        keys.extend([Span::styled(" y", theme::muted()), Span::styled(" copy   ", theme::dim())]);
    }
    keys.extend([Span::styled("esc", theme::muted()), Span::styled(if done { " close " } else { " hide (opens when ready) " }, theme::dim())]);
    let block = Frame_::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(theme::fg(theme::FAINT()))
        .title(Line::from(vec![Span::raw(" "), Span::styled("btw", theme::bold(theme::FG())), Span::styled(" · side question, the agent keeps working ", theme::dim())]))
        .title_bottom(Line::from(keys).right_aligned());
    let inner = block.inner(area);
    f.render_widget(block, area);
    nebula_border(f, area, t, k);
    let inner = Rect { x: inner.x + 1, y: inner.y + 1, width: inner.width.saturating_sub(2), height: inner.height.saturating_sub(1) };
    let mut lines: Vec<Line> = body.into_iter().skip(scroll as usize).take(inner.height as usize).collect();
    anim::fade(&mut lines, k);
    f.render_widget(Paragraph::new(lines), inner);
}

/// A soft nebula glow that travels around a box's border.
fn nebula_border(f: &mut Frame, area: Rect, t: Duration, k: f32) {
    if area.width < 2 || area.height < 2 {
        return;
    }
    let mut ring: Vec<(u16, u16)> = Vec::new();
    ring.extend((area.x..area.x + area.width).map(|x| (x, area.y)));
    ring.extend((area.y + 1..area.y + area.height).map(|y| (area.x + area.width - 1, y)));
    ring.extend((area.x..area.x + area.width - 1).rev().map(|x| (x, area.y + area.height - 1)));
    ring.extend((area.y + 1..area.y + area.height - 1).rev().map(|y| (area.x, y)));
    let n = ring.len() as f32;
    let head = (t.as_secs_f32() * 0.12).fract() * n;
    let buf = f.buffer_mut();
    for (i, (x, y)) in ring.into_iter().enumerate() {
        let d = (i as f32 - head).rem_euclid(n);
        // Tail trails behind the head.
        let tail = (1.0 - d.min(n - d) / (n * 0.25)).max(0.0);
        if let Some(cell) = buf.cell_mut((x, y)) {
            if cell.symbol().chars().all(|c| "─│╭╮╰╯".contains(c)) {
                let base = theme::mix(theme::FAINT(), theme::nebula(i as f32 / n), 0.35 * k);
                cell.set_fg(theme::mix(base, theme::nebula(i as f32 / n + 0.2), tail * k));
            }
        }
    }
}
