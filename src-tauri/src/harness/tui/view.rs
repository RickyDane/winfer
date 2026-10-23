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
use super::picker::PickKind;
use super::transcript::{Block, NoticeKind, Status, diff_lines, truncate};
use super::{anim, theme};
use crate::harness::event::Mode;

const LOGO: [&str; 2] = ["█ █ █ █ █▀▄ █▀▀ █▀▀ █▀▄", "▀▄▀▄▀ █ █ █ █▀  ██▄ █▀▄"];

/// Left margin of the transcript.
const MARGIN: u16 = 2;

pub fn draw(f: &mut Frame, app: &mut App) {
    app.poll_shell_output();
    app.refresh_task_log();
    let area = f.area();
    let now = Instant::now();
    let t = now.duration_since(app.epoch);
    let width = area.width as usize;
    app.refresh_agents();

    // Bottom-up layout.
    let agents = agents_line(app, width, t);
    let status_h = 1 + u16::from(agents.is_some());
    let (mut bottom_lines, cursor) = match app.approvals.front() {
        None => composer_lines(app, width, t),
        Some(a) if !a.questions.is_empty() => (question_lines(app, width, t), None),
        Some(_) => (approval_lines(app, width, t), None),
    };
    // The row right above the prompt: the working line while the agent works, else the
    // activity line (approval waiting, toasts). A toast during work gets a row of its own.
    let act = activity(app, width, t);
    let working = app.working() && app.approvals.is_empty();
    let (top_row, extra) = if working { (working_rule(app, width, t), (act.width() > 0).then_some(act)) } else { (act, None) };
    // A blank row keeps the prompt clear of the conversation (and the plan and queue) above.
    bottom_lines.splice(0..0, [Line::default(), top_row]);
    let cursor = cursor.map(|(x, y)| (x, y + 2));
    let bottom_h = (bottom_lines.len() as u16).min(area.height.saturating_sub(4));
    let activity_h = u16::from(extra.is_some());
    // The plan goes in a right sidebar when there's room (and it's on); else above the prompt.
    let sidebar_w = sidebar_width(app, area.width);
    let todo_lines = if sidebar_w.is_some() { Vec::new() } else { todo_lines(app, width, t) };
    let todo_h = (todo_lines.len() as u16).min(area.height / 3);
    let queue_lines: Vec<Line> = app
        .queue
        .iter()
        .take(3)
        .enumerate()
        .map(|(i, m)| {
            // An empty prompt's enter sends them into the running turn.
            let label = if i == 0 && app.busy && app.composer.is_empty() { "  queued · enter sends now" } else { "  queued" };
            Line::from(vec![
                Span::styled("  ↳ ", theme::fg(theme::VIOLET())),
                Span::styled(truncate(m.text.lines().next().unwrap_or(""), width.saturating_sub(16 + label.len())), theme::muted()),
                Span::styled(label, theme::dim()),
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
    f.render_widget(Paragraph::new(extra.unwrap_or_default()), activity_area);
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
    task_log(f, app, area, t);
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
    // Key tiles when there's room: five small rounded cards.
    const TILES: [(&str, &str, &str); 5] = [("/", "commands", "slash commands"), ("@", "files", "attach context"), ("!", "shell", "run a command"), ("ctrl+t", "model", "switch model"), ("⇧tab", "mode", "cycle modes")];
    const TILE_W: usize = 18;
    if area.width as usize >= TILES.len() * (TILE_W + 2) + 2 {
        let edge = || theme::fg(theme::FAINT());
        let mut rows: [Vec<Span<'static>>; 4] = Default::default();
        for (i, (k, label, what)) in TILES.iter().enumerate() {
            if i > 0 {
                for r in &mut rows {
                    r.push(Span::raw("  "));
                }
            }
            let iw = TILE_W - 4;
            rows[0].push(Span::styled(format!("╭{}╮", "─".repeat(TILE_W - 2)), edge()));
            let top = format!("{k}  {label}");
            rows[1].extend([
                Span::styled("│ ", edge()),
                Span::styled(k.to_string(), theme::bold(theme::nebula(i as f32 / (TILES.len() as f32 * 1.5)))),
                Span::styled(format!("  {label}"), theme::fg(theme::FG())),
                Span::raw(" ".repeat(iw.saturating_sub(top.width()))),
                Span::styled(" │", edge()),
            ]);
            rows[2].extend([Span::styled("│ ", edge()), Span::styled(format!("{what:<iw$}"), theme::dim()), Span::styled(" │", edge())]);
            rows[3].push(Span::styled(format!("╰{}╯", "─".repeat(TILE_W - 2)), edge()));
        }
        lines.extend(rows.into_iter().map(|r| Line::from(r).centered()));
    } else {
        welcome_hints(&mut lines, area.width as usize);
    }

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

/// The welcome screen's key hints on narrow terminals, wrapped onto centered rows.
fn welcome_hints(lines: &mut Vec<Line<'static>>, width: usize) {
    let mut row: Vec<Span<'static>> = Vec::new();
    for (k, d) in [("/", "commands"), ("@", "files"), ("!", "shell"), ("ctrl+t", "model"), ("⇧tab", "mode")] {
        let hint = theme::key_hint(k, d);
        let w: usize = row.iter().chain(hint.iter()).map(|s| s.content.width()).sum::<usize>() + 4;
        if !row.is_empty() && w + 4 > width {
            lines.push(Line::from(std::mem::take(&mut row)).centered());
        }
        if !row.is_empty() {
            row.push(Span::raw("    "));
        }
        row.extend(hint);
    }
    lines.push(Line::from(row).centered());
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
        // The working status has its own row above the prompt (see `working_rule`).
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
    Line::default()
}

fn pad_str() -> String {
    " ".repeat(MARGIN as usize)
}

/// The rule above the prompt while the agent works, with the status set into it:
/// "── ✹ Warping…  1m 14s · ↑ 180k (95% cached) · ↓ 25.6k · 62 tok/s ─────────────── esc to stop ──". A comet runs along it.
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
            if let Some(rate) = app.tps.rate() {
                info.push_str(&format!(" · {rate:.0} tok/s"));
            }
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

/// Border cells of a frame, colored by `color(i, n)`: cell `i` of `n`, clockwise from the
/// top-left corner.
type FrameColor<'a> = &'a dyn Fn(usize, usize) -> ratatui::style::Color;

/// `content` in a rounded frame across the width (less the margins), one space of padding
/// inside. `title` is set into the top border on the left, `top_right` and `bottom_right`
/// into the borders on the right; labels that don't fit are dropped.
fn framed(content: Vec<Line<'static>>, width: usize, color: FrameColor, title: Vec<Span<'static>>, top_right: Vec<Span<'static>>, bottom_right: Vec<Span<'static>>) -> Vec<Line<'static>> {
    let m = MARGIN as usize;
    let bw = width.saturating_sub(m * 2).max(8);
    let h = content.len() + 2;
    let n = 2 * (bw + h) - 4;
    let inner = bw - 4;
    let cell = |s: &'static str, i: usize| Span::styled(s, theme::fg(color(i, n)));
    let sw = |s: &[Span]| s.iter().map(|s| s.content.width()).sum::<usize>();
    let border = |top: bool, left: Vec<Span<'static>>, right: Vec<Span<'static>>| {
        let idx = |x: usize| if top { x } else { bw + h - 2 + (bw - 1 - x) };
        let (lw, rw) = (sw(&left), sw(&right));
        let show_left = lw > 0 && 2 + lw < bw - 1;
        let after_left = if show_left { 2 + lw } else { 1 };
        let show_right = rw > 0 && after_left + 1 + rw + 2 <= bw;
        let mut spans = vec![Span::raw(pad_str()), cell(if top { "╭" } else { "╰" }, idx(0))];
        if show_left {
            spans.push(cell("─", idx(1)));
            spans.extend(left);
        }
        let right_start = if show_right { bw - 2 - rw } else { bw - 1 };
        for x in after_left..right_start {
            spans.push(cell("─", idx(x)));
        }
        if show_right {
            spans.extend(right);
            spans.push(cell("─", idx(bw - 2)));
        }
        spans.push(cell(if top { "╮" } else { "╯" }, idx(bw - 1)));
        Line::from(spans)
    };
    let mut out = vec![border(true, title, top_right)];
    for (j, line) in content.into_iter().enumerate() {
        let r = j + 1;
        let w = line.width();
        let mut spans = vec![Span::raw(pad_str()), cell("│", n - r), Span::raw(" ")];
        spans.extend(line.spans);
        spans.push(Span::raw(" ".repeat(inner.saturating_sub(w) + 1)));
        spans.push(cell("│", bw - 1 + r));
        out.push(Line::from(spans));
    }
    out.push(border(false, Vec::new(), bottom_right));
    out
}

/// Width inside a frame (see `framed`).
fn frame_inner(width: usize) -> usize {
    width.saturating_sub(MARGIN as usize * 2).max(8) - 4
}

/// Triangle wave over 0..1, for gradients that loop without a seam.
fn tri(x: f32) -> f32 {
    1.0 - (2.0 * x.rem_euclid(1.0) - 1.0).abs()
}

/// The prompt: the input rows in a rounded frame with a nebula gradient border (it flows while
/// the agent works), the mode set into the top border and key hints into the bottom one.
fn composer_lines(app: &App, width: usize, t: Duration) -> (Vec<Line<'static>>, Option<(u16, u16)>) {
    let inner = frame_inner(width).saturating_sub(2).max(4);
    let (rows, (cr, cc)) = app.composer.layout(inner);
    let max_rows = 10;
    let first = cr.saturating_sub(max_rows - 1);
    let shown: Vec<&String> = rows.iter().skip(first).take(max_rows).collect();
    let prompt = if app.busy { theme::fg(theme::DIM()) } else { theme::bold(theme::VIOLET()) };
    let lead = |i: usize| if i == 0 && first == 0 { Span::styled("❯ ", prompt) } else { Span::raw("  ") };
    let mut content = Vec::new();
    if app.composer.is_empty() {
        let hint = if app.busy { "steer the agent… (tab queues it for after)" } else { "ask anything…" };
        content.push(Line::from(vec![lead(0), Span::styled(truncate(hint, inner), theme::fg(theme::DIM()))]));
    } else {
        for (i, row) in shown.iter().enumerate() {
            let style = if row.starts_with('/') && i == 0 && first == 0 { theme::bold(theme::VIOLET()) } else { theme::fg(theme::FG()) };
            let mut spans = vec![lead(i)];
            spans.extend(highlight_tags(row, style));
            content.push(Line::from(spans));
        }
    }
    let flow = if app.working() { t.as_secs_f32() * 0.12 } else { 0.0 };
    let color = move |i: usize, n: usize| theme::mix(theme::FAINT(), theme::nebula(i as f32 / n as f32 - flow), 0.75);
    let mode = app.config.mode.label().to_string();
    let mode_style = mode_color(app.config.mode).map(theme::bold).unwrap_or_else(theme::muted);
    let title = vec![Span::raw(" "), Span::styled(mode, mode_style), Span::raw(" ")];
    let mut top_right = vec![Span::raw(" ")];
    top_right.extend(theme::key_hint("⇧tab", "mode"));
    top_right.push(Span::raw(" "));
    let mut bottom_right = vec![Span::raw(" ")];
    for (i, (k, d)) in [("/", "commands"), ("@", "files"), ("!", "shell")].into_iter().enumerate() {
        if i > 0 {
            bottom_right.push(Span::raw("  "));
        }
        bottom_right.extend(theme::key_hint(k, d));
    }
    bottom_right.push(Span::raw(" "));
    let mut out = framed(content, width, &color, title, top_right, bottom_right);
    out.push(Line::default());
    let cursor = (MARGIN + 4 + cc as u16, 1 + (cr - first) as u16);
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
    let inner = frame_inner(width);
    let mut content = Vec::new();
    for line in a.title.lines().take(6) {
        content.push(Line::from(Span::styled(truncate(line, inner), theme::bold(theme::FG()))));
    }
    if !a.detail.is_empty() && a.detail != a.title {
        content.push(Line::from(Span::styled(truncate(&a.detail, inner), theme::dim())));
    }
    if let Some(d) = &a.diff {
        content.push(Line::default());
        let lines = diff_lines(d, inner);
        let max = if app.expanded { 40 } else { 12 };
        let total = lines.len();
        content.extend(lines.into_iter().take(max));
        if total > max {
            content.push(Line::from(Span::styled(format!("{} more lines · ctrl+o", total - max), theme::dim())));
        }
    }
    content.push(Line::default());
    let options = ["yes", "yes, don't ask again this session", "no, tell it what to do"];
    let keys = ["y", "a", "n"];
    // The choice is a pill: "▐ y  yes ▌"; the others are a key and a label.
    let option = |i: usize| -> Vec<Span<'static>> {
        let (o, key) = (options[i], keys[i]);
        if i == app.approval_choice {
            let y = theme::YELLOW();
            vec![Span::styled("▐", theme::fg(y)), Span::styled(format!(" {key}  {o} "), theme::bold(theme::VOID()).bg(y)), Span::styled("▌", theme::fg(y))]
        } else {
            vec![Span::raw(" "), Span::styled(key.to_string(), theme::bold(theme::YELLOW())), Span::styled(format!("  {o} "), theme::muted()), Span::raw(" ")]
        }
    };
    let mut row = Vec::new();
    for i in 0..options.len() {
        if i > 0 {
            row.push(Span::raw("  "));
        }
        row.extend(option(i));
    }
    if row.iter().map(|s| s.content.width()).sum::<usize>() > inner {
        // Narrow terminals: one option per line.
        content.extend((0..options.len()).map(|i| Line::from(option(i))));
    } else {
        content.push(Line::from(row));
    }
    let flow = t.as_secs_f32() * 0.25;
    let color = move |i: usize, n: usize| theme::mix(theme::FAINT(), theme::mix(theme::YELLOW(), theme::PINK(), tri(i as f32 / n as f32 - flow)), 0.45 + 0.55 * k);
    let title = vec![
        Span::raw(" "),
        Span::styled("◆ ", theme::bold(amber)),
        Span::styled("Allow ", theme::bold(theme::FG())),
        Span::styled(a.tool.clone(), theme::bold(theme::YELLOW())),
        Span::styled("? ", theme::bold(theme::FG())),
    ];
    let mut out = framed(content, width, &color, title, Vec::new(), Vec::new());
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
    let body = String::new();
    let inner = frame_inner(width);
    let mut out: Vec<Line<'static>> = Vec::new();
    let mut head = vec![Span::raw(" "), Span::styled("◆ ", theme::bold(amber))];
    head.push(Span::styled(if q.header.is_empty() { "Question".to_string() } else { q.header.clone() }, theme::bold(theme::YELLOW())));
    if a.questions.len() > 1 {
        head.push(Span::styled(format!("  {}/{}", i + 1, a.questions.len()), theme::dim()));
    }
    head.push(Span::raw(" "));
    out.extend(super::markdown::wrap(vec![Span::styled(q.question.clone(), theme::bold(theme::FG()))], inner, Vec::new(), Vec::new()));
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
    let hint = if q.multi { "↑↓ move · space tick · enter confirm · esc decline" } else { "↑↓ move · enter choose · esc decline" };
    let flow = t.as_secs_f32() * 0.25;
    let color = move |i: usize, n: usize| theme::mix(theme::FAINT(), theme::mix(theme::YELLOW(), theme::PINK(), tri(i as f32 / n as f32 - flow)), 0.45 + 0.55 * k);
    let mut out = framed(out, width, &color, head, Vec::new(), vec![Span::styled(format!(" {hint} "), theme::dim())]);
    out.push(Line::default());
    out
}

fn status(app: &App, width: usize) -> Line<'static> {
    let accent = theme::backend(app.kind);
    let model = if !app.actual_model.is_empty() { app.actual_model.clone() } else if !app.model.is_empty() { app.model.clone() } else { "default".into() };
    // The provider as a half-block pill: "▐ Claude Code ▌" (greyed while the backend is down).
    let pill = if app.alive { accent } else { theme::DIM() };
    let mut left = vec![
        Span::raw(pad_str()),
        Span::styled("▐", theme::fg(pill)),
        Span::styled(format!(" {} ", app.kind.label()), theme::bold(theme::VOID()).bg(pill)),
        Span::styled("▌", theme::fg(pill)),
        Span::styled(format!(" {model}"), theme::muted()),
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
        // "ctx ▰▰▱▱▱▱▱▱▱▱ 23%": the filled part on the gradient, warning colors when it's full.
        let warn = if pct > 0.85 { Some(theme::RED()) } else if pct > 0.6 { Some(theme::YELLOW()) } else { None };
        right.push(Span::styled("ctx ", theme::dim()));
        const BAR: usize = 10;
        let filled = ((pct * BAR as f32).round() as usize).clamp(1, BAR);
        for i in 0..BAR {
            let c = if i >= filled { theme::FAINT() } else { warn.unwrap_or_else(|| theme::nebula(i as f32 / (BAR as f32 * 1.5))) };
            right.push(Span::styled(if i < filled { "▰" } else { "▱" }, theme::fg(c)));
        }
        let label = if pct < 0.01 { "<1%".to_string() } else { format!("{:.0}%", pct * 100.0) };
        right.push(Span::styled(format!(" {label}"), theme::fg(warn.unwrap_or_else(theme::FG))));
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
        right.retain(|s| !s.content.contains('⎇') && !s.content.starts_with("   "));
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
                Span::styled(match p.kind { PickKind::Tasks => " output   ", kind if kind.is_live() => " jump   ", _ => " choose   " }, theme::dim()),
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
        let mut row = Line::from(vec![
            Span::styled(if sel { "▌  " } else { "   " }, theme::bold(theme::nebula(0.45 + t.as_secs_f32() * 0.1))),
            Span::styled(label, if sel { theme::bold(theme::FG()) } else { theme::muted() }),
            Span::raw(gap),
            Span::styled(if it.current { "  ●  " } else { "     " }, theme::fg(theme::GREEN())),
            Span::styled(detail, if sel { theme::muted() } else { theme::dim() }),
        ]);
        if sel {
            let w = row.width();
            row.spans.push(Span::raw(" ".repeat(iw.saturating_sub(w))));
            for s in &mut row.spans {
                s.style = s.style.bg(theme::PANEL());
            }
        }
        lines.push(row);
    }
    if visible.is_empty() {
        lines.push(Line::from(Span::styled("   no matches", theme::dim())));
    }
    anim::fade(&mut lines, k);
    f.render_widget(Paragraph::new(lines), inner);
}

/// Hard-wraps log text to `width` columns (output keeps its spacing, unlike prose wrapping).
fn wrap_log(text: &str, width: usize) -> Vec<String> {
    let width = width.max(4);
    let mut out = Vec::new();
    for line in text.trim_end_matches('\n').split('\n') {
        let line = line.replace('\t', "    ");
        let mut cur = String::new();
        let mut w = 0;
        for c in line.chars() {
            let cw = unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
            if w + cw > width {
                out.push(std::mem::take(&mut cur));
                w = 0;
            }
            cur.push(c);
            w += cw;
        }
        out.push(cur);
    }
    out
}

/// A background task's output, live: a header with its state, then the log's tail, following
/// new output unless scrolled up.
fn task_log(f: &mut Frame, app: &mut App, full: Rect, t: Duration) {
    if app.picker.is_some() {
        return;
    }
    let k = app.overlay.value();
    let (state, running) = match app.task_log_task() {
        Some(task) => {
            let took = anim::elapsed(task.ended.unwrap_or_else(Instant::now).duration_since(task.started));
            let state = match (task.status, task.exit_code) {
                (Status::Running, _) => format!("running {took}"),
                (_, Some(code)) => format!("exit {code} after {took}"),
                (Status::Ok, None) => format!("done in {took}"),
                (_, None) => format!("stopped after {took}"),
            };
            let desc = Some(task.description.trim()).filter(|d| !d.is_empty()).map(str::to_string);
            ((state, desc, task.status, task.exit_code), task.status == Status::Running)
        }
        None => (("gone".to_string(), None, Status::Failed, None), false),
    };
    let Some(log) = &mut app.task_log else { return };
    let (state, desc, status, code) = state;
    let w = full.width.saturating_sub(4).min(110);
    let iw = w.saturating_sub(4) as usize;
    let body = if log.text.trim().is_empty() { Vec::new() } else { wrap_log(&log.text, iw) };
    let head = 2 + desc.is_some() as u16;
    let room = full.height.saturating_sub(6 + head).max(3);
    let rows = (body.len() as u16).clamp(3, room);
    let max_scroll = (body.len() as u16).saturating_sub(rows);
    log.scroll = log.scroll.min(max_scroll);
    let scroll = log.scroll;
    let h_full = (rows + head + 3).min(full.height);
    let h = ((h_full as f32) * (0.7 + 0.3 * k)).round() as u16;
    let x = full.x + (full.width.saturating_sub(w)) / 2;
    let y = full.y + (full.height.saturating_sub(h_full)) / 3 + ((1.0 - k) * 2.0).round() as u16;
    let area = Rect { x, y, width: w, height: h.max(3) };
    dim_behind(f, full, k);
    f.render_widget(Clear, area);
    let mut keys = vec![Span::styled(" ↑↓", theme::muted()), Span::styled(" scroll   ", theme::dim())];
    if scroll > 0 {
        keys.extend([Span::styled("f", theme::muted()), Span::styled(" follow   ", theme::dim())]);
    }
    if running {
        keys.extend([Span::styled("ctrl+x", theme::muted()), Span::styled(" stop   ", theme::dim())]);
    }
    keys.extend([
        Span::styled("y", theme::muted()),
        Span::styled(" copy   ", theme::dim()),
        Span::styled("enter", theme::muted()),
        Span::styled(" jump   ", theme::dim()),
        Span::styled("esc", theme::muted()),
        Span::styled(" back ", theme::dim()),
    ]);
    let block = Frame_::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(theme::fg(theme::FAINT()))
        .title(Line::from(vec![Span::raw(" "), Span::styled(truncate(&log.title, iw.saturating_sub(4)), theme::bold(theme::FG())), Span::raw(" ")]))
        .title_bottom(Line::from(keys).right_aligned());
    let inner = block.inner(area);
    f.render_widget(block, area);
    nebula_border(f, area, t, k);
    let inner = Rect { x: inner.x + 1, y: inner.y + 1, width: inner.width.saturating_sub(2), height: inner.height.saturating_sub(1) };
    let (mark, color) = match (status, code) {
        (Status::Running, _) => (anim::dots_spin(t).to_string(), theme::GREEN()),
        (Status::Ok, _) | (_, Some(0)) => ("✓".to_string(), theme::GREEN()),
        (_, Some(_)) => ("✗".to_string(), theme::RED()),
        _ => ("■".to_string(), theme::DIM()),
    };
    let mut where_ = Vec::new();
    if scroll > 0 {
        where_.push(Span::styled(format!("   ↑ {scroll} lines up · paused"), theme::fg(theme::VIOLET())));
    } else if running {
        where_.push(Span::styled("   following live", theme::dim()));
    }
    let mut lines = vec![Line::from([vec![Span::styled(format!("{mark} "), theme::fg(color)), Span::styled(state, theme::muted())], where_].concat())];
    if let Some(d) = desc {
        lines.push(Line::from(Span::styled(truncate(&d, iw), theme::dim())));
    }
    lines.push(Line::default());
    if body.is_empty() {
        lines.push(Line::from(Span::styled(if running { "no output yet" } else { "no output" }, theme::dim())));
    } else {
        let room = inner.height.saturating_sub(lines.len() as u16) as usize;
        let end = body.len().saturating_sub(scroll as usize);
        let start = end.saturating_sub(room);
        lines.extend(body[start..end].iter().map(|l| Line::from(Span::styled(l.clone(), theme::fg(theme::FG())))));
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
