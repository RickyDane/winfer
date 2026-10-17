//! Mouse text selection in the transcript: drag to mark, double-click a word, triple-click a line.
//! The mouse is captured (for wheel scrolling), so the terminal's own selection is off; this puts it
//! back inside the app. Points are anchored to transcript lines, so a selection survives scrolling.

use std::time::{Duration, Instant};

use ratatui::text::Line;
use unicode_width::UnicodeWidthChar;

/// A cell in the transcript: line index from the top, display column.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Point {
    pub line: usize,
    pub col: usize,
}

#[derive(Debug, Clone, Copy)]
pub struct Selection {
    pub anchor: Point,
    pub head: Point,
    pub dragging: bool,
    pub moved: bool,
}

impl Selection {
    pub fn at(p: Point) -> Selection {
        Selection { anchor: p, head: p, dragging: true, moved: false }
    }

    /// Start and end (inclusive), in order.
    pub fn range(&self) -> (Point, Point) {
        if self.anchor <= self.head { (self.anchor, self.head) } else { (self.head, self.anchor) }
    }

    /// Selected columns `[from, to)` of a line, if any.
    pub fn cols(&self, line: usize) -> Option<(usize, usize)> {
        let (a, b) = self.range();
        if line < a.line || line > b.line {
            return None;
        }
        let from = if line == a.line { a.col } else { 0 };
        let to = if line == b.line { b.col.saturating_add(1) } else { usize::MAX };
        (to > from).then_some((from, to))
    }
}

/// Where the transcript was last drawn, to map mouse cells to transcript points.
#[derive(Debug, Clone, Default)]
pub struct View {
    /// Every transcript line, as rendered.
    pub lines: Vec<Line<'static>>,
    /// The transcript's screen area (rows `top..bottom`).
    pub top: u16,
    pub bottom: u16,
    /// Where the first shown line is drawn.
    pub x: u16,
    pub y: u16,
    /// Index of the first shown line and how many are shown.
    pub start: usize,
    pub shown: usize,
}

impl View {
    pub fn contains(&self, row: u16) -> bool {
        self.shown > 0 && row >= self.top && row < self.bottom
    }

    /// The transcript point under a screen cell, clamped to the shown lines.
    pub fn point(&self, col: u16, row: u16) -> Point {
        if self.shown == 0 {
            return Point { line: 0, col: 0 };
        }
        let col = col.saturating_sub(self.x) as usize;
        if row < self.y {
            return Point { line: self.start, col: 0 };
        }
        let r = (row - self.y) as usize;
        if r >= self.shown {
            return Point { line: self.start + self.shown - 1, col: usize::MAX / 2 };
        }
        Point { line: self.start + r, col }
    }

    /// The selected text: trailing spaces trimmed, common indentation removed.
    pub fn text(&self, sel: &Selection) -> String {
        let (a, b) = sel.range();
        let mut rows: Vec<(String, bool)> = Vec::new();
        for i in a.line..=b.line.min(self.lines.len().saturating_sub(1)) {
            let Some((from, to)) = sel.cols(i) else { continue };
            let s = slice(&self.lines[i], from, to);
            rows.push((s.trim_end().to_string(), from == 0));
        }
        let indent = rows.iter().filter(|(s, whole)| *whole && !s.is_empty()).map(|(s, _)| s.len() - s.trim_start().len()).min().unwrap_or(0);
        rows.iter()
            .map(|(s, whole)| if *whole && s.len() >= indent && s.is_char_boundary(indent) { &s[indent..] } else { s.as_str() })
            .collect::<Vec<_>>()
            .join("\n")
            .trim_matches('\n')
            .to_string()
    }

    /// Selects the word under a point (the run of non-space characters).
    pub fn word(&self, p: Point) -> Selection {
        let Some(line) = self.lines.get(p.line) else { return Selection::at(p) };
        let cells = cells(line);
        let Some(i) = cells.iter().position(|&(c, w, _)| p.col >= c && p.col < c + w) else { return Selection::at(p) };
        let is_word = |ch: char| !ch.is_whitespace() && !"│╭╮╰╯─()[]{}<>\"'`,;".contains(ch);
        if !is_word(cells[i].2) {
            return Selection::at(p);
        }
        let mut s = i;
        while s > 0 && is_word(cells[s - 1].2) {
            s -= 1;
        }
        let mut e = i;
        while e + 1 < cells.len() && is_word(cells[e + 1].2) {
            e += 1;
        }
        // Drop trailing punctuation so "file.rs." copies as "file.rs".
        while e > s && ".:!?".contains(cells[e].2) {
            e -= 1;
        }
        let a = Point { line: p.line, col: cells[s].0 };
        let b = Point { line: p.line, col: cells[e].0 + cells[e].1 - 1 };
        Selection { anchor: a, head: b, dragging: false, moved: true }
    }

    /// Selects a whole line.
    pub fn line(&self, p: Point) -> Selection {
        Selection { anchor: Point { line: p.line, col: 0 }, head: Point { line: p.line, col: usize::MAX / 2 }, dragging: false, moved: true }
    }
}

/// `(column, width, char)` for each character of a line.
fn cells(line: &Line) -> Vec<(usize, usize, char)> {
    let mut out = Vec::new();
    let mut col = 0;
    for span in &line.spans {
        for ch in span.content.chars() {
            let w = ch.width().unwrap_or(0);
            if w == 0 {
                continue;
            }
            out.push((col, w, ch));
            col += w;
        }
    }
    out
}

/// The text of display columns `[from, to)` of a line.
fn slice(line: &Line, from: usize, to: usize) -> String {
    cells(line).into_iter().filter(|&(c, _, _)| c >= from && c < to).map(|(_, _, ch)| ch).collect()
}

/// Counts quick repeated clicks on the same cell (1, 2, 3, then back to 1).
#[derive(Debug, Default)]
pub struct Clicks {
    last: Option<(Instant, u16, u16, u8)>,
}

impl Clicks {
    pub fn click(&mut self, col: u16, row: u16) -> u8 {
        let now = Instant::now();
        let n = match self.last {
            Some((t, c, r, n)) if now.duration_since(t) < Duration::from_millis(450) && c.abs_diff(col) <= 1 && r == row => n % 3 + 1,
            _ => 1,
        };
        self.last = Some((now, col, row, n));
        n
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(lines: &[&str]) -> View {
        View { lines: lines.iter().map(|l| Line::from(l.to_string())).collect(), top: 0, bottom: 10, x: 1, y: 0, start: 0, shown: lines.len() }
    }

    #[test]
    fn copies_a_dragged_range_without_indent() {
        let v = view(&["  first line", "  second line   ", "  third"]);
        let mut s = Selection::at(v.point(1, 0));
        s.head = v.point(5, 2);
        assert_eq!(v.text(&s), "first line\nsecond line\nthi");
        // Backwards drag gives the same text.
        let mut r = Selection::at(v.point(5, 2));
        r.head = v.point(1, 0);
        assert_eq!(v.text(&r), v.text(&s));
    }

    #[test]
    fn starts_mid_line() {
        let v = view(&["hello world", "next"]);
        let mut s = Selection::at(v.point(7, 0));
        s.head = v.point(50, 1);
        assert_eq!(v.text(&s), "world\nnext");
    }

    #[test]
    fn selects_words_and_lines() {
        let v = view(&["  edit src/main.rs. now"]);
        let w = v.word(v.point(10, 0));
        assert_eq!(v.text(&w), "src/main.rs");
        assert_eq!(v.text(&v.line(v.point(3, 0))), "edit src/main.rs. now");
    }

    #[test]
    fn handles_wide_characters() {
        let v = view(&["ab日本cd"]);
        let mut s = Selection::at(Point { line: 0, col: 2 });
        s.head = Point { line: 0, col: 5 };
        assert_eq!(v.text(&s), "日本");
    }
}
