//! Markdown → styled, wrapped terminal lines (headings, emphasis, lists, quotes, tables, links,
//! and syntax-highlighted code blocks).

use std::sync::OnceLock;

use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use syntect::easy::HighlightLines;
use syntect::highlighting::ThemeSet;
use syntect::parsing::SyntaxSet;
use unicode_width::UnicodeWidthStr;

use super::theme;

fn syntaxes() -> &'static SyntaxSet {
    static S: OnceLock<SyntaxSet> = OnceLock::new();
    S.get_or_init(SyntaxSet::load_defaults_newlines)
}

/// Syntax colors that suit the terminal: a dark or a light theme.
fn highlight_theme() -> &'static syntect::highlighting::Theme {
    static DARK: OnceLock<syntect::highlighting::Theme> = OnceLock::new();
    static LIGHT: OnceLock<syntect::highlighting::Theme> = OnceLock::new();
    let load = |name: &str| {
        let mut set = ThemeSet::load_defaults();
        set.themes.remove(name).unwrap_or_else(|| set.themes.into_values().next().unwrap())
    };
    if theme::pal().light { LIGHT.get_or_init(|| load("InspiredGitHub")) } else { DARK.get_or_init(|| load("base16-ocean.dark")) }
}

/// Syntax-highlights `code` (one span list per line).
pub fn highlight(code: &str, lang: &str) -> Vec<Vec<Span<'static>>> {
    let ss = syntaxes();
    let syntax = ss
        .find_syntax_by_token(lang)
        .or_else(|| ss.find_syntax_by_extension(lang))
        .unwrap_or_else(|| ss.find_syntax_plain_text());
    let mut h = HighlightLines::new(syntax, highlight_theme());
    let mut out = Vec::new();
    for line in syntect::util::LinesWithEndings::from(code) {
        let spans = match h.highlight_line(line, ss) {
            Ok(regions) => regions
                .into_iter()
                .map(|(style, text)| {
                    let c = style.foreground;
                    Span::styled(text.trim_end_matches('\n').replace('\t', "    "), Style::default().fg(Color::Rgb(c.r, c.g, c.b)))
                })
                .collect(),
            Err(_) => vec![Span::raw(line.trim_end_matches('\n').to_string())],
        };
        out.push(spans);
    }
    out
}

/// Wraps styled spans to `width`, prefixing the first line with `first` and the others with
/// `rest` (both should be the same width).
pub fn wrap(spans: Vec<Span<'static>>, width: usize, first: Vec<Span<'static>>, rest: Vec<Span<'static>>) -> Vec<Line<'static>> {
    let indent = first.iter().map(|s| s.content.width()).sum::<usize>();
    let avail = width.saturating_sub(indent).max(4);
    // Split into words (keeping the spaces as their own tokens) with their styles.
    let mut tokens: Vec<(String, Style)> = Vec::new();
    for span in spans {
        let mut word = String::new();
        for c in span.content.chars() {
            if c == ' ' {
                if !word.is_empty() {
                    tokens.push((std::mem::take(&mut word), span.style));
                }
                tokens.push((" ".into(), span.style));
            } else {
                word.push(c);
            }
        }
        if !word.is_empty() {
            tokens.push((word, span.style));
        }
    }
    let mut lines: Vec<Vec<Span<'static>>> = vec![Vec::new()];
    let mut col = 0;
    for (text, style) in tokens {
        let w = text.width();
        if text == " " {
            if col == 0 && lines.len() > 1 {
                continue;
            }
            if col + 1 <= avail {
                lines.last_mut().unwrap().push(Span::styled(text, style));
                col += 1;
            }
            continue;
        }
        if col + w > avail && col > 0 {
            // Drop trailing spaces before breaking.
            let line = lines.last_mut().unwrap();
            while line.last().is_some_and(|s| s.content == " ") {
                line.pop();
            }
            lines.push(Vec::new());
            col = 0;
        }
        if w > avail {
            // Hard-break long words.
            let mut chunk = String::new();
            let mut cw = 0;
            for c in text.chars() {
                let ch = unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
                if col + cw + ch > avail {
                    lines.last_mut().unwrap().push(Span::styled(std::mem::take(&mut chunk), style));
                    lines.push(Vec::new());
                    col = 0;
                    cw = 0;
                }
                chunk.push(c);
                cw += ch;
            }
            col += cw;
            lines.last_mut().unwrap().push(Span::styled(chunk, style));
            continue;
        }
        lines.last_mut().unwrap().push(Span::styled(text, style));
        col += w;
    }
    lines
        .into_iter()
        .enumerate()
        .map(|(i, mut spans)| {
            let mut prefix = if i == 0 { first.clone() } else { rest.clone() };
            prefix.append(&mut spans);
            Line::from(prefix)
        })
        .collect()
}

#[derive(Clone)]
struct ListState {
    next: Option<u64>,
}

struct Renderer {
    width: usize,
    out: Vec<Line<'static>>,
    spans: Vec<Span<'static>>,
    styles: Vec<Style>,
    lists: Vec<ListState>,
    quote: usize,
    /// Pending bullet for the next paragraph in a list item.
    bullet: Option<String>,
    code: Option<(String, String)>,
    link: Option<String>,
    table: Option<Table>,
    heading: Option<HeadingLevel>,
}

#[derive(Default)]
struct Table {
    rows: Vec<Vec<Vec<Span<'static>>>>,
    cell: Vec<Span<'static>>,
    row: Vec<Vec<Span<'static>>>,
}

impl Renderer {
    fn style(&self) -> Style {
        self.styles.last().copied().unwrap_or_default()
    }

    fn push_style(&mut self, f: impl Fn(Style) -> Style) {
        let s = f(self.style());
        self.styles.push(s);
    }

    fn prefix(&self) -> (Vec<Span<'static>>, Vec<Span<'static>>) {
        let mut base = Vec::new();
        for _ in 0..self.quote {
            base.push(Span::styled("│ ", theme::fg(theme::FAINT())));
        }
        let depth = self.lists.len();
        let indent = "  ".repeat(depth.saturating_sub(1));
        match &self.bullet {
            Some(b) => {
                let mut first = base.clone();
                first.push(Span::raw(indent.clone()));
                first.push(Span::styled(b.clone(), theme::fg(theme::VIOLET())));
                let mut rest = base;
                rest.push(Span::raw(format!("{indent}{}", " ".repeat(b.width()))));
                (first, rest)
            }
            None => {
                let mut first = base.clone();
                if depth > 0 {
                    first.push(Span::raw("  ".repeat(depth)));
                }
                (first.clone(), first)
            }
        }
    }

    fn blank(&mut self) {
        if self.out.last().is_some_and(|l| !l.spans.is_empty() || l.width() > 0) {
            self.out.push(Line::default());
        }
    }

    fn flush(&mut self) {
        if self.spans.is_empty() {
            return;
        }
        let spans = std::mem::take(&mut self.spans);
        let (first, rest) = self.prefix();
        self.bullet = None;
        self.out.extend(wrap(spans, self.width, first, rest));
    }

    fn text(&mut self, text: &str) {
        if let Some(t) = &mut self.table {
            t.cell.push(Span::styled(text.to_string(), self.styles.last().copied().unwrap_or_default()));
            return;
        }
        if let Some((_, code)) = &mut self.code {
            code.push_str(text);
            return;
        }
        let style = self.style();
        // Soft line breaks inside a paragraph are spaces; text has no newlines otherwise.
        for (i, part) in text.split('\n').enumerate() {
            if i > 0 {
                self.flush();
            }
            if !part.is_empty() {
                self.spans.push(Span::styled(part.to_string(), style));
            }
        }
    }

    fn code_block(&mut self, lang: &str, code: &str) {
        self.blank();
        let bar = theme::fg(theme::FAINT());
        let mut head = vec![Span::styled("╭─", bar)];
        if !lang.is_empty() {
            head.push(Span::styled(format!(" {lang}"), theme::dim()));
        }
        self.out.push(Line::from(head));
        for spans in highlight(code.trim_end_matches('\n'), lang) {
            self.out.extend(wrap(spans, self.width, vec![Span::styled("│ ", bar)], vec![Span::styled("│ ", bar)]));
        }
        self.out.push(Line::from(Span::styled("╰─", bar)));
        self.out.push(Line::default());
    }

    fn table(&mut self, t: Table) {
        self.blank();
        let cols = t.rows.iter().map(Vec::len).max().unwrap_or(0);
        if cols == 0 {
            return;
        }
        let cell_w = |c: &Vec<Span>| c.iter().map(|s| s.content.width()).sum::<usize>();
        let mut widths = vec![0; cols];
        for row in &t.rows {
            for (i, c) in row.iter().enumerate() {
                widths[i] = widths[i].max(cell_w(c));
            }
        }
        // Shrink the widest columns until the table fits.
        let total = |w: &Vec<usize>| w.iter().sum::<usize>() + 3 * cols + 1;
        while total(&widths) > self.width && widths.iter().any(|&w| w > 6) {
            let i = (0..cols).max_by_key(|&i| widths[i]).unwrap();
            widths[i] -= 1;
        }
        let border = theme::fg(theme::FAINT());
        let rule = |_l: &str, _m: &str, _r: &str| {
            let w: usize = widths.iter().map(|w| w + 3).sum::<usize>().saturating_sub(1);
            Line::from(Span::styled("─".repeat(w), border))
        };
        
        for (ri, row) in t.rows.iter().enumerate() {
            let mut spans = vec![];
            for (i, w) in widths.iter().enumerate() {
                let cell = row.get(i).cloned().unwrap_or_default();
                let text: String = cell.iter().map(|s| s.content.as_ref()).collect();
                let style = if ri == 0 { theme::bold(theme::MUTED()) } else { cell.first().map(|s| s.style).unwrap_or_default() };
                let mut shown: String = String::new();
                let mut used = 0;
                for c in text.chars() {
                    let cw = unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
                    if used + cw > *w {
                        break;
                    }
                    shown.push(c);
                    used += cw;
                }
                spans.push(Span::styled(shown, style));
                spans.push(Span::raw(" ".repeat(w - used + 3)));
            }
            self.out.push(Line::from(spans));
            if ri == 0 {
                self.out.push(rule("├", "┼", "┤"));
            }
        }
        
        self.out.push(Line::default());
    }

    fn event(&mut self, ev: Event) {
        match ev {
            Event::Start(tag) => match tag {
                Tag::Paragraph => {}
                Tag::Heading { level, .. } => {
                    self.blank();
                    self.heading = Some(level);
                    let color = match level {
                        HeadingLevel::H1 => theme::FG(),
                        HeadingLevel::H2 => theme::FG(),
                        _ => theme::MUTED(),
                    };
                    self.push_style(|s| s.fg(color).add_modifier(Modifier::BOLD));
                }
                Tag::BlockQuote(_) => {
                    self.flush();
                    self.quote += 1;
                    self.push_style(|s| s.fg(theme::MUTED()).add_modifier(Modifier::ITALIC));
                }
                Tag::CodeBlock(kind) => {
                    self.flush();
                    let lang = match kind {
                        CodeBlockKind::Fenced(l) => l.split(|c: char| c == ',' || c.is_whitespace()).next().unwrap_or("").to_string(),
                        CodeBlockKind::Indented => String::new(),
                    };
                    self.code = Some((lang, String::new()));
                }
                Tag::List(start) => {
                    self.flush();
                    if self.lists.is_empty() {
                        self.blank();
                    }
                    self.lists.push(ListState { next: start });
                }
                Tag::Item => {
                    self.flush();
                    let list = self.lists.last_mut();
                    let bullet = match list {
                        Some(ListState { next: Some(n) }) => {
                            let b = format!("{n}. ");
                            *n += 1;
                            b
                        }
                        _ => match self.lists.len() {
                            1 => "• ".to_string(),
                            2 => "◦ ".to_string(),
                            _ => "▪ ".to_string(),
                        },
                    };
                    self.bullet = Some(bullet);
                }
                Tag::Emphasis => self.push_style(|s| s.add_modifier(Modifier::ITALIC)),
                Tag::Strong => self.push_style(|s| s.add_modifier(Modifier::BOLD)),
                Tag::Strikethrough => self.push_style(|s| s.add_modifier(Modifier::CROSSED_OUT)),
                Tag::Link { dest_url, .. } => {
                    self.link = Some(dest_url.to_string());
                    self.push_style(|s| s.fg(theme::CYAN()).add_modifier(Modifier::UNDERLINED));
                }
                Tag::Table(_) => {
                    self.flush();
                    self.table = Some(Table::default());
                }
                Tag::TableHead | Tag::TableRow => {}
                Tag::TableCell => {}
                _ => {}
            },
            Event::End(tag) => match tag {
                TagEnd::Paragraph => {
                    self.flush();
                    if self.lists.is_empty() {
                        self.out.push(Line::default());
                    }
                }
                TagEnd::Heading(_) => {
                    let level = self.heading.take();
                    match level {
                        Some(HeadingLevel::H1) => {
                            // Nebula-tinted title.
                            let mut i = 0;
                            for s in &mut self.spans {
                                let n = s.content.chars().count();
                                s.style = s.style.fg(theme::nebula(i as f32 / 60.0));
                                i += n;
                            }
                        }
                        _ => {}
                    }
                    self.flush();
                    self.styles.pop();
                    self.out.push(Line::default());
                }
                TagEnd::BlockQuote(_) => {
                    self.flush();
                    self.quote = self.quote.saturating_sub(1);
                    self.styles.pop();
                }
                TagEnd::CodeBlock => {
                    if let Some((lang, code)) = self.code.take() {
                        self.code_block(&lang, &code);
                    }
                }
                TagEnd::List(_) => {
                    self.flush();
                    self.lists.pop();
                    if self.lists.is_empty() {
                        self.out.push(Line::default());
                    }
                }
                TagEnd::Item => self.flush(),
                TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough => {
                    self.styles.pop();
                }
                TagEnd::Link => {
                    self.styles.pop();
                    if let Some(url) = self.link.take() {
                        let shown: String = self.spans.iter().rev().take(1).map(|s| s.content.to_string()).collect();
                        if shown != url && !url.starts_with('#') {
                            self.spans.push(Span::styled(format!(" ({url})"), theme::dim()));
                        }
                    }
                }
                TagEnd::TableCell => {
                    if let Some(t) = &mut self.table {
                        let cell = std::mem::take(&mut t.cell);
                        t.row.push(cell);
                    }
                }
                TagEnd::TableHead | TagEnd::TableRow => {
                    if let Some(t) = &mut self.table {
                        let row = std::mem::take(&mut t.row);
                        t.rows.push(row);
                    }
                }
                TagEnd::Table => {
                    if let Some(t) = self.table.take() {
                        self.table(t);
                    }
                }
                _ => {}
            },
            Event::Text(t) => self.text(&t),
            Event::Code(c) => {
                let span = Span::styled(c.to_string(), Style::default().fg(theme::mix(theme::VIOLET(), theme::FG(), 0.4)));
                if let Some(t) = &mut self.table {
                    t.cell.push(span);
                } else {
                    self.spans.push(span);
                }
            }
            Event::SoftBreak => self.text(" "),
            Event::HardBreak => self.flush(),
            Event::Rule => {
                self.flush();
                self.out.push(Line::from(Span::styled("─".repeat(self.width.min(80)), theme::fg(theme::FAINT()))));
                self.out.push(Line::default());
            }
            Event::TaskListMarker(done) => {
                self.bullet = Some(if done { "✓ ".into() } else { "☐ ".into() });
            }
            Event::Html(h) | Event::InlineHtml(h) => self.text(&h),
            _ => {}
        }
    }
}

/// Renders Markdown to lines that fit `width`. Unclosed code fences (while streaming) still
/// render as code.
pub fn render(text: &str, width: usize) -> Vec<Line<'static>> {
    let mut r = Renderer {
        width: width.max(10),
        out: Vec::new(),
        spans: Vec::new(),
        styles: vec![Style::default().fg(theme::FG())],
        lists: Vec::new(),
        quote: 0,
        bullet: None,
        code: None,
        link: None,
        table: None,
        heading: None,
    };
    let opts = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    for ev in Parser::new_ext(text, opts) {
        r.event(ev);
    }
    if let Some((lang, code)) = r.code.take() {
        r.code_block(&lang, &code);
    }
    r.flush();
    while r.out.last().is_some_and(|l| l.width() == 0) {
        r.out.pop();
    }
    r.out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(lines: &[Line]) -> Vec<String> {
        lines.iter().map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect()).collect()
    }

    #[test]
    fn wraps_words() {
        let lines = wrap(vec![Span::raw("hello brave new world")], 12, vec![], vec![]);
        assert_eq!(plain(&lines), vec!["hello brave", "new world"]);
    }

    #[test]
    fn renders_lists_and_code() {
        let out = plain(&render("Intro **bold**\n\n- one\n- two\n\n```rust\nfn x() {}\n```\n", 40));
        assert_eq!(out[0], "Intro bold");
        assert!(out.iter().any(|l| l == "• one"));
        assert!(out.iter().any(|l| l.contains("rust")));
        assert!(out.iter().any(|l| l.contains("fn x() {}")));
    }

    #[test]
    fn renders_tables() {
        let out = plain(&render("| a | b |\n|---|---|\n| 1 | 22 |\n", 40));
        assert!(out.iter().any(|l| l.starts_with("a   b")) && out.iter().any(|l| l.starts_with("1   22")), "{out:?}");
    }

    #[test]
    fn unclosed_fence_is_still_code() {
        let out = plain(&render("```\nlet a = 1;", 40));
        assert!(out.iter().any(|l| l.contains("let a = 1;")));
    }
}
