//! A report as a standalone file: drawn once as SVG (same grid, spans and numbers as the chat,
//! from `code::viz`), then rasterized to PNG/JPG with resvg or converted to PDF with svg2pdf.
//! No UI here; `ui/viz.rs` asks where to save and calls `export`.

use super::viz as calc;
use std::fmt::Write as _;
use std::sync::{Arc, OnceLock};
use winfer_backend::viz::{Align, Block, Column, Kind, Meter, Report, Series, Stat};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Svg,
    Png,
    Jpg,
    Pdf,
}

impl Format {
    pub const ALL: [Format; 4] = [Format::Pdf, Format::Png, Format::Jpg, Format::Svg];

    pub fn ext(self) -> &'static str {
        match self {
            Format::Svg => "svg",
            Format::Png => "png",
            Format::Jpg => "jpg",
            Format::Pdf => "pdf",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Format::Svg => "SVG",
            Format::Png => "PNG",
            Format::Jpg => "JPG",
            Format::Pdf => "PDF",
        }
    }
}

/// The colors a file is drawn in, as `#rrggbb` (the chat's theme at the time of export).
#[derive(Clone, Debug)]
pub struct Theme {
    pub bg: String,
    pub border: String,
    pub strong: String,
    pub fg: String,
    pub muted: String,
    pub subtle: String,
    pub track: String,
    pub series: Vec<String>,
}

impl Theme {
    fn series(&self, i: usize) -> &str {
        self.series.get(i).unwrap_or(&self.subtle)
    }
}

const PAD: f32 = 32.0;
/// Space between columns; blocks are unframed, so it is the only thing between them.
const GAP: f32 = 28.0;
/// Space between grid rows.
const ROW_GAP: f32 = 22.0;
/// A block's rule and section label.
const HEAD: f32 = 40.0;
const PLOT_H: f32 = 150.0;
const AXIS_W: f32 = 40.0;
const LEGEND_H: f32 = 30.0;
/// Width of the labels right of a line plot.
const END_W: f32 = 92.0;
/// Width of one part's share under a part-to-whole bar.
const PART_W: f32 = 104.0;
const SANS: &str = "Inter, 'Helvetica Neue', Helvetica, Arial, sans-serif";
const MONO: &str = "Menlo, 'SF Mono', Consolas, 'DejaVu Sans Mono', monospace";

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// Rough width of `s` at `size` px: enough to cut and wrap labels the way the chat does.
fn est(s: &str, size: f32) -> f32 {
    s.chars().count() as f32 * size * 0.56
}

/// `s` cut with "…" to fit `width`.
fn fit(s: &str, width: f32, size: f32) -> String {
    let max = (width / (size * 0.56)).floor().max(0.0) as usize;
    if s.chars().count() <= max {
        s.to_string()
    } else if max <= 1 {
        "…".into()
    } else {
        format!("{}…", s.chars().take(max - 1).collect::<String>())
    }
}

/// Word-wrapped lines of `s` for `width`.
fn wrap(s: &str, width: f32, size: f32) -> Vec<String> {
    let max = ((width / (size * 0.56)).floor() as usize).max(8);
    let mut out = Vec::new();
    for para in s.lines() {
        let mut line = String::new();
        for word in para.split_whitespace() {
            if !line.is_empty() && line.chars().count() + 1 + word.chars().count() > max {
                out.push(std::mem::take(&mut line));
            }
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(word);
        }
        out.push(line);
    }
    while out.last().is_some_and(String::is_empty) {
        out.pop();
    }
    out
}

/// Markdown as the plain text a file shows: emphasis and code marks dropped, list items bulleted.
fn plain(md: &str) -> String {
    md.lines()
        .map(|l| {
            let t = l.trim_start();
            let t = t.trim_start_matches('#').trim_start();
            let t = if let Some(rest) = t.strip_prefix("- ").or_else(|| t.strip_prefix("* ")) { format!("• {rest}") } else { t.to_string() };
            t.replace("**", "").replace("__", "").replace('`', "")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

struct Svg<'a> {
    out: String,
    t: &'a Theme,
}

#[derive(Clone, Copy)]
enum Anchor {
    Start,
    Middle,
    End,
}

impl Svg<'_> {
    #[allow(clippy::too_many_arguments)]
    fn text(&mut self, x: f32, y: f32, s: &str, size: f32, color: &str, weight: u16, anchor: Anchor, mono: bool) {
        let anchor = match anchor {
            Anchor::Start => "start",
            Anchor::Middle => "middle",
            Anchor::End => "end",
        };
        let _ = write!(
            self.out,
            r#"<text x="{x:.1}" y="{y:.1}" font-family="{}" font-size="{size}" font-weight="{weight}" fill="{color}" text-anchor="{anchor}" dominant-baseline="central">{}</text>"#,
            esc(if mono { MONO } else { SANS }),
            esc(s)
        );
    }

    fn rect(&mut self, x: f32, y: f32, w: f32, h: f32, r: f32, fill: &str, stroke: Option<&str>) {
        let stroke = stroke.map(|s| format!(r#" stroke="{s}" stroke-width="1""#)).unwrap_or_default();
        let _ = write!(self.out, r#"<rect x="{x:.1}" y="{y:.1}" width="{:.1}" height="{:.1}" rx="{r}" fill="{fill}"{stroke}/>"#, w.max(0.0), h.max(0.0));
    }

    fn line(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, color: &str, opacity: f32) {
        let _ = write!(self.out, r#"<line x1="{x1:.1}" y1="{y1:.1}" x2="{x2:.1}" y2="{y2:.1}" stroke="{color}" stroke-width="1" stroke-opacity="{opacity}"/>"#);
    }

}

// ---------------------------------------------------------------------------------------------
// Heights: what each block needs at a given width (blocks in a grid row take the tallest).

const HERO_H: f32 = 44.0 + 8.0 + 18.0;
const FIGURE_W: f32 = 180.0;

/// How many of the smaller figures fit on one line beside (or under) the headline.
fn stats_layout(items: &[Stat], w: f32) -> (bool, usize) {
    let rest = items.len().saturating_sub(1);
    let per_row = ((w / FIGURE_W).floor() as usize).clamp(1, rest.max(1));
    let hero_w = items.first().map(|s| est(&s.value, 44.0).max(est(&s.label, 13.0) + 80.0)).unwrap_or(0.0);
    let beside = rest > 0 && hero_w + 24.0 + rest as f32 * FIGURE_W <= w;
    (beside, per_row)
}

fn body_height(b: &Block, w: f32) -> f32 {
    let legend = |n: usize| if n > 1 { LEGEND_H } else { 0.0 };
    match &b.kind {
        Kind::Stats(items) => {
            let (beside, per_row) = stats_layout(items, w);
            let rest = items.len().saturating_sub(1);
            if rest == 0 || beside { HERO_H } else { HERO_H + 16.0 + rest.div_ceil(per_row) as f32 * 52.0 - 12.0 }
        }
        Kind::Bar { labels, series, horizontal: true, stacked, .. } => legend(series.len()) + labels.len() as f32 * hbar_row(series.len(), *stacked),
        Kind::Bar { series, stacked: true, .. } => legend(series.len()) + 14.0 + PLOT_H + 22.0,
        Kind::Bar { series, .. } | Kind::Line { series, .. } => legend(series.len()) + PLOT_H + 22.0,
        Kind::Pie { slices, center, .. } => {
            let per_row = ((w + 12.0) / (PART_W + 12.0)).floor().max(1.0) as usize;
            let rows = calc::arcs(slices, 8).len().div_ceil(per_row) as f32;
            (if center.is_some() { 28.0 } else { 0.0 }) + 18.0 + 14.0 + rows * 46.0 - 8.0
        }
        Kind::Table { rows, .. } => 28.0 + rows.len() as f32 * 26.0,
        Kind::Progress(items) => items.len() as f32 * 44.0 - 12.0,
        Kind::Text(md) => wrap(&plain(md), w, 13.0).len() as f32 * 19.0,
    }
}

fn hbar_row(series: usize, stacked: bool) -> f32 {
    if series == 1 { 26.0 } else if stacked { 22.0 } else { series as f32 * 13.0 + 9.0 }
}

fn caption_lines(b: &Block, w: f32) -> Vec<String> {
    b.caption.as_deref().map(|c| wrap(c, w, 11.5)).unwrap_or_default()
}

/// Summary tiles lead without a rule or label (unless named); other blocks sit under one.
fn is_lead(b: &Block) -> bool {
    matches!(b.kind, Kind::Stats(_)) && b.title.is_none()
}

/// The block's full height: rule and label, body, caption; or just the text.
fn block_height(b: &Block, w: f32) -> f32 {
    if matches!(b.kind, Kind::Text(_)) {
        return body_height(b, w);
    }
    let caption = caption_lines(b, w);
    let head = if is_lead(b) { 0.0 } else { HEAD };
    head + body_height(b, w) + if caption.is_empty() { 0.0 } else { 10.0 + caption.len() as f32 * 16.0 }
}

// ---------------------------------------------------------------------------------------------
// Drawing.

/// Names with their color: a short stroke for lines, a small square for filled marks.
fn legend(svg: &mut Svg, x: f32, y: f32, series: &[Series], line: bool) {
    let mut at = x;
    let muted = svg.t.muted.clone();
    for (i, s) in series.iter().enumerate() {
        let color = svg.t.series(i).to_string();
        let mark = if line { 14.0 } else { 8.0 };
        if line { svg.rect(at, y + 7.0, mark, 2.0, 1.0, &color, None) } else { svg.rect(at, y + 4.0, mark, mark, 2.0, &color, None) }
        svg.text(at + mark + 6.0, y + 8.0, &s.name, 11.5, &muted, 400, Anchor::Start, false);
        at += mark + 6.0 + est(&s.name, 11.5) + 14.0;
    }
}

fn draw_stats(svg: &mut Svg, items: &[Stat], x: f32, y: f32, w: f32) {
    let t = svg.t.clone();
    let Some((hero, rest)) = items.split_first() else { return };
    svg.text(x, y + 22.0, &fit(&hero.value, w, 44.0), 44.0, &t.fg, 600, Anchor::Start, false);
    let mut lx = x;
    let ly = y + HERO_H - 9.0;
    svg.text(lx, ly, &fit(&hero.label, w, 13.0), 13.0, &t.muted, 400, Anchor::Start, false);
    lx += est(&hero.label, 13.0) + 10.0;
    if let Some(d) = &hero.delta {
        svg.text(lx, ly, d, 11.5, &t.muted, 500, Anchor::Start, false);
        lx += est(d, 11.5) + 10.0;
    }
    if let Some(n) = &hero.note {
        svg.text(lx, ly, &fit(n, (x + w - lx).max(0.0), 11.5), 11.5, &t.subtle, 400, Anchor::Start, false);
    }
    if rest.is_empty() {
        return;
    }
    let (beside, per_row) = stats_layout(items, w);
    let (fx0, fy0) = if beside { (x + w - rest.len() as f32 * FIGURE_W, y + HERO_H - 40.0) } else { (x, y + HERO_H + 16.0) };
    for (i, s) in rest.iter().enumerate() {
        let (col, row) = (i % per_row, i / per_row);
        let (fx, fy) = (fx0 + col as f32 * FIGURE_W, fy0 + row as f32 * 52.0);
        svg.line(fx + 0.5, fy, fx + 0.5, fy + 40.0, &t.strong, 1.0);
        let iw = FIGURE_W - 32.0;
        svg.text(fx + 16.0, fy + 12.0, &fit(&s.value, iw, 18.0), 18.0, &t.fg, 600, Anchor::Start, false);
        if let Some(d) = &s.delta {
            svg.text(fx + 22.0 + est(&s.value, 18.0), fy + 13.0, &fit(d, iw, 11.5), 11.5, &t.muted, 500, Anchor::Start, false);
        }
        let label = match &s.note {
            Some(n) => format!("{} · {n}", s.label),
            None => s.label.clone(),
        };
        svg.text(fx + 16.0, fy + 33.0, &fit(&label, iw, 11.5), 11.5, &t.subtle, 400, Anchor::Start, false);
    }
}

fn draw_hbars(svg: &mut Svg, labels: &[String], series: &[Series], stacked: bool, unit: Option<&str>, x: f32, mut y: f32, w: f32) {
    let t = svg.t.clone();
    let label_w = (w * 0.3).min(104.0);
    let (bx, value_w) = (x + label_w + 12.0, 54.0);
    let bw = (w - label_w - 12.0 - value_w - 8.0).max(10.0);
    if let [only] = series {
        // Lollipops: the largest value in the series color, the rest in gray.
        let (_, top) = calc::nice_ticks(only.values.iter().cloned().fold(0.0, f64::max), 4);
        let lead = only.values.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)).map(|(i, _)| i);
        for (i, label) in labels.iter().enumerate() {
            let cy = y + i as f32 * 26.0 + 13.0;
            let (color, opacity) = if Some(i) == lead { (t.series(0).to_string(), 1.0) } else { (t.subtle.clone(), 0.6) };
            let len = (bw - 5.0) * (only.values[i].max(0.0) / top) as f32;
            svg.text(x, cy, &fit(label, label_w, 11.5), 11.5, &t.muted, 400, Anchor::Start, false);
            let _ = write!(svg.out, r#"<g opacity="{opacity}"><rect x="{bx:.1}" y="{:.1}" width="{len:.1}" height="2" fill="{color}"/><circle cx="{:.1}" cy="{cy:.1}" r="4" fill="{color}" stroke="{}" stroke-width="2"/></g>"#, cy - 1.0, bx + len, t.bg);
            svg.text(x + w, cy, &calc::compact(only.values[i], unit), 11.5, &t.fg, 400, Anchor::End, true);
        }
        return;
    }
    legend(svg, x, y, series, false);
    y += LEGEND_H;
    let values: Vec<Vec<f64>> = series.iter().map(|s| s.values.clone()).collect();
    let (_, top) = calc::nice_ticks(calc::bar_max(&values, stacked), 4);
    let row_h = hbar_row(series.len(), stacked);
    for (i, label) in labels.iter().enumerate() {
        let ry = y + i as f32 * row_h;
        svg.text(x, ry + row_h / 2.0, &fit(label, label_w, 11.5), 11.5, &t.muted, 400, Anchor::Start, false);
        if stacked {
            let (by, h) = (ry + (row_h - 11.0) / 2.0, 11.0);
            let mut at = bx;
            let mut total = 0.0;
            for (k, s) in series.iter().enumerate() {
                let v = s.values[i].max(0.0);
                total += v;
                let len = bw * (v / top) as f32;
                if len > 0.0 {
                    svg.rect(at, by, (len - 2.0).max(1.0), h, 2.0, t.series(k), None);
                }
                at += len;
            }
            svg.text(x + w, ry + row_h / 2.0, &calc::compact(total, unit), 11.5, &t.muted, 400, Anchor::End, true);
        } else {
            for (k, s) in series.iter().enumerate() {
                let by = ry + 4.0 + k as f32 * 13.0;
                svg.rect(bx, by, (bw * (s.values[i].max(0.0) / top) as f32).max(1.0), 8.0, 2.0, t.series(k), None);
                svg.text(x + w, by + 4.0, &calc::compact(s.values[i], unit), 10.5, &t.muted, 400, Anchor::End, true);
            }
        }
    }
}

/// Gridlines and tick labels of a plot `pw` wide from the axis; returns the plot's left edge and width.
#[allow(clippy::too_many_arguments)]
fn axis(svg: &mut Svg, ticks: &[f64], top: f64, unit: Option<&str>, x: f32, y: f32, w: f32) -> (f32, f32) {
    let t = svg.t.clone();
    let (px0, pw) = (x + AXIS_W + 8.0, w - AXIS_W - 8.0);
    for tick in ticks {
        let ty = y + PLOT_H - PLOT_H * (*tick / top) as f32;
        if *tick == 0.0 {
            svg.line(px0, ty, px0 + pw, ty, &t.strong, 1.0);
        } else {
            svg.line(px0, ty, px0 + pw, ty, &t.border, 0.7);
        }
        svg.text(x + AXIS_W, ty, &calc::compact(*tick, unit), 10.5, &t.subtle, 400, Anchor::End, false);
    }
    (px0, pw)
}

fn x_labels(svg: &mut Svg, labels: &[String], px0: f32, pw: f32, y: f32) {
    let t = svg.t.clone();
    let step = calc::label_step(labels.len(), 10);
    let slot = pw / labels.len().max(1) as f32;
    for (i, l) in labels.iter().enumerate().filter(|(i, _)| i % step == 0) {
        svg.text(px0 + slot * (i as f32 + 0.5), y + 12.0, &fit(l, slot * step as f32 - 4.0, 10.5), 10.5, &t.muted, 400, Anchor::Middle, false);
    }
}

/// A bar with its top corners rounded (4px), square at the baseline.
fn top_rounded(svg: &mut Svg, x: f32, y: f32, w: f32, h: f32, color: &str) {
    let r = 4.0f32.min(h).min(w / 2.0);
    let b = y + h;
    let _ = write!(svg.out, r#"<path d="M{x:.1},{b:.1}V{:.1}Q{x:.1},{y:.1} {:.1},{y:.1}H{:.1}Q{:.1},{y:.1} {:.1},{:.1}V{b:.1}Z" fill="{color}"/>"#, y + r, x + r, x + w - r, x + w, x + w, y + r);
}

fn draw_vbars(svg: &mut Svg, labels: &[String], series: &[Series], stacked: bool, unit: Option<&str>, x: f32, mut y: f32, w: f32) {
    let t = svg.t.clone();
    if series.len() > 1 {
        legend(svg, x, y, series, false);
        y += LEGEND_H;
    }
    if stacked {
        y += 14.0;
    }
    let values: Vec<Vec<f64>> = series.iter().map(|s| s.values.clone()).collect();
    let (ticks, top) = calc::nice_ticks(calc::bar_max(&values, stacked), 4);
    let (px0, pw) = axis(svg, &ticks, top, unit, x, y, w);
    let slot = pw / labels.len().max(1) as f32;
    let base = y + PLOT_H;
    for i in 0..labels.len() {
        let cx = px0 + slot * (i as f32 + 0.5);
        if stacked {
            let bw = (slot - 8.0).clamp(2.0, 18.0);
            let mut at = base;
            let visible: Vec<usize> = (0..series.len()).filter(|&k| series[k].values[i] > 0.0).collect();
            let mut total = 0.0;
            for (n, &k) in visible.iter().enumerate() {
                let v = series[k].values[i];
                total += v;
                let h = PLOT_H * (v / top) as f32;
                let gap = if n == 0 { 0.0 } else { 2.0 };
                if n + 1 == visible.len() {
                    top_rounded(svg, cx - bw / 2.0, at - h, bw, (h - gap).max(1.0), t.series(k));
                } else {
                    svg.rect(cx - bw / 2.0, at - h, bw, (h - gap).max(1.0), 0.0, t.series(k), None);
                }
                at -= h;
            }
            svg.text(cx, at - 9.0, &calc::compact(total, unit), 10.0, &t.muted, 400, Anchor::Middle, false);
        } else {
            let n = series.len() as f32;
            let bw = ((slot - 8.0 - 2.0 * (n - 1.0)) / n).clamp(2.0, 20.0);
            let start = cx - (bw * n + 2.0 * (n - 1.0)) / 2.0;
            for (k, s) in series.iter().enumerate() {
                let h = PLOT_H * (s.values[i].max(0.0) / top) as f32;
                if h > 0.0 {
                    top_rounded(svg, start + k as f32 * (bw + 2.0), base - h, bw, h, t.series(k));
                }
            }
        }
    }
    x_labels(svg, labels, px0, pw, base);
}

fn draw_lines(svg: &mut Svg, labels: &[String], series: &[Series], area: bool, unit: Option<&str>, x: f32, mut y: f32, w: f32) {
    let t = svg.t.clone();
    if series.len() > 1 {
        legend(svg, x, y, series, true);
        y += LEGEND_H;
    }
    let n = labels.len();
    let ends = n > 0 && series.len() <= 4 && w >= 360.0;
    let plot_w = if ends { w - END_W - 8.0 } else { w };
    let values: Vec<Vec<f64>> = series.iter().map(|s| s.values.clone()).collect();
    let (ticks, top) = calc::nice_ticks(calc::bar_max(&values, false), 4);
    let (px0, pw) = axis(svg, &ticks, top, unit, x, y, plot_w);
    let base = y + PLOT_H;
    let at = |i: usize, v: f64| (px0 + pw * ((i as f32 + 0.5) / n as f32), base - PLOT_H * (v / top).clamp(0.0, 1.0) as f32);
    for (k, s) in series.iter().enumerate() {
        let color = t.series(k).to_string();
        let pts: Vec<(f32, f32)> = s.values.iter().enumerate().map(|(i, v)| at(i, *v)).collect();
        let list = pts.iter().map(|(px, py)| format!("{px:.1},{py:.1}")).collect::<Vec<_>>().join(" ");
        if area && n > 1 {
            let _ = write!(svg.out, r#"<polygon points="{:.1},{base:.1} {list} {:.1},{base:.1}" fill="{color}" fill-opacity="0.14"/>"#, pts[0].0, pts[n - 1].0);
        }
        if n > 1 {
            let _ = write!(svg.out, r#"<polyline points="{list}" fill="none" stroke="{color}" stroke-width="2" stroke-linejoin="round" stroke-linecap="round"/>"#);
        }
        if let Some((px, py)) = pts.last() {
            let _ = write!(svg.out, r#"<circle cx="{px:.1}" cy="{py:.1}" r="4" fill="{color}" stroke="{}" stroke-width="2"/>"#, t.bg);
        }
    }
    if ends {
        // The last value and name at each line's end, nudged apart so they never overlap.
        let mut at_y: Vec<(usize, f32)> = series.iter().enumerate().map(|(k, s)| (k, at(n - 1, s.values[n - 1]).1)).collect();
        at_y.sort_by(|a, b| a.1.total_cmp(&b.1));
        for i in 1..at_y.len() {
            at_y[i].1 = at_y[i].1.max(at_y[i - 1].1 + 30.0);
        }
        let lx = x + plot_w + 8.0;
        for (k, ly) in at_y {
            svg.text(lx, ly - 6.0, &calc::compact(series[k].values[n - 1], unit), 12.0, &t.fg, 600, Anchor::Start, false);
            svg.text(lx, ly + 8.0, &fit(&series[k].name, END_W, 10.5), 10.5, &t.muted, 400, Anchor::Start, false);
        }
    }
    x_labels(svg, labels, px0, pw, base);
}

/// A pie or ring as one part-to-whole bar, then each part's share set large with its name.
fn draw_pie(svg: &mut Svg, slices: &[winfer_backend::viz::Slice], center: Option<&(String, String)>, unit: Option<&str>, x: f32, mut y: f32, w: f32) {
    let t = svg.t.clone();
    let arcs = calc::arcs(slices, 8);
    let colors: Vec<String> = arcs.iter().enumerate().map(|(i, a)| if a.other { t.subtle.clone() } else { t.series(i).to_string() }).collect();
    if let Some((value, label)) = center {
        svg.text(x, y + 10.0, value, 15.0, &t.fg, 600, Anchor::Start, false);
        svg.text(x + est(value, 15.0) + 8.0, y + 10.0, label, 11.5, &t.muted, 400, Anchor::Start, false);
        y += 28.0;
    }
    let total: f64 = arcs.iter().map(|a| a.value.max(0.0)).sum();
    let gaps = 2.0 * arcs.len().saturating_sub(1) as f32;
    let mut at = x;
    let _ = write!(svg.out, r#"<clipPath id="pb{:.0}{:.0}"><rect x="{x:.1}" y="{y:.1}" width="{w:.1}" height="18" rx="4"/></clipPath><g clip-path="url(#pb{:.0}{:.0})">"#, x, y, x, y);
    for (a, color) in arcs.iter().zip(&colors) {
        let len = ((w - gaps) * (a.value.max(0.0) / total.max(f64::MIN_POSITIVE)) as f32).max(3.0);
        svg.rect(at, y, len, 18.0, 0.0, color, None);
        at += len + 2.0;
    }
    svg.out.push_str("</g>");
    y += 18.0 + 14.0;
    let per_row = ((w + 12.0) / (PART_W + 12.0)).floor().max(1.0) as usize;
    for (i, (a, color)) in arcs.iter().zip(&colors).enumerate() {
        let (px0, py) = (x + (i % per_row) as f32 * (PART_W + 12.0), y + (i / per_row) as f32 * 46.0);
        svg.rect(px0, py + 6.0, 8.0, 8.0, 2.0, color, None);
        svg.text(px0 + 14.0, py + 10.0, &calc::percent(a.percent), 18.0, &t.fg, 600, Anchor::Start, false);
        svg.text(px0, py + 31.0, &fit(&format!("{} · {}", a.label, calc::compact(a.value, unit)), PART_W, 11.0), 11.0, &t.subtle, 400, Anchor::Start, false);
    }
}

fn draw_table(svg: &mut Svg, columns: &[Column], rows: &[Vec<String>], x: f32, y: f32, w: f32) {
    let t = svg.t.clone();
    let cw = w / columns.len().max(1) as f32;
    let cell = |svg: &mut Svg, i: usize, cy: f32, s: &str, color: &str, weight: u16| {
        let (tx, anchor) = match columns[i].align {
            Align::Left => (x + i as f32 * cw + 8.0, Anchor::Start),
            Align::Right => (x + (i + 1) as f32 * cw - 8.0, Anchor::End),
            Align::Center => (x + (i as f32 + 0.5) * cw, Anchor::Middle),
        };
        svg.text(tx, cy, &fit(s, cw - 16.0, 11.5), 11.5, color, weight, anchor, false);
    };
    for (i, c) in columns.iter().enumerate() {
        cell(svg, i, y + 13.0, &c.label, &t.subtle, 500);
    }
    svg.line(x, y + 27.5, x + w, y + 27.5, &t.fg, 1.0);
    for (r, row) in rows.iter().enumerate() {
        let ry = y + 28.0 + r as f32 * 26.0;
        for (i, s) in row.iter().enumerate().take(columns.len()) {
            cell(svg, i, ry + 13.0, s, &t.fg, 400);
        }
        if r + 1 < rows.len() {
            svg.line(x, ry + 25.5, x + w, ry + 25.5, &t.border, 1.0);
        }
    }
}

/// Meters as a row of ticks in the text color.
fn draw_progress(svg: &mut Svg, items: &[Meter], x: f32, y: f32, w: f32) {
    const TICKS: usize = 30;
    let t = svg.t.clone();
    let tick_w = (w - 2.0 * (TICKS - 1) as f32) / TICKS as f32;
    for (i, m) in items.iter().enumerate() {
        let my = y + i as f32 * 44.0;
        let percent = (m.max - 100.0).abs() < f64::EPSILON;
        let value = calc::compact(m.value, percent.then_some("%"));
        let limit = if percent { String::new() } else { format!(" / {}", calc::compact(m.max, None)) };
        svg.text(x + w, my + 8.0, &limit, 11.5, &t.subtle, 400, Anchor::End, true);
        let vx = x + w - est(&limit, 11.5);
        svg.text(vx, my + 8.0, &value, 11.5, &t.fg, 600, Anchor::End, true);
        svg.text(x, my + 8.0, &fit(&m.label, (vx - est(&value, 11.5) - x - 12.0).max(0.0), 11.5), 11.5, &t.fg, 400, Anchor::Start, false);
        let on = ((m.value / m.max).clamp(0.0, 1.0) * TICKS as f64).round() as usize;
        for k in 0..TICKS {
            let (fill, opacity) = if k < on { (&t.fg, 0.78) } else { (&t.track, 1.0) };
            let _ = write!(svg.out, r#"<rect x="{:.1}" y="{:.1}" width="{tick_w:.1}" height="12" rx="1" fill="{fill}" fill-opacity="{opacity}"/>"#, x + k as f32 * (tick_w + 2.0), my + 20.0);
        }
    }
}

fn draw_body(svg: &mut Svg, b: &Block, x: f32, y: f32, w: f32) {
    match &b.kind {
        Kind::Stats(items) => draw_stats(svg, items, x, y, w),
        Kind::Bar { labels, series, horizontal: true, stacked, unit } => draw_hbars(svg, labels, series, *stacked, unit.as_deref(), x, y, w),
        Kind::Bar { labels, series, stacked, unit, .. } => draw_vbars(svg, labels, series, *stacked, unit.as_deref(), x, y, w),
        Kind::Line { labels, series, area, unit } => draw_lines(svg, labels, series, *area, unit.as_deref(), x, y, w),
        Kind::Pie { slices, center, unit, .. } => draw_pie(svg, slices, center.as_ref(), unit.as_deref(), x, y, w),
        Kind::Table { columns, rows } => draw_table(svg, columns, rows, x, y, w),
        Kind::Progress(items) => draw_progress(svg, items, x, y, w),
        Kind::Text(md) => {
            let fg = svg.t.fg.clone();
            for (i, line) in wrap(&plain(md), w, 13.0).iter().enumerate() {
                svg.text(x, y + 9.5 + i as f32 * 19.0, line, 13.0, &fg, 400, Anchor::Start, false);
            }
        }
    }
}

/// A block: a hairline with its title as a section label over the body, or the bare text.
fn draw_block(svg: &mut Svg, b: &Block, x: f32, y: f32, w: f32) {
    if matches!(b.kind, Kind::Text(_)) {
        draw_body(svg, b, x, y, w);
        return;
    }
    let t = svg.t.clone();
    let mut by = y;
    if !is_lead(b) {
        svg.line(x, y + 0.5, x + w, y + 0.5, &t.strong, 1.0);
        let title = b.title.clone().unwrap_or_else(|| b.kind.name().to_string()).to_uppercase();
        let _ = write!(
            svg.out,
            r#"<text x="{x:.1}" y="{:.1}" font-family="{}" font-size="10.5" font-weight="600" letter-spacing="1.2" fill="{}" dominant-baseline="central">{}</text>"#,
            y + 22.0,
            esc(SANS),
            t.muted,
            esc(&fit(&title, w, 12.0))
        );
        by += HEAD;
    }
    draw_body(svg, b, x, by, w);
    let mut cy = by + body_height(b, w) + 10.0;
    for line in caption_lines(b, w) {
        svg.text(x, cy + 8.0, &line, 11.5, &t.muted, 400, Anchor::Start, false);
        cy += 16.0;
    }
}

/// The reports (one, or several merged) as an SVG document `width` px wide plus margins.
pub fn svg(reports: &[Report], width: f32, theme: &Theme) -> String {
    let width = width.max(320.0);
    let cols = calc::columns(width, GAP);
    let mut svg = Svg { out: String::new(), t: theme };
    let mut y = PAD;
    for (n, r) in reports.iter().enumerate() {
        let main = n == 0;
        if !main && (r.title.is_some() || r.subtitle.is_some()) {
            y += 12.0;
        }
        if main && (r.title.is_some() || r.subtitle.is_some()) {
            let _ = write!(
                svg.out,
                r#"<text x="{PAD:.1}" y="{:.1}" font-family="{}" font-size="10.5" font-weight="600" letter-spacing="1.2" fill="{}" dominant-baseline="central">REPORT</text>"#,
                y + 6.0,
                esc(SANS),
                theme.muted
            );
            y += 20.0;
        }
        if let Some(title) = &r.title {
            let size = if main { 22.0 } else { 15.0 };
            svg.text(PAD, y + size / 2.0, &fit(title, width, size), size, &theme.fg, 600, Anchor::Start, false);
            y += size + 6.0;
        }
        if let Some(sub) = &r.subtitle {
            svg.text(PAD, y + 7.0, &fit(sub, width, 12.0), 12.0, &theme.subtle, 400, Anchor::Start, false);
            y += 20.0;
        }
        if r.title.is_some() || r.subtitle.is_some() {
            y += ROW_GAP;
        }
        let spans: Vec<usize> = r.blocks.iter().map(|b| calc::span_of(b, cols)).collect();
        for row in calc::pack(&spans, cols) {
            let used: usize = row.iter().map(|(_, s)| s).sum();
            let cells: Vec<(usize, f32)> = row.iter().map(|&(i, s)| (i, (width + GAP) * s as f32 / used as f32 - GAP)).collect();
            let h = cells.iter().map(|&(i, w)| block_height(&r.blocks[i], w)).fold(0.0, f32::max);
            let mut x = PAD;
            for (i, w) in cells {
                draw_block(&mut svg, &r.blocks[i], x, y, w);
                x += w + GAP;
            }
            y += h + ROW_GAP;
        }
    }
    let (w, h) = (width + 2.0 * PAD, y - ROW_GAP + PAD);
    format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{w:.0}" height="{h:.0}" viewBox="0 0 {w:.0} {h:.0}"><rect width="100%" height="100%" fill="{}"/>{}</svg>"#,
        theme.bg, svg.out
    )
}

fn fontdb() -> Arc<svg2pdf::usvg::fontdb::Database> {
    static DB: OnceLock<Arc<svg2pdf::usvg::fontdb::Database>> = OnceLock::new();
    DB.get_or_init(|| {
        let mut db = svg2pdf::usvg::fontdb::Database::new();
        db.load_system_fonts();
        db.set_sans_serif_family("Helvetica");
        db.set_monospace_family("Menlo");
        Arc::new(db)
    })
    .clone()
}

fn tree(svg: &str) -> Result<svg2pdf::usvg::Tree, String> {
    let opts = svg2pdf::usvg::Options { fontdb: fontdb(), ..Default::default() };
    svg2pdf::usvg::Tree::from_str(svg, &opts).map_err(|e| e.to_string())
}

/// The file's bytes in `format`; rasters at 2x for sharp text.
pub fn encode(svg: &str, format: Format) -> Result<Vec<u8>, String> {
    match format {
        Format::Svg => Ok(svg.as_bytes().to_vec()),
        Format::Pdf => svg2pdf::to_pdf(&tree(svg)?, svg2pdf::ConversionOptions::default(), svg2pdf::PageOptions::default()).map_err(|e| e.to_string()),
        Format::Png | Format::Jpg => {
            let tree = tree(svg)?;
            let scale = 2.0;
            let size = tree.size().to_int_size().scale_by(scale).ok_or("the report is empty")?;
            let mut pixmap = resvg::tiny_skia::Pixmap::new(size.width(), size.height()).ok_or("the report is too large to rasterize")?;
            resvg::render(&tree, resvg::tiny_skia::Transform::from_scale(scale, scale), &mut pixmap.as_mut());
            if format == Format::Png {
                return pixmap.encode_png().map_err(|e| e.to_string());
            }
            // The background is opaque, so the premultiplied pixels are the plain ones.
            let rgb: Vec<u8> = pixmap.data().chunks_exact(4).flat_map(|p| [p[0], p[1], p[2]]).collect();
            let mut out = Vec::new();
            image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 92).encode(&rgb, size.width(), size.height(), image::ExtendedColorType::Rgb8).map_err(|e| e.to_string())?;
            Ok(out)
        }
    }
}

/// A file name for the report: its title, made safe, with the format's extension.
pub fn file_name(reports: &[Report], format: Format) -> String {
    let title = reports.iter().find_map(|r| r.title.clone()).unwrap_or_else(|| "report".into());
    let stem: String = title.chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { ' ' }).collect::<String>().split_whitespace().collect::<Vec<_>>().join("-");
    format!("{}.{}", if stem.is_empty() { "report".into() } else { stem.to_lowercase() }, format.ext())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn theme() -> Theme {
        let s = |x: &str| x.to_string();
        Theme { bg: s("#111111"), border: s("#333333"), strong: s("#444444"), fg: s("#eeeeee"), muted: s("#aaaaaa"), subtle: s("#777777"), track: s("#222222"), series: ["#3987e5", "#d95926", "#199e70"].map(String::from).to_vec() }
    }

    fn sample() -> Vec<Report> {
        vec![
            Report::parse(&json!({ "title": "Build <time> & more", "subtitle": "sample", "blocks": [
                { "type": "stats", "items": [{ "label": "Total", "value": "41 s", "delta": "-8%" }, { "label": "Crates", "value": 318 }] },
                { "type": "ring", "slices": [{ "label": "a", "value": 3 }, { "label": "b", "value": 1 }], "caption": "Most of it is gpui." },
                { "type": "bar", "labels": ["x", "y"], "series": [{ "name": "d", "values": [1, 2] }, { "name": "r", "values": [2, 3] }] },
                { "type": "bar", "orientation": "vertical", "stacked": true, "labels": ["x", "y"], "series": [{ "name": "d", "values": [1, 2] }, { "name": "r", "values": [2, 3] }] },
                { "type": "line", "area": true, "labels": ["a", "b", "c"], "series": [{ "name": "s", "values": [1, 3, 2] }] },
                { "type": "table", "columns": ["Name", "N"], "rows": [["a", 1], ["b", 2]] },
                { "type": "progress", "items": [{ "label": "disk", "value": 40 }] },
                { "type": "text", "markdown": "**Bold** and `code`\n- item" }
            ] })).unwrap(),
            Report::parse(&json!({ "title": "Second", "blocks": [{ "type": "pie", "slices": [{ "label": "only", "value": 1 }] }] })).unwrap(),
        ]
    }

    #[test]
    fn the_svg_holds_every_block_with_text_escaped() {
        let s = svg(&sample(), 1200.0, &theme());
        assert!(s.starts_with("<svg") && s.ends_with("</svg>"));
        assert!(s.contains("Build &lt;time&gt; &amp; more"));
        assert!(s.contains("Second") && s.contains("Most of it is gpui."));
        assert!(s.contains("Bold and code") && s.contains("• item"));
        assert!(s.contains("<polyline") && s.contains("<polygon"));
        assert!(!s.contains("NaN") && !s.contains("inf"));
    }

    #[test]
    fn narrow_exports_stack_into_one_column_and_get_taller() {
        let height = |s: &str| s.split("height=\"").nth(1).and_then(|h| h.split('"').next()).and_then(|h| h.parse::<f32>().ok()).unwrap();
        let (wide, narrow) = (svg(&sample(), 1200.0, &theme()), svg(&sample(), 400.0, &theme()));
        assert!(height(&narrow) > height(&wide));
    }

    #[test]
    #[ignore]
    fn preview_png() {
        let r = vec![Report::parse(&json!({ "title": "Visualization demo", "subtitle": "Marketing funnel sample", "blocks": [
            { "type": "stats", "items": [{ "label": "Visitors", "value": "52,900", "delta": "+11%" }, { "label": "Conversion", "value": "3.4%", "delta": "+0.5%" }, { "label": "CAC", "value": "$21", "delta": "-$2" }] },
            { "type": "bar", "title": "Funnel stages", "labels": ["Visit", "Signup", "Trial", "Paid"], "series": [{ "name": "Users", "values": [52900, 9400, 3100, 1800] }] },
            { "type": "line", "title": "Conversion rate by month", "unit": "%", "labels": ["Jun", "Jul", "Aug", "Sep", "Oct", "Nov"], "series": [{ "name": "Rate", "values": [2.8, 2.9, 3.1, 3.0, 3.3, 3.4] }, { "name": "Target", "values": [3, 3, 3, 3.2, 3.2, 3.2] }] },
            { "type": "pie", "ring": true, "title": "Spend by channel", "center": { "value": "$84k", "label": "spent" }, "slices": [{ "label": "Paid search", "value": 40 }, { "label": "Social", "value": 25 }, { "label": "Email", "value": 15 }, { "label": "Events", "value": 20 }] },
            { "type": "progress", "title": "Campaign goals", "items": [{ "label": "Leads", "value": 88 }, { "label": "Trials", "value": 62 }] },
            { "type": "bar", "orientation": "vertical", "stacked": true, "title": "Leads by week", "labels": ["W1", "W2", "W3", "W4"], "series": [{ "name": "Organic", "values": [30, 34, 28, 40] }, { "name": "Paid", "values": [20, 18, 25, 22] }] },
            { "type": "table", "title": "Top campaigns", "columns": ["Campaign", { "label": "Leads", "align": "right" }], "rows": [["Launch webinar", 640], ["Retargeting", 412]] },
            { "type": "text", "text": "Trial-to-paid is the weakest step." }
        ] })).unwrap()];
        std::fs::write("/tmp/vizpal/export.png", encode(&svg(&r, 1240.0, &theme()), Format::Png).unwrap()).unwrap();
    }

    #[test]
    fn every_format_encodes() {
        let s = svg(&sample(), 900.0, &theme());
        let png = encode(&s, Format::Png).unwrap();
        assert_eq!(&png[1..4], b"PNG");
        let jpg = encode(&s, Format::Jpg).unwrap();
        assert_eq!(&jpg[..2], [0xFF, 0xD8]);
        let pdf = encode(&s, Format::Pdf).unwrap();
        assert!(pdf.starts_with(b"%PDF"));
        assert_eq!(encode(&s, Format::Svg).unwrap(), s.as_bytes());
    }

    #[test]
    fn file_names_come_from_the_title() {
        assert_eq!(file_name(&sample(), Format::Pdf), "build-time-more.pdf");
        assert_eq!(file_name(&sample()[1..], Format::Png), "second.png");
    }

    #[test]
    fn long_text_is_cut_and_wrapped() {
        assert_eq!(fit("abcdefghij", 5.0 * 0.56 * 10.0, 10.0), "abcd…");
        assert_eq!(wrap("one two three four five six seven eight nine ten", 10.0 * 0.56 * 10.0, 10.0).len(), 6);
    }
}
