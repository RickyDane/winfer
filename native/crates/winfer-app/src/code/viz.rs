//! The numbers behind the chat's charts: axis ticks, number formatting, pie arcs and bar
//! scales. No UI here (unit tested); `ui/viz.rs` draws what this computes. The spec itself
//! (`Report`) and its validation live in the backend (`winfer_backend::viz`).

use serde_json::Value;
use winfer_backend::viz::{Block, Kind, Report, Slice, Span, is_visualize, spec_of};

/// The report a tool row shows as a chart: only for a successful `visualize` call whose input
/// still parses (saved sessions replay old inputs).
pub fn report_of(name: &str, input: &Value, ok: Option<bool>) -> Option<Report> {
    if ok == Some(false) || !is_visualize(name) {
        return None;
    }
    Report::parse(spec_of(input)).ok()
}

/// Narrowest a grid cell gets before the report drops a column.
pub const MIN_CELL: f32 = 300.0;
pub const MAX_COLUMNS: usize = 4;

/// Columns a report gets at `width` px: 1 when narrow, else as many cells of at least
/// `MIN_CELL` as fit, up to four.
pub fn columns(width: f32, gap: f32) -> usize {
    (((width + gap) / (MIN_CELL + gap)).floor() as usize).clamp(1, MAX_COLUMNS)
}

/// Columns a block takes in a grid of `cols`: the agent's `span`, else by kind (summary tiles
/// and text run across, tables and long series get two).
pub fn span_of(block: &Block, cols: usize) -> usize {
    let natural = match &block.kind {
        Kind::Stats(_) | Kind::Text(_) => cols,
        Kind::Table { columns, .. } if columns.len() > 4 => cols,
        Kind::Table { .. } => 2,
        Kind::Line { labels, .. } if labels.len() > 12 => 2,
        Kind::Bar { labels, horizontal: false, .. } if labels.len() > 8 => 2,
        _ => 1,
    };
    match block.span {
        Some(Span::Full) => cols,
        Some(Span::Cols(n)) => n as usize,
        None => natural,
    }
    .clamp(1, cols)
}

/// Splits blocks (by their spans) into grid rows, in order: a block that doesn't fit what is
/// left of a row starts the next one. Each entry is (block index, span).
pub fn pack(spans: &[usize], cols: usize) -> Vec<Vec<(usize, usize)>> {
    let mut rows: Vec<Vec<(usize, usize)>> = Vec::new();
    let mut used = cols;
    for (i, &span) in spans.iter().enumerate() {
        let span = span.clamp(1, cols);
        if used + span > cols {
            rows.push(Vec::new());
            used = 0;
        }
        rows.last_mut().unwrap().push((i, span));
        used += span;
    }
    rows
}

/// "34.5%", "8%": one decimal at most.
pub fn percent(p: f64) -> String {
    let s = format!("{p:.1}");
    format!("{}%", s.trim_end_matches(".0"))
}

/// A readable axis: ticks from 0 up to a round `top` at or above `max`, about `want` of them.
pub fn nice_ticks(max: f64, want: usize) -> (Vec<f64>, f64) {
    if !(max.is_finite() && max > 0.0) {
        return (vec![0.0, 1.0], 1.0);
    }
    let raw = max / want.max(1) as f64;
    let mag = 10f64.powf(raw.log10().floor());
    let step = [1.0, 2.0, 2.5, 5.0, 10.0].iter().map(|m| m * mag).find(|s| *s >= raw * 0.999).unwrap_or(10.0 * mag);
    let n = (max / step - 1e-9).ceil().max(1.0) as usize;
    ((0..=n).map(|i| i as f64 * step).collect(), n as f64 * step)
}

/// 1234 → "1.2k", 0.5 → "0.5", 2_500_000 → "2.5M"; the unit follows ("%" hugs, words get a space).
pub fn compact(v: f64, unit: Option<&str>) -> String {
    let a = v.abs();
    let (scaled, suffix) = if a >= 1e9 { (v / 1e9, "B") } else if a >= 1e6 { (v / 1e6, "M") } else if a >= 1e4 { (v / 1e3, "k") } else { (v, "") };
    let mut s = if scaled.fract().abs() < 0.05 { format!("{}", scaled.round() as i64) } else if scaled.abs() < 10.0 { format!("{scaled:.2}") } else { format!("{scaled:.1}") };
    if s.contains('.') {
        s = s.trim_end_matches('0').trim_end_matches('.').to_string();
    }
    s.push_str(suffix);
    match unit {
        Some(u) if matches!(u, "%" | "°" | "x") => s.push_str(u),
        Some(u) if u.chars().count() <= 2 && !u.chars().next().is_some_and(char::is_alphabetic) => s = format!("{u}{s}"),
        Some(u) => s = format!("{s} {u}"),
        None => {}
    }
    s
}

/// A pie slice with its place on the circle (fractions of a full turn, from 12 o'clock).
#[derive(Clone, Debug, PartialEq)]
pub struct Arc {
    pub label: String,
    pub value: f64,
    pub from: f32,
    pub to: f32,
    pub percent: f64,
    /// The folded remainder, drawn in the neutral color.
    pub other: bool,
}

/// At most `max` arcs: the biggest slices keep their place and the rest fold into "Other".
pub fn arcs(slices: &[Slice], max: usize) -> Vec<Arc> {
    let total: f64 = slices.iter().map(|s| s.value).sum();
    if total <= 0.0 {
        return Vec::new();
    }
    let mut parts: Vec<(String, f64, bool)> = slices.iter().filter(|s| s.value > 0.0).map(|s| (s.label.clone(), s.value, false)).collect();
    if parts.len() > max {
        let mut order: Vec<usize> = (0..parts.len()).collect();
        order.sort_by(|a, b| parts[*b].1.total_cmp(&parts[*a].1));
        let keep: std::collections::HashSet<usize> = order.into_iter().take(max.saturating_sub(1)).collect();
        let rest: f64 = parts.iter().enumerate().filter(|(i, _)| !keep.contains(i)).map(|(_, p)| p.1).sum();
        parts = parts.into_iter().enumerate().filter(|(i, _)| keep.contains(i)).map(|(_, p)| p).collect();
        parts.push(("Other".into(), rest, true));
    }
    let mut at = 0.0f64;
    parts
        .into_iter()
        .map(|(label, value, other)| {
            let share = value / total;
            let arc = Arc { label, value, from: at as f32, to: (at + share) as f32, percent: share * 100.0, other };
            at += share;
            arc
        })
        .collect()
}

/// Largest value a bar chart's axis must hold: the stacked totals, or the single biggest bar.
pub fn bar_max(series: &[Vec<f64>], stacked: bool) -> f64 {
    let n = series.iter().map(Vec::len).max().unwrap_or(0);
    (0..n)
        .map(|i| if stacked { series.iter().filter_map(|s| s.get(i)).filter(|v| **v > 0.0).sum::<f64>() } else { series.iter().filter_map(|s| s.get(i)).fold(0.0, |m: f64, v| m.max(*v)) })
        .fold(0.0, f64::max)
}

/// Every `n`th label so they fit `room` slots (the first and last-ish ones survive).
pub fn label_step(count: usize, room: usize) -> usize {
    count.div_ceil(room.max(1)).max(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn slice(label: &str, value: f64) -> Slice {
        Slice { label: label.into(), value }
    }

    #[test]
    fn ticks_are_round_and_cover_the_max() {
        assert_eq!(nice_ticks(87.0, 4), (vec![0.0, 25.0, 50.0, 75.0, 100.0], 100.0));
        assert_eq!(nice_ticks(9.0, 3), (vec![0.0, 5.0, 10.0], 10.0));
        let (t, top) = nice_ticks(0.37, 4);
        assert!(top >= 0.37 && t.len() >= 2, "{t:?}");
        assert_eq!(nice_ticks(0.0, 4).1, 1.0);
        assert_eq!(nice_ticks(f64::NAN, 4).1, 1.0);
        let (_, top) = nice_ticks(100.0, 4);
        assert_eq!(top, 100.0);
    }

    #[test]
    fn numbers_read_short() {
        assert_eq!(compact(1234.0, None), "1234");
        assert_eq!(compact(12_345.0, None), "12.3k");
        assert_eq!(compact(2_500_000.0, None), "2.5M");
        assert_eq!(compact(0.5, None), "0.5");
        assert_eq!(compact(42.0, Some("%")), "42%");
        assert_eq!(compact(3.0, Some("$")), "$3");
        assert_eq!(compact(41.0, Some("ms")), "41 ms");
        assert_eq!(compact(-8.0, None), "-8");
    }

    #[test]
    fn arcs_cover_the_circle_in_order() {
        let a = arcs(&[slice("a", 1.0), slice("b", 3.0)], 7);
        assert_eq!(a.len(), 2);
        assert_eq!((a[0].from, a[0].to), (0.0, 0.25));
        assert!((a[1].to - 1.0).abs() < 1e-6);
        assert!((a[1].percent - 75.0).abs() < 1e-9);
    }

    #[test]
    fn small_slices_fold_into_other_keeping_the_biggest() {
        let slices: Vec<Slice> = (1..=10).map(|i| slice(&format!("s{i}"), i as f64)).collect();
        let a = arcs(&slices, 4);
        assert_eq!(a.len(), 4);
        assert_eq!(a.iter().map(|x| x.label.as_str()).collect::<Vec<_>>(), ["s8", "s9", "s10", "Other"]);
        assert!(a[3].other && (a[3].value - 28.0).abs() < 1e-9);
        assert!((a[3].to - 1.0).abs() < 1e-5);
    }

    #[test]
    fn zero_slices_are_skipped() {
        let a = arcs(&[slice("a", 0.0), slice("b", 2.0)], 7);
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].label, "b");
        assert!(arcs(&[slice("a", 0.0)], 7).is_empty());
    }

    #[test]
    fn bar_axis_holds_stacks_or_the_tallest_bar() {
        let s = vec![vec![1.0, 5.0], vec![2.0, 1.0]];
        assert_eq!(bar_max(&s, false), 5.0);
        assert_eq!(bar_max(&s, true), 6.0);
    }

    #[test]
    fn labels_thin_out_to_fit() {
        assert_eq!(label_step(6, 8), 1);
        assert_eq!(label_step(30, 8), 4);
    }

    #[test]
    fn reports_get_two_to_four_columns_when_there_is_room() {
        assert_eq!(columns(280.0, 10.0), 1);
        assert_eq!(columns(620.0, 10.0), 2);
        assert_eq!(columns(950.0, 10.0), 3);
        assert_eq!(columns(1460.0, 10.0), 4);
        assert_eq!(columns(4000.0, 10.0), 4);
    }

    #[test]
    fn blocks_pack_into_rows_in_order() {
        // stats (full), ring, bars, line, stacked, progress, table (2), text (full) in 4 columns
        let rows = pack(&[4, 1, 1, 1, 1, 1, 2, 4], 4);
        assert_eq!(rows, vec![vec![(0, 4)], vec![(1, 1), (2, 1), (3, 1), (4, 1)], vec![(5, 1), (6, 2)], vec![(7, 4)]]);
        assert_eq!(pack(&[1, 3, 1, 1], 2), vec![vec![(0, 1)], vec![(1, 2)], vec![(2, 1), (3, 1)]]);
    }

    #[test]
    fn spans_follow_the_kind_unless_the_agent_sets_one() {
        let block = |v: serde_json::Value| Report::parse(&json!({ "blocks": [v] })).unwrap().blocks.remove(0);
        let stats = block(json!({ "type": "stats", "items": [{ "label": "a", "value": 1 }] }));
        let table = block(json!({ "type": "table", "columns": ["a"], "rows": [[1]] }));
        let pie = block(json!({ "type": "pie", "slices": [{ "label": "a", "value": 1 }] }));
        let wide_pie = block(json!({ "type": "pie", "span": 3, "slices": [{ "label": "a", "value": 1 }] }));
        assert_eq!((span_of(&stats, 4), span_of(&table, 4), span_of(&pie, 4), span_of(&wide_pie, 4)), (4, 2, 1, 3));
        assert_eq!((span_of(&table, 1), span_of(&wide_pie, 2)), (1, 2));
    }

    #[test]
    fn percents_keep_one_decimal() {
        assert_eq!(percent(7.7734), "7.8%");
        assert_eq!(percent(20.0), "20%");
    }

    #[test]
    fn only_a_good_visualize_call_becomes_a_chart() {
        let input = json!({ "blocks": [{ "type": "text", "markdown": "hi" }] });
        assert!(report_of("winfer · visualize", &input, Some(true)).is_some());
        assert!(report_of("winfer · visualize", &input, None).is_some());
        assert!(report_of("winfer · visualize", &input, Some(false)).is_none());
        assert!(report_of("Bash", &input, Some(true)).is_none());
        // Codex wraps the arguments.
        assert!(report_of("winfer · visualize", &json!({ "arguments": input }), Some(true)).is_some());
        assert!(report_of("winfer · visualize", &json!({ "blocks": [] }), Some(true)).is_none());
    }
}
