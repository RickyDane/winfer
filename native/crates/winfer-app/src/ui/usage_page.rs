//! Settings → Usage (port of `features/usage/UsagePage.tsx`, `UsageHistory.tsx`, `ToolStatsTab.tsx`,
//! `history.ts`, `toolStats.ts` and the Limits list in `UsageMeter.tsx`): token use and API cost
//! over time, per model and per task; the plan limits; the agents' tool calls.

use super::dialogs::relative_time;
use super::orchestration::{format_cost, format_tokens};
use winfer_core::usage::*;
use super::*;
use gpui_kit::component::tooltip::Tooltip;
use serde_json::Value;
use winfer_backend::{tool_stats, usage_history};

/// Agents in chart order.
pub const SERIES: [&str; 4] = ["claude", "codex", "agy", "opencode"];
const SERIES_COLORS: [&str; 4] = ["#cf6e4e", "#6d7cff", "#38bdf8", "#25a28f"];

fn series_name(p: &str) -> &str {
    match p {
        "claude" => "Claude Code",
        "codex" => "Codex",
        "agy" => "Antigravity",
        "opencode" => "OpenCode",
        other => other,
    }
}

fn series_color(p: &str, c: &Colors) -> Hsla {
    SERIES.iter().position(|s| *s == p).map_or(c.subtle, |i| theme::hex(SERIES_COLORS[i]))
}

fn now_ms() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_millis() as f64)
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
    /// When the history and the tool calls were last read (ms since the epoch).
    history_at: Option<f64>,
    tools_at: Option<f64>,
    /// The paired server whose usage is shown (`None`: this machine's).
    pub machine: Option<String>,
    /// Paired servers' usage, as they last sent it (`usage.get {raw}`).
    remote: HashMap<String, RemoteUsage>,
    /// The `usage.get` whose answer is awaited.
    remote_pending: Option<String>,
}

/// A paired server's readings and token history, shown in place of this machine's.
#[derive(Default)]
struct RemoteUsage {
    snapshots: HashMap<String, Snapshot>,
    samples: HashMap<String, Vec<(f64, f64)>>,
    accounts: Vec<Account>,
    rows: Option<Vec<Value>>,
    at: Option<f64>,
}

impl Root {
    /// The paired servers that are up, for the Usage page's machine picker.
    fn usage_machines(&self) -> Vec<(String, String)> {
        let Some(connect) = crate::connect::get() else { return Vec::new() };
        connect.status().peers.into_iter().filter(|p| p.kind == winfer_connect::pairing::Kind::Server && p.link.via.is_some()).map(|p| (p.peer_id, p.name)).collect()
    }

    /// Shows `machine`'s usage (`None`: this one's), asking it for the numbers.
    pub(super) fn show_usage_of(&mut self, machine: Option<String>, cx: &mut Context<Self>) {
        self.usage_page.machine = machine.clone();
        self.usage_page.hover = None;
        if let Some(peer) = machine {
            self.usage_page.remote.entry(peer.clone()).or_default();
            self.request_remote_usage(&peer, false);
        }
        cx.notify();
    }

    fn request_remote_usage(&mut self, peer: &str, refresh: bool) {
        let name = crate::connect::peer_name(peer).unwrap_or_else(|| "the server".into());
        let id = self.connect_request(peer, serde_json::json!({"t": "usage.get", "raw": true, "refresh": refresh}), Some(format!("Couldn't read the usage on {name}")));
        self.usage_page.remote_pending = Some(id);
    }

    #[cfg(debug_assertions)]
    pub(super) fn dev_usage_tab(&self, tab: &str, cx: &mut Context<Self>) {
        self.save("tab", tab, cx);
    }

    /// A `usage.get` that came back with an error instead: asked again later.
    pub(super) fn usage_request_failed(&mut self, id: &str) {
        if self.usage_page.remote_pending.as_deref() == Some(id) {
            self.usage_page.remote_pending = None;
            // Not again right away.
            if let Some(r) = self.usage_page.machine.clone().and_then(|p| self.usage_page.remote.get_mut(&p)) {
                r.at = Some(now_ms());
            }
        }
    }

    /// `usage` from a paired server (the answer to [`Root::request_remote_usage`]).
    pub(super) fn on_remote_usage(&mut self, peer: &str, msg: &Value, cx: &mut Context<Self>) {
        if self.usage_page.remote_pending.as_deref() == msg["id"].as_str() {
            self.usage_page.remote_pending = None;
        }
        let raw = &msg["raw"];
        let remote = RemoteUsage {
            snapshots: serde_json::from_value(raw["snapshots"].clone()).unwrap_or_default(),
            samples: serde_json::from_value(raw["samples"].clone()).unwrap_or_default(),
            accounts: serde_json::from_value(raw["accounts"].clone()).unwrap_or_default(),
            rows: raw["rows"].as_array().cloned(),
            at: Some(now_ms()),
        };
        self.usage_page.remote.insert(peer.to_string(), remote);
        cx.notify();
    }

    /// Puts the shown server's numbers in place of this machine's (and back, called again), so the
    /// Usage page draws them as it draws its own.
    pub(super) fn swap_machine_usage(&mut self) {
        let Some(peer) = self.usage_page.machine.clone() else { return };
        let Some(r) = self.usage_page.remote.get_mut(&peer) else { return };
        std::mem::swap(&mut self.usage.snapshots, &mut r.snapshots);
        std::mem::swap(&mut self.usage.samples, &mut r.samples);
        std::mem::swap(&mut self.usage.accounts, &mut r.accounts);
        std::mem::swap(&mut self.usage_page.rows, &mut r.rows);
        std::mem::swap(&mut self.usage_page.history_at, &mut r.at);
    }

    /// "This Mac ▾": which machine's usage the page shows (only with a paired server up).
    fn usage_machine_picker(&self, c: Colors, cx: &mut Context<Self>) -> Option<Stateful<Div>> {
        let machines = self.usage_machines();
        if machines.is_empty() && self.usage_page.machine.is_none() {
            return None;
        }
        let here = if cfg!(target_os = "macos") { "This Mac" } else { "This machine" };
        let label = self.usage_page.machine.as_deref().map_or_else(|| here.to_string(), |p| crate::connect::peer_name(p).unwrap_or_else(|| "Server".into()));
        let icon = if self.usage_page.machine.is_some() { IconName::Server } else { IconName::Monitor };
        Some(div().id("usage-machine").flex().items_center().gap_1p5().h(px(28.0)).px_2p5().rounded_md().border_1().border_color(c.edge).bg(c.raised).text_size(px(12.0)).text_color(c.fg).cursor_pointer()
            .hover(move |d| d.bg(c.hover))
            .on_mouse_down(MouseButton::Left, cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                cx.stop_propagation();
                let mut items = vec![super::menu::MenuItem::run(here, |this: &mut Root, _: &mut Window, cx: &mut Context<Root>| this.show_usage_of(None, cx))];
                for (peer, name) in this.usage_machines() {
                    items.push(super::menu::MenuItem::run(name, move |this: &mut Root, _: &mut Window, cx: &mut Context<Root>| this.show_usage_of(Some(peer.clone()), cx)));
                }
                this.open_menu(e.position, Some("Usage of".into()), items, cx);
            }))
            .child(Icon::new(icon).size(px(13.0)).text_color(c.subtle))
            .child(label)
            .child(Icon::new(IconName::ChevronDown).size(px(12.0)).text_color(c.subtle)))
    }
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
                    Ok(rows) => {
                        this.usage_page.rows = serde_json::to_value(rows).ok().and_then(|v| v.as_array().cloned());
                        this.usage_page.history_at = Some(now_ms());
                    }
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
                    Ok(report) => {
                        this.usage_page.tools = Some((range, report));
                        this.usage_page.tools_at = Some(now_ms());
                    }
                    Err(e) => this.usage_page.tools_error = Some(e),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// A stat: small label, big value and a detail line.
    fn stat(c: Colors, label: &str, icon: Option<IconName>, value: String, value_color: Hsla, detail: Option<(String, Hsla)>) -> Div {
        super::settings_kit::panel(c).flex_1().min_w_0().gap_1().px_3p5().py_3()
            .child(div().flex().items_center().gap_1p5().text_size(px(11.5)).text_color(c.subtle).children(icon.map(|i| Icon::new(i).size(px(12.0)))).child(label.to_string()))
            .child(div().text_size(px(22.0)).font_weight(FontWeight::SEMIBOLD).text_color(value_color).child(value))
            .when_some(detail, |d, (t, color)| d.child(div().truncate().text_size(px(11.5)).text_color(color).child(t)))
    }

    /// The open tab: Limits, Spend or Tools (remembered across launches).
    fn usage_tab(&self, cx: &App) -> &'static str {
        let saved = match self.saved("tab", cx).as_deref() {
            Some("usage" | "spend") => "spend",
            Some("tools") => "tools",
            _ => "limits",
        };
        self.page_tab("usage", saved)
    }

    /// The tab bar, with the range (Spend and Tools share one), the data's age and Refresh on the right.
    pub(super) fn usage_tabs(&mut self, c: Colors, cx: &mut Context<Self>) -> Div {
        use super::settings_kit::{PageTab, tab_dot};
        let tab = self.usage_tab(cx);
        let now = now_ms();
        let runs_out = self.usage.snapshots.iter().any(|(key, snap)| snap.windows.iter().any(|w| runs_out_at(key, w, &self.usage.samples, now).is_some()));
        let tabs = vec![
            PageTab::new("limits", "Limits", IconName::Gauge).extra(runs_out.then(|| tab_dot(c.warning))),
            PageTab::new("spend", "Spend", IconName::Coins),
            PageTab::new("tools", "Tools", IconName::Wrench),
        ];
        let remote = self.usage_page.machine.clone();
        // A server's numbers, while shown, are asked for again once they're 30 s old.
        if let Some(peer) = &remote
            && self.usage_page.remote_pending.is_none()
            && self.usage_page.history_at.is_none_or(|at| now - at > 30_000.0)
        {
            self.request_remote_usage(peer, false);
        }
        let (loading, at) = match tab {
            _ if remote.is_some() => (self.usage_page.remote_pending.is_some(), self.usage_page.history_at),
            "spend" => (self.usage_page.loading, self.usage_page.history_at),
            "tools" => (self.usage_page.tools_loading, self.usage_page.tools_at),
            _ => (self.usage.is_refreshing(), self.usage.snapshots.values().filter(|s| !s.windows.is_empty()).map(|s| s.at).fold(None, |m: Option<f64>, t| Some(m.map_or(t, |m| m.max(t))))),
        };
        let age = if loading {
            "Reading…".to_string()
        } else {
            at.map_or(String::new(), |t| match relative_time(t).as_str() {
                "now" => "updated just now".to_string(),
                ago => format!("updated {ago} ago"),
            })
        };
        let range = (tab != "limits").then(|| {
            let current = self.usage_range("range", Range::Week, cx);
            self.seg_control("usage-range", Range::ALL.iter().map(|r| (*r, r.short())).collect(), current, c, cx, |this, r, cx| {
                this.usage_page.hover = None;
                this.usage_page.tools_error = None;
                this.save("range", r.id(), cx);
            })
        });
        let refresh = div().id("usage-refresh").size(px(28.0)).flex().items_center().justify_center().rounded_md().text_color(c.subtle).cursor_pointer()
            .hover(move |d| d.bg(c.hover).text_color(c.fg))
            .tooltip(|window, cx| Tooltip::new("Refresh").build(window, cx))
            .on_click(cx.listener(move |this, _, _, cx| match tab {
                _ if this.usage_page.machine.is_some() => {
                    let peer = this.usage_page.machine.clone().unwrap_or_default();
                    this.request_remote_usage(&peer, true);
                    cx.notify();
                }
                "spend" => {
                    this.usage_page.by_dir = None;
                    this.load_usage_history(cx);
                }
                "tools" => {
                    let range = this.usage_range("range", Range::Week, cx);
                    this.load_tools(range, cx);
                }
                _ => this.refresh_all_usage(cx),
            }))
            .child(Icon::new(IconName::RefreshCw).size(px(14.0)));
        self.page_tab_bar("usage", tab, tabs, c, cx)
            .child(div().flex_1())
            .child(div().h(px(38.0)).flex().items_center().gap_2().children(self.usage_machine_picker(c, cx)).children(range).child(div().text_size(px(11.5)).text_color(c.subtle).child(age)).child(refresh))
    }

    /// Settings → Usage & limits: the open tab (the tab bar is [`Root::usage_tabs`]).
    pub(super) fn page_usage(&mut self, c: Colors, cx: &mut Context<Self>) -> Div {
        let tab = self.usage_tab(cx);
        if self.saved("tab", cx).as_deref() != Some(tab) {
            self.save("tab", tab, cx);
        }
        let body = match tab {
            "tools" if self.usage_page.machine.is_some() => div().text_size(px(12.5)).text_color(c.subtle).child("Tool calls are only counted for this machine so far."),
            "tools" => self.tools_tab(c, cx),
            "spend" => self.history_tab(c, cx),
            _ => self.limits_tab(c, cx),
        };
        body.pt_3()
    }

    /// The text of a tab's first line, with the longer explanation behind an info icon.
    fn usage_intro(c: Colors, id: &'static str, text: &'static str, more: &'static str, right: Option<AnyElement>) -> Div {
        div().flex().items_center().gap_2().mx(px(2.0)).text_size(px(12.0)).text_color(c.subtle)
            .child(text)
            .child(div().id(id).text_color(c.subtle).hover(move |d| d.text_color(c.fg)).tooltip(move |window, cx| Tooltip::new(more).build(window, cx)).child(Icon::new(IconName::Info).size(px(13.0))))
            .child(div().flex_1())
            .children(right)
    }

    fn history_tab(&mut self, c: Colors, cx: &mut Context<Self>) -> Div {
        use super::settings_kit::{alpha, mini_switch, panel, section};
        if self.usage_page.machine.is_none() && self.usage_page.rows.is_none() && !self.usage_page.loading && self.usage_page.error.is_none() {
            self.load_usage_history(cx);
        }
        let range = self.usage_range("range", Range::Week, cx);
        let metric = self.saved("metric", cx).unwrap_or_else(|| "cost".into());
        let kind = self.saved("chart", cx).unwrap_or_else(|| "bars".into());
        let mono = crate::fonts::mono(cx);
        // The one setting here sits next to the spend it shows.
        let pane_cost = self.state.read(cx).show_pane_cost;
        let option = div().flex().items_center().gap_2()
            .children(self.reset_mark("usage.cost", c, cx))
            .child(div().id("pane-cost").flex().items_center().gap_2().h(px(28.0)).pl_2p5().pr_2().rounded_md().border_1().border_color(c.edge).bg(c.raised).text_size(px(12.0)).text_color(c.fg).cursor_pointer()
                .hover(move |d| d.bg(c.hover))
                .tooltip(|window, cx| Tooltip::new("Each agent shows what its task has cost so far, at API prices.").build(window, cx))
                .on_click(cx.listener(|this, _, _, cx| this.update_state(cx, |s| s.show_pane_cost = !s.show_pane_cost)))
                .child(Icon::new(IconName::Tag).size(px(13.0)).text_color(c.subtle))
                .child("Show task cost on panes")
                .child(mini_switch(c, pane_cost)));
        let mut page = div().flex().flex_col().gap_4()
            .child(Self::usage_intro(c, "spend-info", "From Claude Code, Codex and OpenCode's logs, at API prices.", "On a subscription plan you don't pay this; it shows what the plan saves. History is kept for 90 days.", Some(option.into_any_element())));
        if let Some(e) = &self.usage_page.error {
            page = page.child(super::settings_kit::banner(c.danger, IconName::CircleAlert, e.clone()));
        }
        let Some(rows) = self.usage_page.rows.clone() else {
            return page.child(div().text_xs().text_color(c.subtle).child("Reading usage from the agents' logs…"));
        };
        let now = now_ms();
        let bars = to_bars(&rows, range, now);
        let models = model_totals(&rows, bars[0].start);
        let sum = |f: fn(&ModelTotal) -> f64| models.iter().map(f).sum::<f64>();
        let unpriced = models.iter().filter(|m| !m.priced).count();
        let providers: Vec<&str> = SERIES.iter().copied().filter(|p| models.iter().any(|m| m.provider == *p)).collect();
        // Each agent's share of the cost (of the tokens when nothing has a price).
        let total_cost = sum(|m| m.cost);
        let by_cost = total_cost > 0.0;
        let shares: Vec<(&str, f64)> = providers.iter().map(|p| (*p, models.iter().filter(|m| m.provider == *p).map(|m| if by_cost { m.cost } else { m.tokens() }).sum::<f64>())).collect();
        let share_total: f64 = shares.iter().map(|s| s.1).sum();
        let mut share_bar = div().flex().h(px(8.0)).gap(px(2.0)).rounded_full().overflow_hidden().bg(c.active);
        let mut share_legend = div().flex().flex_wrap().gap_x_3().gap_y_1();
        for (p, v) in &shares {
            if *v > 0.0 && share_total > 0.0 {
                share_bar = share_bar.child(div().h_full().bg(series_color(p, &c)).flex_grow((*v / share_total) as f32).flex_basis(px(0.0)));
                share_legend = share_legend.child(div().flex().items_center().gap_1p5().text_size(px(11.0)).text_color(c.muted).child(div().size_2().rounded_sm().bg(series_color(p, &c))).child(format!("{} {}%", series_name(p), (v / share_total * 100.0).round())));
            }
        }
        let by_agent = panel(c).flex_1().min_w_0().gap_2().px_3p5().py_3()
            .child(div().flex().items_center().gap_1p5().text_size(px(11.5)).text_color(c.subtle).child(Icon::new(IconName::ChartColumn).size(px(12.0))).child(if by_cost { "By agent" } else { "By agent (tokens)" }))
            .child(if share_total > 0.0 { share_bar.into_any_element() } else { div().text_size(px(12.0)).text_color(c.subtle).child("No usage in this range").into_any_element() })
            .child(share_legend);
        page = page.child(div().flex().gap_2p5()
            .child(Self::stat(c, "Estimated API cost", Some(IconName::Coins), format_cost(total_cost), c.fg, (unpriced > 0).then(|| (format!("{unpriced} model{} without a known price", if unpriced > 1 { "s" } else { "" }), c.warning))))
            .child(Self::stat(c, "Tokens", Some(IconName::Layers), format_tokens(sum(ModelTotal::tokens)), c.fg, Some((format!("{} in · {} out · {} cached", format_tokens(sum(|m| m.input)), format_tokens(sum(|m| m.output)), format_tokens(sum(|m| m.cache_read))), c.subtle))))
            .child(by_agent));
        let metric_toggle = self.seg_control("usage-metric", vec![(true, "Cost"), (false, "Tokens")], metric == "cost", c, cx, |this, cost, cx| this.save("metric", if cost { "cost" } else { "tokens" }, cx));
        let kind_toggle = self.seg_control("usage-kind", vec![(true, "Bars"), (false, "Line")], kind == "bars", c, cx, |this, bars, cx| this.save("chart", if bars { "bars" } else { "line" }, cx));
        let mut legend = div().flex().flex_1().justify_end().gap_3();
        for p in &providers {
            legend = legend.child(div().flex().items_center().gap_1p5().text_size(px(11.0)).text_color(c.muted).child(div().size_2().rounded_sm().bg(series_color(p, &c))).child(series_name(p).to_string()));
        }
        page = page.child(panel(c).gap_3().p_3p5()
            .child(div().flex().items_center().gap_2().child(metric_toggle).child(kind_toggle).child(legend))
            .child(self.usage_chart(&bars, metric == "cost", kind == "bars", range, c, cx)));
        page = page.child(section(c, "Models", None));
        if models.is_empty() {
            page = page.child(panel(c).child(super::settings_kit::empty_state(c, IconName::ChartColumn, "No agent usage in this range.")));
        } else {
            let head = |t: &'static str, right: bool| div().flex_1().when(right, |d| d.text_right()).child(t);
            let mut table = panel(c).overflow_hidden().text_size(px(12.0))
                .child(div().flex().px_3p5().py_2().bg(alpha(c.app, 0.5)).text_size(px(10.5)).font_weight(FontWeight::SEMIBOLD).text_color(c.subtle)
                    .child(head("MODEL", false).flex_grow(2.0)).child(head("INPUT", true)).child(head("OUTPUT", true)).child(head("CACHE READ", true)).child(head("CACHE WRITE", true)).child(head("COST", true)));
            for m in &models {
                let cell = |v: String| div().flex_1().text_right().font_family(mono.clone()).text_size(px(11.5)).text_color(c.muted).child(v);
                let tip = format!("{} · {}", series_name(&m.provider), m.model);
                table = table.child(div().flex().items_center().px_3p5().py_2().border_t_1().border_color(c.border)
                    .child(div().id(SharedString::from(format!("model-{}-{}", m.provider, m.model))).flex_1().flex_grow(2.0).min_w_0().flex().items_center().gap_2()
                        .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
                        .child(div().size_2().flex_shrink_0().rounded_sm().bg(series_color(&m.provider, &c)))
                        .child(div().truncate().text_color(c.fg).child(m.model.clone())))
                    .child(cell(format_tokens(m.input))).child(cell(format_tokens(m.output))).child(cell(format_tokens(m.cache_read))).child(cell(format_tokens(m.cache_write)))
                    .child(div().flex_1().text_right().font_family(mono.clone()).text_size(px(11.5)).text_color(if m.priced { c.fg } else { c.subtle }).child(if m.priced { format_cost(m.cost) } else { "—".into() })));
            }
            page = page.child(table);
        }
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
        // Only counted for this machine's folders so far.
        if self.usage_page.machine.is_some() {
            return None;
        }
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
        let mut table = super::settings_kit::panel(c).overflow_hidden().text_size(px(12.0)).child(div().flex().px_3p5().py_2().bg(super::settings_kit::alpha(c.app, 0.5)).text_size(px(10.5)).font_weight(FontWeight::SEMIBOLD).text_color(c.subtle)
            .child(div().flex_1().flex_grow(3.0).child("TASK")).child(div().w(px(64.0)).child("AGENTS")).child(div().w(px(80.0)).text_right().child("LAST ACTIVE")).child(div().w(px(64.0)).text_right().child("TOKENS")).child(div().w(px(64.0)).text_right().child("COST")));
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
            table = table.child(div().id(("task", i)).flex().items_center().px_3p5().py_2().border_t_1().border_color(c.border)
                .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
                .child(div().flex_1().flex_grow(3.0).min_w_0().child(div().truncate().text_color(c.fg).child(title)).when_some(detail, |x, t| x.child(div().truncate().font_family(mono.clone()).text_size(px(10.0)).text_color(c.subtle).child(t))))
                .child(div().w(px(64.0)).flex().gap_1p5().children(providers.iter().map(|p| div().size_2().rounded_sm().bg(series_color(p, &c)))))
                .child(div().w(px(80.0)).text_right().text_color(c.subtle).child(d["lastAt"].as_f64().map(relative_time).unwrap_or_default()))
                .child(div().w(px(64.0)).text_right().font_family(mono.clone()).text_color(c.muted).child(format_tokens(d["tokens"].as_f64().unwrap_or(0.0))))
                .child(div().w(px(64.0)).text_right().font_family(mono.clone()).text_color(if priced { c.fg } else { c.subtle }).child(if priced { format_cost(cost) } else { "—".into() })));
        }
        let more = rows.len() > 8;
        Some(div().flex().flex_col().gap_4()
            .child(super::settings_kit::section(c, "By task", Some("each worktree an agent worked in: what a feature cost across agents".into())))
            .child(table.when(more, |d| d.child(div().id("tasks-more").border_t_1().border_color(c.border).py_2().text_center().text_size(px(11.5)).text_color(c.subtle).cursor_pointer().hover(move |d| d.bg(c.hover).text_color(c.fg))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.usage_page.all_tasks = !this.usage_page.all_tasks;
                    cx.notify();
                }))
                .child(if all { "Show fewer".to_string() } else { format!("Show all {}", rows.len()) })))))
    }

    fn limits_tab(&mut self, c: Colors, cx: &mut Context<Self>) -> Div {
        use super::settings_kit::{badge, empty_state, intro, panel};
        use super::settings_more::agent_mark;
        let kind = self.saved("limitsChart", cx).unwrap_or_else(|| "line".into());
        let line = kind == "line";
        let toggle = self.seg_control("limits-kind", vec![(true, "Line"), (false, "Bars")], line, c, cx, |this, line, cx| this.save("limitsChart", if line { "line" } else { "bars" }, cx));
        let now = now_ms();
        let mono = crate::fonts::mono(cx);
        let mut page = div().flex().flex_col().gap_5()
            .child(intro(c, "What's left of each plan's limits, like the rings in the title bar. Dashed lines project the current pace to the reset.", Some(toggle.into_any_element())));
        let providers = group_limit_keys(&self.usage.snapshots, &self.usage.accounts);
        if providers.is_empty() {
            page = page.child(panel(c).child(empty_state(c, IconName::Gauge, "No usage reported yet. Refresh to check your signed-in accounts.")));
        } else {
            let render_age = |at: f64| {
                let age = now - at;
                let stale = age > 600_000.0;
                let age_text = format!("{}{}", if stale { "Stale · " } else { "" }, if age < 120_000.0 { "just now".to_string() } else { format!("{} ago", duration(age)) });
                div().flex().items_center().gap_1().text_size(px(11.5)).font_weight(FontWeight::NORMAL).text_color(if stale { c.warning } else { c.subtle })
                    .when(stale, |d| d.child(Icon::new(IconName::Clock).size(px(12.0))))
                    .child(age_text)
            };
            for (provider, keys) in providers {
                let name = self.state.read(cx).provider_name(&provider);
                if keys.len() == 1 {
                    let key = &keys[0];
                    let snap = self.usage.snapshots[key].clone();
                    let account = key.contains(':').then(|| account_name(key, &self.usage.accounts));
                    let header = div().flex().items_center().gap_2().mx(px(2.0)).text_size(px(12.5)).font_weight(FontWeight::MEDIUM)
                        .child(agent_mark(c, &provider, 22.0))
                        .child(div().text_color(c.fg).child(name))
                        .when_some(account, |d, a| d.child(div().font_weight(FontWeight::NORMAL).text_color(c.subtle).child(format!("· {a}"))))
                        .child(div().flex_1())
                        .child(render_age(snap.at));
                    let mut block = div().flex().flex_col().gap_2().child(header);
                    let groups = super::usage::group_windows(&snap.windows);
                    if groups.len() > 1 {
                        for chunk in groups.chunks(2) {
                            let mut row = div().flex().gap_2p5();
                            for (group_title, items) in chunk {
                                let mut card = panel(c).flex_1().min_w_0().gap_2p5().px_3p5().py_3();
                                if let Some(title) = group_title {
                                    card = card.child(div().text_size(px(10.5)).font_weight(FontWeight::SEMIBOLD).text_color(c.subtle).child(title.to_uppercase()));
                                }
                                let mut wins = div().flex().gap_3p5();
                                for (label, w) in items {
                                    wins = wins.child(self.render_limit_window(key, label, w, line, now, c).flex_1().min_w_0());
                                }
                                row = row.child(card.child(wins));
                            }
                            block = block.child(row);
                        }
                    } else if let Some((group_title, items)) = groups.into_iter().next() {
                        if items.len() > 2 {
                            let (primary, extra) = items.split_at(2);
                            let mut left_card = panel(c).flex_1().flex_grow(2.0).min_w_0().gap_2p5().px_3p5().py_3();
                            if let Some(title) = group_title {
                                left_card = left_card.child(div().text_size(px(10.5)).font_weight(FontWeight::SEMIBOLD).text_color(c.subtle).child(title.to_uppercase()));
                            }
                            let mut pair = div().flex().gap_3p5();
                            for (label, w) in primary {
                                pair = pair.child(self.render_limit_window(key, label, w, line, now, c).flex_1().min_w_0());
                            }
                            left_card = left_card.child(pair);
                            let mut right_card = panel(c).flex_1().flex_grow(1.0).min_w_0().gap_3().px_3p5().py_3();
                            for (label, w) in extra {
                                right_card = right_card.child(self.render_limit_window(key, label, w, line, now, c));
                            }
                            block = block.child(div().flex().gap_2p5().child(left_card).child(right_card));
                        } else {
                            let mut card = panel(c).gap_2p5().px_3p5().py_3();
                            if let Some(title) = group_title {
                                card = card.child(div().text_size(px(10.5)).font_weight(FontWeight::SEMIBOLD).text_color(c.subtle).child(title.to_uppercase()));
                            }
                            let mut wins = div().flex().gap_3p5();
                            for (label, w) in &items {
                                wins = wins.child(self.render_limit_window(key, label, w, line, now, c).flex_1().min_w_0());
                            }
                            block = block.child(card.child(wins));
                        }
                    }
                    page = page.child(block);
                } else {
                    // Several accounts under the same provider: one provider heading, one card with
                    // a combined "main" limit graphic across all accounts on top and compact
                    // per-account rows underneath.
                    let pooled = pooled_snapshot(&provider, &keys, &self.usage.snapshots, now);
                    let header = div().flex().items_center().gap_2().mx(px(2.0)).text_size(px(12.5)).font_weight(FontWeight::MEDIUM)
                        .child(agent_mark(c, &provider, 22.0))
                        .child(div().text_color(c.fg).child(name))
                        .child(badge(c.muted, format!("{} accounts", keys.len())))
                        .child(div().flex_1())
                        .child(render_age(pooled.at));
                    let mut card = panel(c).overflow_hidden();

                    // Main combined section taking every account's limit and reset time into account.
                    let mut main = div().flex().flex_col().gap_2p5().px_3p5().py_3();
                    for (group_title, items) in super::usage::group_windows(&pooled.windows) {
                        if let Some(title) = group_title {
                            main = main.child(div().text_size(px(10.5)).font_weight(FontWeight::SEMIBOLD).text_color(c.subtle).child(title.to_uppercase()));
                        }
                        if items.len() > 2 {
                            let (primary, extra) = items.split_at(2);
                            let mut pair = div().flex().flex_1().flex_grow(2.0).min_w_0().gap_3p5();
                            for (wlabel, w) in primary {
                                pair = pair.child(self.render_pooled_window(&provider, &keys, wlabel, w, line, now, c).flex_1().min_w_0());
                            }
                            let mut side = div().flex().flex_col().flex_1().flex_grow(1.0).min_w_0().gap_2p5().pl_3p5().border_l_1().border_color(c.border);
                            for (wlabel, w) in extra {
                                side = side.child(self.render_pooled_window(&provider, &keys, wlabel, w, line, now, c));
                            }
                            main = main.child(div().flex().gap_3p5().child(pair).child(side));
                        } else {
                            let mut wins = div().flex().gap_3p5();
                            for (wlabel, w) in &items {
                                wins = wins.child(self.render_pooled_window(&provider, &keys, wlabel, w, line, now, c).flex_1().min_w_0());
                            }
                            main = main.child(wins);
                        }
                    }
                    card = card.child(main);

                    // Compact per-account breakdown underneath.
                    let mut rows = div().flex().flex_col().border_t_1().border_color(c.border).bg(super::settings_kit::alpha(c.app, 0.35));
                    for (idx, key) in keys.iter().enumerate() {
                        let snap = self.usage.snapshots[key].clone();
                        let label = account_name(key, &self.usage.accounts);
                        let age = now - snap.at;
                        let stale = age > 600_000.0;
                        let age_short = if age < 120_000.0 { "just now".to_string() } else { format!("{} ago", duration(age)) };
                        let left_col = div().w(px(136.0)).flex_shrink_0().flex().items_center().gap_2().min_w_0()
                            .child(div().size(px(18.0)).flex_shrink_0().flex().items_center().justify_center().rounded_full().bg(c.hover).font_family(mono.clone()).text_size(px(10.0)).font_weight(FontWeight::SEMIBOLD).text_color(c.muted).child((idx + 1).to_string()))
                            .child(div().flex_1().min_w_0().flex().flex_col()
                                .child(div().truncate().text_size(px(11.5)).font_weight(FontWeight::MEDIUM).text_color(c.fg).child(label))
                                .child(div().truncate().text_size(px(10.0)).text_color(if stale { c.warning } else { c.subtle }).child(age_short)));
                        let flat: Vec<(String, UsageWindow)> = super::usage::group_windows(&snap.windows)
                            .into_iter()
                            .flat_map(|(grp, items)| {
                                items.into_iter().map(move |(wl, w)| {
                                    let l = match grp {
                                        Some(g) => format!("{g} · {wl}"),
                                        None => wl.to_string(),
                                    };
                                    (l, w.clone())
                                })
                            })
                            .collect();
                        let mut grid = div().flex_1().min_w_0().flex().flex_col().gap_1p5();
                        for chunk in flat.chunks(2) {
                            let mut line_row = div().flex().items_center().gap_3p5();
                            for (wl, w) in chunk {
                                line_row = line_row.child(compact_account_window(wl, w, now, c, mono.clone()).flex_1().min_w_0());
                            }
                            if chunk.len() == 1 && flat.len() > 1 {
                                line_row = line_row.child(div().flex_1());
                            }
                            grid = grid.child(line_row);
                        }
                        rows = rows.child(
                            div().flex().items_center().gap_3().px_3p5().py_2()
                                .when(idx > 0, |d| d.border_t_1().border_color(c.border))
                                .child(left_col)
                                .child(grid),
                        );
                    }
                    card = card.child(rows);
                    page = page.child(div().flex().flex_col().gap_2().child(header).child(card));
                }
            }
        }
        let footer = div().flex().items_center().gap_1p5().mx(px(2.0)).text_size(px(12.0)).text_color(c.subtle)
            .child(Icon::new(IconName::Users).size(px(13.0)))
            .child("Extra accounts and the order agents fail over in are under")
            .child(div().id("limits-accounts-link").text_color(c.accent).cursor_pointer().hover(|d| d.underline())
                .on_click(cx.listener(|this, _, window, cx| this.open_settings_page("accounts", window, cx)))
                .child("Accounts →"));
        page.child(footer)
    }

    /// The Limits tab for the phone (`usage.get`): per agent, the pooled limits and each account's.
    pub(super) fn mobile_usage(&self, cx: &App) -> Value {
        let s = self.state.read(cx);
        limits_json(&self.usage.snapshots, &self.usage.accounts, &self.usage.samples, &|p| s.provider_name(p), now_ms())
    }

    /// Token & cost summary for the phone (`usage.get`), matching the desktop Usage → Spend tab.
    fn mobile_tokens(&self, rows: &[Value], range: Range, now: f64, cx: &App) -> Value {
        let s = self.state.read(cx);
        tokens_json(rows, range, now, &|p| s.provider_name(p))
    }

    /// Serves `usage.get` for the phone with both plan limits and actual token/cost usage.
    pub(super) fn mobile_usage_get(&mut self, req: &Value, reply: impl Fn(Value) + 'static, cx: &mut Context<Self>) {
        let range = req["range"].as_str().and_then(Range::parse).unwrap_or(Range::Week);
        let fresh = self.usage_page.rows.is_some()
            && self.usage_page.history_at.is_some_and(|at| now_ms() - at < 60_000.0);
        if fresh {
            let mut usage = self.mobile_usage(cx);
            let now = usage["now"].as_f64().unwrap_or_else(now_ms);
            if let Some(rows) = self.usage_page.rows.as_deref() {
                usage["tokens"] = self.mobile_tokens(rows, range, now, cx);
            }
            usage["t"] = serde_json::json!("usage");
            return reply(usage);
        }

        cx.spawn(async move |this, cx| {
            let found = tauri::async_runtime::spawn(usage_history::usage_history(backend::app().clone(), 90))
                .await
                .map_err(|e| e.to_string())
                .and_then(|r| r);
            let _ = this.update(cx, |this, cx| {
                if let Ok(rows) = found
                    && let Some(arr) = serde_json::to_value(rows).ok().and_then(|v| v.as_array().cloned())
                {
                    this.usage_page.rows = Some(arr);
                    this.usage_page.history_at = Some(now_ms());
                }
                let mut usage = this.mobile_usage(cx);
                let now = usage["now"].as_f64().unwrap_or_else(now_ms);
                if let Some(rows) = this.usage_page.rows.as_deref() {
                    usage["tokens"] = this.mobile_tokens(rows, range, now, cx);
                }
                usage["t"] = serde_json::json!("usage");
                reply(usage);
            });
        })
        .detach();
    }

    fn render_limit_window(&self, key: &str, label: &str, w: &UsageWindow, line: bool, now: f64, c: Colors) -> Div {
        let out = runs_out_at(key, w, &self.usage.samples, now);
        if line {
            self.window_line(key, w, label, out, now, c)
        } else {
            window_bar(w, label, out, now, c)
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn render_pooled_window(&self, provider: &str, keys: &[String], label: &str, w: &UsageWindow, line: bool, now: f64, c: Colors) -> Div {
        let out = super::usage::pooled_runs_out_at(provider, &w.kind, &self.usage.snapshots, &self.usage.accounts, &self.usage.samples, now);
        let series = pooled_series(&w.kind, keys, &self.usage.snapshots, &self.usage.samples, now);
        let left = left_now(w, now);
        let pct_color = limit_pct_color(left, c);
        let meta = pooled_reset_line(&series, out, now, c);
        if line {
            let line_color = if left < 40.0 { pct_color } else { c.accent };
            let chart = paint_pooled_chart(series, line_color, c.warning, c.border_strong, out.is_some());
            div().flex().flex_col().gap_1p5()
                .child(window_top(label, left, pct_color, c))
                .child(div().relative().h(px(52.0)).w_full().child(chart))
                .child(meta)
        } else {
            let bar_color = if left < 40.0 { pct_color } else { c.success };
            div().flex().flex_col().gap_1p5()
                .child(window_top(label, left, pct_color, c))
                .child(div().my_2().child(super::settings_kit::meter(c, (left / 100.0) as f32, bar_color, 7.0)))
                .child(meta)
        }
    }

    fn window_line(&self, key: &str, w: &UsageWindow, label: &str, out: Option<f64>, now: f64, c: Colors) -> Div {
        use super::settings_kit::alpha;
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
        let pct_color = limit_pct_color(left, c);
        let line_color = if left < 40.0 { pct_color } else { c.accent };
        let proj_target = if live && end > now {
            if let Some(o) = out {
                Some((o.min(end), 0.0, c.warning))
            } else if now > start && left < 100.0 {
                let rate = (100.0 - left) / (now - start);
                let at_end = (left - rate * (end - now)).max(0.0);
                Some((end, at_end, alpha(line_color, 0.7)))
            } else {
                None
            }
        } else {
            None
        };
        let path_points = points.clone();
        let border_color = c.border_strong;
        let chart = canvas(|_, _, _| {}, move |bounds, _, window, _| {
            let denom = (end - start).max(1.0);
            let x = |t: f64| bounds.origin.x + bounds.size.width * (((t - start) / denom).clamp(0.0, 1.0) as f32);
            let y = |v: f64| bounds.origin.y + bounds.size.height * (1.0 - (v / 100.0).clamp(0.0, 1.0) as f32);
            let bottom = bounds.origin.y + bounds.size.height;
            if path_points.len() >= 2 {
                let mut fill = PathBuilder::fill();
                for (i, (t, v)) in path_points.iter().enumerate() {
                    if i == 0 { fill.move_to(point(x(*t), y(*v))) } else { fill.line_to(point(x(*t), y(*v))) }
                }
                if let (Some((first_t, _)), Some((last_t, _))) = (path_points.first(), path_points.last()) {
                    fill.line_to(point(x(*last_t), bottom));
                    fill.line_to(point(x(*first_t), bottom));
                    fill.close();
                }
                if let Ok(path) = fill.build() {
                    window.paint_path(path, alpha(line_color, 0.12));
                }
            }
            let mut stroke = PathBuilder::stroke(px(1.6));
            for (i, (t, v)) in path_points.iter().enumerate() {
                if i == 0 { stroke.move_to(point(x(*t), y(*v))) } else { stroke.line_to(point(x(*t), y(*v))) }
            }
            if let Ok(path) = stroke.build() {
                window.paint_path(path, line_color);
            }
            // Dashed projection from now to reset (or run-out).
            if let Some((pt, pv, pcol)) = proj_target {
                let (x0, y0) = (x(now), y(left));
                let (x1, y1) = (x(pt), y(pv));
                let dx = x1 - x0;
                let dy = y1 - y0;
                let dist = (f32::from(dx).hypot(f32::from(dy))).max(1.0);
                let step = 6.0_f32;
                let mut d = 0.0_f32;
                while d < dist {
                    let t0 = d / dist;
                    let t1 = ((d + 3.0) / dist).min(1.0);
                    let mut seg = PathBuilder::stroke(px(1.3));
                    seg.move_to(point(x0 + dx * t0, y0 + dy * t0));
                    seg.line_to(point(x0 + dx * t1, y0 + dy * t1));
                    if let Ok(path) = seg.build() {
                        window.paint_path(path, pcol);
                    }
                    d += step;
                }
            }
            // Subtle reset-line marker on the right edge when the window is live.
            if live {
                let rx = bounds.origin.x + bounds.size.width - px(0.5);
                let mut y_cur = bounds.origin.y;
                while y_cur < bottom {
                    let mut tick = PathBuilder::stroke(px(1.0));
                    tick.move_to(point(rx, y_cur));
                    tick.line_to(point(rx, (y_cur + px(2.5)).min(bottom)));
                    if let Ok(path) = tick.build() {
                        window.paint_path(path, alpha(border_color, 0.6));
                    }
                    y_cur += px(5.5);
                }
            }
        }).absolute().size_full();
        div().flex().flex_col().gap_1p5()
            .child(window_top(label, left, pct_color, c))
            .child(div().relative().h(px(46.0)).w_full().child(chart))
            .child(reset_line(w, out, now, c))
    }
}

fn paint_pooled_chart(series: PooledSeries, line_color: Hsla, warn_color: Hsla, border_color: Hsla, warns: bool) -> impl IntoElement {
    use super::settings_kit::alpha;
    let proj_color = if warns { warn_color } else { alpha(line_color, 0.75) };
    canvas(|_, _, _| {}, move |bounds, _, window, _| {
        let denom = (series.end - series.start).max(1.0);
        let x = |t: f64| bounds.origin.x + bounds.size.width * (((t - series.start) / denom).clamp(0.0, 1.0) as f32);
        let y = |v: f64| bounds.origin.y + bounds.size.height * (1.0 - (v / 100.0).clamp(0.0, 1.0) as f32);
        let bottom = bounds.origin.y + bounds.size.height;
        // Subtle vertical dashed tick at every account's reset timestamp.
        for &r in &series.resets {
            let rx = x(r).min(bounds.origin.x + bounds.size.width - px(0.5));
            let mut y_cur = bounds.origin.y;
            while y_cur < bottom {
                let mut tick = PathBuilder::stroke(px(1.0));
                tick.move_to(point(rx, y_cur));
                tick.line_to(point(rx, (y_cur + px(2.5)).min(bottom)));
                if let Ok(path) = tick.build() {
                    window.paint_path(path, alpha(border_color, 0.55));
                }
                y_cur += px(5.5);
            }
        }
        if series.past.len() >= 2 {
            let mut fill = PathBuilder::fill();
            for (i, &(t, v)) in series.past.iter().enumerate() {
                if i == 0 { fill.move_to(point(x(t), y(v))) } else { fill.line_to(point(x(t), y(v))) }
            }
            if let (Some(&(first_t, _)), Some(&(last_t, _))) = (series.past.first(), series.past.last()) {
                fill.line_to(point(x(last_t), bottom));
                fill.line_to(point(x(first_t), bottom));
                fill.close();
            }
            if let Ok(path) = fill.build() {
                window.paint_path(path, alpha(line_color, 0.14));
            }
        }
        let mut stroke = PathBuilder::stroke(px(1.75));
        for (i, &(t, v)) in series.past.iter().enumerate() {
            if i == 0 { stroke.move_to(point(x(t), y(v))) } else { stroke.line_to(point(x(t), y(v))) }
        }
        if let Ok(path) = stroke.build() {
            window.paint_path(path, line_color);
        }
        // Dashed stepped projection showing combined drain + each account's refill step.
        for seg in series.future.windows(2) {
            let ((t0, v0), (t1, v1)) = (seg[0], seg[1]);
            let (x0, y0) = (x(t0), y(v0));
            let (x1, y1) = (x(t1), y(v1));
            let dx = x1 - x0;
            let dy = y1 - y0;
            let dist = f32::from(dx).hypot(f32::from(dy));
            if dist < 0.5 {
                continue;
            }
            let step = 6.0_f32;
            let mut d = 0.0_f32;
            while d < dist {
                let s0 = d / dist;
                let s1 = ((d + 3.0) / dist).min(1.0);
                let mut dash = PathBuilder::stroke(px(1.3));
                dash.move_to(point(x0 + dx * s0, y0 + dy * s0));
                dash.line_to(point(x0 + dx * s1, y0 + dy * s1));
                if let Ok(path) = dash.build() {
                    window.paint_path(path, proj_color);
                }
                d += step;
            }
        }
    })
    .absolute()
    .size_full()
}

fn pooled_reset_line(series: &PooledSeries, out: Option<f64>, now: f64, c: Colors) -> Div {
    let text = match (series.resets.first(), series.resets.last()) {
        (Some(&first), Some(&last)) if (last - first).abs() >= 120_000.0 => format!("next reset in {} · full in {}", duration(first - now), duration(last - now)),
        (Some(&first), _) => format!("resets in {}", duration(first - now)),
        (None, _) => "all reset — full again".to_string(),
    };
    div().flex().flex_col().gap_0p5().text_size(px(11.0)).text_color(c.subtle)
        .child(text)
        .when_some(out, |d, o| d.child(div().flex().items_start().gap_1().text_color(c.warning)
            .child(div().pt(px(1.0)).flex_shrink_0().child(Icon::new(IconName::TriangleAlert).size(px(11.0))))
            .child(div().flex_1().min_w_0().child(format!("at this pace runs out in {}", duration(o - now))))))
}

/// One compact window cell inside a multi-account provider's per-account row.
fn compact_account_window(label: &str, w: &UsageWindow, now: f64, c: Colors, mono: SharedString) -> Div {
    let left = left_now(w, now);
    let pct_color = limit_pct_color(left, c);
    let bar_color = if left < 40.0 { pct_color } else { c.success };
    let reset = w.resets_at.map(|r| if r > now { duration(r - now) } else { "full".into() });
    div().flex().flex_col().gap_1()
        .child(div().flex().items_baseline().gap_1p5()
            .child(div().flex_1().min_w_0().truncate().text_size(px(11.0)).text_color(c.muted).child(label.to_string()))
            .when_some(reset, |d, r| d.child(div().flex_shrink_0().text_size(px(10.0)).text_color(c.subtle).child(r)))
            .child(div().flex_shrink_0().font_family(mono).text_size(px(11.0)).font_weight(FontWeight::SEMIBOLD).text_color(pct_color).child(format!("{}%", left.round()))))
        .child(super::settings_kit::meter(c, (left / 100.0) as f32, bar_color, 4.0))
}

fn limit_pct_color(left: f64, c: Colors) -> Hsla {
    if left < 15.0 { c.danger } else if left < 40.0 { c.warning } else { c.fg }
}

fn window_top(label: &str, left: f64, pct_color: Hsla, c: Colors) -> Div {
    div().flex().items_baseline().gap_2()
        .child(div().flex_1().min_w_0().truncate().text_size(px(12.0)).font_weight(FontWeight::MEDIUM).text_color(c.muted).child(label.to_string()))
        .child(div().flex_shrink_0().flex().items_baseline().gap_0p5()
            .child(div().text_size(px(17.0)).font_weight(FontWeight::SEMIBOLD).text_color(pct_color).child(format!("{}%", left.round())))
            .child(div().text_size(px(11.0)).text_color(c.subtle).child("left")))
}

fn reset_line(w: &UsageWindow, out: Option<f64>, now: f64, c: Colors) -> Div {
    let reset = w.resets_at.map(|r| if r > now { format!("resets in {}", duration(r - now)) } else { "reset — full again".into() }).unwrap_or_default();
    div().flex().flex_col().gap_0p5().text_size(px(11.0)).text_color(c.subtle)
        .child(reset)
        .when_some(out, |d, o| d.child(div().flex().items_start().gap_1().text_color(c.warning)
            .child(div().pt(px(1.0)).flex_shrink_0().child(Icon::new(IconName::TriangleAlert).size(px(11.0))))
            .child(div().flex_1().min_w_0().child(format!("at this pace runs out in {}", duration(o - now))))))
}

fn window_bar(w: &UsageWindow, label: &str, out: Option<f64>, now: f64, c: Colors) -> Div {
    let left = left_now(w, now);
    let pct_color = limit_pct_color(left, c);
    let bar_color = if left < 40.0 { pct_color } else { c.success };
    div().flex().flex_col().gap_1p5()
        .child(window_top(label, left, pct_color, c))
        .child(div().my_2().child(super::settings_kit::meter(c, (left / 100.0) as f32, bar_color, 7.0)))
        .child(reset_line(w, out, now, c))
}

fn format_count(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
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
        let range = self.usage_range("range", Range::Week, cx);
        let sort = self.saved("toolsSort", cx).and_then(|s| ToolSort::ALL.iter().find(|x| x.1 == s).map(|x| x.0)).unwrap_or_default();
        if self.usage_page.tools.as_ref().is_none_or(|(r, _)| *r != range) && !self.usage_page.tools_loading && self.usage_page.tools_error.is_none() {
            self.load_tools(range, cx);
        }
        let mono = crate::fonts::mono(cx);
        let sorts = self.seg_control("tools-sort", ToolSort::ALL.iter().map(|(s, _, l)| (*s, *l)).collect(), sort, c, cx, |this, s, cx| {
            let id = ToolSort::ALL.iter().find(|x| x.0 == s).map_or("calls", |x| x.1);
            this.save("toolsSort", id, cx);
        });
        let sort_bar = div().flex().items_center().gap_2()
            .child(div().text_size(px(11.5)).text_color(c.subtle).child("Sort"))
            .child(sorts);
        let mut page = div().flex().flex_col().gap_4()
            .child(Self::usage_intro(c, "tools-info", "Tool calls and shell commands from Claude Code and Codex's logs.", "How often each ran, how many failed, how long they took and roughly how many tokens their input and output added to the context.", Some(sort_bar.into_any_element())));
        if let Some(e) = &self.usage_page.tools_error {
            page = page.child(super::settings_kit::banner(c.danger, IconName::CircleAlert, e.clone()));
        }
        let Some((shown_range, rep)) = self.usage_page.tools.clone() else {
            return page.child(div().text_xs().text_color(c.subtle).child("Reading tool calls from the agents' logs…"));
        };
        let num = |k: &str| rep[k].as_f64().unwrap_or(0.0);
        let rate = fail_rate(&rep);
        let calls = num("calls");
        let failed = num("failed");
        let mut body = div().flex().flex_col().gap_4().when(shown_range != range, |d| d.opacity(0.6))
            .child(div().flex().gap_2p5()
                .child(Self::stat(c, "Tool calls", None, format_count(calls.round() as u64), c.fg, Some((format!("{} sessions", num("sessions").round() as u64), c.subtle))))
                .child(Self::stat(c, "Failed", None, format_count(failed.round() as u64), if failed > 0.0 { c.danger } else { c.fg }, (calls > 0.0).then(|| (format!("{}% of calls", if rate < 1.0 && failed > 0.0 { "<1".to_string() } else { format!("{rate:.1}").trim_end_matches(".0").to_string() }), c.subtle))))
                .child(Self::stat(c, "Time in tools", None, elapsed(num("durationMs")), c.fg, (calls > 0.0).then(|| (format!("{} per call", elapsed(num("durationMs") / calls)), c.subtle))))
                .child(Self::stat(c, "Tokens (est.)", None, format_tokens(num("tokens")), c.fg, Some(("input and results".into(), c.subtle)))));
        if calls == 0.0 {
            body = body.child(super::settings_kit::panel(c).child(super::settings_kit::empty_state(c, IconName::Wrench, "No tool calls in this range.")));
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
        use super::settings_kit::{alpha, panel, section};
        let rows = sort_stats(stats, sort);
        if rows.is_empty() {
            return None;
        }
        let all = self.usage_page.all_tools.get(key).copied().unwrap_or(false);
        let max = rows.iter().map(|s| sort.key(s)).fold(1.0, f64::max);
        let head = |t: &'static str, w: f32| div().w(px(w)).text_right().child(t);
        let mut table = panel(c).overflow_hidden().text_size(px(12.0))
            .child(div().flex().px_3p5().py_2().bg(alpha(c.app, 0.5)).text_size(px(10.5)).font_weight(FontWeight::SEMIBOLD).text_color(c.subtle)
                .child(div().flex_1().child("NAME")).child(head("CALLS", 56.0)).child(head("FAILED", 76.0)).child(head("TIME · AVG", 104.0)).child(head("MAX", 58.0)).child(head("TOKENS", 64.0)));
        for s in rows.iter().take(if all { rows.len() } else { 12 }) {
            let value = sort.key(s);
            let failed = s["failed"].as_f64().unwrap_or(0.0);
            let rate = fail_rate(s);
            let timed = s["timed"].as_f64().unwrap_or(0.0);
            let duration_ms = s["durationMs"].as_f64().unwrap_or(0.0);
            let cell = |w: f32| div().w(px(w)).text_right().font_family(mono.clone()).text_size(px(11.5));
            table = table.child(div().flex().items_center().px_3p5().py_2().border_t_1().border_color(c.border)
                .child(div().relative().flex_1().min_w_0().pr_2()
                    .child(div().absolute().top(px(-2.0)).bottom(px(-2.0)).left(px(-6.0)).rounded(px(4.0)).bg(alpha(c.accent, 0.14)).w(relative((value / max) as f32)))
                    .child(div().relative().truncate().font_family(mono.clone()).text_size(px(11.5)).text_color(c.fg).child(format!("{prefix}{}", s["name"].as_str().unwrap_or_default()))))
                .child(cell(56.0).text_color(c.fg).child(format_count(s["calls"].as_u64().unwrap_or(0))))
                .child(cell(76.0).text_color(if failed > 0.0 { if rate >= 10.0 { c.danger } else { c.muted } } else { c.subtle }).child(if failed > 0.0 { format!("{} · {}%", failed.round() as u64, if rate < 1.0 { "<1".to_string() } else { format!("{}", rate.round()) }) } else { "–".into() }))
                .child(cell(104.0).text_color(c.muted).child(if timed > 0.0 { format!("{} · {}", elapsed(duration_ms), elapsed(duration_ms / timed)) } else { "–".into() }))
                .child(cell(58.0).text_color(c.subtle).child(if timed > 0.0 { elapsed(s["maxMs"].as_f64().unwrap_or(0.0)) } else { "–".into() }))
                .child(cell(64.0).text_color(c.muted).child(format_tokens(s["tokens"].as_f64().unwrap_or(0.0)))));
        }
        let more = rows.len() > 12;
        Some(div().flex().flex_col().gap_4()
            .child(section(c, title, None))
            .child(table.when(more, |d| d.child(div().id(SharedString::from(format!("more-{key}"))).border_t_1().border_color(c.border).py_2().text_center().text_size(px(11.5)).text_color(c.accent).cursor_pointer().hover(move |d| d.bg(c.hover))
                .on_click(cx.listener(move |this, _, _, cx| {
                    let e = this.usage_page.all_tools.entry(key.to_string()).or_default();
                    *e = !*e;
                    cx.notify();
                }))
                .child(if all { "Show fewer".to_string() } else { format!("Show all {}", rows.len()) })))))
    }

    fn failures(&self, rep: &Value, c: Colors, mono: SharedString, cx: &mut Context<Self>) -> Option<Div> {
        use super::settings_kit::{panel, section};
        let list = rep["failures"].as_array().filter(|l| !l.is_empty())?;
        let failed = rep["failed"].as_u64().unwrap_or(0);
        let note = (failed as usize > list.len()).then(|| format!("latest {} of {}", list.len(), format_count(failed)));
        let mut rows = panel(c).overflow_hidden();
        for (i, f) in list.iter().enumerate() {
            let open = self.usage_page.open_failure == Some(i);
            let error = f["error"].as_str().unwrap_or_default().to_string();
            let first_line = error.lines().find(|l| !l.trim().is_empty()).unwrap_or_default().to_string();
            let meta = match f["durationMs"].as_f64() {
                Some(ms) => format!("{} · {}", elapsed(ms), relative_time(f["at"].as_f64().unwrap_or(0.0))),
                None => relative_time(f["at"].as_f64().unwrap_or(0.0)),
            };
            rows = rows.child(div().id(("failure", i)).flex().flex_col().px_3p5().py_2().cursor_pointer().hover(move |d| d.bg(c.hover)).when(i > 0, |d| d.border_t_1().border_color(c.border))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.usage_page.open_failure = if this.usage_page.open_failure == Some(i) { None } else { Some(i) };
                    cx.notify();
                }))
                .child(div().flex().items_center().gap_2()
                    .child(Icon::new(if open { IconName::ChevronDown } else { IconName::ChevronRight }).size(px(12.0)).text_color(c.subtle).flex_shrink_0())
                    .child(div().flex_shrink_0().font_family(mono.clone()).text_size(px(11.5)).font_weight(FontWeight::MEDIUM).text_color(c.danger).child(f["tool"].as_str().unwrap_or_default().to_string()))
                    .child(div().min_w_0().flex_1().truncate().font_family(mono.clone()).text_size(px(11.0)).text_color(c.muted).child(f["input"].as_str().unwrap_or_default().to_string()))
                    .child(div().flex_shrink_0().font_family(mono.clone()).text_size(px(10.5)).text_color(c.subtle).child(meta)))
                .when(open, |d| d.child(div().mt_1p5().ml_5().flex().flex_col().gap_1()
                    .child(div().rounded(px(7.0)).bg(c.app).px_2p5().py_2().font_family(mono.clone()).text_size(px(11.0)).text_color(c.muted).child(if error.is_empty() { "No output.".to_string() } else { error.clone() }))
                    .child(div().text_size(px(10.5)).text_color(c.subtle).child(format!("{}{}", if f["provider"] == "codex" { "Codex" } else { "Claude Code" }, f["cwd"].as_str().map_or(String::new(), |d| format!(" · {d}")))))))
                .when(!open && !first_line.is_empty(), |d| d.child(div().ml_5().mt_0p5().truncate().text_size(px(11.0)).text_color(c.subtle).child(first_line))));
        }
        Some(div().flex().flex_col().gap_4().child(section(c, "Failed calls", note)).child(rows))
    }
}

#[cfg(test)]
mod tests {
    use super::{Account, Range, Snapshot, ToolSort, UsageWindow, account_name, elapsed, group_limit_keys, model_totals, nice_max, pooled_series, sort_stats, to_bars};
    use serde_json::json;
    use std::collections::HashMap;

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

    #[test]
    fn limit_keys_group_by_provider_in_account_order() {
        let w = UsageWindow { kind: "session".into(), label: "5-hour".into(), used_percent: 40.0, resets_at: None };
        let snap = |p: &str| Snapshot { provider: p.into(), windows: vec![w.clone()], at: 1.0, reset_credits: None };
        let mut snaps = HashMap::new();
        // Real snapshots store the full account key in `snap.provider` (e.g. `"claude:account-2"`).
        snaps.insert("claude".into(), snap("claude"));
        snaps.insert("claude:account-2".into(), snap("claude:account-2"));
        snaps.insert("claude:account-3".into(), snap("claude:account-3"));
        snaps.insert("codex".into(), snap("codex"));
        let acc = |id: &str, label: &str| Account { id: id.into(), provider: "claude".into(), label: label.into(), enabled: true, signed_in: true, email: None, exhausted_until: None, dir: String::new() };
        let accounts = vec![acc("claude:account-2", "Work"), acc("claude:default", "Personal"), acc("claude:account-3", "Backup")];
        let grouped = group_limit_keys(&snaps, &accounts);
        assert_eq!(grouped.len(), 2);
        assert_eq!(grouped[0], ("claude".into(), vec!["claude:account-2".into(), "claude".into(), "claude:account-3".into()]));
        assert_eq!(grouped[1], ("codex".into(), vec!["codex".into()]));
        assert_eq!(account_name("claude", &accounts), "Personal");
        assert_eq!(account_name("claude:account-2", &accounts), "Work");
        assert_eq!(account_name("codex", &accounts), "Default");
    }

    #[test]
    fn pooled_series_averages_past_and_steps_up_on_each_account_reset() {
        let now = 1_800_000_000_000.0;
        let min = 60_000.0;
        let hr = 3_600_000.0;
        let mut snaps = HashMap::new();
        snaps.insert("claude".into(), Snapshot { provider: "claude".into(), windows: vec![UsageWindow { kind: "session".into(), label: "5-hour".into(), used_percent: 100.0, resets_at: Some(now + 4.0 * hr) }], at: now, reset_credits: None });
        snaps.insert("claude:account-2".into(), Snapshot { provider: "claude:account-2".into(), windows: vec![UsageWindow { kind: "session".into(), label: "5-hour".into(), used_percent: 100.0, resets_at: Some(now + 10.0 * min) }], at: now, reset_credits: None });
        snaps.insert("claude:account-3".into(), Snapshot { provider: "claude:account-3".into(), windows: vec![UsageWindow { kind: "session".into(), label: "5-hour".into(), used_percent: 100.0, resets_at: Some(now + 50.0 * min) }], at: now, reset_credits: None });
        let keys = vec!["claude".into(), "claude:account-2".into(), "claude:account-3".into()];
        let s = pooled_series("session", &keys, &snaps, &HashMap::new(), now);
        assert_eq!(s.resets.len(), 3);
        assert_eq!(s.past.last().unwrap().1.round(), 0.0);
        // At +10m first account refills (+33%), at +50m second (+67%), at +4h all full (100%).
        assert_eq!(s.future.first().unwrap().1.round(), 0.0);
        let after_first = s.future.iter().rfind(|(t, _)| (*t - (now + 10.0 * min)).abs() < 1.0).unwrap().1;
        let after_second = s.future.iter().rfind(|(t, _)| (*t - (now + 50.0 * min)).abs() < 1.0).unwrap().1;
        let after_third = s.future.last().unwrap().1;
        assert!((after_first - 33.33).abs() < 0.5, "got {after_first}");
        assert!((after_second - 66.67).abs() < 0.5, "got {after_second}");
        assert!((after_third - 100.0).abs() < 0.1, "got {after_third}");
    }
}
