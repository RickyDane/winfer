//! Plan usage: live numbers, title-bar rings, and the agent tools `usage_status` and
//! `wait_for_usage_reset` (port of `features/usage/usage.ts`, `agentUsage.ts`, `accounts.ts`).

use super::*;
use serde::{Deserialize, Serialize};
use winfer_backend::{accounts, usage};

const WARN_AT: [f64; 2] = [20.0, 5.0];
const MARGIN_MS: f64 = 60_000.0;
const DEFAULT_WAKE: &str = "Your usage limit has reset. Call usage_status to check how much is left, then continue where you left off.";

fn now_ms() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_millis() as f64)
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct UsageWindow {
    pub kind: String,
    pub label: String,
    pub used_percent: f64,
    pub resets_at: Option<f64>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Snapshot {
    pub provider: String,
    pub windows: Vec<UsageWindow>,
    pub at: f64,
    /// Codex: rate limit resets the account can redeem.
    #[serde(default, rename = "resetCredits")]
    pub reset_credits: Option<i64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    pub id: String,
    pub provider: String,
    pub label: String,
    pub enabled: bool,
    #[serde(default)]
    pub signed_in: bool,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub exhausted_until: Option<f64>,
    #[serde(default)]
    pub dir: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Wake {
    #[serde(rename = "paneId")]
    pub pane_id: String,
    pub at: f64,
    pub message: String,
}

#[derive(Default)]
pub struct UsageStore {
    pub snapshots: HashMap<String, Snapshot>,
    pub samples: HashMap<String, Vec<(f64, f64)>>,
    pub accounts: Vec<Account>,
    pub wakes: Vec<Wake>,
    pub popover: Option<String>,
    pub warned: std::collections::HashSet<String>,
    pub claude_live_at: f64,
    /// Readings still on their way after a refresh (the Refresh button spins until they're in).
    pub refreshing: usize,
}

pub fn used_now(w: &UsageWindow, now: f64) -> f64 {
    if w.resets_at.is_some_and(|r| r <= now) { 0.0 } else { w.used_percent }
}

pub fn left_now(w: &UsageWindow, now: f64) -> f64 {
    (100.0 - used_now(w, now)).clamp(0.0, 100.0)
}

/// The window that runs out first; credits count only once every plan limit is out.
pub fn tightest(s: Option<&Snapshot>, now: f64) -> Option<&UsageWindow> {
    let s = s.filter(|s| !s.windows.is_empty())?;
    fn least<'a>(ws: impl Iterator<Item = &'a UsageWindow>, now: f64) -> Option<&'a UsageWindow> {
        ws.fold(None, |best: Option<&UsageWindow>, w| match best {
            Some(b) if left_now(w, now) >= left_now(b, now) => Some(b),
            _ => Some(w),
        })
    }
    let is_credit = |w: &&UsageWindow| w.kind == "month-credit";
    let Some(plan) = least(s.windows.iter().filter(|w| !is_credit(w)), now) else { return least(s.windows.iter(), now) };
    if left_now(plan, now) > 0.0 {
        return Some(plan);
    }
    s.windows.iter().filter(is_credit).fold(None, |best: Option<&UsageWindow>, w| match best {
        Some(b) if left_now(w, now) <= left_now(b, now) => Some(b),
        _ => Some(w),
    }).or(Some(plan))
}

pub fn duration(ms: f64) -> String {
    let minutes = ((ms / 60_000.0).round() as i64).max(1);
    let (days, hours, mins) = (minutes / 1440, (minutes % 1440) / 60, minutes % 60);
    match (days, hours, mins) {
        (d, 0, _) if d > 0 => format!("{d}d"),
        (d, h, _) if d > 0 => format!("{d}d {h}h"),
        (_, h, 0) if h > 0 => format!("{h}h"),
        (_, h, m) if h > 0 => format!("{h}h {m}m"),
        (_, _, m) => format!("{m}m"),
    }
}

fn window_key(provider: &str, w: &UsageWindow) -> String {
    format!("{provider}:{}:{}", w.kind, w.resets_at.map_or(String::new(), |r| r.to_string()))
}

/// When the window runs out at the pace measured in it, if that's before it resets.
pub fn runs_out_at(key: &str, w: &UsageWindow, samples: &HashMap<String, Vec<(f64, f64)>>, now: f64) -> Option<f64> {
    let resets = w.resets_at.filter(|r| *r > now)?;
    let points = samples.get(&window_key(key, w))?;
    let (&(t0, u0), &(t1, u1)) = (points.first()?, points.last()?);
    if points.len() < 2 || t1 - t0 < 15.0 * 60_000.0 || u1 <= u0 {
        return None;
    }
    let at = t1 + (100.0 - u1) / ((u1 - u0) / (t1 - t0));
    (at < resets).then_some(at)
}

/// "2026-10-15 14:00 (in 1h 48m)": local time, which is what the agent's machine shows too.
pub(super) fn at_text(ms: f64, now: f64) -> String {
    let local = local_time(ms);
    if ms > now { format!("{local} (in {})", duration(ms - now)) } else { format!("{local} (passed)") }
}

/// Development aid: while set, the demo numbers stay (no refresh from the backend).
#[cfg(debug_assertions)]
static DEMO: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// A short "when": "Mon 21:16" within the week, else the date ("Nov 30 21:16").
pub(super) fn short_when(ms: f64, now: f64) -> String {
    let secs = (ms / 1000.0) as libc_time;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&secs, &mut tm) };
    const DAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let time = format!("{:02}:{:02}", tm.tm_hour, tm.tm_min);
    if (ms - now).abs() < 6.0 * 24.0 * 3_600_000.0 { format!("{} {time}", DAYS[tm.tm_wday as usize % 7]) } else { format!("{} {} {time}", MONTHS[tm.tm_mon as usize % 12], tm.tm_mday) }
}

pub(super) fn local_time(ms: f64) -> String {
    let secs = (ms / 1000.0) as libc_time;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&secs, &mut tm) };
    format!("{:04}-{:02}-{:02} {:02}:{:02}", tm.tm_year + 1900, tm.tm_mon + 1, tm.tm_mday, tm.tm_hour, tm.tm_min)
}

#[allow(non_camel_case_types)]
type libc_time = libc::time_t;

fn round1(n: f64) -> f64 {
    (n * 10.0).round() / 10.0
}

pub fn default_account_id(provider: &str) -> String {
    format!("{provider}:default")
}

pub fn usage_key(id: &str) -> String {
    if id.ends_with(":default") { id.split(':').next().unwrap_or(id).to_string() } else { id.to_string() }
}

fn usable(a: &Account, now: f64) -> bool {
    a.enabled && a.signed_in && a.exhausted_until.is_none_or(|u| u > 0.0 && u <= now)
}

/// The account new agents start on: the first with usage left (`None` = the default sign-in).
pub fn start_account(provider: &str, list: &[Account], now: f64) -> Option<String> {
    if provider != "claude" && provider != "codex" {
        return None;
    }
    let first = list.iter().filter(|a| a.provider == provider).find(|a| usable(a, now))?;
    (!first.id.ends_with(":default")).then(|| first.id.clone())
}

impl Root {
    pub(super) fn start_usage(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_secs(3)).await;
            let mut tick = 0u64;
            loop {
                if this.update(cx, |this, cx| {
                    if tick % 2 == 0 {
                        this.refresh_accounts(cx);
                    }
                    this.deliver_wakes(cx);
                    this.refresh_usage(cx);
                }).is_err() {
                    break;
                }
                tick += 1;
                cx.background_executor().timer(Duration::from_secs(60)).await;
            }
        })
        .detach();
    }

    fn refresh_accounts(&mut self, cx: &mut Context<Self>) {
        #[cfg(debug_assertions)]
        if DEMO.load(std::sync::atomic::Ordering::Relaxed) {
            return;
        }
        cx.spawn(async move |this, cx| {
            let list = backend::background(|| serde_json::to_value(accounts::accounts_list()).unwrap_or_default()).await.unwrap_or_default();
            let parsed: Vec<Account> = serde_json::from_value(list).unwrap_or_default();
            let _ = this.update(cx, |this, cx| {
                this.usage.accounts = parsed;
                cx.notify();
            });
        })
        .detach();
    }

    fn refresh_usage(&mut self, cx: &mut Context<Self>) {
        #[cfg(debug_assertions)]
        if DEMO.load(std::sync::atomic::Ordering::Relaxed) {
            return;
        }
        self.refresh_accounts(cx);
        let accounts: Vec<Account> = self.usage.accounts.iter().filter(|a| a.enabled && a.signed_in).cloned().collect();
        let mut targets: Vec<(String, Option<String>)> = vec![("claude".to_string(), None), ("codex".to_string(), None)];
        targets.extend(accounts.iter().filter(|a| !a.id.ends_with(":default")).map(|a| (a.provider.clone(), Some(a.id.clone()))));
        for (provider, account) in targets {
            self.refresh_one(provider, account, cx);
        }
    }

    pub(super) fn refresh_one(&mut self, provider: String, account: Option<String>, cx: &mut Context<Self>) {
        self.usage.refreshing += 1;
        cx.spawn(async move |this, cx| {
            let (p, a) = (provider.clone(), account.clone());
            let value = tauri::async_runtime::spawn(async move {
                if p == "claude" {
                    usage::usage_claude(a).await.ok().and_then(|s| serde_json::to_value(s).ok())
                } else {
                    usage::usage_codex(a).await.and_then(|s| serde_json::to_value(s).ok())
                }
            })
            .await
            .ok()
            .flatten();
            let Some(value) = value else {
                let _ = this.update(cx, |this, cx| {
                    this.usage.refreshing = this.usage.refreshing.saturating_sub(1);
                    cx.notify();
                });
                return;
            };
            let key = account.as_deref().map_or_else(|| provider.clone(), usage_key);
            let live_claude = provider == "claude";
            let _ = this.update(cx, |this, cx| {
                this.usage.refreshing = this.usage.refreshing.saturating_sub(1);
                if live_claude {
                    this.usage.claude_live_at = now_ms();
                }
                if let Ok(mut snapshot) = serde_json::from_value::<Snapshot>(value) {
                    snapshot.provider = key;
                    this.record_usage(snapshot, cx);
                }
            });
        })
        .detach();
    }

    fn record_usage(&mut self, snapshot: Snapshot, cx: &mut Context<Self>) {
        let now = now_ms();
        let key = snapshot.provider.clone();
        let mut exhausted = Vec::new();
        for w in &snapshot.windows {
            let points = self.usage.samples.entry(window_key(&key, w)).or_default();
            if points.last().is_none_or(|(t, u)| *u != w.used_percent || now - t > 60_000.0) {
                points.push((now, w.used_percent));
                if points.len() > 240 {
                    points.remove(0);
                }
            }
            let left = left_now(w, now);
            if left <= 0.0 {
                exhausted.push(w.resets_at);
            }
            for threshold in WARN_AT {
                let id = format!("{}:{}", window_key(&key, w), threshold);
                if left <= threshold && self.usage.warned.insert(id) {
                    let provider = key.split(':').next().unwrap_or(&key).to_string();
                    let name = self.state.read(cx).provider_name(&provider);
                    let reset = w.resets_at.filter(|r| *r > now).map_or(String::new(), |r| format!(" (resets in {})", duration(r - now)));
                    if left <= 0.0 {
                        // Out, with another account to move to: the failover says what happens instead.
                        if self.usage.accounts.iter().filter(|a| a.provider == provider && a.enabled).count() > 1 {
                            break;
                        }
                        self.limit_reached(&provider, format!("{name}: {} limit reached{reset}", w.label.to_lowercase()), snapshot.reset_credits.unwrap_or(0), cx);
                    } else {
                        self.toast(format!("{name}: {:.0}% of the {} limit left{reset}", left, w.label.to_lowercase()), cx);
                    }
                    break;
                }
            }
        }
        self.usage.snapshots.insert(key.clone(), snapshot);
        if let Some(reset) = exhausted.into_iter().next() {
            self.on_account_exhausted(&key, reset, cx);
        }
        cx.notify();
    }

    /// A `usage://report` event, from the status line or an agent's own numbers.
    pub(super) fn on_usage_report(&mut self, payload: &Value, cx: &mut Context<Self>) {
        let provider = payload["provider"].as_str().unwrap_or_default().to_string();
        let account = payload["paneId"].as_str().and_then(|id| self.state.read(cx).pane(id)).and_then(|p| p.account_id.clone()).filter(|a| a.split(':').next() == Some(&provider));
        let key = account.as_deref().map_or_else(|| provider.clone(), usage_key);
        let windows: Vec<UsageWindow> = serde_json::from_value(payload["report"]["windows"].clone()).unwrap_or_default();
        // Status-line limits may be cached by Claude Code: a recent account-wide read wins.
        if !windows.is_empty() && !(key == "claude" && now_ms() - self.usage.claude_live_at < 5.0 * 60_000.0) {
            self.record_usage(Snapshot { provider: key, windows, at: now_ms(), reset_credits: None }, cx);
        }
    }

    fn deliver_wakes(&mut self, cx: &mut Context<Self>) {
        let now = now_ms();
        let due: Vec<Wake> = self.usage.wakes.iter().filter(|w| w.at <= now || self.state.read(cx).pane(&w.pane_id).is_none()).cloned().collect();
        if due.is_empty() {
            return;
        }
        self.usage.wakes.retain(|w| !due.iter().any(|d| d.pane_id == w.pane_id));
        self.save_wakes(cx);
        for wake in due {
            let Some(workspace) = self.state.read(cx).pane(&wake.pane_id).map(|p| p.workspace_id.clone()) else { continue };
            self.type_into(&wake.pane_id, &wake.message, false, cx);
            if let Some(session) = SESSIONS.get(&wake.pane_id) {
                session.send("\r");
            }
            self.update_state(cx, |s| s.log_activity(&workspace, "Woke an agent: its usage limit has reset".to_string(), None));
        }
    }

    fn save_wakes(&self, cx: &mut Context<Self>) {
        let value = serde_json::to_value(&self.usage.wakes).unwrap_or_default();
        self.update_state(cx, |s| {
            s.rest.insert("usageWakes".to_string(), value);
        });
    }

    pub(super) fn load_wakes(&mut self, cx: &App) {
        if let Some(v) = self.state.read(cx).rest.get("usageWakes") {
            self.usage.wakes = serde_json::from_value(v.clone()).unwrap_or_default();
        }
    }

    /// Whose limits a pane's agent uses, and where they're kept.
    fn limits_of(&self, pane: &Pane) -> (String, String) {
        if pane.provider == "claude" || pane.provider == "codex" {
            let id = pane.account_id.clone().unwrap_or_else(|| default_account_id(&pane.provider));
            return (pane.provider.clone(), usage_key(&id));
        }
        (pane.provider.clone(), pane.provider.clone())
    }

    fn name_of(&self, provider: &str, cx: &App) -> String {
        self.state.read(cx).provider_name(provider)
    }

    fn accounts_report(&self, provider: &str, key: &str, now: f64, cx: &App) -> Option<String> {
        let list: Vec<&Account> = self.usage.accounts.iter().filter(|a| a.provider == provider && a.enabled).collect();
        if list.len() < 2 {
            return None;
        }
        let rows: Vec<String> = list
            .iter()
            .enumerate()
            .map(|(i, a)| {
                let w = tightest(self.usage.snapshots.get(&usage_key(&a.id)), now);
                let state = if a.exhausted_until.is_some() {
                    "out of usage".to_string()
                } else if !a.signed_in {
                    "not signed in".to_string()
                } else {
                    w.map_or("no reading yet".to_string(), |w| format!("{}% of {} left", round1(left_now(w, now)), w.label.to_lowercase()))
                };
                format!("{}. {}{}: {state}", i + 1, a.label, if usage_key(&a.id) == key { " (this one)" } else { "" })
            })
            .collect();
        Some(format!("{} has {} accounts, used in this order; when one runs out of usage winfer moves you to the next and resumes your session there:\n{}", self.name_of(provider, cx), list.len(), rows.join("\n")))
    }

    pub(super) fn usage_report(&self, provider: &str, key: &str, cx: &App) -> String {
        let now = now_ms();
        let name = self.name_of(provider, cx);
        let Some(snapshot) = self.usage.snapshots.get(key).filter(|s| !s.windows.is_empty()) else {
            return match provider {
                "claude" | "codex" => format!("winfer has no usage numbers for {name} yet. They are reported after the agent's first answer in winfer; ask again after your next reply."),
                _ => format!("winfer can't read usage limits for {name}."),
            };
        };
        let age = now - snapshot.at;
        let measured = if age < 60_000.0 { "just now".to_string() } else { format!("{} ago", duration(age)) };
        let mut lines = vec![format!("{name} plan usage (measured {measured}, current time {}):", local_time(now))];
        for w in &snapshot.windows {
            let mut parts = vec![format!("- {}: {}% used, {}% left", w.label, round1(used_now(w, now)), round1(left_now(w, now)))];
            match w.resets_at {
                Some(r) if r <= now => parts.push("(this window has reset since it was measured)".to_string()),
                Some(r) => parts.push(format!("resets {}", at_text(r, now))),
                None => {}
            }
            if let Some(out) = runs_out_at(key, w, &self.usage.samples, now) {
                parts.push(format!("at the current pace it runs out {}, before it resets", at_text(out, now)));
            }
            lines.push(parts.join(", "));
        }
        if let Some(t) = tightest(Some(snapshot), now) {
            lines.push(format!("Least left: {}, {}%.", t.label, round1(left_now(t, now))));
        }
        if age > 30.0 * 60_000.0 {
            lines.push("These numbers may be out of date: no agent has reported usage for a while.".to_string());
        }
        if let Some(a) = self.accounts_report(provider, key, now, cx) {
            lines.push(a);
        }
        lines.join("\n")
    }

    /// `usage_status`.
    pub(super) fn tool_usage_status(&mut self, me: &Pane, cx: &App) -> String {
        let (provider, key) = self.limits_of(me);
        let mut lines = vec![self.usage_report(&provider, &key, cx)];
        if let Some(w) = self.usage.wakes.iter().find(|w| w.pane_id == me.id) {
            lines.push(format!("You are paused: winfer will wake you at {}.", local_time(w.at)));
        }
        lines.join("\n")
    }

    /// `wait_for_usage_reset`.
    pub(super) fn tool_wait(&mut self, me: &Pane, a: &Value, cx: &mut Context<Self>) -> Result<String, String> {
        if a["cancel"] == true {
            let before = self.usage.wakes.len();
            self.usage.wakes.retain(|w| w.pane_id != me.id);
            self.save_wakes(cx);
            return Ok(if self.usage.wakes.len() != before { "Cancelled: winfer won't wake you.".to_string() } else { "You had no wake-up scheduled.".to_string() });
        }
        let (provider, key) = self.limits_of(me);
        let now = now_ms();
        let minutes = a["minutes"].as_f64();
        if minutes.is_none() {
            let other = self.usage.accounts.iter().find(|x| x.provider == provider && usage_key(&x.id) != key && usable(x, now));
            if let Some(other) = other {
                return Ok(format!("No need to wait: your account still has another one to fall back on (\"{}\"). When this account runs out of usage, winfer moves you to it and resumes your session. Keep working.", other.label));
            }
        }
        let message = a["message"].as_str().map(str::trim).filter(|m| !m.is_empty()).unwrap_or(DEFAULT_WAKE).to_string();
        let (due, reason) = match minutes {
            Some(m) if !m.is_finite() || m <= 0.0 => return Err("minutes must be a positive number".to_string()),
            Some(m) => (now + m * 60_000.0, format!("after {}", duration(m * 60_000.0))),
            None => {
                let name = self.name_of(&provider, cx);
                let pending: Vec<&UsageWindow> = self.usage.snapshots.get(&key).map_or_else(Vec::new, |s| s.windows.iter().filter(|w| w.resets_at.is_some_and(|r| r > now)).collect());
                if pending.is_empty() {
                    return Err(format!("winfer doesn't know when {name}'s limits reset (no usage numbers yet, or they have all reset already). Call usage_status, or pass minutes to wait a set time."));
                }
                let below = a["below"].as_f64().unwrap_or(20.0);
                let mut wait_for: Vec<&UsageWindow> = pending.iter().copied().filter(|w| left_now(w, now) < below).collect();
                if wait_for.is_empty() {
                    let least = pending.iter().copied().min_by(|x, y| left_now(x, now).total_cmp(&left_now(y, now))).unwrap();
                    wait_for.push(least);
                }
                let last = wait_for.iter().map(|w| w.resets_at.unwrap()).fold(0.0, f64::max);
                let labels: Vec<String> = wait_for.iter().map(|w| w.label.to_lowercase()).collect();
                (last + MARGIN_MS, format!("when the {} limit{}", labels.join(" and "), if labels.len() > 1 { "s reset" } else { " resets" }))
            }
        };
        self.usage.wakes.retain(|w| w.pane_id != me.id);
        self.usage.wakes.push(Wake { pane_id: me.id.clone(), at: due, message: message.clone() });
        self.save_wakes(cx);
        let name = self.name_of(&provider, cx);
        let (ws, text) = (me.workspace_id.clone(), format!("{name} paused until {}", at_text(due, now)));
        let provider_id = provider.clone();
        self.update_state(cx, |s| s.log_activity(&ws, text, Some(provider_id)));
        Ok(format!("Paused. winfer will type this into your pane {reason}, at {}: “{message}”\nStop working now: finish your reply without starting anything new, and winfer will wake you.", at_text(due, now)))
    }

    pub(super) fn render_usage_meter(&self, c: Colors, cx: &mut Context<Self>) -> Div {
        let now = now_ms();
        let mut row = div().flex().items_center().gap_1();
        for provider in ["claude", "codex"] {
            let snapshot = self.usage.snapshots.get(provider);
            let Some(w) = tightest(snapshot, now) else { continue };
            let left = left_now(w, now);
            let color = if left < 10.0 { c.danger } else if left < 30.0 { c.warning } else { c.success };
            let toggle = provider.to_string();
            row = row.child(
                div().id(SharedString::from(format!("usage-{provider}"))).flex().items_center().gap_1p5().rounded_md().px_1p5().py_1().cursor_pointer().hover(move |d| d.bg(c.hover))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.usage.popover = if this.usage.popover.as_ref() == Some(&toggle) { None } else { Some(toggle.clone()) };
                        cx.notify();
                    }))
                    .child(usage_ring(provider, left, color, 22.0, c))
                    .child(div().text_size(px(11.0)).text_color(c.muted).child(format!("{left:.0}%"))),
            );
        }
        row
    }

    pub(super) fn render_usage_popover(&self, c: &Colors, cx: &mut Context<Self>) -> Option<AnyElement> {
        let provider = self.usage.popover.clone()?;
        let c = *c;
        let now = now_ms();
        let name = self.name_of(&provider, cx);
        let snapshot = self.usage.snapshots.get(&provider)?;
        let mut list = div().flex().flex_col().gap_2();
        for w in &snapshot.windows {
            let left = left_now(w, now);
            let color = if left < 10.0 { c.danger } else if left < 30.0 { c.warning } else { c.success };
            let reset = w.resets_at.filter(|r| *r > now).map(|r| format!("resets {}", at_text(r, now)));
            // Only warn when the pace runs it out before it resets.
            let out = runs_out_at(&provider, w, &self.usage.samples, now).filter(|o| w.resets_at.is_none_or(|r| *o < r)).map(|o| format!("Runs out in {} at this pace", duration(o - now)));
            list = list.child(
                div().flex().flex_col().gap_1p5().rounded_lg().border_1().border_color(c.border).bg(c.panel).p_3()
                    .child(div().flex().items_baseline().gap_2()
                        .child(div().flex_1().text_size(px(12.0)).text_color(c.muted).child(w.label.clone()))
                        .child(div().text_size(px(18.0)).font_weight(FontWeight::SEMIBOLD).text_color(color).child(format!("{left:.0}%")))
                        .child(div().text_size(px(11.0)).text_color(c.subtle).child("left")))
                    .child(div().h(px(6.0)).w_full().rounded_full().bg(c.active).child(div().h_full().rounded_full().bg(color).w(relative((left / 100.0).clamp(0.0, 1.0) as f32))))
                    .when_some(reset, |d, t| d.child(div().text_size(px(11.0)).text_color(c.subtle).child(t)))
                    .when_some(out, |d, t| d.child(div().flex().items_center().gap_1p5().mt_0p5().rounded_md().bg(Hsla { a: 0.1, ..c.warning }).px_2().py_1().text_size(px(11.0)).text_color(c.warning)
                        .child(Icon::new(IconName::TriangleAlert).size(px(11.0))).child(t))),
            );
        }
        let accounts: Vec<&Account> = self.usage.accounts.iter().filter(|a| a.provider == provider).collect();
        if accounts.len() > 1 {
            let mut rows = div().flex().flex_col().gap_2().pt_1()
                .child(div().text_size(px(11.0)).font_weight(FontWeight::SEMIBOLD).text_color(c.subtle).child("ACCOUNTS"));
            for a in accounts {
                let snapshot = self.usage.snapshots.get(&usage_key(&a.id));
                let left = tightest(snapshot, now).map(|w| left_now(w, now));
                let color = left.map_or(c.subtle, |l| if l < 10.0 { c.danger } else if l < 30.0 { c.warning } else { c.success });
                let mut card = div().flex().flex_col().gap_1().rounded_lg().border_1().border_color(c.border).bg(c.panel).p_2p5()
                    .child(div().flex().items_center().gap_2()
                        .child(div().size(px(6.0)).flex_shrink_0().rounded_full().bg(color))
                        .child(div().flex().flex_1().min_w_0().flex_col()
                            .child(div().truncate().text_size(px(12.5)).font_weight(FontWeight::MEDIUM).text_color(c.fg).child(a.label.clone()))
                            .when_some(a.email.clone(), |d, e| d.child(div().truncate().text_size(px(11.0)).text_color(c.subtle).child(e))))
                        .child(div().flex_shrink_0().text_size(px(11.0)).text_color(c.subtle).child(left.map_or("no reading".into(), |l| format!("{l:.0}% left")))));
                // Every window of the account (5-hour, weekly, …) with when it resets.
                for w in snapshot.map_or(&[][..], |s| s.windows.as_slice()) {
                    let wl = left_now(w, now);
                    let wc = if wl < 10.0 { c.danger } else if wl < 30.0 { c.warning } else { c.success };
                    let reset = match w.resets_at {
                        Some(r) if r > now => format!("resets {}", at_text(r, now)),
                        Some(_) => "reset — full again".to_string(),
                        None => String::new(),
                    };
                    card = card.child(div().flex().flex_col().pl(px(14.0)).text_size(px(11.0))
                        .child(div().flex().items_baseline().gap_2()
                            .child(div().w(px(46.0)).flex_shrink_0().truncate().text_color(c.muted).child(w.label.clone()))
                            .child(div().flex_1().h(px(4.0)).rounded_full().bg(c.active).child(div().h_full().rounded_full().bg(wc).w(relative((wl / 100.0).clamp(0.0, 1.0) as f32))))
                            .child(div().w(px(62.0)).flex_shrink_0().text_right().font_family(crate::fonts::mono(cx)).text_color(wc).child(format!("{wl:.0}% left"))))
                        .when(!reset.is_empty(), |d| d.child(div().pl(px(54.0)).text_size(px(10.5)).text_color(c.subtle).child(reset))));
                }
                rows = rows.child(card);
            }
            list = list.child(rows);
        }
        let busy = self.usage.refreshing > 0;
        let refresh = div().id("usage-refresh").flex().items_center().gap_1p5().h(px(26.0)).px_2().rounded_md().text_size(px(11.5)).text_color(c.muted)
            .when(!busy, |d| d.cursor_pointer().hover(move |d| d.bg(c.hover).text_color(c.fg)).on_click(cx.listener(|this, _, _, cx| this.refresh_usage(cx))))
            .when(busy, |d| d.opacity(0.6))
            .child(Icon::new(IconName::RefreshCw).size(px(12.0)).with_animation("usage-refresh-spin", Animation::new(Duration::from_millis(900)).repeat(), move |i, t| if busy { i.transform(Transformation::rotate(percentage(t))) } else { i }))
            .child(if busy { "Refreshing…" } else { "Refresh" });
        let settings = div().id("usage-settings").flex().items_center().gap_1p5().pt_2().border_t_1().border_color(c.border).text_size(px(12.0)).text_color(c.accent).cursor_pointer()
            .on_click(cx.listener(|this, _, window, cx| {
                this.usage.popover = None;
                this.execute(super::commands::Command::Settings(Some("usage".into())), window, cx);
            }))
            .child("Usage & limits settings").child(Icon::new(IconName::ArrowRight).size(px(12.0)));
        Some(
            div().id("usage-backdrop").absolute().size_full().occlude()
                .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                    this.usage.popover = None;
                    cx.notify();
                }))
                .child(
                    div().id("usage-popover").absolute().top(px(40.0)).right(px(120.0)).w(px(340.0)).flex().flex_col().gap_3().rounded(px(12.0)).border_1().border_color(c.border_strong).bg(c.raised).shadow_2xl().p_4()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(div().flex().items_center().gap_2()
                            .child(div().size(px(26.0)).flex().items_center().justify_center().rounded_lg().bg(c.hover).child(crate::ui::code_pickers::mark(c, &provider, 14.0, 1.0)))
                            .child(div().flex_1().text_size(px(14.0)).font_weight(FontWeight::SEMIBOLD).text_color(c.fg).child(format!("{name} usage")))
                            .child(refresh))
                        .child(list)
                        .child(settings),
                )
                .into_any_element(),
        )
    }

    /// Development aid: usage numbers to look at, and the popover open.
    #[cfg(debug_assertions)]
    pub(super) fn demo_usage(&mut self, cx: &mut Context<Self>) {
        DEMO.store(true, std::sync::atomic::Ordering::Relaxed);
        let now = now_ms();
        let window = |kind: &str, label: &str, used: f64, hours: f64| UsageWindow { kind: kind.into(), label: label.into(), used_percent: used, resets_at: Some(now + hours * 3_600_000.0) };
        self.usage.snapshots.insert("claude".into(), Snapshot { provider: "claude".into(), windows: vec![window("session", "5-hour", 78.0, 2.5), window("week", "Weekly", 36.0, 60.0)], at: now, reset_credits: None });
        self.usage.snapshots.insert("codex".into(), Snapshot { provider: "codex".into(), windows: vec![window("session", "5-hour", 22.0, 3.0), window("week", "Weekly", 91.0, 30.0)], at: now, reset_credits: None });
        self.usage.samples.insert(format!("claude:session:{}", (now + 2.5 * 3_600_000.0) as i64), vec![(now - 3_600_000.0, 50.0), (now, 78.0)]);
        // Three Claude Code accounts with their own readings.
        let account = |id: &str, label: &str, email: Option<&str>| Account { id: id.into(), provider: "claude".into(), label: label.into(), enabled: true, signed_in: true, email: email.map(str::to_string), exhausted_until: None, dir: String::new() };
        self.usage.accounts = vec![account("a1", "Default", Some("dane@example.com")), account("a2", "Account 2", Some("work@example.org")), account("a3", "Account 3", None)];
        for (id, session, week) in [("a1", 100.0, 41.0), ("a2", 99.0, 12.0), ("a3", 35.0, 60.0)] {
            self.usage.snapshots.insert(usage_key(id), Snapshot { provider: "claude".into(), windows: vec![window("session", "5-hour", session, 6.3), window("week", "Weekly", week, 70.0)], at: now, reset_credits: None });
        }
        self.usage.popover = Some("claude".into());
        cx.notify();
    }

    /// A pane's account for its next start: the first with usage left.
    pub(super) fn account_for_start(&self, provider: &str) -> Option<String> {
        start_account(provider, &self.usage.accounts, now_ms())
    }
}

/// A ring that drains as the limit is used up, with the provider's mark inside.
pub(super) fn usage_ring(provider: &str, left: f64, color: Hsla, size: f32, c: Colors) -> impl IntoElement {
    let fraction = (left / 100.0).clamp(0.0, 1.0) as f32;
    let stroke = (size / 9.0).max(2.0);
    let ring = canvas(|_, _, _| {}, move |bounds, _, window, _| {
        let center = bounds.center();
        let radius = (bounds.size.width.min(bounds.size.height) - px(stroke)) / 2.0;
        // The track (a full circle), then the arc from 12 o'clock, clockwise.
        let mut arc = |from: f32, to: f32, color: Hsla| {
            let steps = (((to - from).abs() * 64.0).ceil() as usize).max(2);
            let mut path = PathBuilder::stroke(px(stroke));
            for i in 0..=steps {
                let t = from + (to - from) * i as f32 / steps as f32;
                let angle = t * std::f32::consts::TAU - std::f32::consts::FRAC_PI_2;
                let p = point(center.x + radius * angle.cos(), center.y + radius * angle.sin());
                if i == 0 { path.move_to(p) } else { path.line_to(p) }
            }
            if let Ok(path) = path.build() {
                window.paint_path(path, color);
            }
        };
        arc(0.0, 1.0, c.border_strong);
        if fraction > 0.0 {
            arc(0.0, fraction, color);
        }
    }).absolute().size_full();
    div().relative().size(px(size)).flex_shrink_0().flex().items_center().justify_center()
        .child(ring)
        .child(crate::ui::code_pickers::mark(c, provider, size * 0.45, 1.0))
}

#[cfg(test)]
mod tests {
    use super::{Account, Snapshot, UsageWindow, duration, left_now, runs_out_at, start_account, tightest, usage_key};
    use std::collections::HashMap;

    fn window(kind: &str, used: f64, resets: Option<f64>) -> UsageWindow {
        UsageWindow { kind: kind.into(), label: kind.into(), used_percent: used, resets_at: resets }
    }

    #[test]
    fn durations() {
        assert_eq!(duration(60_000.0), "1m");
        assert_eq!(duration(108.0 * 60_000.0), "1h 48m");
        assert_eq!(duration(3.0 * 1440.0 * 60_000.0), "3d");
    }

    #[test]
    fn reset_windows_start_over_and_tightest_wins() {
        let s = Snapshot { provider: "claude".into(), windows: vec![window("session", 90.0, Some(1000.0)), window("week", 50.0, None)], at: 0.0, reset_credits: None };
        assert_eq!(left_now(&s.windows[0], 2000.0), 100.0);
        assert_eq!(tightest(Some(&s), 2000.0).unwrap().kind, "week");
        assert_eq!(tightest(Some(&s), 500.0).unwrap().kind, "session");
    }

    #[test]
    fn credits_only_once_the_plan_is_out() {
        let s = Snapshot { provider: "x".into(), windows: vec![window("week", 100.0, None), window("month-credit", 20.0, None)], at: 0.0, reset_credits: None };
        assert_eq!(tightest(Some(&s), 0.0).unwrap().kind, "month-credit");
    }

    #[test]
    fn projection_needs_a_real_trend() {
        let mut samples = HashMap::new();
        let w = window("session", 50.0, Some(10_000_000.0));
        samples.insert("claude:session:10000000".to_string(), vec![(0.0, 0.0), (1_800_000.0, 50.0)]);
        assert_eq!(runs_out_at("claude", &w, &samples, 2_000_000.0), Some(3_600_000.0));
    }

    #[test]
    fn accounts_in_order() {
        let list = vec![
            Account { id: "claude:default".into(), provider: "claude".into(), label: "d".into(), enabled: true, signed_in: true, email: None, exhausted_until: Some(0.0), dir: String::new() },
            Account { id: "claude:a2".into(), provider: "claude".into(), label: "b".into(), enabled: true, signed_in: true, email: None, exhausted_until: None, dir: String::new() },
        ];
        assert_eq!(start_account("claude", &list, 5.0).as_deref(), Some("claude:a2"));
        assert_eq!(usage_key("claude:default"), "claude");
    }
}
