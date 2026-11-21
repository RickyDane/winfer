//! Settings → Usage (port of `features/usage/UsagePage.tsx`, `UsageHistory.tsx`, `ToolStatsTab.tsx`,
//! `history.ts`, `toolStats.ts` and the Limits list in `UsageMeter.tsx`): token use and API cost
//! over time, per model and per task; the plan limits; the agents' tool calls.

use super::dialogs::relative_time;
use super::orchestration::{format_cost, format_tokens};
use super::review::small_button;
use super::usage::{UsageWindow, duration, left_now, runs_out_at};
use super::*;
use gpui_kit::component::tooltip::Tooltip;
use serde_json::Value;
use winfer_backend::{tool_stats, usage_history};

/// Agents in chart order.
pub const SERIES: [&str; 3] = ["claude", "codex", "opencode"];
const SERIES_COLORS: [&str; 3] = ["#cf6e4e", "#6d7cff", "#25a28f"];

fn series_name(p: &str) -> &str {
    match p {
        "claude" => "Claude Code",
        "codex" => "Codex",
        "opencode" => "OpenCode",
        other => other,
    }
}

fn series_color(p: &str, c: &Colors) -> Hsla {
    SERIES.iter().position(|s| *s == p).map_or(c.subtle, |i| theme::hex(SERIES_COLORS[i]))
}

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum Range {
    Day,
    #[default]
    Week,
    Month,
    Quarter,
}

impl Range {
    pub const ALL: [Range; 4] = [Range::Day, Range::Week, Range::Month, Range::Quarter];

    pub fn label(self) -> &'static str {
        match self {
            Self::Day => "24 hours",
            Self::Week => "7 days",
            Self::Month => "30 days",
            Self::Quarter => "90 days",
        }
    }

    pub fn days(self) -> i64 {
        match self {
            Self::Day => 1,
            Self::Week => 7,
            Self::Month => 30,
            Self::Quarter => 90,
        }
    }

    pub fn step_hours(self) -> i64 {
        match self {
            Self::Day => 1,
            Self::Week => 6,
            Self::Month | Self::Quarter => 24,
        }
    }

    fn id(self) -> &'static str {
        match self {
            Self::Day => "24h",
            Self::Week => "7d",
            Self::Month => "30d",
            Self::Quarter => "90d",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|r| r.id() == s)
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Bar {
    pub start: f64,
    pub end: f64,
    pub cost: HashMap<String, f64>,
    pub tokens: HashMap<String, f64>,
}

fn tokens_of(b: &Value) -> f64 {
    ["input", "output", "cacheRead", "cacheWrite"].iter().filter_map(|k| b[*k].as_f64()).sum()
}

/// Local calendar arithmetic, so days stay at midnight across daylight saving changes.
mod local {
    #[cfg(unix)]
    fn tm_of(ms: f64) -> libc::tm {
        let secs = (ms / 1000.0).floor() as libc::time_t;
        let mut tm: libc::tm = unsafe { std::mem::zeroed() };
        unsafe { libc::localtime_r(&secs, &mut tm) };
        tm
    }

    #[cfg(unix)]
    fn ms_of(mut tm: libc::tm) -> f64 {
        tm.tm_isdst = -1;
        (unsafe { libc::mktime(&mut tm) }) as f64 * 1000.0
    }

    /// Start of the local step (hour, 6 hours or day) `at` falls in.
    #[cfg(unix)]
    pub fn step_start(at: f64, step_hours: i64) -> f64 {
        let mut tm = tm_of(at);
        tm.tm_min = 0;
        tm.tm_sec = 0;
        if step_hours >= 24 {
            tm.tm_hour = 0;
        } else {
            tm.tm_hour -= tm.tm_hour % step_hours as i32;
        }
        ms_of(tm)
    }

    #[cfg(unix)]
    pub fn add_hours(at: f64, hours: i64) -> f64 {
        let mut tm = tm_of(at);
        if hours.abs() >= 24 {
            tm.tm_mday += (hours / 24) as i32;
        } else {
            tm.tm_hour += hours as i32;
        }
        ms_of(tm)
    }

    #[cfg(not(unix))]
    pub fn step_start(at: f64, step_hours: i64) -> f64 {
        let step = step_hours as f64 * 3_600_000.0;
        (at / step).floor() * step
    }

    #[cfg(not(unix))]
    pub fn add_hours(at: f64, hours: i64) -> f64 {
        at + hours as f64 * 3_600_000.0
    }
}

/// The range's bars, oldest first, the last one holding `now`; empty steps included.
pub fn to_bars(rows: &[Value], range: Range, now: f64) -> Vec<Bar> {
    let step = range.step_hours();
    let count = (range.days() * 24 / step) as usize;
    let mut start = local::step_start(now, step);
    for _ in 0..count - 1 {
        start = local::add_hours(start, -step);
    }
    let mut bars = Vec::with_capacity(count);
    for _ in 0..count {
        let end = local::add_hours(start, step);
        bars.push(Bar { start, end, ..Default::default() });
        start = end;
    }
    let first = bars[0].start;
    for r in rows {
        let hour = r["hour"].as_f64().unwrap_or(0.0);
        if hour < first {
            continue;
        }
        let i = bars.iter().rposition(|b| b.start <= hour).unwrap_or(0);
        let provider = r["provider"].as_str().unwrap_or_default().to_string();
        *bars[i].cost.entry(provider.clone()).or_default() += r["cost"].as_f64().unwrap_or(0.0);
        *bars[i].tokens.entry(provider).or_default() += tokens_of(r);
    }
    bars
}

#[derive(Clone, Debug, PartialEq)]
pub struct ModelTotal {
    pub provider: String,
    pub model: String,
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub cache_write: f64,
    pub cost: f64,
    pub priced: bool,
}

impl ModelTotal {
    fn tokens(&self) -> f64 {
        self.input + self.output + self.cache_read + self.cache_write
    }
}

/// Totals per model since `since`, most expensive (then most tokens) first.
pub fn model_totals(rows: &[Value], since: f64) -> Vec<ModelTotal> {
    let mut by: Vec<ModelTotal> = Vec::new();
    for r in rows.iter().filter(|r| r["hour"].as_f64().unwrap_or(0.0) >= since) {
        let (provider, model) = (r["provider"].as_str().unwrap_or_default(), r["model"].as_str().unwrap_or_default());
        let i = match by.iter().position(|t| t.provider == provider && t.model == model) {
            Some(i) => i,
            None => {
                by.push(ModelTotal { provider: provider.into(), model: model.into(), input: 0.0, output: 0.0, cache_read: 0.0, cache_write: 0.0, cost: 0.0, priced: true });
                by.len() - 1
            }
        };
        let t = &mut by[i];
        t.input += r["input"].as_f64().unwrap_or(0.0);
        t.output += r["output"].as_f64().unwrap_or(0.0);
        t.cache_read += r["cacheRead"].as_f64().unwrap_or(0.0);
        t.cache_write += r["cacheWrite"].as_f64().unwrap_or(0.0);
        t.cost += r["cost"].as_f64().unwrap_or(0.0);
        t.priced &= r["priced"] != false;
    }
    by.sort_by(|a, b| b.cost.total_cmp(&a.cost).then(b.tokens().total_cmp(&a.tokens())));
    by
}

/// A round axis maximum at or above `n`.
pub fn nice_max(n: f64) -> f64 {
    if n <= 0.0 {
        return 1.0;
    }
    let pow = 10f64.powf(n.log10().floor());
    [1.0, 2.0, 2.5, 5.0, 10.0].iter().map(|s| s * pow).find(|v| *v >= n).unwrap_or(10.0 * pow)
}

/// A tool or command's time: "320ms", "4.2s", "3m 10s", "1h 5m".
pub fn elapsed(ms: f64) -> String {
    if ms < 1000.0 {
        return format!("{}ms", ms.round());
    }
    let s = ms / 1000.0;
    if s < 60.0 {
        return if s < 10.0 { format!("{s:.1}s") } else { format!("{}s", s.floor()) };
    }
    let whole = s.floor() as u64;
    let m = whole / 60;
    if m < 60 {
        return format!("{m}m {:02}s", whole % 60);
    }
    format!("{}h {}m", m / 60, m % 60)
}

fn fail_rate(s: &Value) -> f64 {
    let calls = s["calls"].as_f64().unwrap_or(0.0);
    if calls > 0.0 { s["failed"].as_f64().unwrap_or(0.0) * 100.0 / calls } else { 0.0 }
}

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum ToolSort {
    #[default]
    Calls,
    Failed,
    Time,
    Tokens,
}

impl ToolSort {
    const ALL: [(ToolSort, &'static str, &'static str); 4] = [(Self::Calls, "calls", "Calls"), (Self::Failed, "failed", "Failed"), (Self::Time, "time", "Time"), (Self::Tokens, "tokens", "Tokens")];

    fn key(self, s: &Value) -> f64 {
        let k = match self {
            Self::Calls => "calls",
            Self::Failed => "failed",
            Self::Time => "durationMs",
            Self::Tokens => "tokens",
        };
        s[k].as_f64().unwrap_or(0.0)
    }
}

pub fn sort_stats(stats: &[Value], by: ToolSort) -> Vec<Value> {
    let mut out = stats.to_vec();
    out.sort_by(|a, b| {
        by.key(b).total_cmp(&by.key(a))
            .then(b["calls"].as_f64().unwrap_or(0.0).total_cmp(&a["calls"].as_f64().unwrap_or(0.0)))
            .then(a["name"].as_str().unwrap_or_default().cmp(b["name"].as_str().unwrap_or_default()))
    });
    out
}

#[derive(Default)]
pub(super) struct UsagePage {
    rows: Option<Vec<Value>>,
    by_dir: Option<(Range, Vec<Value>)>,
    loading: bool,
    error: Option<String>,
    hover: Option<usize>,
    all_tasks: bool,
    tools: Option<(Range, Value)>,
    tools_loading: bool,
    tools_error: Option<String>,
    all_tools: HashMap<String, bool>,
    open_failure: Option<usize>,
}

impl Root {
    fn saved(&self, key: &str, cx: &App) -> Option<String> {
        self.state.read(cx).rest.get("usagePage").and_then(|v| v[key].as_str()).map(str::to_string)
    }

    fn save(&self, key: &'static str, value: &str, cx: &mut Context<Self>) {
        self.set_rest("usagePage", key, Value::String(value.to_string()), cx);
    }

    fn usage_range(&self, key: &str, fallback: Range, cx: &App) -> Range {
        self.saved(key, cx).and_then(|s| Range::parse(&s)).unwrap_or(fallback)
    }

    fn load_usage_history(&mut self, cx: &mut Context<Self>) {
        self.usage_page.loading = true;
        self.usage_page.error = None;
        cx.spawn(async move |this, cx| {
            let found = tauri::async_runtime::spawn(usage_history::usage_history(backend::app().clone(), 90)).await.map_err(|e| e.to_string()).and_then(|r| r);
            let _ = this.update(cx, |this, cx| {
                this.usage_page.loading = false;
                match found {
                    Ok(rows) => this.usage_page.rows = serde_json::to_value(rows).ok().and_then(|v| v.as_array().cloned()),
                    Err(e) => this.usage_page.error = Some(e),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn load_by_dir(&mut self, range: Range, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let found = tauri::async_runtime::spawn(usage_history::usage_by_dir(backend::app().clone(), range.days())).await;
            let rows = found.ok().and_then(Result::ok).and_then(|v| serde_json::to_value(v).ok()).and_then(|v| v.as_array().cloned()).unwrap_or_default();
            let _ = this.update(cx, |this, cx| {
                this.usage_page.by_dir = Some((range, rows));
                cx.notify();
            });
        })
        .detach();
    }

    fn load_tools(&mut self, range: Range, cx: &mut Context<Self>) {
        self.usage_page.tools_loading = true;
        self.usage_page.tools_error = None;
        cx.spawn(async move |this, cx| {
            let found = tauri::async_runtime::spawn(tool_stats::usage_tool_stats(range.days())).await.map_err(|e| e.to_string()).and_then(|r| r);
            let _ = this.update(cx, |this, cx| {
                this.usage_page.tools_loading = false;
                match found.and_then(|r| serde_json::to_value(r).map_err(|e| e.to_string())) {
                    Ok(report) => this.usage_page.tools = Some((range, report)),
                    Err(e) => this.usage_page.tools_error = Some(e),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn segmented<T: Copy + PartialEq + 'static>(&self, id: &'static str, value: T, options: Vec<(T, String)>, c: Colors, cx: &mut Context<Self>, pick: fn(&mut Self, T, &mut Context<Self>)) -> Div {
        let mut row = div().flex().rounded_md().border_1().border_color(c.edge).bg(c.app).p_0p5();
        for (i, (v, label)) in options.into_iter().enumerate() {
            let on = v == value;
            row = row.child(div().id((id, i)).rounded_sm().px_2().py_0p5().text_size(px(11.0)).cursor_pointer()
                .when(on, |d| d.bg(c.raised).text_color(c.fg))
                .when(!on, |d| d.text_color(c.muted).hover(move |d| d.text_color(c.fg)))
                .on_click(cx.listener(move |this, _, _, cx| pick(this, v, cx)))
                .child(label));
        }
        row
    }

    fn stat(c: Colors, mono: SharedString, label: &str, value: String, detail: Option<String>) -> Div {
        let shadow = if c.light {
            vec![
                BoxShadow { color: hsla(0.0, 0.0, 0.0, 0.06), offset: point(px(0.0), px(2.0)), blur_radius: px(8.0), spread_radius: px(-2.0), inset: false },
                BoxShadow { color: hsla(0.0, 0.0, 0.0, 0.03), offset: point(px(0.0), px(1.0)), blur_radius: px(2.0), spread_radius: px(0.0), inset: false },
            ]
        } else {
            vec![
                BoxShadow { color: hsla(0.0, 0.0, 0.0, 0.32), offset: point(px(0.0), px(3.0)), blur_radius: px(10.0), spread_radius: px(-2.0), inset: false },
            ]
        };
        div().min_w_0().flex_1().rounded_lg().border_1().border_color(c.edge).bg(c.raised).shadow(shadow).px_3().py_2p5()
            .child(div().text_size(px(11.0)).text_color(c.subtle).child(label.to_string()))
            .child(div().mt_0p5().font_family(mono).text_lg().text_color(c.fg).child(value))
            .when_some(detail, |d, t| d.child(div().truncate().text_size(px(11.0)).text_color(c.subtle).child(t)))
    }

    /// Settings → Usage: the tabs.
    pub(super) fn page_usage(&mut self, c: Colors, cx: &mut Context<Self>) -> Div {
        let tab = self.saved("tab", cx).unwrap_or_else(|| "usage".into());
        let tabs = [("usage", "Usage", IconName::ChartColumn), ("limits", "Limits", IconName::Gauge), ("tools", "Tools", IconName::Wrench)];
        let mut bar = div().flex().w_auto().items_center().gap_0p5().rounded_lg().border_1().border_color(c.edge).bg(c.app).p_0p5();
        for (id, label, icon) in tabs {
            let on = tab == id;
            bar = bar.child(div().id(id).flex().items_center().gap_1p5().rounded_md().px_2p5().py_1().text_xs().cursor_pointer()
                .when(on, |d| d.bg(c.raised).text_color(c.fg))
                .when(!on, |d| d.text_color(c.muted).hover(move |d| d.bg(c.hover).text_color(c.fg)))
                .on_click(cx.listener(move |this, _, _, cx| this.save("tab", id, cx)))
                .child(Icon::new(icon).size(px(12.0)))
                .child(label));
        }
        let body = match tab.as_str() {
            "tools" => self.tools_tab(c, cx),
            "limits" => self.limits_tab(c, cx),
            _ => self.history_tab(c, cx),
        };
        div().flex().flex_col().gap_6().child(div().flex().child(bar)).child(body)
    }

    fn history_tab(&mut self, c: Colors, cx: &mut Context<Self>) -> Div {
        if self.usage_page.rows.is_none() && !self.usage_page.loading && self.usage_page.error.is_none() {
            self.load_usage_history(cx);
        }
        let range = self.usage_range("range", Range::Week, cx);
        let metric = self.saved("metric", cx).unwrap_or_else(|| "cost".into());
        let kind = self.saved("chart", cx).unwrap_or_else(|| "bars".into());
        let mono = crate::fonts::mono(cx);
        let ranges = self.segmented("usage-range", range, Range::ALL.iter().map(|r| (*r, r.label().to_string())).collect(), c, cx, |this, r, cx| {
            this.usage_page.hover = None;
            this.save("range", r.id(), cx);
        });
        let loading = self.usage_page.loading;
        let mut page = div().flex().flex_col().gap_4()
            .child(div().text_xs().text_color(c.muted).child("Tokens your agents used, read from Claude Code, Codex and OpenCode's logs, and what they would cost at standard API prices. On a subscription plan you don't pay this; it shows what the plan saves. History is kept for 90 days."))
            .child(div().flex().items_center().gap_2().child(ranges).child(div().flex_1())
                .child(small_button(c, "usage-reload", if loading { "Reading…" } else { "Refresh" }).on_click(cx.listener(|this, _, _, cx| {
                    this.usage_page.by_dir = None;
                    this.load_usage_history(cx);
                }))));
        if let Some(e) = &self.usage_page.error {
            page = page.child(div().text_xs().text_color(c.danger).child(e.clone()));
        }
        let Some(rows) = self.usage_page.rows.clone() else {
            return page.child(div().text_xs().text_color(c.subtle).child("Reading usage from the agents' logs…"));
        };
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_millis() as f64);
        let bars = to_bars(&rows, range, now);
        let models = model_totals(&rows, bars[0].start);
        let sum = |f: fn(&ModelTotal) -> f64| models.iter().map(f).sum::<f64>();
        let unpriced = models.iter().filter(|m| !m.priced).count();
        page = page.child(div().flex().gap_2()
            .child(Self::stat(c, mono.clone(), "Estimated API cost", format_cost(sum(|m| m.cost)), (unpriced > 0).then(|| format!("{unpriced} model{} without a known price", if unpriced > 1 { "s" } else { "" }))))
            .child(Self::stat(c, mono.clone(), "Tokens", format_tokens(sum(ModelTotal::tokens)), Some(format!("{} in · {} out · {} cached", format_tokens(sum(|m| m.input)), format_tokens(sum(|m| m.output)), format_tokens(sum(|m| m.cache_read)))))));
        let providers: Vec<&str> = SERIES.iter().copied().filter(|p| models.iter().any(|m| m.provider == *p)).collect();
        let metric_toggle = self.segmented("usage-metric", metric == "cost", vec![(true, "Cost".into()), (false, "Tokens".into())], c, cx, |this, cost, cx| this.save("metric", if cost { "cost" } else { "tokens" }, cx));
        let kind_toggle = self.segmented("usage-kind", kind == "bars", vec![(true, "Bars".into()), (false, "Line".into())], c, cx, |this, bars, cx| this.save("chart", if bars { "bars" } else { "line" }, cx));
        let mut legend = div().flex().flex_1().justify_end().gap_3();
        for p in &providers {
            legend = legend.child(div().flex().items_center().gap_1p5().text_size(px(11.0)).text_color(c.muted).child(div().size_2().rounded_sm().bg(series_color(p, &c))).child(series_name(p).to_string()));
        }
        page = page.child(div().flex().flex_col().gap_2()
            .child(div().flex().items_center().gap_3().child(metric_toggle).child(kind_toggle).child(legend))
            .child(self.usage_chart(&bars, metric == "cost", kind == "bars", range, c, cx)));
        if models.is_empty() {
            page = page.child(div().text_xs().text_color(c.subtle).child("No agent usage in this range."));
        } else {
            let head = |t: &'static str, right: bool| div().flex_1().when(right, |d| d.text_right()).pb_1p5().text_size(px(11.0)).text_color(c.subtle).child(t);
            let mut table = div().flex().flex_col().text_xs()
                .child(div().flex().child(head("Model", false).flex_grow(2.0)).child(head("Input", true)).child(head("Output", true)).child(head("Cache read", true)).child(head("Cache write", true)).child(head("Cost", true)));
            for m in &models {
                let cell = |v: String| div().flex_1().text_right().py_1p5().font_family(mono.clone()).text_color(c.muted).child(v);
                let tip = format!("{} · {}", series_name(&m.provider), m.model);
                table = table.child(div().flex()
                    .child(div().id(SharedString::from(format!("model-{}-{}", m.provider, m.model))).flex_1().flex_grow(2.0).min_w_0().flex().items_center().gap_1p5().py_1p5()
                        .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
                        .child(div().size_2().flex_shrink_0().rounded_sm().bg(series_color(&m.provider, &c)))
                        .child(div().truncate().text_color(c.fg).child(m.model.clone())))
                    .child(cell(format_tokens(m.input))).child(cell(format_tokens(m.output))).child(cell(format_tokens(m.cache_read))).child(cell(format_tokens(m.cache_write)))
                    .child(div().flex_1().text_right().py_1p5().font_family(mono.clone()).text_color(if m.priced { c.fg } else { c.subtle }).child(if m.priced { format_cost(m.cost) } else { "—".into() })));
            }
            page = page.child(table);
        }
        let on = self.state.read(cx).show_pane_cost;
        let toggle = self.toggle("pane-cost", on, c, cx, |this, cx| this.update_state(cx, |s| s.show_pane_cost = !s.show_pane_cost));
        page = page.child(settings::card(c, "Panes", None).child(settings::row_marked(c, "Show task cost on panes", self.reset_mark("usage.cost", c, cx), Some("Each agent shows what its task has cost so far, at API prices.".to_string()), toggle)));
        page.children(self.by_task(range, c, cx))
    }

    fn usage_chart(&self, bars: &[Bar], cost: bool, stacked: bool, range: Range, c: Colors, cx: &mut Context<Self>) -> Div {
        let value = |b: &Bar, p: &str| if cost { b.cost.get(p).copied().unwrap_or(0.0) } else { b.tokens.get(p).copied().unwrap_or(0.0) };
        let totals: Vec<f64> = bars.iter().map(|b| SERIES.iter().map(|p| value(b, p)).sum()).collect();
        // Bars stack the agents; lines each stand alone, so the axis fits the biggest single value.
        let peak = if stacked { totals.iter().copied().fold(0.0, f64::max) } else { bars.iter().flat_map(|b| SERIES.iter().map(move |p| value(b, p))).fold(0.0, f64::max) };
        let max = nice_max(peak);
        let fmt = move |n: f64| if cost { format_cost(n).trim_end_matches(".00").to_string() } else { format_tokens(n) };
        let mono = crate::fonts::mono(cx);
        let hover = self.usage_page.hover;
        let mut columns = div().absolute().inset_0().flex().items_end().gap(px(if bars.len() > 40 { 1.0 } else { 2.0 }));
        for (i, bar) in bars.iter().enumerate() {
            let mut column = div().id(("usage-col", i)).flex().h_full().min_w_0().flex_1().items_end()
                .on_hover(cx.listener(move |this, on: &bool, _, cx| {
                    if *on {
                        this.usage_page.hover = Some(i);
                    } else if this.usage_page.hover == Some(i) {
                        this.usage_page.hover = None;
                    }
                    cx.notify();
                }));
            if stacked && totals[i] > 0.0 {
                let mut stack = div().flex().flex_col_reverse().w_full().overflow_hidden().rounded_t(px(4.0)).h(relative((totals[i] / max) as f32))
                    .when(hover.is_some_and(|h| h != i), |d| d.opacity(0.6));
                for p in SERIES {
                    let v = value(bar, p);
                    if v > 0.0 {
                        stack = stack.child(div().w_full().min_h(px(1.0)).bg(series_color(p, &c)).h(relative((v / totals[i]) as f32)));
                    }
                }
                column = column.child(stack);
            }
            columns = columns.child(column);
        }
        let lines: Vec<(Hsla, Vec<f64>)> = if stacked { Vec::new() } else {
            SERIES.iter().filter(|p| bars.iter().any(|b| value(b, p) > 0.0)).map(|p| (series_color(p, &c), bars.iter().map(|b| value(b, p)).collect())).collect()
        };
        let line_canvas = canvas(|_, _, _| {}, move |bounds, _, window, _| {
            for (color, values) in &lines {
                let n = values.len() as f32;
                let mut path = PathBuilder::stroke(px(1.75));
                for (i, v) in values.iter().enumerate() {
                    let x = bounds.origin.x + bounds.size.width * ((i as f32 + 0.5) / n);
                    let y = bounds.origin.y + bounds.size.height * (1.0 - (*v / max) as f32);
                    if i == 0 { path.move_to(point(x, y)) } else { path.line_to(point(x, y)) }
                }
                if let Ok(path) = path.build() {
                    window.paint_path(path, *color);
                }
            }
        }).absolute().size_full();
        let ticks = [max, max / 2.0, 0.0];
        let mut grid = div().absolute().inset_0();
        for (i, _) in ticks.iter().enumerate() {
            grid = grid.child(div().absolute().left_0().right_0().top(relative(i as f32 / 2.0)));
        }
        let mut axis = div().h(px(160.0)).flex().flex_col().justify_between().text_right().font_family(mono.clone()).text_size(px(10.0)).text_color(c.subtle);
        for t in ticks {
            axis = axis.child(fmt(t));
        }
        let card = hover.and_then(|h| bars.get(h).map(|b| (h, b.clone()))).map(|(h, bar)| {
            let mut card = div().absolute().top_1().w(px(200.0)).rounded_md().border_1().border_color(c.edge).bg(c.panel).px_2p5().py_2().text_xs()
                .child(div().mb_1().text_color(c.muted).child(bar_label(&bar, range)));
            let left = (h as f32 + 1.0) / bars.len() as f32;
            card = if h < bars.len() / 2 { card.left(relative(left)).ml(px(6.0)) } else { card.right(relative(1.0 - h as f32 / bars.len() as f32)).mr(px(6.0)) };
            let mut any = false;
            for p in SERIES {
                if bar.tokens.get(p).copied().unwrap_or(0.0) > 0.0 {
                    any = true;
                    card = card.child(div().flex().items_center().gap_2()
                        .child(div().size_2().rounded_sm().bg(series_color(p, &c)))
                        .child(div().flex_1().text_color(c.fg).child(series_name(p).to_string()))
                        .child(div().font_family(mono.clone()).text_color(c.fg).child(format_cost(bar.cost.get(p).copied().unwrap_or(0.0))))
                        .child(div().w(px(48.0)).text_right().font_family(mono.clone()).text_color(c.subtle).child(format_tokens(bar.tokens[p]))));
                }
            }
            if any {
                let total_cost: f64 = bar.cost.values().sum();
                let total_tokens: f64 = bar.tokens.values().sum();
                card.child(div().mt_1().flex().items_center().gap_2().pt_1()
                    .child(div().flex_1().pl_4().text_color(c.muted).child("Total"))
                    .child(div().font_family(mono.clone()).text_color(c.fg).child(format_cost(total_cost)))
                    .child(div().w(px(48.0)).text_right().font_family(mono.clone()).text_color(c.subtle).child(format_tokens(total_tokens))))
            } else {
                card.child(div().text_color(c.subtle).child("No usage"))
            }
        });
        let labelled = [0, (bars.len() - 1) / 2, bars.len() - 1];
        let mut labels = div().relative().mt_1().h(px(14.0)).font_family(mono).text_size(px(10.0)).text_color(c.subtle);
        for i in labelled {
            let text = axis_label(bars[i].start, range);
            labels = labels.child(if i == 0 { div().absolute().left_0().child(text) } else if i == bars.len() - 1 { div().absolute().right_0().child(text) } else { div().absolute().left(relative((i as f32 + 0.5) / bars.len() as f32)).child(text) });
        }
        div().flex().gap_2().child(axis).child(div().relative().min_w_0().flex_1()
            .child(div().relative().h(px(160.0)).rounded_md().bg(c.app).border_1().border_color(c.edge).child(grid).when(!stacked, |d| d.child(line_canvas)).child(columns).children(card))
            .child(labels))
    }

    /// Spend per task: each worktree (or folder) the agents worked in, most expensive first.
    fn by_task(&mut self, range: Range, c: Colors, cx: &mut Context<Self>) -> Option<Div> {
        if self.usage_page.by_dir.as_ref().is_none_or(|(r, _)| *r != range) {
            self.usage_page.by_dir = Some((range, Vec::new()));
            self.load_by_dir(range, cx);
        }
        let rows = self.usage_page.by_dir.as_ref().map(|(_, r)| r.clone()).unwrap_or_default();
        if rows.is_empty() {
            return None;
        }
        let all = self.usage_page.all_tasks;
        let mono = crate::fonts::mono(cx);
        let s = self.state.read(cx);
        let panes: Vec<&Pane> = s.all_panes().collect();
        let mut table = div().flex().flex_col().text_xs().child(div().flex().text_size(px(11.0)).text_color(c.subtle).pb_1p5()
            .child(div().flex_1().flex_grow(3.0).child("Task")).child(div().w(px(64.0)).child("Agents")).child(div().w(px(80.0)).text_right().child("Last active")).child(div().w(px(64.0)).text_right().child("Tokens")).child(div().w(px(64.0)).text_right().child("Cost")));
        for (i, d) in rows.iter().take(if all { rows.len() } else { 8 }).enumerate() {
            let dir = d["dir"].as_str().unwrap_or_default().to_string();
            let norm = |p: &str| p.replace('\\', "/").trim_end_matches('/').to_string();
            let pane = panes.iter().find(|p| p.worktree.as_ref().is_some_and(|t| norm(&t.path) == norm(&dir))).or_else(|| panes.iter().find(|p| p.worktree.is_none() && norm(&p.cwd) == norm(&dir)));
            let workspace = s.workspaces.iter().find(|w| norm(&w.root_dir) == norm(&dir));
            let leaf = dir.rsplit('/').find(|p| !p.is_empty()).unwrap_or(&dir).to_string();
            let (title, detail) = match (pane, workspace) {
                (Some(p), _) => (s.pane_label(p), Some(p.worktree.as_ref().map_or(dir.clone(), |t| t.branch.clone()))),
                (None, Some(w)) => (w.name.clone(), Some("project folder".to_string())),
                _ => (leaf, Some(dir.clone())),
            };
            let mut providers: Vec<String> = Vec::new();
            for m in d["models"].as_array().into_iter().flatten() {
                if let Some(p) = m["provider"].as_str().filter(|p| !providers.iter().any(|x| x == p)) {
                    providers.push(p.to_string());
                }
            }
            let tip = format!("{dir}\n\n{}", pane_header::cost_breakdown(d, "in this worktree"));
            let cost = d["cost"].as_f64().unwrap_or(0.0);
            let priced = d["priced"] == true || cost > 0.0;
            table = table.child(div().id(("task", i)).flex().items_center().py_1p5()
                .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
                .child(div().flex_1().flex_grow(3.0).min_w_0().child(div().truncate().text_color(c.fg).child(title)).when_some(detail, |x, t| x.child(div().truncate().font_family(mono.clone()).text_size(px(10.0)).text_color(c.subtle).child(t))))
                .child(div().w(px(64.0)).flex().gap_1p5().children(providers.iter().map(|p| div().size_2().rounded_sm().bg(series_color(p, &c)))))
                .child(div().w(px(80.0)).text_right().text_color(c.subtle).child(d["lastAt"].as_f64().map(relative_time).unwrap_or_default()))
                .child(div().w(px(64.0)).text_right().font_family(mono.clone()).text_color(c.muted).child(format_tokens(d["tokens"].as_f64().unwrap_or(0.0))))
                .child(div().w(px(64.0)).text_right().font_family(mono.clone()).text_color(if priced { c.fg } else { c.subtle }).child(if priced { format_cost(cost) } else { "—".into() })));
        }
        let more = rows.len() > 8;
        Some(div().flex().flex_col().gap_1p5()
            .child(div().text_size(px(11.0)).text_color(c.subtle).child("By task — each worktree an agent worked in, so you can see what a feature cost across agents"))
            .child(table)
            .when(more, |d| d.child(div().id("tasks-more").text_size(px(11.0)).text_color(c.subtle).cursor_pointer().hover(move |d| d.text_color(c.fg))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.usage_page.all_tasks = !this.usage_page.all_tasks;
                    cx.notify();
                }))
                .child(if all { "Show fewer".to_string() } else { format!("Show all {}", rows.len()) }))))
    }

    fn limits_tab(&mut self, c: Colors, cx: &mut Context<Self>) -> Div {
        let kind = self.saved("limitsChart", cx).unwrap_or_else(|| "line".into());
        let line = kind == "line";
        let toggle = self.segmented("limits-kind", line, vec![(false, "Bars".into()), (true, "Line".into())], c, cx, |this, line, cx| this.save("limitsChart", if line { "line" } else { "bars" }, cx));
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_millis() as f64);
        let mono = crate::fonts::mono(cx);
        let mut page = div().flex().flex_col().gap_4()
            .child(div().flex().items_start().gap_3()
                .child(div().flex_1().min_w_0().text_xs().text_color(c.muted).child("How much of each plan's usage limits is left, as the title bar's rings show it. The lines follow each window since it started; the projection says when it runs out at the current pace."))
                .child(toggle))
            .child(div().flex().justify_end().child(small_button(c, "limits-refresh", "Refresh").on_click(cx.listener(|this, _, _, cx| this.refresh_all_usage(cx)))));
        let mut keys: Vec<String> = self.usage.snapshots.keys().cloned().collect();
        keys.sort_by_key(|k| (k.split(':').next().unwrap_or_default().to_string(), k.contains(':'), k.clone()));
        if keys.iter().all(|k| self.usage.snapshots[k].windows.is_empty()) {
            return page.child(div().text_size(px(11.0)).text_color(c.subtle).child("No usage reported yet. Refresh to check your signed-in accounts."));
        }
        for key in keys {
            let snap = self.usage.snapshots[&key].clone();
            if snap.windows.is_empty() {
                continue;
            }
            let provider = key.split(':').next().unwrap_or(&key).to_string();
            let account = key.contains(':').then(|| self.usage.accounts.iter().find(|a| a.id == key).map_or(key.clone(), |a| a.label.clone()));
            let name = self.state.read(cx).provider_name(&provider);
            let age = now - snap.at;
            let mut section = div().flex().flex_col().gap_2().child(div().flex().items_center().gap_2().text_size(px(13.0))
                .child(div().flex_1().text_color(c.fg).child(match &account { Some(a) => format!("{name} · {a}"), None => name.clone() }))
                .child(div().text_size(px(10.0)).text_color(if age > 600_000.0 { c.warning } else { c.subtle }).child(format!("{}{}", if age > 600_000.0 { "Stale · " } else { "" }, if age < 120_000.0 { "just now".to_string() } else { format!("{} ago", duration(age)) }))));
            for (group_title, items) in super::usage::group_windows(&snap.windows) {
                let mut card = div().flex().flex_col().rounded_lg().border_1().border_color(c.edge).bg(c.panel).p_3();
                if let Some(title) = group_title {
                    card = card.child(div().pb_2().text_size(px(12.0)).font_weight(FontWeight::MEDIUM).text_color(c.fg).child(title.to_string()));
                }
                for (idx, (label, w)) in items.into_iter().enumerate() {
                    if idx > 0 {
                        card = card.child(div().my_2p5().h(px(1.0)).w_full().bg(c.border));
                    }
                    let out = runs_out_at(&key, w, &self.usage.samples, now);
                    card = card.child(if line {
                        self.window_line(&key, w, label, out, now, c, mono.clone())
                    } else {
                        window_bar(w, label, out, now, c, mono.clone())
                    });
                }
                section = section.child(card);
            }
            page = page.child(section);
        }
        page
    }

    fn window_line(&self, key: &str, w: &UsageWindow, label: &str, out: Option<f64>, now: f64, c: Colors, mono: SharedString) -> Div {
        let left = left_now(w, now);
        let live = w.resets_at.is_some_and(|r| r > now);
        let span = match w.kind.as_str() {
            "month-credit" => Some(30.0 * 86_400_000.0),
            k if k == "session" || k.starts_with("session-") => Some(5.0 * 3_600_000.0),
            k if k == "week" || k.starts_with("week-") => Some(7.0 * 86_400_000.0),
            _ => None,
        };
        let prefix = format!("{key}:{}:", w.kind);
        let samples = self.usage.samples.iter().find(|(k, _)| k.starts_with(&prefix)).map(|(_, v)| v.clone()).unwrap_or_default();
        let mut points: Vec<(f64, f64)> = if live { samples.iter().filter(|(t, _)| *t <= now).map(|(t, used)| (*t, 100.0 - used)).collect() } else { Vec::new() };
        let window_start = if live { span.map(|s| w.resets_at.unwrap() - s) } else { None };
        let start = window_start.unwrap_or(f64::INFINITY).min(points.first().map_or(f64::INFINITY, |p| p.0)).min(now - 3_600_000.0);
        // A window starts full.
        if let Some(ws) = window_start
            && points.first().map_or(now, |p| p.0) > ws
        {
            points.insert(0, (ws, 100.0));
        }
        if points.is_empty() {
            points.push((start, left));
        }
        points.push((now, left));
        let end = if live { w.resets_at.unwrap() } else { now };
        let color = super::usage::level_color(left, &c);
        let pct_color = if left < 40.0 { color } else { c.fg };
        let path_points = points.clone();
        let chart = canvas(|_, _, _| {}, move |bounds, _, window, _| {
            let x = |t: f64| bounds.origin.x + bounds.size.width * (((t - start) / (end - start)) as f32);
            let y = |v: f64| bounds.origin.y + bounds.size.height * (1.0 - (v / 100.0) as f32);
            let mut path = PathBuilder::stroke(px(1.75));
            for (i, (t, v)) in path_points.iter().enumerate() {
                if i == 0 { path.move_to(point(x(*t), y(*v))) } else { path.line_to(point(x(*t), y(*v))) }
            }
            if let Ok(path) = path.build() {
                window.paint_path(path, color);
            }
        }).absolute().size_full();
        let mut grid = div().absolute().inset_0();
        for i in 0..3 {
            grid = grid.child(div().absolute().left_0().right_0().top(relative(i as f32 / 2.0)));
        }
        div().flex().flex_col().gap_1()
            .child(div().flex().items_baseline().gap_2().text_xs().child(div().flex_1().text_color(c.muted).child(label.to_string())).child(div().font_family(mono).text_size(px(11.0)).text_color(pct_color).child(format!("{}% left", left.round()))))
            .child(div().relative().h(px(64.0)).rounded_md().bg(c.app).border_1().border_color(c.edge).child(grid).child(chart))
            .child(reset_line(w, out, now, c))
    }
}

fn reset_line(w: &UsageWindow, out: Option<f64>, now: f64, c: Colors) -> Div {
    let reset = w.resets_at.map(|r| if r > now { format!("resets in {}", duration(r - now)) } else { "reset — full again".into() }).unwrap_or_default();
    div().text_size(px(11.0)).text_color(c.subtle).child(reset).when_some(out, |d, o| d.child(div().text_color(c.warning).child(format!(" · at this pace runs out in {}", duration(o - now)))))
}

fn window_bar(w: &UsageWindow, label: &str, out: Option<f64>, now: f64, c: Colors, mono: SharedString) -> Div {
    let left = left_now(w, now);
    let color = super::usage::level_color(left, &c);
    let pct_color = if left < 40.0 { color } else { c.fg };
    div().flex().flex_col().gap_1()
        .child(div().flex().items_baseline().gap_2().text_xs()
            .child(div().flex_1().min_w_0().truncate().text_color(c.muted).child(label.to_string()))
            .child(div().flex_shrink_0().font_family(mono).text_size(px(11.0)).text_color(pct_color).child(format!("{}% left", left.round()))))
        .child(div().h(px(4.0)).w_full().overflow_hidden().rounded_full().bg(c.hover).child(div().h_full().rounded_full().bg(color).w(relative((left / 100.0) as f32))))
        .child(reset_line(w, out, now, c))
}

fn bar_label(bar: &Bar, range: Range) -> String {
    let (start, end) = (super::usage::local_time(bar.start), super::usage::local_time(bar.end));
    match range {
        Range::Day | Range::Week => format!("{}–{}", start, &end[11..]),
        _ => start[..10].to_string(),
    }
}

fn axis_label(t: f64, range: Range) -> String {
    let local = super::usage::local_time(t);
    if range == Range::Day { local[11..].to_string() } else { local[5..10].to_string() }
}

impl Root {
    fn tools_tab(&mut self, c: Colors, cx: &mut Context<Self>) -> Div {
        let range = self.usage_range("toolsRange", Range::Day, cx);
        let sort = self.saved("toolsSort", cx).and_then(|s| ToolSort::ALL.iter().find(|x| x.1 == s).map(|x| x.0)).unwrap_or_default();
        if self.usage_page.tools.as_ref().is_none_or(|(r, _)| *r != range) && !self.usage_page.tools_loading && self.usage_page.tools_error.is_none() {
            self.load_tools(range, cx);
        }
        let mono = crate::fonts::mono(cx);
        let ranges = self.segmented("tools-range", range, Range::ALL.iter().map(|r| (*r, r.label().to_string())).collect(), c, cx, |this, r, cx| {
            this.usage_page.tools_error = None;
            this.save("toolsRange", r.id(), cx);
        });
        let sorts = self.segmented("tools-sort", sort, ToolSort::ALL.iter().map(|(s, _, l)| (*s, l.to_string())).collect(), c, cx, |this, s, cx| {
            let id = ToolSort::ALL.iter().find(|x| x.0 == s).map_or("calls", |x| x.1);
            this.save("toolsSort", id, cx);
        });
        let loading = self.usage_page.tools_loading;
        let mut page = div().flex().flex_col().gap_4()
            .child(div().text_xs().text_color(c.muted).child("Tool calls and shell commands your agents ran, read from Claude Code and Codex's logs: how often, how many failed, how long they took and roughly how many tokens their input and output added to the context."))
            .child(div().flex().items_center().gap_2().child(ranges).child(div().flex_1()).child(sorts)
                .child(small_button(c, "tools-reload", if loading { "Reading…" } else { "Refresh" }).on_click(cx.listener(move |this, _, _, cx| this.load_tools(range, cx)))));
        if let Some(e) = &self.usage_page.tools_error {
            page = page.child(div().text_xs().text_color(c.danger).child(e.clone()));
        }
        let Some((shown_range, rep)) = self.usage_page.tools.clone() else {
            return page.child(div().text_xs().text_color(c.subtle).child("Reading tool calls from the agents' logs…"));
        };
        let num = |k: &str| rep[k].as_f64().unwrap_or(0.0);
        let rate = fail_rate(&rep);
        let calls = num("calls");
        let mut body = div().flex().flex_col().gap_4().when(shown_range != range, |d| d.opacity(0.6))
            .child(div().flex().gap_2()
                .child(Self::stat(c, mono.clone(), "Tool calls", format!("{calls}"), Some(format!("{} sessions", num("sessions")))))
                .child(Self::stat(c, mono.clone(), "Failed", format!("{}", num("failed")), (calls > 0.0).then(|| format!("{}% of calls", if rate < 1.0 && num("failed") > 0.0 { "<1".to_string() } else { format!("{}", rate.round()) }))))
                .child(Self::stat(c, mono.clone(), "Time in tools", elapsed(num("durationMs")), (calls > 0.0).then(|| format!("{} per call", elapsed(num("durationMs") / calls)))))
                .child(Self::stat(c, mono.clone(), "Tokens (est.)", format_tokens(num("tokens")), Some("input and results".into()))));
        if calls == 0.0 {
            body = body.child(div().text_xs().text_color(c.subtle).child("No tool calls in this range."));
        } else {
            for (title, key, prefix) in [("Tools", "tools", ""), ("Shell commands", "commands", "$ ")] {
                let stats = rep[key].as_array().cloned().unwrap_or_default();
                if let Some(t) = self.stat_table(title, key, &stats, sort, prefix, c, mono.clone(), cx) {
                    body = body.child(t);
                }
            }
            body = body.children(self.failures(&rep, c, mono, cx));
        }
        page.child(body)
    }

    #[allow(clippy::too_many_arguments)]
    fn stat_table(&self, title: &str, key: &'static str, stats: &[Value], sort: ToolSort, prefix: &str, c: Colors, mono: SharedString, cx: &mut Context<Self>) -> Option<Div> {
        let rows = sort_stats(stats, sort);
        if rows.is_empty() {
            return None;
        }
        let all = self.usage_page.all_tools.get(key).copied().unwrap_or(false);
        let max = rows.iter().map(|s| sort.key(s)).fold(1.0, f64::max);
        let head = |t: &'static str, w: f32| div().w(px(w)).text_right().child(t);
        let mut table = div().flex().flex_col().overflow_hidden().rounded_lg().border_1().border_color(c.edge).text_size(px(11.0))
            .child(div().flex().bg(c.panel).px_2p5().py_1p5().text_color(c.subtle).child(div().flex_1().child("Name")).child(head("Calls", 56.0)).child(head("Failed", 72.0)).child(head("Time · avg", 96.0)).child(head("Max", 56.0)).child(head("Tokens", 64.0)));
        for s in rows.iter().take(if all { rows.len() } else { 12 }) {
            let value = sort.key(s);
            let failed = s["failed"].as_f64().unwrap_or(0.0);
            let rate = fail_rate(s);
            let timed = s["timed"].as_f64().unwrap_or(0.0);
            let duration_ms = s["durationMs"].as_f64().unwrap_or(0.0);
            let cell = |w: f32| div().w(px(w)).text_right().font_family(mono.clone());
            table = table.child(div().flex().items_center().px_2p5().py_1p5()
                .child(div().relative().flex_1().min_w_0()
                    .child(div().absolute().top_0().bottom_0().left_0().rounded_r_sm().bg(c.hover).w(relative((value / max) as f32)))
                    .child(div().relative().truncate().font_family(mono.clone()).text_color(c.fg).child(format!("{prefix}{}", s["name"].as_str().unwrap_or_default()))))
                .child(cell(56.0).text_color(c.fg).child(format!("{}", s["calls"].as_u64().unwrap_or(0))))
                .child(cell(72.0).text_color(if failed > 0.0 { if rate >= 10.0 { c.danger } else { c.fg } } else { c.subtle }).child(if failed > 0.0 { format!("{failed} · {}%", if rate < 1.0 { "<1".to_string() } else { format!("{}", rate.round()) }) } else { "–".into() }))
                .child(cell(96.0).text_color(c.muted).child(if timed > 0.0 { format!("{} · {}", elapsed(duration_ms), elapsed(duration_ms / timed)) } else { "–".into() }))
                .child(cell(56.0).text_color(c.subtle).child(if timed > 0.0 { elapsed(s["maxMs"].as_f64().unwrap_or(0.0)) } else { "–".into() }))
                .child(cell(64.0).text_color(c.muted).child(format_tokens(s["tokens"].as_f64().unwrap_or(0.0)))));
        }
        let more = rows.len() > 12;
        Some(div().flex().flex_col().gap_1p5()
            .child(div().text_size(px(11.0)).text_color(c.muted).child(title.to_string()))
            .child(table)
            .when(more, |d| d.child(div().id(SharedString::from(format!("more-{key}"))).text_size(px(11.0)).text_color(c.subtle).cursor_pointer().hover(move |d| d.text_color(c.fg))
                .on_click(cx.listener(move |this, _, _, cx| {
                    let e = this.usage_page.all_tools.entry(key.to_string()).or_default();
                    *e = !*e;
                    cx.notify();
                }))
                .child(if all { "Show fewer".to_string() } else { format!("Show all {}", rows.len()) }))))
    }

    fn failures(&self, rep: &Value, c: Colors, mono: SharedString, cx: &mut Context<Self>) -> Option<Div> {
        let list = rep["failures"].as_array().filter(|l| !l.is_empty())?;
        let failed = rep["failed"].as_u64().unwrap_or(0);
        let title = format!("Failed calls{}", if failed as usize > list.len() { format!(" (latest {} of {failed})", list.len()) } else { String::new() });
        let mut rows = div().flex().flex_col().overflow_hidden().rounded_lg().border_1().border_color(c.edge);
        for (i, f) in list.iter().enumerate() {
            let open = self.usage_page.open_failure == Some(i);
            let error = f["error"].as_str().unwrap_or_default().to_string();
            let first_line = error.lines().find(|l| !l.trim().is_empty()).unwrap_or_default().to_string();
            rows = rows.child(div().id(("failure", i)).flex().flex_col().px_2p5().py_1p5().text_size(px(11.0)).cursor_pointer().hover(move |d| d.bg(c.hover)).when(i > 0, |d| d)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.usage_page.open_failure = if this.usage_page.open_failure == Some(i) { None } else { Some(i) };
                    cx.notify();
                }))
                .child(div().flex().items_baseline().gap_2()
                    .child(div().flex_shrink_0().font_family(mono.clone()).text_color(c.danger).child(f["tool"].as_str().unwrap_or_default().to_string()))
                    .child(div().min_w_0().flex_1().truncate().font_family(mono.clone()).text_color(c.muted).child(f["input"].as_str().unwrap_or_default().to_string()))
                    .when_some(f["durationMs"].as_f64(), |d, ms| d.child(div().flex_shrink_0().font_family(mono.clone()).text_color(c.subtle).child(elapsed(ms))))
                    .child(div().flex_shrink_0().text_color(c.subtle).child(relative_time(f["at"].as_f64().unwrap_or(0.0)))))
                .when(open, |d| d.child(div().mt_1().flex().flex_col().gap_1()
                    .child(div().rounded_sm().bg(c.panel).p_2().font_family(mono.clone()).text_size(px(10.0)).text_color(c.muted).child(if error.is_empty() { "No output.".to_string() } else { error.clone() }))
                    .child(div().text_size(px(10.0)).text_color(c.subtle).child(format!("{}{}", if f["provider"] == "codex" { "Codex" } else { "Claude Code" }, f["cwd"].as_str().map_or(String::new(), |d| format!(" · {d}")))))))
                .when(!open && !first_line.is_empty(), |d| d.child(div().truncate().text_size(px(10.0)).text_color(c.subtle).child(first_line))));
        }
        Some(div().flex().flex_col().gap_1p5().child(div().text_size(px(11.0)).text_color(c.muted).child(title)).child(rows))
    }
}

#[cfg(test)]
mod tests {
    use super::{Range, ToolSort, elapsed, model_totals, nice_max, sort_stats, to_bars};
    use serde_json::json;

    #[test]
    fn axis_maxima_are_round() {
        assert_eq!(nice_max(0.0), 1.0);
        assert_eq!(nice_max(7.3), 10.0);
        assert_eq!(nice_max(1.7), 2.0);
        assert_eq!(nice_max(230.0), 250.0);
    }

    #[test]
    fn elapsed_reads_like_the_tauri_app() {
        assert_eq!(elapsed(320.0), "320ms");
        assert_eq!(elapsed(4200.0), "4.2s");
        assert_eq!(elapsed(190_000.0), "3m 10s");
        assert_eq!(elapsed(3_900_000.0), "1h 5m");
    }

    #[test]
    fn bars_cover_the_range_and_sum_rows() {
        let now = 1_800_001_800_000.0;
        let rows = vec![
            json!({ "hour": now - 1000.0, "provider": "claude", "model": "a", "input": 10, "output": 5, "cacheRead": 0, "cacheWrite": 0, "cost": 1.0, "priced": true }),
            json!({ "hour": now - 2000.0, "provider": "claude", "model": "a", "input": 1, "output": 0, "cacheRead": 0, "cacheWrite": 0, "cost": 0.5, "priced": true }),
            json!({ "hour": now - 90.0 * 86_400_000.0, "provider": "codex", "model": "b", "input": 1, "output": 0, "cacheRead": 0, "cacheWrite": 0, "cost": 9.0, "priced": false }),
        ];
        let bars = to_bars(&rows, Range::Day, now);
        assert_eq!(bars.len(), 24);
        assert_eq!(bars.last().unwrap().cost["claude"], 1.5);
        assert_eq!(bars.last().unwrap().tokens["claude"], 16.0);
        let models = model_totals(&rows, 0.0);
        assert_eq!((models[0].model.as_str(), models[0].priced), ("b", false));
        assert_eq!(models[1].cost, 1.5);
    }

    #[test]
    fn tool_stats_sort_with_ties_by_calls_then_name() {
        let stats = vec![json!({ "name": "b", "calls": 2, "failed": 1 }), json!({ "name": "a", "calls": 2, "failed": 1 }), json!({ "name": "c", "calls": 9, "failed": 0 })];
        let names: Vec<String> = sort_stats(&stats, ToolSort::Failed).iter().map(|s| s["name"].as_str().unwrap().to_string()).collect();
        assert_eq!(names, ["a", "b", "c"]);
    }
}
