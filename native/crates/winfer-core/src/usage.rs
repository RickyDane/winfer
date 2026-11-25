//! Plan usage without a window: limit snapshots and their samples, polling backoff, pooled
//! accounts, token totals, and the `usage.get` answer a phone or paired desktop gets. Shared by
//! the desktop's Usage page and the server (`winferd`).

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;

pub fn now_ms() -> f64 {
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UsageErrorKind {
    RateLimited,
    AuthExpired,
    Transient,
}

#[derive(Default, Clone, Debug)]
pub struct PollState {
    pub last_attempt_at: f64,
    pub last_ok_at: f64,
    pub next_allowed_at: f64,
    pub rate_limit_streak: u32,
    pub last_error: Option<String>,
    pub error_kind: Option<UsageErrorKind>,
}

pub const CLAUDE_POLL_INTERVAL_MS: f64 = 5.0 * 60_000.0;
pub const AGY_POLL_INTERVAL_MS: f64 = 5.0 * 60_000.0;
pub const CODEX_POLL_INTERVAL_MS: f64 = 60_000.0;
pub const RATE_LIMIT_BASE_MS: f64 = 2.0 * 60_000.0;
pub const RATE_LIMIT_MAX_MS: f64 = 15.0 * 60_000.0;
pub const AUTH_RETRY_MS: f64 = 5.0 * 60_000.0;
pub const TRANSIENT_NO_DATA_RETRY_MS: f64 = 60_000.0;
pub const TRANSIENT_WITH_DATA_RETRY_MS: f64 = 3.0 * 60_000.0;
pub const CLAUDE_STAGGER_MS: u64 = 350;


#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct UsageCache {
    #[serde(default)]
    pub snapshots: HashMap<String, Snapshot>,
    #[serde(default)]
    pub samples: HashMap<String, Vec<(f64, f64)>>,
}

pub fn usage_cache_path() -> Option<std::path::PathBuf> {
    use tauri::Manager;
    crate::backend::app().path().app_data_dir().ok().map(|d| d.join("usage-cache.json"))
}

pub fn prune_samples(samples: &mut HashMap<String, Vec<(f64, f64)>>, now: f64) {
    samples.retain(|key, pts| {
        if pts.is_empty() {
            return false;
        }
        let expired = key.rsplit(':').next().and_then(|s| s.parse::<f64>().ok()).is_some_and(|r| r > 0.0 && r < now);
        !expired
    });
}

pub fn poll_interval_ms(provider: &str) -> f64 {
    match provider {
        "claude" => CLAUDE_POLL_INTERVAL_MS,
        "agy" => AGY_POLL_INTERVAL_MS,
        _ => CODEX_POLL_INTERVAL_MS,
    }
}

pub fn should_auto_poll(provider: &str, state: Option<&PollState>, now: f64) -> bool {
    let Some(state) = state else { return true };
    if now < state.next_allowed_at {
        return false;
    }
    now - state.last_ok_at >= poll_interval_ms(provider)
}

pub fn apply_poll_error(state: &mut PollState, kind: UsageErrorKind, message: String, retry_after_secs: Option<u64>, has_snapshot: bool, now: f64) {
    state.last_attempt_at = now;
    state.last_error = Some(message);
    state.error_kind = Some(kind);
    let wait_ms = match kind {
        UsageErrorKind::RateLimited => {
            state.rate_limit_streak = state.rate_limit_streak.saturating_add(1).min(6);
            let exp_ms = RATE_LIMIT_BASE_MS * (1u64 << (state.rate_limit_streak - 1)) as f64;
            let header_ms = retry_after_secs.map_or(0.0, |s| s as f64 * 1000.0);
            exp_ms.max(header_ms).min(RATE_LIMIT_MAX_MS)
        }
        UsageErrorKind::AuthExpired => {
            state.rate_limit_streak = 0;
            AUTH_RETRY_MS
        }
        UsageErrorKind::Transient => {
            state.rate_limit_streak = 0;
            if has_snapshot { TRANSIENT_WITH_DATA_RETRY_MS } else { TRANSIENT_NO_DATA_RETRY_MS }
        }
    };
    state.next_allowed_at = now + wait_ms;
}

pub fn apply_poll_success(state: &mut PollState, provider: &str, now: f64) {
    state.last_attempt_at = now;
    state.last_ok_at = now;
    state.next_allowed_at = now + poll_interval_ms(provider);
    state.rate_limit_streak = 0;
    state.last_error = None;
    state.error_kind = None;
}

pub fn error_short_label(kind: UsageErrorKind) -> &'static str {
    match kind {
        UsageErrorKind::RateLimited => "rate limited",
        UsageErrorKind::AuthExpired => "sign-in expired",
        UsageErrorKind::Transient => "refresh failed",
    }
}

pub fn used_now(w: &UsageWindow, now: f64) -> f64 {
    if w.resets_at.is_some_and(|r| r <= now) { 0.0 } else { w.used_percent }
}

pub fn left_now(w: &UsageWindow, now: f64) -> f64 {
    (100.0 - used_now(w, now)).clamp(0.0, 100.0)
}


pub fn least_window<'a>(ws: impl Iterator<Item = &'a UsageWindow>, now: f64) -> Option<&'a UsageWindow> {
    ws.fold(None, |best: Option<&UsageWindow>, w| match best {
        Some(b) if left_now(w, now) >= left_now(b, now) => Some(b),
        _ => Some(w),
    })
}

pub fn richest_credit<'a>(s: &'a Snapshot, now: f64) -> Option<&'a UsageWindow> {
    s.windows.iter().filter(|w| w.kind == "month-credit").fold(None, |best: Option<&UsageWindow>, w| match best {
        Some(b) if left_now(w, now) <= left_now(b, now) => Some(b),
        _ => Some(w),
    })
}

/// The window that runs out first; credits count only once every plan limit is out.
pub fn tightest(s: Option<&Snapshot>, now: f64) -> Option<&UsageWindow> {
    let s = s.filter(|s| !s.windows.is_empty())?;
    let is_credit = |w: &&UsageWindow| w.kind == "month-credit";
    let Some(plan) = least_window(s.windows.iter().filter(|w| !is_credit(w)), now) else { return least_window(s.windows.iter(), now) };
    if left_now(plan, now) > 0.0 {
        return Some(plan);
    }
    richest_credit(s, now).or(Some(plan))
}

/// Overall percentage left and next reset time across independent model groups (for rings and
/// provider-level summaries). Within each independent model group the tightest window governs the
/// group, and separate groups are averaged rather than letting one exhausted group pull the whole
/// provider to 0%. Credits count only once every plan limit is out.
pub fn overall_summary(s: Option<&Snapshot>, now: f64) -> Option<(f64, Option<f64>)> {
    let s = s.filter(|s| !s.windows.is_empty())?;
    let plan: Vec<&UsageWindow> = s.windows.iter().filter(|w| w.kind != "month-credit").collect();
    if plan.is_empty() {
        return least_window(s.windows.iter(), now).map(|w| (left_now(w, now), w.resets_at));
    }
    let mut named: Vec<(&str, Vec<&UsageWindow>)> = Vec::new();
    let mut ungrouped: Vec<&UsageWindow> = Vec::new();
    for &w in &plan {
        if let Some(group) = split_group(&w.label).1 {
            if let Some((_, ws)) = named.iter_mut().find(|(g, _)| *g == group) {
                ws.push(w);
            } else {
                named.push((group, vec![w]));
            }
        } else {
            ungrouped.push(w);
        }
    }
    let pools: Vec<Vec<&UsageWindow>> = if ungrouped.is_empty() {
        named.into_iter().map(|(_, ws)| ws).collect()
    } else if ungrouped.iter().any(|w| w.kind == "gemini") {
        let (gemini, other): (Vec<&UsageWindow>, Vec<&UsageWindow>) = ungrouped.into_iter().partition(|w| w.kind == "gemini");
        let mut pools: Vec<Vec<&UsageWindow>> = named.into_iter().map(|(_, ws)| ws).collect();
        if !other.is_empty() {
            pools.push(other);
        }
        pools.push(gemini);
        pools
    } else {
        vec![plan]
    };
    let group_tightests: Vec<&UsageWindow> = pools.into_iter().filter_map(|ws| least_window(ws.into_iter(), now)).collect();
    let plan_left = group_tightests.iter().map(|w| left_now(w, now)).sum::<f64>() / group_tightests.len() as f64;
    let plan_reset = if group_tightests.len() == 1 {
        group_tightests[0].resets_at
    } else {
        group_tightests.iter().filter_map(|w| w.resets_at).filter(|r| *r > now).reduce(f64::min)
    };
    if plan_left > 0.0 {
        return Some((plan_left, plan_reset));
    }
    Some(richest_credit(s, now).map_or((0.0, plan_reset), |w| (left_now(w, now), w.resets_at)))
}

pub fn overall_left(s: Option<&Snapshot>, now: f64) -> Option<f64> {
    overall_summary(s, now).map(|(left, _)| left)
}

/// Splits `"5-hour (Gemini Models)"` into `("5-hour", Some("Gemini Models"))`,
/// or `"5-hour"` into `("5-hour", None)`.
pub fn split_group(label: &str) -> (&str, Option<&str>) {
    if let Some(rest) = label.strip_suffix(')')
        && let Some((base, group)) = rest.rsplit_once(" (")
        && !base.is_empty()
        && !group.is_empty()
    {
        return (base, Some(group));
    }
    (label, None)
}

pub fn is_primary_window(kind: &str) -> bool {
    kind == "session" || kind == "week" || kind.starts_with("session-") || kind.starts_with("week-")
}

pub fn window_order(kind: &str) -> u8 {
    if kind == "session" || kind.starts_with("session-") {
        0
    } else if kind == "week" || kind.starts_with("week-") {
        1
    } else {
        2
    }
}

/// Groups paired 5-hour and weekly limits (overall or per model group) so they render together
/// inside one visual card. Standalone windows (e.g. `Weekly (Opus)` or credits) keep their own card.
pub fn group_windows<'a>(windows: &'a [UsageWindow]) -> Vec<(Option<&'a str>, Vec<(&'a str, &'a UsageWindow)>)> {
    let mut groups: Vec<(Option<&'a str>, bool, Vec<(&'a str, &'a UsageWindow)>)> = Vec::new();
    for w in windows {
        if is_primary_window(&w.kind) {
            let (short, group) = split_group(&w.label);
            if let Some(existing) = groups.iter_mut().find(|(g, primary, _)| *primary && *g == group) {
                existing.2.push((short, w));
            } else {
                groups.push((group, true, vec![(short, w)]));
            }
        } else {
            groups.push((None, false, vec![(&w.label, w)]));
        }
    }
    groups
        .into_iter()
        .map(|(title, _, mut items)| {
            if items.len() > 1 {
                items.sort_by_key(|(_, w)| window_order(&w.kind));
                (title, items)
            } else {
                let (_, w) = items[0];
                (None, vec![(w.label.as_str(), w)])
            }
        })
        .collect()
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

pub fn window_key(provider: &str, w: &UsageWindow) -> String {
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


pub fn round1(n: f64) -> f64 {
    (n * 10.0).round() / 10.0
}

pub fn default_account_id(provider: &str) -> String {
    format!("{provider}:default")
}

pub fn usage_key(id: &str) -> String {
    if id.ends_with(":default") { id.split(':').next().unwrap_or(id).to_string() } else { id.to_string() }
}

pub fn usable(a: &Account, now: f64) -> bool {
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

/// Where the usage of each of an agent's accounts that count toward its pooled limits is kept
/// (enabled and signed in, so failover can move work there); just the default sign-in when there
/// are no other accounts.
pub fn pool_keys(provider: &str, list: &[Account]) -> Vec<String> {
    let accounts: Vec<&Account> = list.iter().filter(|a| a.provider == provider).collect();
    if accounts.len() < 2 {
        return vec![provider.to_string()];
    }
    accounts.into_iter().filter(|a| a.enabled && a.signed_in).map(|a| usage_key(&a.id)).collect()
}

/// An agent's limits over all its counted accounts (port of `pooled` in usage.ts): each account
/// is 100%, so a window's "left" is the average of what each has left. It resets (partly) when
/// the first account's does, and is as old as the oldest reading in it. The bool is whether more
/// than one account is pooled.
pub fn pooled(provider: &str, snapshots: &HashMap<String, Snapshot>, list: &[Account], now: f64) -> Option<(Snapshot, bool)> {
    let members: Vec<&Snapshot> = pool_keys(provider, list).iter().filter_map(|k| snapshots.get(k)).filter(|s| !s.windows.is_empty()).collect();
    match members.as_slice() {
        [] => snapshots.get(provider).filter(|s| !s.windows.is_empty()).map(|s| (s.clone(), false)),
        [one] => Some((Snapshot { provider: provider.to_string(), ..(*one).clone() }, false)),
        _ => {
            let mut kinds: Vec<&str> = Vec::new();
            for w in members.iter().flat_map(|s| &s.windows) {
                if !kinds.contains(&w.kind.as_str()) {
                    kinds.push(&w.kind);
                }
            }
            let windows = kinds.into_iter().map(|kind| {
                let ws: Vec<&UsageWindow> = members.iter().flat_map(|s| s.windows.iter().filter(|w| w.kind == kind)).collect();
                let left = ws.iter().map(|w| left_now(w, now)).sum::<f64>() / ws.len() as f64;
                let resets_at = ws.iter().filter_map(|w| w.resets_at).filter(|r| *r > now).reduce(f64::min);
                UsageWindow { kind: kind.to_string(), label: ws[0].label.clone(), used_percent: 100.0 - left, resets_at }
            }).collect();
            let at = members.iter().map(|s| s.at).reduce(f64::min).unwrap_or(now);
            Some((Snapshot { provider: provider.to_string(), windows, at, reset_credits: None }, true))
        }
    }
}

/// When a pooled window runs out at the combined pace of its accounts, if before any of them resets.
pub fn pooled_runs_out_at(provider: &str, kind: &str, snapshots: &HashMap<String, Snapshot>, list: &[Account], samples: &HashMap<String, Vec<(f64, f64)>>, now: f64) -> Option<f64> {
    let members: Vec<(String, &UsageWindow)> = pool_keys(provider, list).into_iter()
        .filter_map(|k| snapshots.get(&k).and_then(|s| s.windows.iter().find(|w| w.kind == kind)).map(|w| (k, w)))
        .collect();
    if members.len() < 2 {
        let (key, w) = members.first()?;
        return runs_out_at(key, w, samples, now);
    }
    let (mut left, mut per_ms, mut first_reset) = (0.0, 0.0, f64::INFINITY);
    for (key, w) in &members {
        left += left_now(w, now);
        if let Some(r) = w.resets_at.filter(|r| *r > now) {
            first_reset = first_reset.min(r);
        }
        let Some(points) = samples.get(&window_key(key, w)) else { continue };
        if let (Some(&(t0, u0)), Some(&(t1, u1))) = (points.first(), points.last()) {
            if points.len() >= 2 && t1 - t0 >= 15.0 * 60_000.0 && u1 > u0 {
                per_ms += (u1 - u0) / (t1 - t0);
            }
        }
    }
    if per_ms <= 0.0 {
        return None;
    }
    let at = now + left / per_ms;
    (at < first_reset).then_some(at)
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

    /// The label in the tab bar's range control.
    pub fn short(self) -> &'static str {
        match self {
            Self::Day => "24 h",
            Self::Week => "7 d",
            Self::Month => "30 d",
            Self::Quarter => "90 d",
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Self::Day => "24h",
            Self::Week => "7d",
            Self::Month => "30d",
            Self::Quarter => "90d",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
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


pub fn tokens_of(b: &Value) -> f64 {
    ["input", "output", "cacheRead", "cacheWrite"].iter().filter_map(|k| b[*k].as_f64()).sum()
}

/// Local calendar arithmetic, so days stay at midnight across daylight saving changes.
pub mod local {
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
        super::wall::step_start(&utc_offset, at, step_hours)
    }

    #[cfg(not(unix))]
    pub fn add_hours(at: f64, hours: i64) -> f64 {
        super::wall::add_hours(&utc_offset, at, hours)
    }

    /// Milliseconds to add to a Unix time (ms) to get the local wall-clock time, read off
    /// `localtime_s`; Windows has no `mktime` in `libc`, so it goes through `wall` instead.
    #[cfg(windows)]
    fn utc_offset(at: f64) -> f64 {
        let secs = (at / 1000.0).floor() as libc::time_t;
        let mut tm: libc::tm = unsafe { std::mem::zeroed() };
        unsafe { libc::localtime_s(&mut tm, &secs) };
        let days = super::wall::days_from_civil(tm.tm_year as i64 + 1900, tm.tm_mon as i64 + 1, tm.tm_mday as i64);
        let local = days * 86_400 + tm.tm_hour as i64 * 3600 + tm.tm_min as i64 * 60 + tm.tm_sec as i64;
        (local - secs as i64) as f64 * 1000.0
    }

    #[cfg(all(not(unix), not(windows)))]
    fn utc_offset(_: f64) -> f64 {
        0.0
    }
}

/// Local calendar arithmetic from a UTC offset function (ms to add to a UTC time to get the local
/// wall-clock time), for platforms without `mktime`: steps start at local hours and midnights, and
/// a day is a calendar day even when daylight saving makes it 23 or 25 hours.
#[cfg_attr(unix, allow(dead_code))]
mod wall {
    const HOUR: f64 = 3_600_000.0;

    /// The UTC time whose wall-clock reading is `wall` (ms as if the wall clock were UTC).
    fn to_utc(offset: &impl Fn(f64) -> f64, wall: f64) -> f64 {
        let guess = wall - offset(wall);
        wall - offset(guess)
    }

    /// Start of the local step (hour, 6 hours or day) `at` falls in.
    pub fn step_start(offset: &impl Fn(f64) -> f64, at: f64, step_hours: i64) -> f64 {
        let wall = at + offset(at);
        let step = step_hours.min(24) as f64 * HOUR;
        to_utc(offset, (wall / step).floor() * step)
    }

    /// `hours` of wall-clock time after (or, negative, before) `at`.
    pub fn add_hours(offset: &impl Fn(f64) -> f64, at: f64, hours: i64) -> f64 {
        to_utc(offset, at + offset(at) + hours as f64 * HOUR)
    }

    /// Days since 1970-01-01 of a civil date (proleptic Gregorian).
    pub fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
        let y = if m <= 2 { y - 1 } else { y };
        let era = y.div_euclid(400);
        let yoe = y - era * 400;
        let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        era * 146_097 + doe - 719_468
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        const DAY: f64 = 24.0 * HOUR;
        /// 2024-03-31 01:00 UTC, when Central European time moves from +1 to +2.
        const JUMP: f64 = 19_813.0 * DAY + HOUR;
        fn cet(t: f64) -> f64 {
            if t < JUMP { HOUR } else { 2.0 * HOUR }
        }

        #[test]
        fn civil_days_count_from_the_epoch() {
            assert_eq!(days_from_civil(1970, 1, 1), 0);
            assert_eq!(days_from_civil(2024, 3, 31), 19_813);
        }

        #[test]
        fn days_start_at_local_midnight_and_follow_daylight_saving() {
            let noon_before = 19_812.0 * DAY + 12.0 * HOUR;
            let midnight = step_start(&cet, noon_before, 24);
            assert_eq!(midnight, 19_811.0 * DAY + 23.0 * HOUR); // 00:00 at +1
            assert_eq!(add_hours(&cet, midnight, 24), 19_812.0 * DAY + 23.0 * HOUR); // 00:00 on the 31st
            // The 31st is a 23-hour day: its next midnight comes 23 hours later, at +2.
            let after = add_hours(&cet, midnight, 48);
            assert_eq!(after, 19_813.0 * DAY + 22.0 * HOUR);
            assert_eq!(step_start(&cet, after + 5.0 * HOUR, 24), after);
        }

        #[test]
        fn hour_steps_floor_to_the_local_hour() {
            let at = 19_813.0 * DAY + 8.0 * HOUR + 1_234_567.0; // 10:20 local
            assert_eq!(step_start(&cet, at, 1), 19_813.0 * DAY + 8.0 * HOUR);
            assert_eq!(step_start(&cet, at, 6), 19_813.0 * DAY + 4.0 * HOUR); // 06:00 local
        }
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
    pub fn tokens(&self) -> f64 {
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


/// The base provider id for a snapshot entry (real snapshots store the full account key such as
/// `"claude:account-2"` in `snap.provider`, while demo snapshots store `"claude"`).
pub fn snapshot_provider<'a>(key: &'a str, snap: &'a Snapshot) -> &'a str {
    let raw = if !snap.provider.is_empty() { snap.provider.as_str() } else { key };
    raw.split(':').next().unwrap_or(raw)
}

/// Groups non-empty snapshot keys by provider, ordering each provider's accounts by their
/// failover order in `accounts` (with the default sign-in first when not listed).
pub fn group_limit_keys(snapshots: &HashMap<String, Snapshot>, accounts: &[Account]) -> Vec<(String, Vec<String>)> {
    let mut grouped: Vec<(String, Vec<String>)> = Vec::new();
    let mut keys: Vec<&String> = snapshots.iter().filter(|(_, s)| !s.windows.is_empty()).map(|(k, _)| k).collect();
    keys.sort_by_key(|k| {
        let provider = snapshot_provider(k, &snapshots[*k]);
        let order = accounts.iter().position(|a| usage_key(&a.id) == **k).unwrap_or(usize::MAX);
        (provider.to_string(), order, k.contains(':'), (*k).clone())
    });
    for k in keys {
        let provider = snapshot_provider(k, &snapshots[k]).to_string();
        match grouped.iter_mut().find(|(p, _)| *p == provider) {
            Some((_, list)) => list.push(k.clone()),
            None => grouped.push((provider, vec![k.clone()])),
        }
    }
    grouped
}

/// Display label for one account key on the Limits tab (falls back to "Default" for the base key).
pub fn account_name(key: &str, accounts: &[Account]) -> String {
    accounts.iter().find(|a| usage_key(&a.id) == key).map_or_else(|| if key.contains(':') { key.to_string() } else { "Default".into() }, |a| a.label.clone())
}

pub fn window_span(kind: &str) -> Option<f64> {
    match kind {
        "month-credit" => Some(30.0 * 86_400_000.0),
        k if k == "session" || k.starts_with("session-") => Some(5.0 * 3_600_000.0),
        k if k == "week" || k.starts_with("week-") => Some(7.0 * 86_400_000.0),
        _ => None,
    }
}

/// Combined snapshot across all of a provider's listed account keys: each window's remaining %
/// is the average across accounts (matching the title bar rings), and `resets_at` is the soonest
/// live reset.
pub fn pooled_snapshot(provider: &str, keys: &[String], snapshots: &HashMap<String, Snapshot>, now: f64) -> Snapshot {
    let members: Vec<&Snapshot> = keys.iter().filter_map(|k| snapshots.get(k)).filter(|s| !s.windows.is_empty()).collect();
    let mut kinds: Vec<&str> = Vec::new();
    for w in members.iter().flat_map(|s| &s.windows) {
        if !kinds.contains(&w.kind.as_str()) {
            kinds.push(&w.kind);
        }
    }
    let windows = kinds
        .into_iter()
        .map(|kind| {
            let ws: Vec<&UsageWindow> = members.iter().flat_map(|s| s.windows.iter().filter(|w| w.kind == kind)).collect();
            let left = ws.iter().map(|w| left_now(w, now)).sum::<f64>() / ws.len().max(1) as f64;
            let resets_at = ws.iter().filter_map(|w| w.resets_at).filter(|r| *r > now).reduce(f64::min);
            UsageWindow { kind: kind.to_string(), label: ws[0].label.clone(), used_percent: 100.0 - left, resets_at }
        })
        .collect();
    let at = members.iter().map(|s| s.at).reduce(f64::min).unwrap_or(now);
    Snapshot { provider: provider.to_string(), windows, at, reset_credits: None }
}

/// Combined historical curve + stepped future projection across every account of a provider.
pub struct PooledSeries {
    pub start: f64,
    pub end: f64,
    pub past: Vec<(f64, f64)>,
    pub future: Vec<(f64, f64)>,
    /// Distinct future reset timestamps across accounts (sorted ascending).
    pub resets: Vec<f64>,
}

pub fn eval_piecewise(pts: &[(f64, f64)], t: f64) -> f64 {
    let Some(&(t0, v0)) = pts.first() else { return 100.0 };
    if t <= t0 {
        return v0;
    }
    for pair in pts.windows(2) {
        let ((ta, va), (tb, vb)) = (pair[0], pair[1]);
        if t <= tb {
            let span = (tb - ta).max(1.0);
            return va + (vb - va) * ((t - ta) / span);
        }
    }
    pts.last().map_or(100.0, |p| p.1)
}

pub fn pooled_series(kind: &str, keys: &[String], snapshots: &HashMap<String, Snapshot>, samples: &HashMap<String, Vec<(f64, f64)>>, now: f64) -> PooledSeries {
    struct Member {
        pts: Vec<(f64, f64)>,
        left: f64,
        rate: f64,
        reset: Option<f64>,
    }
    let span = window_span(kind);
    let mut members: Vec<Member> = Vec::new();
    let mut earliest = now - 3_600_000.0;
    let mut latest = now;
    for key in keys {
        let Some(w) = snapshots.get(key).and_then(|s| s.windows.iter().find(|w| w.kind == kind)) else { continue };
        let left = left_now(w, now).clamp(0.0, 100.0);
        let reset = w.resets_at.filter(|r| *r > now);
        if let Some(r) = reset {
            latest = latest.max(r);
        }
        let ws = reset.and_then(|r| span.map(|s| r - s));
        let prefix = format!("{key}:{kind}:");
        let raw_samples = samples.iter().find(|(k, _)| k.starts_with(&prefix)).map(|(_, v)| v.clone()).unwrap_or_default();
        let mut pts: Vec<(f64, f64)> = if reset.is_some() {
            raw_samples.iter().filter(|(t, _)| *t <= now).map(|(t, used)| (*t, (100.0 - used).clamp(0.0, 100.0))).collect()
        } else {
            Vec::new()
        };
        if let Some(wstart) = ws
            && pts.first().map_or(now, |p| p.0) > wstart
        {
            pts.insert(0, (wstart, 100.0));
        }
        if let Some((first_t, _)) = pts.first() {
            earliest = earliest.min(*first_t);
        }
        pts.push((now, left));
        // Estimate drain pace (% per ms) for this account.
        let sample_rate = if let (Some(&(t0, u0)), Some(&(t1, u1))) = (raw_samples.first(), raw_samples.last())
            && raw_samples.len() >= 2
            && t1 - t0 >= 15.0 * 60_000.0
            && u1 > u0
        {
            Some((u1 - u0) / (t1 - t0))
        } else {
            None
        };
        let rate = sample_rate.unwrap_or_else(|| {
            if reset.is_some() && ws.is_some_and(|wstart| now > wstart) && left < 100.0 {
                (100.0 - left) / (now - ws.unwrap())
            } else {
                0.0
            }
        });
        members.push(Member { pts, left, rate, reset });
    }
    if let Some(s) = span {
        earliest = earliest.max(now - s);
    }
    let start = earliest.min(now - 60_000.0);
    let end = latest.max(now);
    let n = members.len().max(1) as f64;

    // Combine historical curve across all accounts at the union of timestamps in [start, now].
    let mut times: Vec<f64> = vec![start, now];
    for m in &members {
        for &(t, _) in &m.pts {
            if t >= start && t <= now {
                times.push(t);
            }
        }
    }
    times.sort_by(f64::total_cmp);
    times.dedup_by(|a, b| (*a - *b).abs() < 1_000.0);
    let past: Vec<(f64, f64)> = times.into_iter().map(|t| (t, members.iter().map(|m| eval_piecewise(&m.pts, t)).sum::<f64>() / n)).collect();

    // Future projection from `now` to `end`: drains at combined pace, stepping up each time an
    // account resets and refills to 100%.
    let mut resets: Vec<f64> = members.iter().filter_map(|m| m.reset).collect();
    resets.sort_by(f64::total_cmp);
    resets.dedup_by(|a, b| (*a - *b).abs() < 60_000.0);
    let mut cur_left: Vec<f64> = members.iter().map(|m| m.left).collect();
    let mut cur_rate: Vec<f64> = members.iter().map(|m| m.rate).collect();
    let avg = |v: &[f64]| v.iter().sum::<f64>() / n;
    let mut future: Vec<(f64, f64)> = vec![(now, avg(&cur_left))];
    let mut t = now;
    for &r in &resets {
        let dt = (r - t).max(0.0);
        // If any active member drains to 0% before `r`, add the knee point so the slope stays accurate.
        let mut knees: Vec<f64> = cur_left
            .iter()
            .zip(&cur_rate)
            .filter_map(|(&l, &rate)| (l > 0.0 && rate > 0.0).then_some(t + l / rate))
            .filter(|&kt| kt > t + 1_000.0 && kt < r - 1_000.0)
            .collect();
        knees.sort_by(f64::total_cmp);
        knees.dedup_by(|a, b| (*a - *b).abs() < 1_000.0);
        for kt in knees {
            let kdt = (kt - t).max(0.0);
            for (l, rate) in cur_left.iter_mut().zip(&cur_rate) {
                *l = (*l - *rate * kdt).max(0.0);
            }
            t = kt;
            future.push((t, avg(&cur_left)));
        }
        let rem = (r - t).max(0.0);
        for (l, rate) in cur_left.iter_mut().zip(&cur_rate) {
            *l = (*l - *rate * rem).max(0.0);
        }
        if dt > 0.0 {
            future.push((r, avg(&cur_left)));
        }
        // Accounts resetting at `r` refill to 100%.
        for (i, m) in members.iter().enumerate() {
            if m.reset.is_some_and(|mr| (mr - r).abs() < 60_000.0) {
                cur_left[i] = 100.0;
                cur_rate[i] = 0.0;
            }
        }
        t = r;
        future.push((r, avg(&cur_left)));
    }
    PooledSeries { start, end, past, future, resets }
}


    /// The Limits tab for the phone (`usage.get`): per agent, the pooled limits and each account's.
pub fn limits_json(snapshots: &HashMap<String, Snapshot>, accounts: &[Account], samples: &HashMap<String, Vec<(f64, f64)>>, name: &dyn Fn(&str) -> String, now: f64) -> Value {
        let windows = |key: &str, ws: &[UsageWindow], pooled: Option<(&str, &[String])>| -> Vec<Value> {
            ws.iter().map(|w| {
                let (out, pts): (Option<f64>, Vec<[f64; 2]>) = match pooled {
                    Some((provider, keys)) => {
                        let out = pooled_runs_out_at(provider, &w.kind, &snapshots, &accounts, &samples, now);
                        let s = pooled_series(&w.kind, keys, &snapshots, &samples, now);
                        (out, s.past.into_iter().map(|(t, l)| [t, l]).collect())
                    }
                    None => {
                        let out = runs_out_at(key, w, &samples, now);
                        let prefix = format!("{key}:{}:", w.kind);
                        let raw = samples.iter().find(|(k, _)| k.starts_with(&prefix)).map(|(_, v)| v.as_slice()).unwrap_or(&[]);
                        let pts = raw.iter().filter(|(t, _)| *t <= now).map(|(t, used)| [*t, (100.0 - used).clamp(0.0, 100.0)]).collect();
                        (out, pts)
                    }
                };
                json!({ "kind": w.kind, "label": w.label, "left": left_now(w, now), "resetsAt": w.resets_at, "runsOutAt": out, "samples": pts })
            }).collect()
        };
        let providers: Vec<Value> = group_limit_keys(&snapshots, &accounts).into_iter().map(|(provider, keys)| {
            let accounts: Vec<Value> = keys.iter().map(|k| {
                let snap = &snapshots[k];
                json!({ "key": k, "label": account_name(k, &accounts), "at": snap.at, "windows": windows(k, &snap.windows, None) })
            }).collect();
            let pooled = (keys.len() > 1).then(|| {
                let p = pooled_snapshot(&provider, &keys, &snapshots, now);
                json!({ "at": p.at, "windows": windows(&provider, &p.windows, Some((&provider, &keys))) })
            });
            json!({ "id": provider, "name": name(&provider), "accounts": accounts, "pooled": pooled })
        }).collect();
        json!({ "providers": providers, "now": now })
    }


    /// Token & cost summary for the phone (`usage.get`), matching the desktop Usage → Spend tab.
pub fn tokens_json(rows: &[Value], range: Range, now: f64, name: &dyn Fn(&str) -> String) -> Value {
        let bars = to_bars(rows, range, now);
        let since = bars.first().map_or(now - range.days() as f64 * 86_400_000.0, |b| b.start);
        let models = model_totals(rows, since);
        let sum = |f: fn(&ModelTotal) -> f64| -> f64 { models.iter().map(f).sum() };

        let mut by_provider: HashMap<String, (f64, f64)> = HashMap::new();
        for m in &models {
            let entry = by_provider.entry(m.provider.clone()).or_default();
            entry.0 += m.tokens();
            entry.1 += m.cost;
        }
        let mut providers: Vec<Value> = by_provider
            .into_iter()
            .filter(|(_, (tok, cost))| *tok > 0.0 || *cost > 0.0)
            .map(|(id, (tokens, cost))| {
                let name = name(&id);
                json!({ "id": id, "name": name, "tokens": tokens, "cost": cost })
            })
            .collect();
        providers.sort_by(|a, b| {
            b["tokens"].as_f64().unwrap_or(0.0).total_cmp(&a["tokens"].as_f64().unwrap_or(0.0))
        });

        let buckets: Vec<Value> = bars
            .iter()
            .map(|b| {
                let tokens: f64 = b.tokens.values().sum();
                let cost: f64 = b.cost.values().sum();
                json!({
                    "start": b.start,
                    "end": b.end,
                    "tokens": tokens,
                    "cost": cost,
                    "byProvider": b.tokens,
                })
            })
            .collect();

        let model_rows: Vec<Value> = models
            .iter()
            .map(|m| {
                json!({
                    "provider": m.provider,
                    "model": m.model,
                    "input": m.input,
                    "output": m.output,
                    "cacheRead": m.cache_read,
                    "cacheWrite": m.cache_write,
                    "cost": m.cost,
                    "priced": m.priced,
                })
            })
            .collect();

        json!({
            "range": range.id(),
            "input": sum(|m| m.input),
            "output": sum(|m| m.output),
            "cacheRead": sum(|m| m.cache_read),
            "cacheWrite": sum(|m| m.cache_write),
            "cost": sum(|m| m.cost),
            "buckets": buckets,
            "models": model_rows,
            "providers": providers,
        })
    }


// ---- polling without a window (the server; the desktop runs its own on GPUI tasks) ----

/// What a poll of one agent account brought.
pub enum Fetch {
    Ok(Snapshot),
    Err(UsageErrorKind, String, Option<u64>),
    None,
}

/// Reads one agent account's limits from its service (blocking).
pub fn fetch(provider: &str, account: Option<String>) -> Fetch {
    use winfer_backend::usage;
    let parse = |v: Option<Value>| v.and_then(|v| serde_json::from_value::<Snapshot>(v).ok());
    match provider {
        "claude" => match tauri::async_runtime::block_on(usage::usage_claude_detailed(account)) {
            Ok(s) => parse(serde_json::to_value(s).ok()).map_or(Fetch::Err(UsageErrorKind::Transient, "Invalid usage response.".into(), None), Fetch::Ok),
            Err(usage::ClaudeUsageError::RateLimited { retry_after_secs, message }) => Fetch::Err(UsageErrorKind::RateLimited, message, retry_after_secs),
            Err(usage::ClaudeUsageError::AuthExpired(message)) => Fetch::Err(UsageErrorKind::AuthExpired, message, None),
            Err(usage::ClaudeUsageError::Transient(message)) => Fetch::Err(UsageErrorKind::Transient, message, None),
        },
        "agy" => match tauri::async_runtime::block_on(usage::usage_agy()) {
            Ok(s) => parse(serde_json::to_value(s).ok()).map_or(Fetch::None, Fetch::Ok),
            Err(msg) => {
                let kind = if msg.contains("limiting how often") {
                    UsageErrorKind::RateLimited
                } else if msg.contains("expired") || msg.contains("unavailable") {
                    UsageErrorKind::AuthExpired
                } else {
                    UsageErrorKind::Transient
                };
                Fetch::Err(kind, msg, None)
            }
        },
        _ => parse(tauri::async_runtime::block_on(usage::usage_codex(account)).and_then(|s| serde_json::to_value(s).ok())).map_or(Fetch::None, Fetch::Ok),
    }
}

/// The accounts, and Claude Code's own cached readings of them (blocking).
pub fn load_accounts() -> (Vec<Account>, Vec<(String, Snapshot)>) {
    use winfer_backend::{accounts, usage};
    let parsed: Vec<Account> = serde_json::to_value(accounts::accounts_list()).ok().and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default();
    let cached = parsed
        .iter()
        .filter(|a| a.provider == "claude" && a.enabled && a.signed_in)
        .filter_map(|a| {
            let arg = (!a.id.ends_with(":default")).then_some(a.id.as_str());
            let snap = usage::claude_cached_usage(arg).and_then(|s| serde_json::to_value(s).ok()).and_then(|v| serde_json::from_value::<Snapshot>(v).ok())?;
            Some((usage_key(&a.id), snap))
        })
        .collect();
    (parsed, cached)
}

/// Which accounts to poll now: the default sign-ins (unless turned off), Antigravity, and every
/// extra account that's on and signed in, each when its interval and backoff allow.
pub fn due_targets(accounts: &[Account], poll: &HashMap<String, PollState>, manual: bool, now: f64) -> Vec<(String, Option<String>)> {
    let off = |id: &str| accounts.iter().any(|a| a.id == id && !a.enabled);
    let mut targets: Vec<(String, Option<String>)> = Vec::new();
    for p in ["claude", "codex"] {
        if !off(&default_account_id(p)) {
            targets.push((p.to_string(), None));
        }
    }
    targets.push(("agy".to_string(), None));
    targets.extend(accounts.iter().filter(|a| a.enabled && a.signed_in && !a.id.ends_with(":default")).map(|a| (a.provider.clone(), Some(a.id.clone()))));
    targets.retain(|(provider, account)| {
        let key = account.as_deref().map_or_else(|| provider.clone(), usage_key);
        manual || should_auto_poll(provider, poll.get(&key), now)
    });
    targets
}

/// Adds a reading's windows to their sample series (one point a minute, or on change).
pub fn add_samples(samples: &mut HashMap<String, Vec<(f64, f64)>>, snapshot: &Snapshot, now: f64) {
    for w in &snapshot.windows {
        let points = samples.entry(window_key(&snapshot.provider, w)).or_default();
        if points.last().is_none_or(|(t, u)| *u != w.used_percent || now - t > 60_000.0) {
            points.push((now, w.used_percent));
            if points.len() > 240 {
                points.remove(0);
            }
        }
    }
}

/// A server's usage: readings polled from its own sign-ins, kept like the desktop's.
#[derive(Default)]
pub struct Poller {
    pub snapshots: HashMap<String, Snapshot>,
    pub samples: HashMap<String, Vec<(f64, f64)>>,
    pub accounts: Vec<Account>,
    pub poll: HashMap<String, PollState>,
    pub claude_live_at: HashMap<String, f64>,
}

impl Poller {
    /// The saved readings (`usage-cache.json`).
    pub fn load(&mut self) {
        let Some(cache) = usage_cache_path().and_then(|p| std::fs::read_to_string(p).ok()).and_then(|t| serde_json::from_str::<UsageCache>(&t).ok()) else { return };
        let mut samples = cache.samples;
        prune_samples(&mut samples, now_ms());
        self.snapshots.extend(cache.snapshots.into_iter().filter(|(_, s)| !s.windows.is_empty()));
        self.samples.extend(samples);
    }

    pub fn save(&self) {
        let Some(path) = usage_cache_path() else { return };
        let mut samples = self.samples.clone();
        prune_samples(&mut samples, now_ms());
        let cache = UsageCache { snapshots: self.snapshots.clone(), samples };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(bytes) = serde_json::to_vec(&cache) {
            let tmp = path.with_extension("json.tmp");
            if std::fs::write(&tmp, bytes).is_ok() {
                let _ = std::fs::rename(&tmp, &path);
            }
        }
    }

    /// The accounts and Claude Code's cached readings, newer ones over what's kept.
    pub fn set_accounts(&mut self, accounts: Vec<Account>, cached: Vec<(String, Snapshot)>) {
        self.accounts = accounts;
        for (key, mut snap) in cached {
            if !snap.windows.is_empty() && self.snapshots.get(&key).is_none_or(|cur| snap.at > cur.at) {
                snap.provider = key.clone();
                self.snapshots.insert(key, snap);
            }
        }
    }

    pub fn record(&mut self, snapshot: Snapshot, now: f64) {
        add_samples(&mut self.samples, &snapshot, now);
        self.snapshots.insert(snapshot.provider.clone(), snapshot);
    }

    /// The outcome of polling `provider` (`account`, `None` = its default sign-in).
    pub fn apply(&mut self, provider: &str, account: Option<&str>, outcome: Fetch, now: f64) {
        let key = account.map_or_else(|| provider.to_string(), usage_key);
        match outcome {
            Fetch::Ok(mut snapshot) => {
                apply_poll_success(self.poll.entry(key.clone()).or_default(), provider, now);
                if provider == "claude" {
                    self.claude_live_at.insert(key.clone(), now);
                }
                snapshot.provider = key;
                self.record(snapshot, now);
            }
            Fetch::Err(kind, message, retry) => {
                let has = self.snapshots.get(&key).is_some_and(|s| !s.windows.is_empty());
                apply_poll_error(self.poll.entry(key).or_default(), kind, message, retry, has, now);
            }
            Fetch::None => self.poll.entry(key).or_default().last_attempt_at = now,
        }
    }

    /// A `usage://report` (an agent's status line), for `account` (its pane's) or the default.
    pub fn report(&mut self, provider: &str, account: Option<&str>, windows: Vec<UsageWindow>, now: f64) {
        let key = account.map_or_else(|| provider.to_string(), usage_key);
        // Status-line limits may be cached by Claude Code: a recent live read for this account wins.
        let recent_live = provider == "claude" && self.claude_live_at.get(&key).is_some_and(|&at| now - at < CLAUDE_POLL_INTERVAL_MS);
        if windows.is_empty() || recent_live {
            return;
        }
        if let Some(state) = self.poll.get_mut(&key) {
            state.last_error = None;
            state.error_kind = None;
        }
        self.record(Snapshot { provider: key, windows, at: now, reset_credits: None }, now);
    }
}
