//! The chat's charts: what a successful `visualize` tool call draws instead of a tool row.
//! Bars, tables, tiles and meters are plain divs (crisp, themed, selectable text); rings, pies
//! and lines are painted on a canvas with their labels laid over as divs. The numbers come from
//! `code::viz`, the spec from `winfer_backend::viz`.

use crate::code::viz::{self as calc, Arc as Slice};
use crate::code::viz_svg::{self, Format};
use std::rc::Rc;
use crate::theme::{self, Colors};
use gpui_kit::assets::IconName;
use gpui_kit::component::Icon;
use gpui_kit::component::text::TextView;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use winfer_backend::viz::{Align, Block, Column, Kind, Meter, Report, Series, Stat};

/// What the renderers need from the transcript's environment.
#[derive(Clone)]
pub struct Style {
    pub c: Colors,
    pub mono: SharedString,
    pub scale: f32,
    /// Width the report has, in px.
    pub width: f32,
    /// Saves the report in a format (asks where); None hides the export buttons.
    pub on_export: Option<Rc<dyn Fn(Format, &mut App)>>,
}

impl Style {
    fn small(&self) -> Pixels {
        px(11.5 * self.scale)
    }

    fn body(&self) -> Pixels {
        px(12.5 * self.scale)
    }
}

/// The categorical colors in their fixed order (never cycled; see the dataviz palette), light
/// and dark steps. Beyond the eighth series, and for a pie's "Other", the neutral gray.
const LIGHT: [&str; 8] = ["#2a78d6", "#eb6834", "#1baf7a", "#eda100", "#e87ba4", "#008300", "#6250d6", "#e34948"];
const DARK: [&str; 8] = ["#3987e5", "#d95926", "#199e70", "#c98500", "#d55181", "#0ca30c", "#9085e9", "#e66767"];

fn series_color(c: &Colors, i: usize) -> Hsla {
    match (if c.light { LIGHT } else { DARK }).get(i) {
        Some(hex) => theme::hex(hex),
        None => c.subtle,
    }
}

fn alpha(c: Hsla, a: f32) -> Hsla {
    Hsla { a, ..c }
}

/// Space between the grid's columns; blocks are unframed, so it is the only thing between them.
const GAP: f32 = 28.0;

/// What the marks sit on: the transcript itself (blocks have no card of their own).
fn surface(c: &Colors) -> Hsla {
    c.panel
}

/// A section label: small caps-like, muted, above a block's hairline.
fn eyebrow(s: &Style, text: &str) -> Div {
    div().truncate().text_size(px(10.5 * s.scale)).font_weight(FontWeight::SEMIBOLD).text_color(s.c.muted).child(text.to_uppercase())
}

fn heading(s: &Style, title: Option<String>, subtitle: Option<String>, main: bool) -> Option<Div> {
    let c = s.c;
    (title.is_some() || subtitle.is_some()).then(|| {
        div().when(!main, |d| d.pt_4()).flex().flex_col().gap_0p5()
            .when(main, |d| d.child(div().pb_1().child(eyebrow(s, "Report"))))
            .when_some(title, |d, t| d.child(div().text_size(px(if main { 20.0 } else { 15.0 } * s.scale)).font_weight(FontWeight::SEMIBOLD).text_color(c.fg).child(t)))
            .when_some(subtitle, |d, t| d.child(div().text_size(s.small()).text_color(c.subtle).child(t)))
    })
}

/// One report from one or more `visualize` calls made in a row: the first one's title heads it,
/// later ones' titles head their own section, and each section's blocks fill a grid of 1-4
/// columns (by the width there is).
pub fn report(s: &Style, key: &str, reports: &[Report]) -> Div {
    let c = s.c;
    let cols = calc::columns(s.width, GAP);
    let mut col = div().w_full().flex().flex_col().gap_5().py_1p5();
    for (n, r) in reports.iter().enumerate() {
        if let Some(h) = heading(s, r.title.clone(), r.subtitle.clone(), n == 0) {
            col = col.child(h);
        }
        let spans: Vec<usize> = r.blocks.iter().map(|b| calc::span_of(b, cols)).collect();
        for row in calc::pack(&spans, cols) {
            let used: usize = row.iter().map(|(_, span)| span).sum();
            // Cells carry half the gap as padding and the row takes it back at its edges, so a
            // two-column cell lines up with two one-column cells below it. A short row's cells
            // grow to fill it.
            let mut line = div().flex().mx(px(-GAP / 2.0));
            for (i, span) in row {
                let cell_w = (s.width + GAP) * span as f32 / used as f32 - GAP;
                let mut cell = div().flex().flex_col().min_w_0().flex_basis(px(0.0)).px(px(GAP / 2.0));
                cell.style().flex_grow = Some(span as f32);
                line = line.child(cell.child(block(s, &format!("{key}-{n}-{i}"), &r.blocks[i], cell_w)));
            }
            col = col.child(line);
        }
    }
    // The export buttons close the report, like a figure's footer.
    let buttons = s.on_export.clone().map(|on_export| {
        div().flex().flex_wrap().gap_1().children(Format::ALL.map(|f| {
            let on_export = on_export.clone();
            div().id(SharedString::from(format!("viz-{key}-export-{}", f.ext()))).flex().items_center().gap_1p5().px_2().py_0p5().rounded_md().border_1().border_color(c.border_strong)
                .cursor_pointer().text_size(px(11.0 * s.scale)).text_color(c.muted)
                .hover(move |d| d.bg(c.hover).text_color(c.fg))
                .child(Icon::new(IconName::Download).size(px(11.0)))
                .child(f.label())
                .tooltip(move |window, cx| Tooltip::new(format!("Export the report as {}", f.label())).build(window, cx))
                .on_click(move |_, _, cx| on_export(f, cx))
        }))
    });
    col.children(buttons)
}

fn block(s: &Style, id: &str, b: &Block, width: f32) -> AnyElement {
    let c = s.c;
    let id = format!("viz-{id}");
    // Narrative text sits between the cards without a frame of its own.
    if let Kind::Text(md) = &b.kind {
        return div().w_full().px_0p5().text_size(s.body()).text_color(c.fg).child(TextView::markdown(SharedString::from(id), SharedString::from(md.clone())).style(super::code_view::md_style(c)).selectable(true)).into_any_element();
    }
    let body = match &b.kind {
        Kind::Stats(items) => stats(s, items).into_any_element(),
        Kind::Bar { labels, series, horizontal: true, stacked, unit } => hbars(s, labels, series, *stacked, unit.as_deref(), width).into_any_element(),
        Kind::Bar { labels, series, horizontal: false, stacked, unit } => vbars(s, labels, series, *stacked, unit.as_deref()).into_any_element(),
        Kind::Line { labels, series, area, unit } => lines(s, labels, series, *area, unit.as_deref(), width).into_any_element(),
        Kind::Pie { slices, center, unit, .. } => pie(s, slices, center.as_ref(), unit.as_deref()).into_any_element(),
        Kind::Table { columns, rows } => table(s, columns, rows).into_any_element(),
        Kind::Progress(items) => progress(s, items).into_any_element(),
        Kind::Text(_) => unreachable!(),
    };
    let copy = b.to_csv();
    // Summary tiles open the report without a rule or a label (unless the agent named them);
    // every other block sits under a hairline with its title as a section label.
    let lead = matches!(b.kind, Kind::Stats(_));
    let title = b.title.clone().or_else(|| (!lead).then(|| b.kind.name().to_string()));
    div().flex_1().w_full().flex().flex_col().gap_2p5()
        .when(!lead, |d| d.border_t_1().border_color(c.border_strong).pt_3())
        .child(
            div().flex().items_center().gap_2().min_h(px(20.0))
                .child(div().flex_1().min_w_0().children(title.map(|t| eyebrow(s, &t))))
                .child(
                    div().id(SharedString::from(format!("{id}-copy"))).size(px(22.0)).flex_shrink_0().flex().items_center().justify_center().rounded_md().cursor_pointer().text_color(c.subtle)
                        .hover(move |d| d.bg(c.hover).text_color(c.fg))
                        .child(Icon::new(IconName::Copy).size(px(12.0)))
                        .tooltip(move |window, cx| Tooltip::new("Copy data").build(window, cx))
                        .on_click(move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(copy.clone()))),
                ),
        )
        .child(body)
        .when_some(b.caption.clone(), |d, t| d.child(div().text_size(s.small()).line_height(relative(1.45)).text_color(c.muted).child(t)))
        .into_any_element()
}

/// Names with their color: a short stroke for lines, a square for filled marks.
fn legend(s: &Style, names: &[&str], line: bool) -> Div {
    let c = s.c;
    let mark = move |color: Hsla| if line { div().w(px(14.0)).h(px(2.0)).flex_shrink_0().rounded(px(1.0)).bg(color) } else { div().size(px(8.0)).flex_shrink_0().rounded(px(2.0)).bg(color) };
    div().flex().flex_wrap().gap_x_3().gap_y_1().children(names.iter().enumerate().map(|(i, n)| div().flex().items_center().gap_1p5().text_size(s.small()).text_color(c.muted).child(mark(series_color(&c, i))).child(n.to_string())))
}

fn names(series: &[Series]) -> Vec<&str> {
    series.iter().map(|s| s.name.as_str()).collect()
}

/// The first tile is the headline, set large; the rest follow as a row of figures divided by
/// hairlines.
fn stats(s: &Style, items: &[Stat]) -> Div {
    let c = s.c;
    let Some((hero, rest)) = items.split_first() else { return div() };
    let delta = |x: String| div().flex_shrink_0().text_size(s.small()).font_weight(FontWeight::MEDIUM).text_color(c.muted).child(x);
    let headline = div().flex().flex_col().gap_1p5().min_w_0()
        .child(div().truncate().text_size(px(44.0 * s.scale)).line_height(relative(1.0)).font_weight(FontWeight::SEMIBOLD).text_color(c.fg).child(hero.value.clone()))
        .child(div().flex().items_center().gap_2().min_w_0().text_size(s.body())
            .child(div().truncate().text_color(c.muted).child(hero.label.clone()))
            .when_some(hero.delta.clone(), |d, x| d.child(delta(x)))
            .when_some(hero.note.clone(), |d, x| d.child(div().truncate().text_size(s.small()).text_color(c.subtle).child(x))));
    let figures = rest.iter().map(|t| {
        let label = match &t.note {
            Some(note) => format!("{} · {note}", t.label),
            None => t.label.clone(),
        };
        div().flex().flex_col().gap_0p5().min_w_0().max_w(px(220.0)).px_4().border_l_1().border_color(c.border_strong)
            .child(div().flex().items_center().gap_1p5()
                .child(div().truncate().text_size(px(18.0 * s.scale)).font_weight(FontWeight::SEMIBOLD).text_color(c.fg).child(t.value.clone()))
                .when_some(t.delta.clone(), |d, x| d.child(delta(x))))
            .child(div().truncate().text_size(s.small()).text_color(c.subtle).child(label))
    });
    div().flex().flex_wrap().items_end().justify_between().gap_x_6().gap_y_4()
        .child(headline)
        .when(!rest.is_empty(), |d| d.child(div().flex().flex_wrap().gap_y_3().children(figures)))
}

/// Meters as a row of ticks, filled in the text color: a budget reads as a count, not a hue.
fn progress(s: &Style, items: &[Meter]) -> Div {
    const TICKS: usize = 30;
    let c = s.c;
    div().flex().flex_col().gap_3().children(items.iter().map(|m| {
        let frac = (m.value / m.max).clamp(0.0, 1.0) as f32;
        let on = (frac * TICKS as f32).round() as usize;
        let percent = (m.max - 100.0).abs() < f64::EPSILON;
        div().flex().flex_col().gap_1p5()
            .child(div().flex().items_center().gap_2().text_size(s.small())
                .child(div().flex_1().min_w_0().truncate().text_color(c.fg).child(m.label.clone()))
                .child(div().flex().flex_shrink_0().font_family(s.mono.clone())
                    .child(div().font_weight(FontWeight::SEMIBOLD).text_color(c.fg).child(calc::compact(m.value, percent.then_some("%"))))
                    .when(!percent, |d| d.child(div().text_color(c.subtle).child(format!(" / {}", calc::compact(m.max, None)))))))
            .child(div().flex().gap(px(2.0)).h(px(12.0)).w_full().children((0..TICKS).map(|k| {
                div().flex_1().h_full().rounded(px(1.0)).bg(if k < on { alpha(c.fg, 0.78) } else { c.hover })
            })))
    }))
}

fn table(s: &Style, columns: &[Column], rows: &[Vec<String>]) -> Div {
    const SHOWN: usize = 15;
    let c = s.c;
    let cell = |text: String, align: Align| {
        let d = div().flex_1().min_w_0().px_2().py_1().flex();
        let d = match align {
            Align::Left => d,
            Align::Right => d.justify_end(),
            Align::Center => d.justify_center(),
        };
        d.child(div().min_w_0().truncate().child(text))
    };
    div().flex().flex_col().text_size(s.small())
        .child(div().flex().border_b_1().border_color(c.fg).text_color(c.subtle).font_weight(FontWeight::MEDIUM).children(columns.iter().map(|col| cell(col.label.clone(), col.align))))
        .children(rows.iter().take(SHOWN).map(|row| {
            div().flex().border_b_1().border_color(c.border).text_color(c.fg).children(row.iter().zip(columns).map(|(t, col)| cell(t.clone(), col.align)))
        }))
        .when(rows.len() > SHOWN, |d| d.child(div().px_2().pt_1p5().text_color(c.subtle).child(format!("+{} more rows. Copy data for all of them.", rows.len() - SHOWN))))
}

fn hbars(s: &Style, labels: &[String], series: &[Series], stacked: bool, unit: Option<&str>, width: f32) -> Div {
    let c = s.c;
    let label_w = (width * 0.3).min(104.0 * s.scale);
    if let [only] = series {
        return lollipops(s, labels, &only.values, unit, label_w);
    }
    let values: Vec<Vec<f64>> = series.iter().map(|x| x.values.clone()).collect();
    let (_, top) = calc::nice_ticks(calc::bar_max(&values, stacked), 4);
    let grouped = !stacked && series.len() > 1;
    let bar_h = if grouped { 8.0 } else { 11.0 };
    let line = |color: Hsla, frac: f32, value: String| {
        div().flex().items_center().gap_2().h(px(bar_h + 5.0))
            .child(div().relative().flex_1().h(px(bar_h)).child(div().absolute().top_0().left_0().h_full().w(relative(frac)).bg(color).rounded_r_sm()))
            .child(div().w(px(54.0 * s.scale)).flex_shrink_0().text_size(s.small()).font_family(s.mono.clone()).text_color(c.muted).flex().justify_end().child(value))
    };
    let rows = labels.iter().enumerate().map(|(i, label)| {
        let bars: Vec<AnyElement> = if stacked {
            let total: f64 = series.iter().map(|x| x.values[i].max(0.0)).sum();
            let segments = series.iter().enumerate().filter(|(_, x)| x.values[i] > 0.0).map(|(k, x)| div().h_full().w(relative((x.values[i] / top) as f32)).bg(series_color(&c, k)).border_r_1().border_color(c.raised));
            vec![div().flex().items_center().gap_2().h(px(bar_h + 5.0))
                .child(div().flex().flex_1().h(px(bar_h)).overflow_hidden().rounded_r_sm().children(segments))
                .child(div().w(px(54.0 * s.scale)).flex_shrink_0().text_size(s.small()).font_family(s.mono.clone()).text_color(c.muted).flex().justify_end().child(calc::compact(total, unit)))
                .into_any_element()]
        } else {
            series.iter().enumerate().map(|(k, x)| line(series_color(&c, k), (x.values[i].max(0.0) / top) as f32, calc::compact(x.values[i], unit)).into_any_element()).collect()
        };
        div().flex().items_center().gap_3()
            .child(div().w(px(label_w)).flex_shrink_0().truncate().text_size(s.small()).text_color(c.muted).child(label.clone()))
            .child(div().flex_1().min_w_0().flex().flex_col().children(bars))
    });
    div().flex().flex_col().gap_1p5()
        .when(series.len() > 1, |d| d.child(legend(s, &names(series), false)))
        .children(rows)
}

/// One series as lollipops: a hairline stem and a dot at the value. The largest value wears
/// the series color and the rest recede to gray, so the leader reads first.
fn lollipops(s: &Style, labels: &[String], values: &[f64], unit: Option<&str>, label_w: f32) -> Div {
    const DOT: f32 = 10.0;
    let c = s.c;
    let (_, top) = calc::nice_ticks(values.iter().cloned().fold(0.0, f64::max), 4);
    let lead = values.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)).map(|(i, _)| i);
    let rest = alpha(c.subtle, 0.6);
    let ring = surface(&c);
    div().flex().flex_col().children(labels.iter().zip(values).enumerate().map(|(i, (label, v))| {
        let color = if Some(i) == lead { series_color(&c, 0) } else { rest };
        let frac = (v.max(0.0) / top) as f32;
        div().flex().items_center().gap_3().h(px(26.0 * s.scale))
            .child(div().w(px(label_w)).flex_shrink_0().truncate().text_size(s.small()).text_color(c.muted).child(label.clone()))
            .child(div().relative().flex_1().h(px(DOT)).mr(px(DOT / 2.0))
                .child(div().absolute().left_0().top(px(DOT / 2.0 - 1.0)).h(px(2.0)).w(relative(frac)).bg(color))
                .child(div().absolute().top_0().left(relative(frac)).ml(px(-DOT / 2.0)).size(px(DOT)).rounded_full().bg(color).border_2().border_color(ring)))
            .child(div().w(px(54.0 * s.scale)).flex_shrink_0().text_size(s.small()).font_family(s.mono.clone()).text_color(c.fg).flex().justify_end().child(calc::compact(*v, unit)))
    }))
}

const PLOT_H: f32 = 150.0;
const AXIS_W: f32 = 40.0;

/// Tick labels down the left of a plot `PLOT_H` high; a label sits on its gridline.
fn y_axis(s: &Style, ticks: &[f64], top: f64, unit: Option<&str>) -> Div {
    let c = s.c;
    div().relative().w(px(AXIS_W)).h(px(PLOT_H)).flex_shrink_0().children(ticks.iter().map(|t| {
        div().absolute().right_0().bottom(relative((t / top) as f32)).mb(px(-7.0)).h(px(14.0)).flex().items_center().text_size(px(10.5 * s.scale)).text_color(c.subtle).child(calc::compact(*t, unit))
    }))
}

fn x_labels(s: &Style, labels: &[String]) -> Div {
    let c = s.c;
    let step = calc::label_step(labels.len(), 10);
    div().flex().pl(px(AXIS_W + 8.0)).pt_1p5().children(labels.iter().enumerate().map(|(i, l)| {
        div().flex_1().min_w_0().flex().justify_center().text_size(px(10.5 * s.scale)).text_color(c.muted).when(i % step == 0, |d| d.child(div().truncate().child(l.clone())))
    }))
}

fn vbars(s: &Style, labels: &[String], series: &[Series], stacked: bool, unit: Option<&str>) -> Div {
    let c = s.c;
    let gap = surface(&c);
    let values: Vec<Vec<f64>> = series.iter().map(|x| x.values.clone()).collect();
    let (ticks, top) = calc::nice_ticks(calc::bar_max(&values, stacked), 4);
    let plot = div().relative().flex_1().h(px(PLOT_H))
        .children(ticks.iter().map(|t| div().absolute().left_0().right_0().bottom(relative((t / top) as f32)).h(px(1.0)).bg(if *t == 0.0 { c.border_strong } else { alpha(c.border, 0.7) })))
        .child(div().absolute().inset_0().flex().items_end().children(labels.iter().enumerate().map(|(i, _)| {
            let group = div().flex_1().h_full().min_w_0().px_1().flex().items_end().justify_center().gap(px(2.0));
            if stacked {
                let total: f64 = series.iter().map(|x| x.values[i].max(0.0)).sum();
                // The total rides on the cap, outside the bar, so it never crowds a segment.
                group.child(
                    div().relative().h(relative((total / top) as f32)).w_full().max_w(px(18.0)).flex().flex_col().justify_end()
                        .child(div().absolute().bottom_full().left(px(-20.0)).right(px(-20.0)).pb_1().flex().justify_center().text_size(px(10.0 * s.scale)).text_color(c.muted).child(calc::compact(total, unit)))
                        .child(div().size_full().flex().flex_col().justify_end().overflow_hidden().rounded_t(px(4.0))
                            .children(series.iter().enumerate().rev().filter(|(_, x)| x.values[i] > 0.0).map(|(k, x)| div().w_full().h(relative((x.values[i] / total) as f32)).bg(series_color(&c, k)).border_t_2().border_color(gap)))),
                )
            } else {
                group.children(series.iter().enumerate().map(|(k, x)| div().flex_1().max_w(px(20.0)).h(relative((x.values[i].max(0.0) / top) as f32)).bg(series_color(&c, k)).rounded_t(px(4.0))))
            }
        })));
    div().flex().flex_col()
        .when(series.len() > 1, |d| d.child(div().pb_4().child(legend(s, &names(series), false))))
        .child(div().flex().gap_2().child(y_axis(s, &ticks, top, unit)).child(plot))
        .child(x_labels(s, labels))
}

/// Width of the column right of a line plot that names each series at its last value.
const END_W: f32 = 92.0;

fn lines(s: &Style, labels: &[String], series: &[Series], area: bool, unit: Option<&str>, width: f32) -> Div {
    let c = s.c;
    let all: Vec<Vec<f64>> = series.iter().map(|x| x.values.clone()).collect();
    let (ticks, top) = calc::nice_ticks(calc::bar_max(&all, false), 4);
    let fractions: Vec<f32> = ticks.iter().map(|t| (t / top) as f32).collect();
    let n = labels.len();
    let paths: Vec<(Hsla, Vec<f32>)> = series.iter().enumerate().map(|(k, x)| (series_color(&c, k), x.values.iter().map(|v| (v / top).clamp(0.0, 1.0) as f32).collect())).collect();
    let (grid, base) = (alpha(c.border, 0.7), c.border_strong);
    let ring = surface(&c);
    let plot = canvas(|_, _, _| {}, move |bounds, _, window, _| {
        for f in &fractions {
            let y = bounds.bottom() - bounds.size.height * *f;
            window.paint_quad(fill(Bounds::new(point(bounds.left(), y - px(0.5)), size(bounds.size.width, px(1.0))), if *f == 0.0 { base } else { grid }));
        }
        let at = |i: usize, f: f32| point(bounds.left() + bounds.size.width * ((i as f32 + 0.5) / n as f32), bounds.bottom() - bounds.size.height * f);
        for (color, fr) in &paths {
            if area && n > 1 {
                let mut p = PathBuilder::fill();
                p.move_to(point(at(0, 0.0).x, bounds.bottom()));
                for (i, f) in fr.iter().enumerate() {
                    p.line_to(at(i, *f));
                }
                p.line_to(point(at(n - 1, 0.0).x, bounds.bottom()));
                p.close();
                if let Ok(p) = p.build() {
                    window.paint_path(p, alpha(*color, 0.14));
                }
            }
            if n > 1 {
                let mut p = PathBuilder::stroke(px(2.0));
                for (i, f) in fr.iter().enumerate() {
                    if i == 0 { p.move_to(at(i, *f)) } else { p.line_to(at(i, *f)) }
                }
                if let Ok(p) = p.build() {
                    window.paint_path(p, *color);
                }
            }
            // One marker at the end of each line, with a surface ring where lines meet; one point
            // alone is only a marker.
            if let Some(f) = fr.last() {
                let p = at(n - 1, *f);
                window.paint_quad(quad(Bounds::new(point(p.x - px(4.5), p.y - px(4.5)), size(px(9.0), px(9.0))), px(4.5), *color, px(2.0), ring, BorderStyle::default()));
            }
        }
    })
    .absolute()
    .size_full();
    // Direct labels at the line ends when there is room for them, nudged apart so they never
    // overlap; the legend stays as well.
    let ends = (n > 0 && series.len() <= 4 && width >= 360.0).then(|| {
        let min_gap = 30.0 / PLOT_H;
        let mut at: Vec<(usize, f32)> = series.iter().enumerate().map(|(k, x)| (k, (x.values[n - 1] / top).clamp(0.0, 1.0) as f32)).collect();
        at.sort_by(|a, b| b.1.total_cmp(&a.1));
        for i in 1..at.len() {
            at[i].1 = at[i].1.min(at[i - 1].1 - min_gap);
        }
        div().relative().w(px(END_W)).h(px(PLOT_H)).flex_shrink_0().children(at.into_iter().map(|(k, f)| {
            div().absolute().left_0().right_0().bottom(relative(f)).mb(px(-14.0)).flex().flex_col()
                .child(div().truncate().text_size(px(12.0 * s.scale)).font_weight(FontWeight::SEMIBOLD).text_color(c.fg).child(calc::compact(series[k].values[n - 1], unit)))
                .child(div().truncate().text_size(px(10.5 * s.scale)).text_color(c.muted).child(series[k].name.clone()))
        }))
    });
    let has_ends = ends.is_some();
    div().flex().flex_col()
        .when(series.len() > 1, |d| d.child(div().pb_3().child(legend(s, &names(series), true))))
        .child(div().flex().gap_2().child(y_axis(s, &ticks, top, unit)).child(div().relative().flex_1().h(px(PLOT_H)).child(plot)).children(ends))
        .child(div().when(has_ends, |d| d.pr(px(END_W + 8.0))).child(x_labels(s, labels)))
}

/// A pie or ring as one part-to-whole bar: segments split by a surface gap, then each part's
/// share set large with its name and value under it. Shares compare along one axis, which
/// reads more exactly than angles.
fn pie(s: &Style, slices: &[winfer_backend::viz::Slice], center: Option<&(String, String)>, unit: Option<&str>) -> Div {
    let c = s.c;
    let arcs: Vec<Slice> = calc::arcs(slices, 8);
    let colors: Vec<Hsla> = arcs.iter().enumerate().map(|(i, a)| if a.other { c.subtle } else { series_color(&c, i) }).collect();
    let last = arcs.len().saturating_sub(1);
    let bar = div().flex().gap(px(2.0)).h(px(18.0)).w_full().children(arcs.iter().zip(&colors).enumerate().map(|(i, (a, k))| {
        let mut seg = div().h_full().min_w(px(3.0)).flex_basis(px(0.0)).bg(*k).when(i == 0, |d| d.rounded_l(px(4.0))).when(i == last, |d| d.rounded_r(px(4.0)));
        seg.style().flex_grow = Some(a.value.max(0.0) as f32);
        seg
    }));
    let parts = arcs.iter().zip(&colors).map(|(a, k)| {
        div().w(px(104.0 * s.scale)).flex().flex_col().gap_0p5()
            .child(div().flex().items_center().gap_1p5()
                .child(div().size(px(8.0)).flex_shrink_0().rounded(px(2.0)).bg(*k))
                .child(div().text_size(px(18.0 * s.scale)).font_weight(FontWeight::SEMIBOLD).text_color(c.fg).child(calc::percent(a.percent))))
            .child(div().truncate().text_size(px(11.0 * s.scale)).text_color(c.subtle).child(format!("{} · {}", a.label, calc::compact(a.value, unit))))
    });
    div().flex().flex_col().gap_3()
        .when_some(center.cloned(), |d, (value, label)| d.child(div().flex().items_center().gap_1p5()
            .child(div().text_size(px(15.0 * s.scale)).font_weight(FontWeight::SEMIBOLD).text_color(c.fg).child(value))
            .child(div().text_size(s.small()).text_color(c.muted).child(label))))
        .child(bar)
        .child(div().flex().flex_wrap().gap_x_3().gap_y_3().children(parts))
}

/// The chat's colors as the file's: the pane background, cards and tiles, text, the palette.
pub fn file_theme(c: &Colors) -> viz_svg::Theme {
    let hex = |h: Hsla| {
        let r = Rgba::from(h);
        format!("#{:02x}{:02x}{:02x}", (r.r * 255.0).round() as u8, (r.g * 255.0).round() as u8, (r.b * 255.0).round() as u8)
    };
    viz_svg::Theme {
        bg: hex(c.panel),
        card: hex(c.raised),
        tile: hex(c.app),
        border: hex(c.edge),
        strong: hex(c.border_strong),
        fg: hex(c.fg),
        muted: hex(c.muted),
        subtle: hex(c.subtle),
        accent: hex(c.accent),
        track: hex(c.hover),
        series: (if c.light { LIGHT } else { DARK }).map(String::from).to_vec(),
    }
}

/// Width exports are laid out at: four columns, whatever the pane's width.
const EXPORT_WIDTH: f32 = 1240.0;

/// Asks where to save the report, then writes it in `format` off the main thread. `done` gets
/// the saved path or the error.
pub fn export(reports: Vec<Report>, theme: viz_svg::Theme, format: Format, cx: &mut App, done: impl FnOnce(Result<std::path::PathBuf, String>, &mut App) + 'static) {
    let dir = dirs::download_dir().or_else(dirs::home_dir).unwrap_or_default();
    let target = cx.prompt_for_new_path(&dir, Some(&viz_svg::file_name(&reports, format)));
    cx.spawn(async move |cx| {
        let Ok(Ok(Some(mut path))) = target.await else { return };
        if path.extension().and_then(|e| e.to_str()).is_none_or(|e| !e.eq_ignore_ascii_case(format.ext()) && !(format == Format::Jpg && e.eq_ignore_ascii_case("jpeg"))) {
            let name = format!("{}.{}", path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(), format.ext());
            path.set_file_name(name);
        }
        let result = crate::backend::background(move || {
            let svg = viz_svg::svg(&reports, EXPORT_WIDTH, &theme);
            let bytes = viz_svg::encode(&svg, format)?;
            std::fs::write(&path, bytes).map_err(|e| e.to_string())?;
            Ok(path)
        })
        .await
        .map_err(|e| e.to_string())
        .and_then(|r| r);
        let _ = cx.update(|cx| done(result, cx));
    })
    .detach();
}
