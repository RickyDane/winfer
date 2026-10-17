//! Drawing: transcript, welcome screen, todo panel, activity line, composer or approval prompt,
//! status bar, completions and pickers.

use std::time::{Duration, Instant};

use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block as Frame_, BorderType, Borders, Clear, Paragraph};
use unicode_width::UnicodeWidthStr;

use super::app::App;
use super::transcript::{NoticeKind, diff_lines, truncate};
use super::{anim, theme};
use crate::harness::event::Mode;

const LOGO: [&str; 2] = ["█ █ █ █ █▀▄ █▀▀ █▀▀ █▀▄", "▀▄▀▄▀ █ █ █ █▀  ██▄ █▀▄"];

pub fn draw(f: &mut Frame, app: &mut App) {
    let area = f.area();
    let now = Instant::now();
    let t = now.duration_since(app.epoch);
    let width = area.width as usize;

    // Bottom-up layout.
    let status_h = 1u16;
    let (bottom_lines, cursor) = if app.approvals.is_empty() { composer_lines(app, width, t) } else { (approval_lines(app, width, t), None) };
    let bottom_h = (bottom_lines.len() as u16).min(area.height.saturating_sub(4));
    let activity_h = 1u16;
    let todo_lines = todo_lines(app, width);
    let todo_h = (todo_lines.len() as u16).min(area.height / 4);
    let queue_lines: Vec<Line> = app
        .queue
        .iter()
        .take(3)
        .map(|m| Line::from(vec![Span::styled("  ⏵ ", theme::fg(theme::FAINT)), Span::styled(truncate(m.text.lines().next().unwrap_or(""), width.saturating_sub(16)), theme::dim()), Span::styled("  queued", theme::fg(theme::FAINT))]))
        .collect();
    let queue_h = queue_lines.len() as u16;
    let fixed = status_h + bottom_h + activity_h + todo_h + queue_h;
    let main_h = area.height.saturating_sub(fixed);

    let main = Rect { x: area.x, y: area.y, width: area.width, height: main_h };
    let mut y = main.y + main_h;
    let todo_area = Rect { x: area.x, y, width: area.width, height: todo_h };
    y += todo_h;
    let queue_area = Rect { x: area.x, y, width: area.width, height: queue_h };
    y += queue_h;
    let activity_area = Rect { x: area.x, y, width: area.width, height: activity_h };
    y += activity_h;
    let bottom_area = Rect { x: area.x, y, width: area.width, height: bottom_h };
    y += bottom_h;
    let status_area = Rect { x: area.x, y, width: area.width, height: status_h };

    if app.items.is_empty() {
        app.view = Default::default();
        welcome(f, app, main, t);
    } else {
        transcript(f, app, main, now);
    }
    f.render_widget(Paragraph::new(todo_lines), todo_area);
    f.render_widget(Paragraph::new(queue_lines), queue_area);
    f.render_widget(Paragraph::new(activity(app, width, t)), activity_area);
    f.render_widget(Paragraph::new(bottom_lines), bottom_area);
    if let Some((cx, cy)) = cursor {
        if app.picker.is_none() {
            f.set_cursor_position(Position { x: bottom_area.x + cx, y: bottom_area.y + cy });
        }
    }
    f.render_widget(Paragraph::new(status(app, width)), status_area);
    completion(f, app, main);
    picker(f, app, area);
}

fn transcript(f: &mut Frame, app: &mut App, area: Rect, now: Instant) {
    let width = (area.width as usize).saturating_sub(2).max(10);
    let mut lines: Vec<Line<'static>> = Vec::new();
    let expanded = app.expanded;
    let epoch = app.epoch;
    let n = app.items.len();
    for (i, item) in app.items.iter_mut().enumerate() {
        let item_lines = item.lines(width, expanded, now, epoch);
        if item_lines.is_empty() {
            continue;
        }
        lines.extend(item_lines);
        if i + 1 < n {
            lines.push(Line::default());
        }
    }
    let h = area.height as usize;
    let total = lines.len();
    let max_scroll = total.saturating_sub(h);
    app.scroll = app.scroll.min(max_scroll);
    let end = total - app.scroll;
    let start = end.saturating_sub(h);
    let visible: Vec<Line> = lines[start..end].to_vec();
    // Bottom-align short transcripts so the conversation sits by the prompt.
    let pad = h.saturating_sub(visible.len()) as u16;
    let shown = visible.len();
    let inner = Rect { x: area.x + 1, y: area.y + pad, width: area.width.saturating_sub(1), height: area.height - pad };
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
                    cell.set_bg(theme::SELECTION);
                }
            }
        }
    }
    app.view = super::selection::View { lines, top: area.y, bottom: area.y + area.height, x: inner.x, y: inner.y, start, shown };
    if app.scroll > 0 {
        let label = format!(" ↓ {} more lines · ctrl+l to jump ", app.scroll);
        let w = label.width() as u16;
        let r = Rect { x: area.x + area.width.saturating_sub(w + 2), y: area.y + area.height.saturating_sub(1), width: w, height: 1 };
        f.render_widget(Paragraph::new(Line::from(Span::styled(label, Style::default().fg(theme::FG).bg(theme::FAINT)))), r);
    }
}

fn welcome(f: &mut Frame, app: &App, area: Rect, t: Duration) {
    let accent = theme::backend(app.kind);
    let mut lines: Vec<Line> = Vec::new();
    for row in LOGO {
        lines.push(Line::from(anim::gradient(row, t, theme::ACCENT, accent)).centered());
    }
    lines.push(Line::default());
    lines.push(Line::from(vec![Span::styled("winfer code", theme::bold(theme::FG)), Span::styled(format!("  v{}", env!("CARGO_PKG_VERSION")), theme::dim())]).centered());
    lines.push(Line::default());
    let model = if app.actual_model.is_empty() { app.model.clone() } else { app.actual_model.clone() };
    lines.push(
        Line::from(vec![
            Span::styled("● ", theme::fg(accent)),
            Span::styled(app.kind.label(), theme::fg(theme::FG)),
            Span::styled(if model.is_empty() { String::new() } else { format!(" · {model}") }, theme::muted()),
            Span::styled(format!("   {} mode", app.config.mode.label()), theme::dim()),
        ])
        .centered(),
    );
    lines.push(Line::from(Span::styled(crate::harness::tools::short_path(&app.cwd.to_string_lossy()), theme::dim())).centered());
    lines.push(Line::default());
    let tips = [
        ("/", "commands"),
        ("@", "attach files"),
        ("!", "run a shell command"),
        ("ctrl+t", "switch model"),
        ("shift+tab", "permission mode"),
    ];
    for (k, d) in tips {
        lines.push(Line::from(vec![Span::styled(format!("{k:>10}  "), theme::fg(theme::ACCENT)), Span::styled(format!("{d:<22}"), theme::muted())]).centered());
    }
    let h = lines.len() as u16;
    let y = area.y + area.height.saturating_sub(h) / 2;
    f.render_widget(Paragraph::new(lines), Rect { x: area.x, y, width: area.width, height: h.min(area.height) });
}

fn todo_lines(app: &App, width: usize) -> Vec<Line<'static>> {
    if app.todos.is_empty() || app.todos.iter().all(|t| t.status == "completed") && !app.busy {
        return Vec::new();
    }
    let done = app.todos.iter().filter(|t| t.status == "completed").count();
    let mut out = vec![Line::from(vec![
        Span::styled("  ☰ ", theme::fg(theme::ACCENT)),
        Span::styled("Tasks ", theme::bold(theme::FG)),
        Span::styled(format!("{done}/{}", app.todos.len()), theme::dim()),
    ])];
    for todo in app.todos.iter().take(8) {
        let (mark, style) = match todo.status.as_str() {
            "completed" => ("✓", theme::fg(theme::GREEN)),
            "in_progress" => ("▸", theme::bold(theme::ACCENT)),
            _ => ("○", theme::dim()),
        };
        let text_style = match todo.status.as_str() {
            "completed" => theme::dim().add_modifier(Modifier::CROSSED_OUT),
            "in_progress" => theme::fg(theme::FG),
            _ => theme::muted(),
        };
        out.push(Line::from(vec![Span::styled(format!("    {mark} "), style), Span::styled(truncate(&todo.content, width.saturating_sub(8)), text_style)]));
    }
    out
}

fn activity(app: &App, width: usize, t: Duration) -> Line<'static> {
    if !app.approvals.is_empty() {
        let k = anim::pulse(t, 1400);
        return Line::from(vec![
            Span::styled("  ◆ ", theme::fg(theme::mix(theme::YELLOW, theme::FG, k * 0.5))),
            Span::styled("Waiting for your approval", theme::fg(theme::YELLOW)),
            if app.approvals.len() > 1 { Span::styled(format!(" (+{} more)", app.approvals.len() - 1), theme::dim()) } else { Span::raw("") },
        ]);
    }
    if app.busy {
        let elapsed = app.turn_started.elapsed();
        let mut spans = vec![Span::styled(format!("  {} ", anim::orb(t)), theme::fg(theme::backend(app.kind)))];
        spans.extend(anim::shimmer(&format!("{}…", app.verb), t, theme::backend(app.kind), theme::FG));
        let mut info = format!(" {}", anim::elapsed(elapsed));
        if app.usage.output > 0 {
            info.push_str(&format!(" · ↓ {}", anim::tokens(app.usage.output)));
        }
        info.push_str(" · esc to interrupt");
        spans.push(Span::styled(info, theme::dim()));
        return Line::from(spans);
    }
    if let Some((text, at, kind)) = &app.toast {
        let age = at.elapsed().as_millis() as f32;
        if age < 4000.0 {
            let fade = ((age - 2500.0) / 1500.0).clamp(0.0, 1.0);
            let base = match kind {
                NoticeKind::Error => theme::RED,
                NoticeKind::Success => theme::GREEN,
                NoticeKind::Info => theme::FG,
            };
            let color = theme::mix(base, theme::FAINT, fade);
            return Line::from(vec![Span::styled("  ", Style::default()), Span::styled(truncate(text, width.saturating_sub(4)), theme::fg(color))]);
        }
    }
    if app.quit_armed.is_some_and(|q| q.elapsed() < Duration::from_millis(1500)) {
        return Line::from(Span::styled("  Press ctrl+c again to exit", theme::fg(theme::YELLOW)));
    }
    Line::default()
}

/// The prompt box; returns its lines and the cursor position inside it.
fn composer_lines(app: &App, width: usize, t: Duration) -> (Vec<Line<'static>>, Option<(u16, u16)>) {
    let inner = width.saturating_sub(6).max(4);
    let (rows, (cr, cc)) = app.composer.layout(inner);
    let max_rows = 10;
    let first = cr.saturating_sub(max_rows - 1);
    let shown: Vec<&String> = rows.iter().skip(first).take(max_rows).collect();
    let accent = theme::backend(app.kind);
    let border = if app.busy {
        theme::mix(theme::FAINT, accent, 0.35 + 0.65 * anim::pulse(t, 1800))
    } else if app.composer.is_empty() {
        theme::FAINT
    } else {
        theme::mix(theme::FAINT, theme::ACCENT, 0.6)
    };
    let b = theme::fg(border);
    let mut out = vec![Line::from(vec![Span::styled("╭", b), Span::styled("─".repeat(width.saturating_sub(2)), b), Span::styled("╮", b)])];
    let text_style = theme::fg(theme::FG);
    if app.composer.is_empty() {
        let hint = if app.busy { "Queue a follow-up…" } else { "Ask anything…  / commands · @ files · ! shell" };
        out.push(Line::from(vec![
            Span::styled("│ ", b),
            Span::styled("› ", theme::bold(theme::ACCENT)),
            Span::styled(truncate(hint, inner), theme::fg(theme::FAINT)),
            Span::raw(" ".repeat(inner.saturating_sub(hint.width().min(inner)))),
            Span::styled(" │", b),
        ]));
    } else {
        for (i, row) in shown.iter().enumerate() {
            let lead = if i == 0 && first == 0 { Span::styled("› ", theme::bold(theme::ACCENT)) } else { Span::raw("  ") };
            let style = if row.starts_with('/') && i == 0 { theme::bold(theme::ACCENT) } else { text_style };
            let mut spans = vec![Span::styled("│ ", b), lead];
            spans.extend(highlight_tags(row, style));
            spans.push(Span::raw(" ".repeat(inner.saturating_sub(row.width()))));
            spans.push(Span::styled(" │", b));
            out.push(Line::from(spans));
        }
    }
    out.push(Line::from(vec![Span::styled("╰", b), Span::styled("─".repeat(width.saturating_sub(2)), b), Span::styled("╯", b)]));
    let cursor = (4 + cc as u16, 1 + (cr - first) as u16);
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
        out.push(Span::styled(rest[i..end].to_string(), theme::fg(theme::CYAN)));
        rest = &rest[end..];
    }
    out
}

fn approval_lines(app: &App, width: usize, t: Duration) -> Vec<Line<'static>> {
    let a = app.approvals.front().unwrap();
    let k = anim::pulse(t, 1400);
    let b = theme::fg(theme::mix(theme::mix(theme::FAINT, theme::YELLOW, 0.6), theme::YELLOW, k));
    let inner = width.saturating_sub(4);
    let row = |spans: Vec<Span<'static>>| {
        let w: usize = spans.iter().map(|s| s.content.width()).sum();
        let mut line = vec![Span::styled("│ ", b)];
        line.extend(spans);
        line.push(Span::raw(" ".repeat(inner.saturating_sub(w))));
        line.push(Span::styled(" │", b));
        Line::from(line)
    };
    let title = format!(" Allow {}? ", a.tool);
    let mut out = vec![Line::from(vec![
        Span::styled("╭─", b),
        Span::styled(title.clone(), theme::bold(theme::YELLOW)),
        Span::styled("─".repeat(width.saturating_sub(title.width() + 3)), b),
        Span::styled("╮", b),
    ])];
    for line in a.title.lines().take(6) {
        out.push(row(vec![Span::styled(truncate(line, inner), theme::fg(theme::FG))]));
    }
    if !a.detail.is_empty() && a.detail != a.title {
        out.push(row(vec![Span::styled(truncate(&a.detail, inner), theme::dim())]));
    }
    if let Some(d) = &a.diff {
        let lines = diff_lines(d, inner);
        let max = if app.expanded { 40 } else { 12 };
        let total = lines.len();
        for l in lines.into_iter().take(max) {
            out.push(row(l.spans));
        }
        if total > max {
            out.push(row(vec![Span::styled(format!("… +{} lines (ctrl+o)", total - max), theme::dim())]));
        }
    }
    out.push(row(vec![]));
    let options = ["Yes", "Yes, don't ask again this session", "No, tell it what to do"];
    let keys = ["y", "a", "n"];
    let mut spans = Vec::new();
    for (i, (o, key)) in options.iter().zip(keys).enumerate() {
        let selected = i == app.approval_choice;
        let style = if selected { Style::default().fg(theme::SURFACE).bg(theme::YELLOW).add_modifier(Modifier::BOLD) } else { theme::fg(theme::FG) };
        spans.push(Span::styled(format!(" {key} "), if selected { style } else { theme::fg(theme::YELLOW) }));
        spans.push(Span::styled(format!("{o} "), style));
        spans.push(Span::raw("  "));
    }
    let w: usize = spans.iter().map(|s| s.content.width()).sum();
    if w > inner {
        // Narrow terminals: one option per line.
        for (i, (o, key)) in options.iter().zip(keys).enumerate() {
            let selected = i == app.approval_choice;
            let style = if selected { theme::bold(theme::YELLOW) } else { theme::fg(theme::FG) };
            out.push(row(vec![Span::styled(format!("{} {key} ", if selected { "›" } else { " " }), theme::fg(theme::YELLOW)), Span::styled(o.to_string(), style)]));
        }
    } else {
        out.push(row(spans));
    }
    out.push(Line::from(vec![Span::styled("╰", b), Span::styled("─".repeat(width.saturating_sub(2)), b), Span::styled("╯", b)]));
    out
}

fn status(app: &App, width: usize) -> Line<'static> {
    let accent = theme::backend(app.kind);
    let model = if !app.actual_model.is_empty() { app.actual_model.clone() } else if !app.model.is_empty() { app.model.clone() } else { "default".into() };
    let dot = if app.alive { "●" } else { "○" };
    let mut left = vec![
        Span::styled(format!(" {dot} "), theme::fg(accent)),
        Span::styled(app.kind.label().to_string(), theme::fg(theme::FG)),
        Span::styled(format!(" {model}"), theme::muted()),
    ];
    if let Some(e) = &app.config.effort {
        left.push(Span::styled(format!(" · {e}"), theme::dim()));
    }
    let mode_color = match app.config.mode {
        Mode::Ask => None,
        Mode::AutoEdit => Some(theme::GREEN),
        Mode::Plan => Some(theme::CYAN),
        Mode::Yolo => Some(theme::RED),
    };
    left.push(Span::raw("  "));
    match mode_color {
        Some(color) => left.push(Span::styled(format!(" {} ", app.config.mode.label()), Style::default().fg(theme::SURFACE).bg(color).add_modifier(Modifier::BOLD))),
        None => left.push(Span::styled(format!("{} mode", app.config.mode.label()), theme::dim())),
    }
    let mut right = Vec::new();
    let pct = app.ctx_shown.value();
    if app.usage.window > 0 && pct > 0.0 {
        let color = if pct > 0.85 { theme::RED } else if pct > 0.6 { theme::YELLOW } else { theme::DIM };
        let bar_w = 8;
        let filled = ((pct * bar_w as f32).round() as usize).min(bar_w);
        right.push(Span::styled("━".repeat(filled), theme::fg(color)));
        right.push(Span::styled("━".repeat(bar_w - filled), theme::fg(theme::FAINT)));
        let label = if pct < 0.01 { " <1%".to_string() } else { format!(" {:.0}%", pct * 100.0) };
        right.push(Span::styled(label, theme::fg(color)));
    }
    if let Some(c) = app.usage.cost {
        right.push(Span::styled(format!("  ${c:.2}"), theme::dim()));
    }
    let dir = crate::harness::tools::short_path(&app.cwd.to_string_lossy());
    let dir = dir.rsplit('/').next().unwrap_or(&dir).to_string();
    right.push(Span::styled(format!("  {dir}"), theme::dim()));
    if let Some(b) = &app.branch {
        right.push(Span::styled(format!(" ⎇ {b}"), theme::fg(theme::FAINT)));
    }
    right.push(Span::raw(" "));
    let lw: usize = left.iter().map(|s| s.content.width()).sum();
    let rw: usize = right.iter().map(|s| s.content.width()).sum();
    if lw + rw + 1 > width {
        // Narrow: drop the directory and branch first.
        right.retain(|s| !s.content.contains('⎇') && !s.content.starts_with("  ") || s.content.contains('%') || s.content.contains('$'));
    }
    let rw: usize = right.iter().map(|s| s.content.width()).sum();
    let mut spans = left;
    spans.push(Span::raw(" ".repeat(width.saturating_sub(lw + rw))));
    spans.extend(right);
    Line::from(spans)
}

fn completion(f: &mut Frame, app: &App, main: Rect) {
    let Some(c) = &app.completion else { return };
    let h = (c.items.len() as u16 + 2).min(main.height);
    let w = (main.width.saturating_sub(4)).min(72);
    let area = Rect { x: main.x + 2, y: main.y + main.height.saturating_sub(h), width: w, height: h };
    f.render_widget(Clear, area);
    let block = Frame_::default().borders(Borders::ALL).border_type(BorderType::Rounded).border_style(theme::fg(theme::FAINT)).style(Style::default().bg(theme::SURFACE));
    let inner = block.inner(area);
    f.render_widget(block, area);
    let iw = inner.width as usize;
    let label_w = c.items.iter().map(|i| i.label.width()).max().unwrap_or(0).min(iw.saturating_sub(4));
    let lines: Vec<Line> = c
        .items
        .iter()
        .enumerate()
        .map(|(i, it)| {
            let sel = i == c.selected;
            let bg = if sel { theme::FAINT } else { theme::SURFACE };
            let label = truncate(&it.label, label_w);
            let gap = " ".repeat(label_w.saturating_sub(label.width()) + 2);
            let detail = truncate(&it.detail, iw.saturating_sub(label_w + 4));
            let pad = iw.saturating_sub(label_w + detail.width() + 4);
            Line::from(vec![
                Span::styled(if sel { "› " } else { "  " }, theme::fg(theme::ACCENT).bg(bg)),
                Span::styled(label, if sel { theme::bold(theme::FG).bg(bg) } else { theme::fg(theme::FG).bg(bg) }),
                Span::styled(gap, Style::default().bg(bg)),
                Span::styled(detail, if sel { theme::muted().bg(bg) } else { theme::dim().bg(bg) }),
                Span::styled(" ".repeat(pad), Style::default().bg(bg)),
            ])
        })
        .collect();
    f.render_widget(Paragraph::new(lines), inner);
}

fn picker(f: &mut Frame, app: &App, full: Rect) {
    let Some(p) = &app.picker else { return };
    let k = app.overlay.value();
    let visible = p.visible();
    let w = full.width.saturating_sub(4).min(76);
    let groups = if p.filter.is_empty() { visible.iter().map(|&i| &p.items[i].group).filter(|g| !g.is_empty()).collect::<std::collections::HashSet<_>>().len() } else { 0 };
    let rows = (full.height.saturating_sub(8)).min(18).min((visible.len() + groups).max(4) as u16) as usize;
    let h_full = (rows as u16 + 4).min(full.height);
    let h = ((h_full as f32) * (0.6 + 0.4 * k)).round() as u16;
    let x = full.x + (full.width.saturating_sub(w)) / 2;
    let y = full.y + (full.height.saturating_sub(h_full)) / 3 + ((1.0 - k) * 3.0) as u16;
    let area = Rect { x, y, width: w, height: h.max(3) };
    f.render_widget(Clear, area);
    let border = theme::mix(theme::FAINT, theme::ACCENT, k);
    let block = Frame_::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(theme::fg(border))
        .title(Line::from(vec![Span::styled(format!(" {} ", p.title), theme::bold(theme::FG))]))
        .title_bottom(Line::from(Span::styled(" ↑↓ select · enter choose · esc close ", theme::dim())).right_aligned())
        .style(Style::default().bg(theme::SURFACE));
    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.height == 0 {
        return;
    }
    let iw = inner.width as usize;
    let mut lines = vec![Line::from(vec![
        Span::styled(" ⌕ ", theme::fg(theme::ACCENT)),
        if p.filter.is_empty() { Span::styled("type to filter", theme::fg(theme::FAINT)) } else { Span::styled(p.filter.clone(), theme::fg(theme::FG)) },
        if p.loading { Span::styled("   loading models…", theme::dim()) } else { Span::raw("") },
    ])];
    lines.push(Line::from(Span::styled("─".repeat(iw), theme::fg(theme::FAINT))));
    let room = inner.height.saturating_sub(2) as usize;
    // Keep the selection in view.
    let mut entries: Vec<(Option<String>, usize)> = Vec::new();
    let mut last_group = String::new();
    for &i in &visible {
        let g = &p.items[i].group;
        if p.filter.is_empty() && !g.is_empty() && *g != last_group {
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
            lines.push(Line::from(Span::styled(format!(" {g}"), theme::bold(theme::DIM))));
            continue;
        }
        let it = &p.items[*i];
        let sel = Some(*i) == sel_item;
        let bg = if sel { theme::FAINT } else { theme::SURFACE };
        let label_style = if sel { theme::bold(theme::FG).bg(bg) } else { theme::fg(theme::FG).bg(bg) };
        let mark = if it.current { "✓" } else { " " };
        let label = truncate(&it.label, label_w);
        let gap = " ".repeat(label_w.saturating_sub(label.width()));
        let detail = truncate(&it.detail, iw.saturating_sub(label_w + 7));
        let pad = iw.saturating_sub(label_w + detail.width() + 7);
        lines.push(Line::from(vec![
            Span::styled(if sel { " › " } else { "   " }, theme::fg(theme::ACCENT).bg(bg)),
            Span::styled(label, label_style),
            Span::styled(gap, Style::default().bg(bg)),
            Span::styled(format!(" {mark}  "), theme::fg(theme::GREEN).bg(bg)),
            Span::styled(detail, theme::dim().bg(bg)),
            Span::styled(" ".repeat(pad), Style::default().bg(bg)),
        ]));
    }
    if visible.is_empty() {
        lines.push(Line::from(Span::styled("   No matches", theme::dim())));
    }
    f.render_widget(Paragraph::new(lines), inner);
}
