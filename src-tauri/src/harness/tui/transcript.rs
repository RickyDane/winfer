//! The conversation as blocks (user, assistant, thinking, tool calls, notices) and how each
//! renders. Static parts are cached per width; only running blocks re-render every frame.

use std::time::{Duration, Instant};

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use super::markdown::{self, wrap};
use super::{anim, theme};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Running,
    Ok,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoticeKind {
    Info,
    Error,
    Success,
}

#[derive(Debug, Clone)]
pub enum Block {
    User { text: String },
    Assistant { id: String, text: String },
    Reasoning { id: String, text: String, started: Instant, ended: Option<Instant> },
    Tool { id: String, name: String, title: String, output: String, diff: Option<String>, status: Status, started: Instant, ended: Option<Instant> },
    Notice { text: String, kind: NoticeKind },
}

pub struct Item {
    pub block: Block,
    cache: Option<(usize, bool, Vec<Line<'static>>)>,
}

impl Item {
    pub fn new(block: Block) -> Self {
        Item { block, cache: None }
    }

    pub fn touch(&mut self) {
        self.cache = None;
    }

    pub fn animated(&self) -> bool {
        match &self.block {
            Block::Tool { status, .. } => *status == Status::Running,
            Block::Reasoning { ended, .. } => ended.is_none(),
            _ => false,
        }
    }

    pub fn lines(&mut self, width: usize, expanded: bool, now: Instant, epoch: Instant) -> Vec<Line<'static>> {
        if !self.animated() {
            if let Some((w, e, lines)) = &self.cache {
                if *w == width && *e == expanded {
                    return lines.clone();
                }
            }
        }
        let lines = render(&self.block, width, expanded, now, epoch);
        if !self.animated() {
            self.cache = Some((width, expanded, lines.clone()));
        }
        lines
    }
}

fn render(block: &Block, width: usize, expanded: bool, now: Instant, epoch: Instant) -> Vec<Line<'static>> {
    let t = now.duration_since(epoch);
    match block {
        Block::User { text } => {
            let bar = Span::styled("▌ ", theme::fg(theme::ACCENT));
            let style = Style::default().fg(theme::FG).bg(theme::SURFACE);
            let mut out = Vec::new();
            for line in text.lines() {
                let spans = vec![Span::styled(line.to_string(), style)];
                for l in wrap(spans, width.saturating_sub(1), vec![bar.clone()], vec![bar.clone()]) {
                    // Fill the row so the block reads as one bubble.
                    let w = l.width();
                    let mut spans = l.spans;
                    spans.push(Span::styled(" ".repeat(width.saturating_sub(w + 1)), style));
                    if let Some(first) = spans.get_mut(0) {
                        first.style = first.style.bg(theme::SURFACE);
                    }
                    out.push(Line::from(spans));
                }
            }
            if out.is_empty() {
                out.push(Line::from(vec![bar]));
            }
            out
        }
        Block::Assistant { text, .. } => {
            let mut lines = markdown::render(text, width.saturating_sub(2));
            for (i, l) in lines.iter_mut().enumerate() {
                let lead = if i == 0 { Span::styled("◆ ", theme::fg(theme::ACCENT)) } else { Span::raw("  ") };
                l.spans.insert(0, lead);
            }
            lines
        }
        Block::Reasoning { text, started, ended, .. } => {
            let mut out = Vec::new();
            match ended {
                None => {
                    let mut head = vec![Span::styled(format!("{} ", anim::orb(t)), theme::fg(theme::ACCENT))];
                    head.extend(anim::shimmer("Thinking…", t, theme::DIM, theme::FG));
                    out.push(Line::from(head));
                }
                // Redacted thinking (no text) isn't worth a line once it's over.
                Some(_) if text.trim().is_empty() => return Vec::new(),
                Some(end) => {
                    let secs = end.duration_since(*started).as_secs().max(1);
                    let hint = if expanded || text.trim().is_empty() { String::new() } else { "  (ctrl+o)".into() };
                    out.push(Line::from(vec![
                        Span::styled("✻ ", theme::dim()),
                        Span::styled(format!("Thought for {secs}s"), theme::dim().add_modifier(Modifier::ITALIC)),
                        Span::styled(hint, theme::fg(theme::FAINT)),
                    ]));
                }
            }
            let body = text.trim();
            if !body.is_empty() && (expanded || ended.is_none()) {
                let style = theme::dim().add_modifier(Modifier::ITALIC);
                let all: Vec<Line> = body
                    .lines()
                    .flat_map(|l| wrap(vec![Span::styled(l.to_string(), style)], width, vec![Span::raw("  ")], vec![Span::raw("  ")]))
                    .collect();
                let keep = if expanded { all.len() } else { 3 };
                let skip = all.len().saturating_sub(keep);
                out.extend(all.into_iter().skip(skip));
            }
            out
        }
        Block::Tool { name, title, output, diff, status, started, ended, .. } => {
            let mut out = Vec::new();
            let (dot, color) = match status {
                Status::Running => (anim::spinner(t).to_string(), theme::ACCENT),
                Status::Ok => ("●".into(), theme::GREEN),
                Status::Failed => ("●".into(), theme::RED),
            };
            let mut head = vec![Span::styled(format!("{dot} "), theme::fg(color)), Span::styled(name.clone(), theme::bold(theme::FG))];
            let room = width.saturating_sub(name.width() + 4 + 10);
            if !title.is_empty() {
                head.push(Span::raw("  "));
                head.push(Span::styled(truncate(title, room), theme::muted()));
            }
            let took = ended.unwrap_or(now).duration_since(*started);
            if *status == Status::Running && took > Duration::from_secs(2) {
                head.push(Span::styled(format!("  {}", anim::elapsed(took)), theme::dim()));
            }
            out.push(Line::from(head));
            let gutter = || vec![Span::styled("  ⎿ ", theme::fg(theme::FAINT))];
            let pad = || vec![Span::raw("    ")];
            if let Some(d) = diff.as_deref().filter(|d| !d.trim().is_empty()) {
                let (add, del) = crate::harness::diff::stat(d);
                let mut first = gutter();
                first.push(Span::styled(format!("+{add}", ), theme::fg(theme::GREEN)));
                first.push(Span::styled(format!(" -{del}"), theme::fg(theme::RED)));
                out.push(Line::from(first));
                let lines = diff_lines(d, width.saturating_sub(4));
                let max = if expanded { usize::MAX } else { 16 };
                let total = lines.len();
                for l in lines.into_iter().take(max) {
                    let mut spans = pad();
                    spans.extend(l.spans);
                    out.push(Line::from(spans));
                }
                if total > max {
                    out.push(Line::from(vec![Span::raw("    "), Span::styled(format!("… +{} lines (ctrl+o to expand)", total - max), theme::dim())]));
                }
                return out;
            }
            let text = output.trim_end();
            if text.is_empty() {
                if *status == Status::Failed {
                    let mut l = gutter();
                    l.push(Span::styled("failed", theme::fg(theme::RED)));
                    out.push(Line::from(l));
                }
                return out;
            }
            let style = if *status == Status::Failed { theme::fg(theme::RED) } else { theme::dim() };
            let raw: Vec<&str> = text.lines().collect();
            let max = if expanded { usize::MAX } else if *status == Status::Running { 5 } else { 4 };
            // While running, show the tail (live output); when done, the head.
            let (shown, hidden): (Vec<&str>, usize) = if *status == Status::Running {
                let skip = raw.len().saturating_sub(max);
                (raw[skip..].to_vec(), skip)
            } else {
                (raw.iter().take(max).copied().collect(), raw.len().saturating_sub(max))
            };
            for (i, line) in shown.iter().enumerate() {
                let first = if i == 0 { gutter() } else { pad() };
                let line = truncate(&crate::harness::tools::strip_ansi(line).replace('\t', "  "), width.saturating_sub(5).max(10));
                out.push(Line::from([first, vec![Span::styled(line, style)]].concat()));
            }
            if hidden > 0 {
                out.push(Line::from(vec![Span::raw("    "), Span::styled(format!("… +{hidden} lines (ctrl+o to expand)"), theme::fg(theme::FAINT))]));
            }
            out
        }
        Block::Notice { text, kind } => {
            let (mark, style) = match kind {
                NoticeKind::Info => ("· ", theme::dim()),
                NoticeKind::Error => ("✗ ", theme::fg(theme::RED)),
                NoticeKind::Success => ("✓ ", theme::fg(theme::GREEN)),
            };
            text.lines()
                .enumerate()
                .flat_map(|(i, l)| {
                    let first = if i == 0 { vec![Span::styled(mark, style)] } else { vec![Span::raw("  ")] };
                    wrap(vec![Span::styled(l.to_string(), style)], width, first, vec![Span::raw("  ")])
                })
                .collect()
        }
    }
}

pub fn truncate(s: &str, max: usize) -> String {
    if s.width() <= max {
        return s.to_string();
    }
    let mut out = String::new();
    let mut w = 0;
    for c in s.chars() {
        let cw = unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
        if w + cw + 1 > max {
            break;
        }
        out.push(c);
        w += cw;
    }
    out.push('…');
    out
}

/// A unified diff as colored lines with line numbers.
pub fn diff_lines(diff: &str, width: usize) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    let (mut old_n, mut new_n) = (0usize, 0usize);
    let mut first_hunk = true;
    for line in diff.lines() {
        if line.starts_with("+++") {
            continue;
        }
        if let Some(path) = line.strip_prefix("--- ") {
            let path = path.trim_start_matches("a/");
            if path != "/dev/null" && diff.matches("\n--- ").count() + usize::from(diff.starts_with("--- ")) > 1 {
                out.push(Line::from(Span::styled(path.to_string(), theme::bold(theme::MUTED))));
            }
            first_hunk = true;
            continue;
        }
        if let Some(rest) = line.strip_prefix("@@") {
            // @@ -a,b +c,d @@
            let nums: Vec<usize> = rest
                .split(|c: char| !c.is_ascii_digit())
                .filter(|s| !s.is_empty())
                .filter_map(|s| s.parse().ok())
                .collect();
            let plus = rest.find('+').unwrap_or(0);
            let before: Vec<usize> = rest[..plus].split(|c: char| !c.is_ascii_digit()).filter_map(|s| s.parse().ok()).collect();
            old_n = before.first().copied().unwrap_or(1);
            new_n = rest[plus..].split(|c: char| !c.is_ascii_digit()).find_map(|s| s.parse().ok()).unwrap_or(old_n);
            let _ = nums;
            if !first_hunk {
                out.push(Line::from(Span::styled("    ⋯", theme::fg(theme::FAINT))));
            }
            first_hunk = false;
            continue;
        }
        let (num, sign, style) = if let Some(rest) = line.strip_prefix('+') {
            new_n += 1;
            (new_n - 1, "+", (rest, Style::default().fg(theme::GREEN).bg(theme::DIFF_ADD_BG)))
        } else if let Some(rest) = line.strip_prefix('-') {
            old_n += 1;
            (old_n - 1, "-", (rest, Style::default().fg(theme::RED).bg(theme::DIFF_DEL_BG)))
        } else if line.starts_with('\\') {
            continue;
        } else {
            old_n += 1;
            new_n += 1;
            (new_n - 1, " ", (line.strip_prefix(' ').unwrap_or(line), theme::dim()))
        };
        let (text, style) = style;
        let text = truncate(&text.replace('\t', "    "), width.saturating_sub(8).max(10));
        let fill = width.saturating_sub(7 + text.width());
        let mut spans = vec![Span::styled(format!("{num:>4} "), theme::fg(theme::FAINT)), Span::styled(format!("{sign} "), style), Span::styled(text, style)];
        if sign != " " {
            spans.push(Span::styled(" ".repeat(fill), style));
        }
        out.push(Line::from(spans));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diff_lines_number_and_color() {
        let d = crate::harness::diff::unified("a.txt", "a\nb\nc\n", "a\nB\nc\n");
        let lines = diff_lines(&d, 40);
        let text: Vec<String> = lines.iter().map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect::<String>().trim_end().to_string()).collect();
        assert_eq!(text, vec!["   1   a", "   2 - b", "   2 + B", "   3   c"]);
    }

    #[test]
    fn truncates_with_ellipsis() {
        assert_eq!(truncate("hello world", 6), "hello…");
        assert_eq!(truncate("hi", 6), "hi");
    }
}
