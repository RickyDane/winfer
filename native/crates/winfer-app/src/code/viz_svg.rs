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
    pub card: String,
    pub tile: String,
    pub border: String,
    pub strong: String,
    pub fg: String,
    pub muted: String,
    pub subtle: String,
    pub accent: String,
    pub track: String,
    pub series: Vec<String>,
}

impl Theme {
    fn series(&self, i: usize) -> &str {
        self.series.get(i).unwrap_or(&self.subtle)
    }
}

const PAD: f32 = 28.0;
const GAP: f32 = 10.0;
const CARD_PAD: f32 = 14.0;
const CARD_HEAD: f32 = 28.0;
const PLOT_H: f32 = 150.0;
const AXIS_W: f32 = 40.0;
const LEGEND_H: f32 = 22.0;
const PIE: f32 = 148.0;
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

    fn dot(&mut self, x: f32, y: f32, color: &str) {
        let _ = write!(self.out, r#"<circle cx="{x:.1}" cy="{y:.1}" r="4" fill="{color}"/>"#);
    }
}

// ---------------------------------------------------------------------------------------------
// Heights: what each block needs at a given width (cards in a grid row take the tallest).

fn stats_layout(items: &[Stat], w: f32) -> (usize, f32) {
    let per_row = (((w + 8.0) / 148.0).floor() as usize).clamp(1, items.len().max(1));
    let tile_h = 16.0 + 16.0 + 30.0 + if items.iter().any(|s| s.delta.is_some()) { 16.0 } else { 0.0 } + if items.iter().any(|s| s.note.is_some()) { 15.0 } else { 0.0 };
    (per_row, tile_h)
}

fn hbar_row(series: usize, stacked: bool) -> f32 {
    if stacked || series == 1 { 22.0 } else { series as f32 * 13.0 + 9.0 }
}

fn body_height(b: &Block, w: f32) -> f32 {
    let legend = |n: usize| if n > 1 { LEGEND_H } else { 0.0 };
    match &b.kind {
        Kind::Stats(items) => {
            let (per_row, tile_h) = stats_layout(items, w);
            let rows = items.len().div_ceil(per_row) as f32;
            rows * tile_h + (rows - 1.0) * 8.0
        }
        Kind::Bar { labels, series, horizontal: true, stacked, .. } => legend(series.len()) + labels.len() as f32 * hbar_row(series.len(), *stacked),
        Kind::Bar { series, .. } | Kind::Line { series, .. } => legend(series.len()) + PLOT_H + 22.0,
        Kind::Pie { slices, .. } => {
            let rows = calc::arcs(slices, 8).len() as f32 * 22.0;
            if w >= 400.0 { PIE.max(rows) } else { PIE + 14.0 + rows }
        }
        Kind::Table { rows, .. } => 28.0 + rows.len() as f32 * 26.0,
        Kind::Progress(items) => items.len() as f32 * 36.0 - 6.0,
        Kind::Text(md) => wrap(&plain(md), w, 13.0).len() as f32 * 19.0,
    }
}

fn caption_lines(b: &Block, w: f32) -> Vec<String> {
    b.caption.as_deref().map(|c| wrap(c, w, 11.5)).unwrap_or_default()
}

/// The block's full height: its card (title, body, caption), or just the text.
fn block_height(b: &Block, w: f32) -> f32 {
    if matches!(b.kind, Kind::Text(_)) {
        return body_height(b, w);
    }
    let inner = w - 2.0 * CARD_PAD;
    let caption = caption_lines(b, inner);
    CARD_PAD + CARD_HEAD + body_height(b, inner) + if caption.is_empty() { 0.0 } else { 10.0 + caption.len() as f32 * 16.0 } + CARD_PAD
}

// ---------------------------------------------------------------------------------------------
// Drawing.

fn legend(svg: &mut Svg, x: f32, y: f32, series: &[Series]) {
    let mut at = x;
    for (i, s) in series.iter().enumerate() {
        let color = svg.t.series(i).to_string();
        svg.dot(at + 4.0, y + 8.0, &color);
        let muted = svg.t.muted.clone();
        svg.text(at + 13.0, y + 8.0, &s.name, 11.5, &muted, 400, Anchor::Start, false);
        at += 13.0 + est(&s.name, 11.5) + 16.0;
    }
}

fn draw_stats(svg: &mut Svg, items: &[Stat], x: f32, y: f32, w: f32) {
    let (per_row, tile_h) = stats_layout(items, w);
    let t = svg.t.clone();
    for (row, chunk) in items.chunks(per_row).enumerate() {
        let tile_w = (w - 8.0 * (chunk.len() as f32 - 1.0)) / chunk.len() as f32;
        for (i, s) in chunk.iter().enumerate() {
            let (tx, ty) = (x + i as f32 * (tile_w + 8.0), y + row as f32 * (tile_h + 8.0));
            svg.rect(tx, ty, tile_w, tile_h, 8.0, &t.tile, Some(&t.border));
            let iw = tile_w - 24.0;
            svg.text(tx + 12.0, ty + 18.0, &fit(&s.label, iw, 11.5), 11.5, &t.muted, 400, Anchor::Start, false);
            svg.text(tx + 12.0, ty + 42.0, &fit(&s.value, iw, 21.0), 21.0, &t.fg, 600, Anchor::Start, false);
            let mut line = ty + 66.0;
            if let Some(d) = &s.delta {
                svg.text(tx + 12.0, line, &fit(d, iw, 11.5), 11.5, &t.muted, 400, Anchor::Start, true);
                line += 16.0;
            }
            if let Some(n) = &s.note {
                svg.text(tx + 12.0, line, &fit(n, iw, 11.0), 11.0, &t.subtle, 400, Anchor::Start, false);
            }
        }
    }
}

fn draw_hbars(svg: &mut Svg, labels: &[String], series: &[Series], stacked: bool, unit: Option<&str>, x: f32, mut y: f32, w: f32) {
    let t = svg.t.clone();
    if series.len() > 1 {
        legend(svg, x, y, series);
        y += LEGEND_H;
    }
    let values: Vec<Vec<f64>> = series.iter().map(|s| s.values.clone()).collect();
    let (_, top) = calc::nice_ticks(calc::bar_max(&values, stacked), 4);
    let label_w = (w * 0.3).min(104.0);
    let (bx, value_w) = (x + label_w + 12.0, 54.0);
    let bw = (w - label_w - 12.0 - value_w - 8.0).max(10.0);
    let row_h = hbar_row(series.len(), stacked);
    for (i, label) in labels.iter().enumerate() {
        let ry = y + i as f32 * row_h;
        svg.text(x, ry + row_h / 2.0, &fit(label, label_w, 11.5), 11.5, &t.muted, 400, Anchor::Start, false);
        if stacked || series.len() == 1 {
            let (by, h) = (ry + (row_h - 11.0) / 2.0, 11.0);
            let mut at = bx;
            let mut total = 0.0;
            for (k, s) in series.iter().enumerate() {
                let v = s.values[i].max(0.0);
                total += v;
                let len = bw * (v / top) as f32;
                if len > 0.0 {
                    svg.rect(at, by, (len - if stacked && series.len() > 1 { 2.0 } else { 0.0 }).max(1.0), h, 2.0, t.series(k), None);
                }
                at += len;
            }
            let shown = if stacked { total } else { series[0].values[i] };
            svg.text(x + w, ry + row_h / 2.0, &calc::compact(shown, unit), 11.5, &t.muted, 400, Anchor::End, true);
        } else {
            for (k, s) in series.iter().enumerate() {
                let by = ry + 4.0 + k as f32 * 13.0;
                svg.rect(bx, by, (bw * (s.values[i].max(0.0) / top) as f32).max(1.0), 8.0, 2.0, t.series(k), None);
                svg.text(x + w, by + 4.0, &calc::compact(s.values[i], unit), 10.5, &t.muted, 400, Anchor::End, true);
            }
        }
    }
}

/// Gridlines and tick labels of a plot; returns the plot's left edge and width.
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

fn draw_vbars(svg: &mut Svg, labels: &[String], series: &[Series], stacked: bool, unit: Option<&str>, x: f32, mut y: f32, w: f32) {
    let t = svg.t.clone();
    if series.len() > 1 {
        legend(svg, x, y, series);
        y += LEGEND_H;
    }
    let values: Vec<Vec<f64>> = series.iter().map(|s| s.values.clone()).collect();
    let (ticks, top) = calc::nice_ticks(calc::bar_max(&values, stacked), 4);
    let (px0, pw) = axis(svg, &ticks, top, unit, x, y, w);
    let slot = pw / labels.len().max(1) as f32;
    let base = y + PLOT_H;
    for i in 0..labels.len() {
        let cx = px0 + slot * (i as f32 + 0.5);
        if stacked {
            let bw = (slot - 8.0).clamp(2.0, 36.0);
            let mut at = base;
            for (k, s) in series.iter().enumerate() {
                let h = PLOT_H * (s.values[i].max(0.0) / top) as f32;
                if h > 0.0 {
                    svg.rect(cx - bw / 2.0, at - h, bw, (h - 2.0).max(1.0), 2.0, t.series(k), None);
                }
                at -= h;
            }
        } else {
            let n = series.len() as f32;
            let bw = ((slot - 8.0 - 2.0 * (n - 1.0)) / n).clamp(2.0, 28.0);
            let start = cx - (bw * n + 2.0 * (n - 1.0)) / 2.0;
            for (k, s) in series.iter().enumerate() {
                let h = PLOT_H * (s.values[i].max(0.0) / top) as f32;
                svg.rect(start + k as f32 * (bw + 2.0), base - h, bw, h, 2.0, t.series(k), None);
            }
        }
    }
    x_labels(svg, labels, px0, pw, base);
}

fn draw_lines(svg: &mut Svg, labels: &[String], series: &[Series], area: bool, unit: Option<&str>, x: f32, mut y: f32, w: f32) {
    let t = svg.t.clone();
    if series.len() > 1 {
        legend(svg, x, y, series);
        y += LEGEND_H;
    }
    let values: Vec<Vec<f64>> = series.iter().map(|s| s.values.clone()).collect();
    let (ticks, top) = calc::nice_ticks(calc::bar_max(&values, false), 4);
    let (px0, pw) = axis(svg, &ticks, top, unit, x, y, w);
    let n = labels.len();
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
            let _ = write!(svg.out, r#"<polyline points="{list}" fill="none" stroke="{color}" stroke-width="2" stroke-linejoin="round"/>"#);
        }
        if n <= 24 {
            for (px, py) in &pts {
                let _ = write!(svg.out, r#"<circle cx="{px:.1}" cy="{py:.1}" r="3.5" fill="{color}" stroke="{}" stroke-width="2"/>"#, t.card);
            }
        }
    }
    x_labels(svg, labels, px0, pw, base);
}

/// A slice outline (a sector, or a ring segment when `inner` > 0), as polygon points.
fn sector(cx: f32, cy: f32, outer: f32, inner: f32, from: f32, to: f32) -> String {
    let at = |r: f32, f: f32| {
        let a = f * std::f32::consts::TAU - std::f32::consts::FRAC_PI_2;
        format!("{:.2},{:.2}", cx + r * a.cos(), cy + r * a.sin())
    };
    let steps = (((to - from) * 96.0).ceil() as usize).max(2);
    let mut pts: Vec<String> = (0..=steps).map(|i| at(outer, from + (to - from) * i as f32 / steps as f32)).collect();
    if inner > 0.0 {
        pts.extend((0..=steps).rev().map(|i| at(inner, from + (to - from) * i as f32 / steps as f32)));
    } else {
        pts.push(format!("{cx:.2},{cy:.2}"));
    }
    pts.join(" ")
}

#[allow(clippy::too_many_arguments)]
fn draw_pie(svg: &mut Svg, slices: &[winfer_backend::viz::Slice], ring: bool, center: Option<&(String, String)>, unit: Option<&str>, x: f32, y: f32, w: f32) {
    let t = svg.t.clone();
    let arcs = calc::arcs(slices, 8);
    let total: f64 = arcs.iter().map(|a| a.value).sum();
    let side = w >= 400.0;
    let rows_h = arcs.len() as f32 * 22.0;
    let (cx, cy) = if side { (x + PIE / 2.0, y + PIE.max(rows_h) / 2.0) } else { (x + w / 2.0, y + PIE / 2.0) };
    let r = PIE / 2.0;
    let many = arcs.len() > 1;
    for (i, a) in arcs.iter().enumerate() {
        let color = if a.other { t.subtle.clone() } else { t.series(i).to_string() };
        let gap = if many && a.to - a.from > 0.02 { 0.0035 } else { 0.0 };
        let _ = write!(svg.out, r#"<polygon points="{}" fill="{color}"/>"#, sector(cx, cy, r, if ring { r * 0.62 } else { 0.0 }, a.from + gap, a.to - gap));
    }
    if ring {
        let (value, label) = center.cloned().unwrap_or_else(|| (calc::compact(total, unit), String::new()));
        let inner = r * 2.0 * 0.56;
        if label.is_empty() {
            svg.text(cx, cy, &fit(&value, inner, 19.0), 19.0, &t.fg, 600, Anchor::Middle, false);
        } else {
            svg.text(cx, cy - 8.0, &fit(&value, inner, 19.0), 19.0, &t.fg, 600, Anchor::Middle, false);
            svg.text(cx, cy + 13.0, &fit(&label, inner, 10.5), 10.5, &t.muted, 400, Anchor::Middle, false);
        }
    }
    let (lx, ly, lw) = if side {
        let lx = x + PIE + 20.0;
        (lx, y + (PIE.max(rows_h) - rows_h) / 2.0, (w - PIE - 20.0).min(420.0))
    } else {
        (x, y + PIE + 14.0, w.min(420.0))
    };
    let lx = if side { lx } else { x + (w - lw) / 2.0 };
    for (i, a) in arcs.iter().enumerate() {
        let color = if a.other { t.subtle.clone() } else { t.series(i).to_string() };
        let ry = ly + i as f32 * 22.0 + 11.0;
        svg.dot(lx + 4.0, ry, &color);
        let pct = calc::percent(a.percent);
        let value = calc::compact(a.value, unit);
        let right = 48.0 + est(&value, 11.5) + 10.0;
        svg.text(lx + 14.0, ry, &fit(&a.label, lw - 14.0 - right, 11.5), 11.5, &t.fg, 400, Anchor::Start, false);
        svg.text(lx + lw - 48.0, ry, &value, 11.5, &t.muted, 400, Anchor::End, true);
        svg.text(lx + lw, ry, &pct, 11.5, &t.subtle, 400, Anchor::End, true);
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
        cell(svg, i, y + 13.0, &c.label, &t.muted, 500);
    }
    svg.line(x, y + 27.5, x + w, y + 27.5, &t.strong, 1.0);
    for (r, row) in rows.iter().enumerate() {
        let ry = y + 28.0 + r as f32 * 26.0;
        for (i, s) in row.iter().enumerate().take(columns.len()) {
            cell(svg, i, ry + 13.0, s, &t.fg, 400);
        }
        svg.line(x, ry + 25.5, x + w, ry + 25.5, &t.border, 1.0);
    }
}

fn draw_progress(svg: &mut Svg, items: &[Meter], x: f32, y: f32, w: f32) {
    let t = svg.t.clone();
    for (i, m) in items.iter().enumerate() {
        let my = y + i as f32 * 36.0;
        let value = if (m.max - 100.0).abs() < f64::EPSILON { calc::compact(m.value, Some("%")) } else { format!("{} / {}", calc::compact(m.value, None), calc::compact(m.max, None)) };
        svg.text(x, my + 8.0, &fit(&m.label, w - est(&value, 11.5) - 12.0, 11.5), 11.5, &t.muted, 400, Anchor::Start, false);
        svg.text(x + w, my + 8.0, &value, 11.5, &t.fg, 400, Anchor::End, true);
        svg.rect(x, my + 18.0, w, 6.0, 3.0, &t.track, None);
        let frac = (m.value / m.max).clamp(0.0, 1.0) as f32;
        if frac > 0.0 {
            svg.rect(x, my + 18.0, (w * frac).max(6.0), 6.0, 3.0, &t.accent, None);
        }
    }
}

fn draw_body(svg: &mut Svg, b: &Block, x: f32, y: f32, w: f32) {
    match &b.kind {
        Kind::Stats(items) => draw_stats(svg, items, x, y, w),
        Kind::Bar { labels, series, horizontal: true, stacked, unit } => draw_hbars(svg, labels, series, *stacked, unit.as_deref(), x, y, w),
        Kind::Bar { labels, series, stacked, unit, .. } => draw_vbars(svg, labels, series, *stacked, unit.as_deref(), x, y, w),
        Kind::Line { labels, series, area, unit } => draw_lines(svg, labels, series, *area, unit.as_deref(), x, y, w),
        Kind::Pie { slices, ring, center, unit } => draw_pie(svg, slices, *ring, center.as_ref(), unit.as_deref(), x, y, w),
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

/// A block in a cell `h` tall: a card around the body, or the bare text.
fn draw_block(svg: &mut Svg, b: &Block, x: f32, y: f32, w: f32, h: f32) {
    if matches!(b.kind, Kind::Text(_)) {
        draw_body(svg, b, x, y, w);
        return;
    }
    let t = svg.t.clone();
    svg.rect(x + 0.5, y + 0.5, w - 1.0, h - 1.0, 12.0, &t.card, Some(&t.border));
    let inner = w - 2.0 * CARD_PAD;
    let title = b.title.clone().unwrap_or_else(|| b.kind.name().replace("stat row", "Summary"));
    let mut chars = title.chars();
    let title: String = chars.next().map(|f| f.to_uppercase().chain(chars).collect()).unwrap_or_default();
    svg.text(x + CARD_PAD, y + CARD_PAD + 9.0, &fit(&title, inner, 12.5), 12.5, &t.fg, 500, Anchor::Start, false);
    let by = y + CARD_PAD + CARD_HEAD;
    draw_body(svg, b, x + CARD_PAD, by, inner);
    let mut cy = by + body_height(b, inner) + 10.0;
    for line in caption_lines(b, inner) {
        svg.text(x + CARD_PAD, cy + 8.0, &line, 11.5, &t.muted, 400, Anchor::Start, false);
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
            y += 10.0;
        }
        if let Some(title) = &r.title {
            let size = if main { 18.0 } else { 15.0 };
            svg.text(PAD, y + size / 2.0, &fit(title, width, size), size, &theme.fg, 600, Anchor::Start, false);
            y += size + 6.0;
        }
        if let Some(sub) = &r.subtitle {
            svg.text(PAD, y + 7.0, &fit(sub, width, 12.0), 12.0, &theme.muted, 400, Anchor::Start, false);
            y += 20.0;
        }
        if r.title.is_some() || r.subtitle.is_some() {
            y += 6.0;
        }
        let spans: Vec<usize> = r.blocks.iter().map(|b| calc::span_of(b, cols)).collect();
        for row in calc::pack(&spans, cols) {
            let used: usize = row.iter().map(|(_, s)| s).sum();
            let cells: Vec<(usize, f32)> = row.iter().map(|&(i, s)| (i, (width + GAP) * s as f32 / used as f32 - GAP)).collect();
            let h = cells.iter().map(|&(i, w)| block_height(&r.blocks[i], w)).fold(0.0, f32::max);
            let mut x = PAD;
            for (i, w) in cells {
                draw_block(&mut svg, &r.blocks[i], x, y, w, h);
                x += w + GAP;
            }
            y += h + GAP;
        }
    }
    let (w, h) = (width + 2.0 * PAD, y - GAP + PAD);
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
        Theme { bg: s("#111111"), card: s("#1a1a1a"), tile: s("#0d0d0d"), border: s("#333333"), strong: s("#444444"), fg: s("#eeeeee"), muted: s("#aaaaaa"), subtle: s("#777777"), accent: s("#3987e5"), track: s("#222222"), series: ["#3987e5", "#d95926", "#199e70"].map(String::from).to_vec() }
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
