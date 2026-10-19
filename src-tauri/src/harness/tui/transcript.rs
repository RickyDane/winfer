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
    /// A thin rule closing a turn, with how long it took in the middle.
    TurnTime { took: Duration },
}

pub struct Item {
    pub block: Block,
    pub background: bool,
    pub exit_code: Option<i32>,
    /// When it appeared, for the fade-in.
    pub born: Instant,
    /// Changes whenever the item does (see `touch`); the view caches by it.
    pub version: u64,
    /// A tool card clicked open: shows everything, like ctrl+o does for all of them.
    pub open: bool,
    cache: Option<(usize, bool, Vec<Line<'static>>)>,
}

fn next_version() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

/// How long a new block takes to fade in, and a finished tool's dot to settle.
const FADE_IN: Duration = Duration::from_millis(320);
const SETTLE: Duration = Duration::from_millis(700);

impl Item {
    pub fn new(block: Block) -> Self {
        Item { block, background: false, exit_code: None, born: Instant::now(), version: next_version(), open: false, cache: None }
    }

    pub fn touch(&mut self) {
        self.cache = None;
        self.version = next_version();
    }

    pub fn animated(&self) -> bool {
        if self.born.elapsed() < FADE_IN {
            return true;
        }
        match &self.block {
            Block::Tool { status, ended, .. } => *status == Status::Running || ended.is_some_and(|e| e.elapsed() < SETTLE),
            Block::Reasoning { ended, .. } => ended.is_none(),
            Block::TurnTime { .. } => self.born.elapsed() < RULE_GROW,
            _ => false,
        }
    }

    pub fn lines(&mut self, width: usize, expanded: bool, now: Instant, epoch: Instant) -> Vec<Line<'static>> {
        let expanded = expanded || self.open;
        if !self.animated() {
            if let Some((w, e, lines)) = &self.cache {
                if *w == width && *e == expanded {
                    return lines.clone();
                }
            }
        }
        let age = now.saturating_duration_since(self.born);
        let mut lines = match &self.block {
            Block::TurnTime { took } if age < RULE_GROW => turn_rule(*took, width, age.as_secs_f32() / RULE_GROW.as_secs_f32()),
            block => render(block, width, expanded, now, epoch, self.background, self.exit_code),
        };
        if age < FADE_IN {
            anim::fade(&mut lines, age.as_secs_f32() / FADE_IN.as_secs_f32());
        }
        if !self.animated() {
            self.cache = Some((width, expanded, lines.clone()));
        }
        lines
    }
}

fn render(block: &Block, width: usize, expanded: bool, now: Instant, epoch: Instant, background: bool, exit_code: Option<i32>) -> Vec<Line<'static>> {
    let t = now.duration_since(epoch);
    match block {
        Block::User { text } => {
            let style = theme::bold(theme::FG());
            let mut out = Vec::new();
            for (i, line) in text.lines().enumerate() {
                let first = if i == 0 { vec![Span::styled("❯ ", theme::bold(theme::nebula(0.45)))] } else { vec![Span::raw("  ")] };
                out.extend(wrap(vec![Span::styled(line.to_string(), style)], width, first, vec![Span::raw("  ")]));
            }
            if out.is_empty() {
                out.push(Line::from(Span::styled("❯", theme::bold(theme::nebula(0.45)))));
            }
            out
        }
        Block::Assistant { text, .. } => {
            let mut lines = markdown::render(text, width.saturating_sub(2));
            while lines.last().is_some_and(|l| l.width() == 0) {
                lines.pop();
            }
            for (i, l) in lines.iter_mut().enumerate() {
                let lead = if i == 0 { Span::styled("✦ ", theme::fg(theme::nebula(0.2))) } else { Span::raw("  ") };
                l.spans.insert(0, lead);
            }
            lines
        }
        Block::Reasoning { text, started, ended, .. } => {
            let mut out = Vec::new();
            match ended {
                None => {
                    let mut head = vec![Span::styled(format!("{} ", anim::dots_spin(t)), anim::spin_style())];
                    head.extend(anim::nebula_shimmer("thinking", t, theme::DIM()));
                    let secs = now.duration_since(*started).as_secs();
                    if secs >= 1 {
                        head.push(Span::styled(format!("  {secs}s"), theme::dim()));
                    }
                    out.push(Line::from(head));
                }
                // Redacted thinking (no text) isn't worth a line once it's over.
                Some(_) if text.trim().is_empty() => return Vec::new(),
                Some(end) => {
                    let secs = end.duration_since(*started).as_secs().max(1);
                    let mut head = vec![Span::styled("◌ ", theme::fg(theme::DIM())), Span::styled(format!("thought for {secs}s"), theme::dim())];
                    if !expanded {
                        head.push(Span::styled("  ctrl+o", theme::dim()));
                    }
                    out.push(Line::from(head));
                }
            }
            let body = text.trim();
            if !body.is_empty() && (expanded || ended.is_none()) {
                let style = theme::dim().add_modifier(Modifier::ITALIC);
                let rail = || vec![Span::styled("┊ ", theme::fg(theme::FAINT()))];
                let all: Vec<Line> = body.lines().flat_map(|l| wrap(vec![Span::styled(l.to_string(), style)], width, rail(), rail())).collect();
                let keep = if expanded { all.len() } else { 3 };
                let skip = all.len().saturating_sub(keep);
                out.extend(all.into_iter().skip(skip));
            }
            out
        }
        Block::Tool { name, title, output, diff, status, started, ended, .. } => {
            let mut out = Vec::new();
            let (body, collapsible) = tool_body(output, diff.as_deref(), *status, width, expanded);
            let took = ended.unwrap_or(now).duration_since(*started);
            // A finished dot flashes bright and settles into its color.
            let settle = ended.map(|e| (now.saturating_duration_since(e).as_secs_f32() / SETTLE.as_secs_f32()).min(1.0)).unwrap_or(1.0);
            let (dot, dot_style) = match status {
                Status::Running => (anim::dots_spin(t), anim::spin_style()),
                Status::Ok => ("●", theme::bold(theme::mix(theme::GREEN(), theme::MUTED(), anim::ease_out(settle)))),
                Status::Failed => ("○", theme::bold(theme::mix(theme::FG(), theme::RED(), anim::ease_out(settle)))),
            };
            let (label, kind) = tool_kind(name);
            let mut head = vec![Span::styled(format!("{dot} "), dot_style)];
            head.extend(chip(&label, kind));
            if background {
                head.push(Span::styled(" ⇢ bg", theme::fg(theme::PINK())));
            }
            // Right-aligned readout: diff stat, failure, duration.
            let mut right: Vec<Span<'static>> = Vec::new();
            if let Some(d) = diff.as_deref().filter(|d| !d.trim().is_empty()) {
                let (add, del) = crate::harness::diff::stat(d);
                right.push(Span::styled(format!("+{add}"), theme::fg(theme::GREEN())));
                right.push(Span::styled(format!(" −{del}"), theme::fg(theme::RED())));
            }
            if let Some(code) = exit_code {
                right.push(Span::styled(format!("exit {code}"), if code == 0 { theme::dim() } else { theme::fg(theme::RED()) }));
            } else if *status == Status::Failed {
                right.push(Span::styled("failed", theme::fg(theme::RED())));
            }
            if *status == Status::Running && took > Duration::from_secs(1) || *status != Status::Running && took >= Duration::from_millis(1000) {
                if !right.is_empty() {
                    right.push(Span::styled(" · ", theme::dim()));
                }
                right.push(Span::styled(anim::elapsed(took), theme::dim()));
            }
            // Clickable like the plan: ▸ collapsed, ▾ open, when the card has more to show.
            if collapsible {
                right.push(Span::styled(if expanded { " ▾" } else { " ▸" }, theme::fg(theme::VIOLET())));
            }
            let rw: usize = right.iter().map(|s| s.content.width()).sum();
            let lw: usize = head.iter().map(|s| s.content.width()).sum();
            if !title.is_empty() {
                let room = width.saturating_sub(lw + rw + 3);
                head.push(Span::raw(" "));
                head.push(Span::styled(truncate(title, room), theme::fg(theme::FG())));
            }
            if rw > 0 {
                let used: usize = head.iter().map(|s| s.content.width()).sum();
                head.push(Span::raw(" ".repeat(width.saturating_sub(used + rw).max(2))));
                head.extend(right);
            }
            out.push(Line::from(head));
            out.extend(body);
            out
        }
        Block::Notice { text, kind } => {
            let (mark, color, text_style) = match kind {
                NoticeKind::Info => ("· ", theme::DIM(), theme::muted()),
                NoticeKind::Error => ("✗ ", theme::RED(), theme::fg(theme::RED())),
                NoticeKind::Success => ("✓ ", theme::GREEN(), theme::muted()),
            };
            text.lines()
                .enumerate()
                .flat_map(|(i, l)| {
                    let first = if i == 0 { vec![Span::styled(mark, theme::bold(color))] } else { vec![Span::raw("  ")] };
                    wrap(vec![Span::styled(l.to_string(), text_style)], width, first, vec![Span::raw("  ")])
                })
                .collect()
        }
        Block::TurnTime { took } => turn_rule(*took, width, 1.0),
    }
}

/// How long a turn's rule takes to draw itself out from the middle.
const RULE_GROW: Duration = Duration::from_millis(600);

/// A turn's closing rule, `p` of the way through its intro: the time counts up to `took`
/// while the line grows out to both edges.
fn turn_rule(took: Duration, width: usize, p: f32) -> Vec<Line<'static>> {
    let k = anim::ease_out(p.clamp(0.0, 1.0));
    let label = format!(" {} ", anim::elapsed(took.mul_f32(k)));
    let side = width.saturating_sub(label.chars().count());
    let (left, right) = (side / 2, side - side / 2);
    let (l, r) = ((left as f32 * k).round() as usize, (right as f32 * k).round() as usize);
    let rule = |n: usize| Span::styled("─".repeat(n), theme::fg(theme::FAINT()));
    vec![Line::from(vec![
        Span::raw(" ".repeat(left - l)),
        rule(l),
        Span::styled(label, theme::dim()),
        rule(r),
    ])]
}

/// Output lines a collapsed tool card shows (clicking its header or ctrl+o shows all).
const PREVIEW: usize = 2;

/// A tool card's lines under its header (a diff, else its output), and whether collapsing
/// hides any of it (so the header gets a ▸/▾ to click).
fn tool_body(output: &str, diff: Option<&str>, status: Status, width: usize, expanded: bool) -> (Vec<Line<'static>>, bool) {
    let mut out = Vec::new();
    // The rail runs down the output and curls right under its last line, but only once the
    // output is really over: still running or cut short, it stays straight.
    let rail = |last: bool| vec![Span::styled(if last { "╰─ " } else { "│  " }, theme::fg(theme::FAINT()))];
    if let Some(d) = diff.filter(|d| !d.trim().is_empty()) {
        let all = diff_lines(d, width.saturating_sub(3));
        let changes = all.iter().filter(|l| is_change(l)).count();
        let collapsible = all.len() > changes.min(PREVIEW);
        // Collapsed: just the first changed lines.
        let lines: Vec<Line> = if expanded { all } else { all.into_iter().filter(is_change).collect() };
        let total = lines.len();
        let max = if expanded { usize::MAX } else { PREVIEW };
        let shown = total.min(max);
        for (i, l) in lines.into_iter().take(max).enumerate() {
            let mut spans = rail(i + 1 == shown && total == shown && status != Status::Running);
            spans.extend(l.spans);
            out.push(with_more(spans, if i + 1 == shown { total - shown } else { 0 }, width));
        }
        return (out, collapsible);
    }
    let text = output.trim_end();
    if text.is_empty() {
        return (out, false);
    }
    let style = if status == Status::Failed { theme::fg(theme::mix(theme::RED(), theme::DIM(), 0.3)) } else { theme::dim() };
    // Markdown scaffolding (fences, headings, blank lines) says nothing in a preview.
    let scaffolding = |l: &str| l.trim().is_empty() || l.trim_start().starts_with("```") || l.trim_start().starts_with('#');
    let collapsible = text.lines().count() > text.lines().filter(|l| !scaffolding(l)).count().min(PREVIEW);
    let raw: Vec<String> = text
        .lines()
        .map(|l| crate::harness::tools::strip_ansi(l).replace('\t', "  "))
        .filter(|l| expanded || !scaffolding(l))
        .collect();
    let max = if expanded { usize::MAX } else { PREVIEW };
    // While running, show the tail (live output); when done, the head.
    let (shown, hidden): (Vec<String>, usize) = if status == Status::Running {
        let skip = raw.len().saturating_sub(max);
        (raw[skip..].to_vec(), skip)
    } else {
        let hidden = raw.len().saturating_sub(max);
        (raw.into_iter().take(max).collect(), hidden)
    };
    let n = shown.len();
    for (i, line) in shown.into_iter().enumerate() {
        let line = truncate(&line, width.saturating_sub(14).max(10));
        out.push(with_more([rail(i + 1 == n && hidden == 0 && status != Status::Running), vec![Span::styled(line, style)]].concat(), if i + 1 == n { hidden } else { 0 }, width));
    }
    (out, collapsible)
}

/// A preview row, with "+N lines" right-aligned when the card hides more.
fn with_more(mut spans: Vec<Span<'static>>, hidden: usize, width: usize) -> Line<'static> {
    if hidden > 0 {
        let tag = format!("+{hidden} line{}", if hidden == 1 { "" } else { "s" });
        // Diff rows pad their background to the full width; make room for the tag.
        let over = (spans.iter().map(|s| s.content.width()).sum::<usize>() + tag.width() + 2).saturating_sub(width);
        if let Some(pad) = spans.last_mut().filter(|s| over > 0 && !s.content.is_empty() && s.content.trim().is_empty()) {
            pad.content = " ".repeat(pad.content.len().saturating_sub(over)).into();
        }
        let used: usize = spans.iter().map(|s| s.content.width()).sum();
        spans.push(Span::raw(" ".repeat(width.saturating_sub(used + tag.width()).max(2))));
        spans.push(Span::styled(tag, theme::dim()));
    }
    Line::from(spans)
}

/// Whether a `diff_lines` row is an added or removed line (not context or a file header).
fn is_change(l: &Line) -> bool {
    l.spans.get(1).is_some_and(|s| s.content.starts_with('+') || s.content.starts_with('-'))
}

/// What a tool does, for its chip color.
#[derive(Clone, Copy)]
pub enum ToolKind {
    Read,
    Edit,
    Shell,
    Agent,
    Mcp,
    Skill,
    Other,
}

/// A tool's chip: a small glyph in its kind's color, then its name in grey:
/// "◇ read", "✎ edit", "❯ bash" (or "❯ powershell" on Windows), "◈ navigate" (MCP), "✦ agent", "✧ skill".
fn chip(label: &str, kind: ToolKind) -> Vec<Span<'static>> {
    let (icon, color) = match kind {
        ToolKind::Read => ("◇", theme::BLUE()),
        ToolKind::Edit => ("✎", theme::YELLOW()),
        ToolKind::Shell => ("❯", theme::PINK()),
        ToolKind::Agent => ("✦", theme::VIOLET()),
        ToolKind::Mcp => ("◈", theme::VIOLET()),
        ToolKind::Skill => ("✧", theme::GREEN()),
        ToolKind::Other => ("·", theme::DIM()),
    };
    vec![Span::styled(format!(" {icon} "), theme::fg(color)), Span::styled(format!("{label:<5}"), theme::muted())]
}

/// Short lowercase label and kind for a tool: `Bash` → `bash`, an MCP tool
/// `winfer-browser · browser_navigate` → `navigate`, the plan tools → `plan`.
pub fn tool_kind(name: &str) -> (String, ToolKind) {
    if matches!(name, "TodoWrite" | "TaskCreate" | "TaskUpdate" | "TaskList" | "TaskGet") {
        return ("plan".into(), ToolKind::Other);
    }
    if let Some((server, tool)) = name.split_once(" · ").or_else(|| name.strip_prefix("mcp__").and_then(|r| r.split_once("__"))) {
        // Drop a prefix the server name already says: winfer-browser's browser_click → click.
        let tool = match tool.split_once('_') {
            Some((head, rest)) if !rest.is_empty() && server.to_lowercase().contains(&head.to_lowercase()) => rest,
            _ => tool,
        };
        return (truncate(&tool.to_lowercase(), 16), ToolKind::Mcp);
    }
    let kind = match name {
        "Read" | "Grep" | "Glob" | "LS" | "WebFetch" | "WebSearch" | "read_file" | "list_dir" | "search" | "grep" | "glob" => ToolKind::Read,
        "Edit" | "Write" | "MultiEdit" | "NotebookEdit" | "apply_patch" | "edit_file" | "write_file" | "edit" | "write" => ToolKind::Edit,
        "Bash" | "shell" | "exec" | "exec_command" | "bash" | "PowerShell" | "Zsh" | "Sh" => ToolKind::Shell,
        "Agent" | "Task" => ToolKind::Agent,
        "Skill" => ToolKind::Skill,
        _ => ToolKind::Other,
    };
    // Generic shell tools run whatever the platform's shell is: PowerShell on Windows.
    if cfg!(windows) && matches!(name, "shell" | "exec" | "exec_command") {
        return ("powershell".into(), kind);
    }
    (truncate(&name.to_lowercase(), 16), kind)
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
                out.push(Line::from(vec![Span::styled("◇ ", theme::fg(theme::VIOLET())), Span::styled(path.to_string(), theme::bold(theme::MUTED()))]));
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
                out.push(Line::from(Span::styled("   ┈┈", theme::fg(theme::FAINT()))));
            }
            first_hunk = false;
            continue;
        }
        let (num, sign, style) = if let Some(rest) = line.strip_prefix('+') {
            new_n += 1;
            (new_n - 1, "+", (rest, Style::default().fg(theme::mix(theme::GREEN(), theme::FG(), 0.55)).bg(theme::DIFF_ADD_BG())))
        } else if let Some(rest) = line.strip_prefix('-') {
            old_n += 1;
            (old_n - 1, "-", (rest, Style::default().fg(theme::mix(theme::RED(), theme::FG(), 0.45)).bg(theme::DIFF_DEL_BG())))
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
        let gutter = if sign == " " { theme::dim() } else { Style::default().fg(theme::mix(style.fg.unwrap_or(theme::DIM()), theme::DIM(), 0.5)).bg(style.bg.unwrap_or_default()) };
        let mut spans = vec![Span::styled(format!("{num:>4} "), gutter), Span::styled(format!("{sign} "), style), Span::styled(text, style)];
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
    fn turn_rule_grows_from_the_middle_and_counts_up() {
        let text = |p: f32| turn_rule(Duration::from_secs(64), 40, p)[0].spans.iter().map(|s| s.content.to_string()).collect::<String>();
        let start = text(0.0);
        assert_eq!(start.trim(), "0s");
        let mid = text(0.3);
        assert!(mid.starts_with(' ') && mid.contains('─') && !mid.contains("1m 04s"), "{mid:?}");
        let end = text(1.0);
        assert!(end.starts_with('─') && end.ends_with('─') && end.contains(" 1m 04s "), "{end:?}");
        assert_eq!(end.chars().count(), 40);
    }

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

    fn plain(lines: &[Line]) -> Vec<String> {
        lines.iter().map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect::<String>().trim_end().to_string()).collect()
    }

    #[test]
    fn tool_cards_stay_compact() {
        let now = Instant::now();
        let tool = |name: &str, output: &str, diff: Option<String>| Block::Tool { id: "1".into(), name: name.into(), title: "t".into(), output: output.into(), diff, status: Status::Ok, started: now, ended: Some(now) };
        // Header plus two preview lines; markdown scaffolding is skipped; the rest is counted.
        let out = "### Result\n\n```js\nawait page.goto('x');\n```\nline a\nline b\nline c";
        let lines = plain(&render(&tool("winfer-browser · browser_navigate", out, None), 80, false, now, now, false, None));
        assert_eq!(lines.len(), 3, "{lines:?}");
        assert!(lines[0].contains(" navigate ") && lines[1].contains("await page.goto") && lines[2].contains("line a") && lines[2].ends_with("+2 lines"), "{lines:?}");
        // Expanded shows everything.
        assert!(render(&tool("Bash", out, None), 80, true, now, now, false, None).len() > 5);
        // Diffs: only changed lines, two at most.
        let d = crate::harness::diff::unified("a.txt", "a\nb\nc\nd\n", "a\nB\nc\nD\n");
        let lines = plain(&render(&tool("Edit", "", Some(d)), 80, false, now, now, false, None));
        assert_eq!(lines.len(), 3, "{lines:?}");
        assert!(lines[1].contains("- b") && lines[2].contains("+ B") && lines[2].ends_with("+2 lines") && lines[2].len() <= 80 + 8, "{lines:?}");
    }

    #[test]
    fn chips_are_an_icon_and_a_name() {
        let text = |kind| chip("bash", kind).iter().map(|s| s.content.to_string()).collect::<String>();
        assert_eq!(text(ToolKind::Shell), " ❯ bash ");
        assert!(matches!(tool_kind("PowerShell"), (l, ToolKind::Shell) if l == "powershell"));
        assert!(matches!(tool_kind("Zsh"), (l, ToolKind::Shell) if l == "zsh"));
        assert_eq!(text(ToolKind::Read), " ◇ bash ");
    }
}
