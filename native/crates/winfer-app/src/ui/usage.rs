//! Plan usage: live numbers, title-bar rings, and the agent tools `usage_status` and
//! `wait_for_usage_reset` (port of `features/usage/usage.ts`, `agentUsage.ts`, `accounts.ts`).

use super::*;

use winfer_backend::{accounts, usage};
pub use winfer_core::usage::*;

const WARN_AT: [f64; 2] = [20.0, 5.0];
const MARGIN_MS: f64 = 60_000.0;
const DEFAULT_WAKE: &str = "Your usage limit has reset. Call usage_status to check how much is left, then continue where you left off.";

fn now_ms() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_millis() as f64)
}

#[derive(Default)]
pub struct UsageStore {
    pub snapshots: HashMap<String, Snapshot>,
    pub samples: HashMap<String, Vec<(f64, f64)>>,
    pub accounts: Vec<Account>,
    pub wakes: Vec<Wake>,
    pub popover: Option<String>,
    pub warned: std::collections::HashSet<String>,
    /// Per-account timestamp of the last live Claude API read (so cached statusline data won't overwrite it).
    pub claude_live_at: HashMap<String, f64>,
    /// Per-account/provider polling schedule, backoff and last error.
    pub poll: HashMap<String, PollState>,
    /// Usage keys with a request currently in flight (deduplicates overlapping refreshes).
    pub in_flight: std::collections::HashSet<String>,
    /// Usage keys with a banked reset being redeemed.
    pub redeeming: std::collections::HashSet<String>,
}

impl UsageStore {
    pub fn is_refreshing(&self) -> bool {
        !self.in_flight.is_empty()
    }
}

/// Usage meter color that shifts smoothly from `success` → `warning` → `danger` as quota runs low:
/// - `>= 60%`: `success` (green)
/// - `30%..60%`: smooth blend from `warning` (amber) to `success` (green)
/// - `10%..30%`: smooth blend from `danger` (red) to `warning` (amber)
/// - `<= 10%`: `danger` (red)
pub(super) fn level_color(left: f64, c: &Colors) -> Hsla {
    let left = left.clamp(0.0, 100.0);
    if left <= 10.0 {
        c.danger
    } else if left < 30.0 {
        theme::blend(c.danger, c.warning, ((left - 10.0) / 20.0) as f32)
    } else if left < 60.0 {
        theme::blend(c.warning, c.success, ((left - 30.0) / 30.0) as f32)
    } else {
        c.success
    }
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
pub(crate) fn short_when(ms: f64, now: f64) -> String {
    let tm = local_tm(ms);
    const DAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let time = format!("{:02}:{:02}", tm.tm_hour, tm.tm_min);
    if (ms - now).abs() < 6.0 * 24.0 * 3_600_000.0 { format!("{} {time}", DAYS[tm.tm_wday as usize % 7]) } else { format!("{} {} {time}", MONTHS[tm.tm_mon as usize % 12], tm.tm_mday) }
}

pub(super) fn local_time(ms: f64) -> String {
    let tm = local_tm(ms);
    format!("{:04}-{:02}-{:02} {:02}:{:02}", tm.tm_year + 1900, tm.tm_mon + 1, tm.tm_mday, tm.tm_hour, tm.tm_min)
}

/// The local calendar time of a Unix time in ms.
fn local_tm(ms: f64) -> libc::tm {
    let secs = (ms / 1000.0) as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    #[cfg(unix)]
    unsafe { libc::localtime_r(&secs, &mut tm) };
    #[cfg(windows)]
    unsafe { libc::localtime_s(&mut tm, &secs) };
    tm
}

impl Root {
    pub(super) fn start_usage(&mut self, cx: &mut Context<Self>) {
        // Restore persisted snapshots/samples right away so accounts don't flash "no reading" on launch.
        self.load_usage_cache(cx);
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_secs(3)).await;
            loop {
                if this.update(cx, |this, cx| {
                    this.deliver_wakes(cx);
                    this.refresh_usage(false, cx);
                }).is_err() {
                    break;
                }
                cx.background_executor().timer(Duration::from_secs(60)).await;
            }
        })
        .detach();
    }

    fn load_usage_cache(&mut self, cx: &mut Context<Self>) {
        #[cfg(debug_assertions)]
        if DEMO.load(std::sync::atomic::Ordering::Relaxed) {
            return;
        }
        cx.spawn(async move |this, cx| {
            let cache: Option<UsageCache> = backend::background(|| {
                let path = usage_cache_path()?;
                let text = std::fs::read_to_string(path).ok()?;
                serde_json::from_str(&text).ok()
            })
            .await
            .ok()
            .flatten();
            let Some(mut cache) = cache else { return };
            prune_samples(&mut cache.samples, now_ms());
            let _ = this.update(cx, |this, cx| {
                #[cfg(debug_assertions)]
                if DEMO.load(std::sync::atomic::Ordering::Relaxed) {
                    return;
                }
                for (key, snap) in cache.snapshots {
                    if !snap.windows.is_empty() && this.usage.snapshots.get(&key).is_none_or(|cur| snap.at > cur.at) {
                        this.usage.snapshots.insert(key, snap);
                    }
                }
                for (key, pts) in cache.samples {
                    this.usage.samples.entry(key).or_insert(pts);
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn save_usage_cache(&self) {
        #[cfg(debug_assertions)]
        if DEMO.load(std::sync::atomic::Ordering::Relaxed) {
            return;
        }
        let mut samples = self.usage.samples.clone();
        prune_samples(&mut samples, now_ms());
        let cache = UsageCache { snapshots: self.usage.snapshots.clone(), samples };
        let _ = backend::background(move || {
            let Some(path) = usage_cache_path() else { return };
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            if let Ok(bytes) = serde_json::to_vec(&cache) {
                let tmp = path.with_extension("json.tmp");
                if std::fs::write(&tmp, bytes).is_ok() {
                    let _ = std::fs::rename(&tmp, &path);
                }
            }
        });
    }

    /// Manual refresh triggered by a Refresh button (bypasses normal poll intervals, loads fresh
    /// accounts first, and deduplicates in-flight calls).
    pub(super) fn refresh_usage_manual(&mut self, cx: &mut Context<Self>) {
        self.refresh_usage(true, cx);
    }

    /// Loads the latest accounts first (avoiding startup races where extra accounts aren't loaded
    /// yet), seeds any missing Claude accounts from Claude Code's local cache, then refreshes due targets.
    fn refresh_usage(&mut self, manual: bool, cx: &mut Context<Self>) {
        #[cfg(debug_assertions)]
        if DEMO.load(std::sync::atomic::Ordering::Relaxed) {
            return;
        }
        cx.spawn(async move |this, cx| {
            let (parsed, cached_claude): (Vec<Account>, Vec<(String, Snapshot)>) = backend::background(|| {
                let list = serde_json::to_value(accounts::accounts_list()).unwrap_or_default();
                let parsed: Vec<Account> = serde_json::from_value(list).unwrap_or_default();
                let mut cached = Vec::new();
                for a in parsed.iter().filter(|a| a.provider == "claude" && a.enabled && a.signed_in) {
                    let arg = (!a.id.ends_with(":default")).then_some(a.id.as_str());
                    if let Some(snap) = usage::claude_cached_usage(arg)
                        .and_then(|s| serde_json::to_value(s).ok())
                        .and_then(|v| serde_json::from_value::<Snapshot>(v).ok())
                    {
                        let key = usage_key(&a.id);
                        cached.push((key, snap));
                    }
                }
                (parsed, cached)
            })
            .await
            .unwrap_or_default();

            let _ = this.update(cx, |this, cx| {
                #[cfg(debug_assertions)]
                if DEMO.load(std::sync::atomic::Ordering::Relaxed) {
                    return;
                }
                this.usage.accounts = parsed;
                // Seed from Claude Code's own local cache only when it's newer than what we have.
                let mut seeded = false;
                for (key, mut snap) in cached_claude {
                    if !snap.windows.is_empty() && this.usage.snapshots.get(&key).is_none_or(|cur| snap.at > cur.at) {
                        snap.provider = key.clone();
                        this.usage.snapshots.insert(key, snap);
                        seeded = true;
                    }
                }
                if seeded {
                    this.save_usage_cache();
                }

                let now = now_ms();
                let claude_default_disabled = this.usage.accounts.iter().any(|a| a.id == "claude:default" && !a.enabled);
                let codex_default_disabled = this.usage.accounts.iter().any(|a| a.id == "codex:default" && !a.enabled);
                let mut targets: Vec<(String, Option<String>)> = Vec::new();
                if !claude_default_disabled {
                    targets.push(("claude".to_string(), None));
                }
                if !codex_default_disabled {
                    targets.push(("codex".to_string(), None));
                }
                targets.push(("agy".to_string(), None));
                let extras: Vec<(String, Option<String>)> = this
                    .usage
                    .accounts
                    .iter()
                    .filter(|a| a.enabled && a.signed_in && !a.id.ends_with(":default"))
                    .map(|a| (a.provider.clone(), Some(a.id.clone())))
                    .collect();
                targets.extend(extras);

                let mut claude_idx: u64 = 0;
                for (provider, account) in targets {
                    let key = account.as_deref().map_or_else(|| provider.clone(), usage_key);
                    if !manual && !should_auto_poll(&provider, this.usage.poll.get(&key), now) {
                        continue;
                    }
                    let delay_ms = if provider == "claude" {
                        let d = claude_idx * CLAUDE_STAGGER_MS;
                        claude_idx += 1;
                        d
                    } else {
                        0
                    };
                    this.refresh_one_delayed(provider, account, delay_ms, cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn refresh_one(&mut self, provider: String, account: Option<String>, cx: &mut Context<Self>) {
        self.refresh_one_delayed(provider, account, 0, cx);
    }

    fn refresh_one_delayed(&mut self, provider: String, account: Option<String>, delay_ms: u64, cx: &mut Context<Self>) {
        let key = account.as_deref().map_or_else(|| provider.clone(), usage_key);
        if !self.usage.in_flight.insert(key.clone()) {
            return;
        }
        cx.notify();
        cx.spawn(async move |this, cx| {
            if delay_ms > 0 {
                cx.background_executor().timer(Duration::from_millis(delay_ms)).await;
            }
            let (p, a) = (provider.clone(), account.clone());
            enum FetchOutcome {
                Ok(Value),
                Err(UsageErrorKind, String, Option<u64>),
                None,
            }
            let outcome = tauri::async_runtime::spawn(async move {
                if p == "claude" {
                    match usage::usage_claude_detailed(a).await {
                        Ok(s) => match serde_json::to_value(s) {
                            Ok(v) => FetchOutcome::Ok(v),
                            Err(e) => FetchOutcome::Err(UsageErrorKind::Transient, e.to_string(), None),
                        },
                        Err(usage::ClaudeUsageError::RateLimited { retry_after_secs, message }) => {
                            FetchOutcome::Err(UsageErrorKind::RateLimited, message, retry_after_secs)
                        }
                        Err(usage::ClaudeUsageError::AuthExpired(message)) => {
                            FetchOutcome::Err(UsageErrorKind::AuthExpired, message, None)
                        }
                        Err(usage::ClaudeUsageError::Transient(message)) => {
                            FetchOutcome::Err(UsageErrorKind::Transient, message, None)
                        }
                    }
                } else if p == "agy" {
                    match usage::usage_agy().await {
                        Ok(s) => match serde_json::to_value(s) {
                            Ok(v) => FetchOutcome::Ok(v),
                            Err(e) => FetchOutcome::Err(UsageErrorKind::Transient, e.to_string(), None),
                        },
                        Err(msg) => {
                            let kind = if msg.contains("limiting how often") {
                                UsageErrorKind::RateLimited
                            } else if msg.contains("expired") || msg.contains("unavailable") {
                                UsageErrorKind::AuthExpired
                            } else {
                                UsageErrorKind::Transient
                            };
                            FetchOutcome::Err(kind, msg, None)
                        }
                    }
                } else {
                    match usage::usage_codex(a).await.and_then(|s| serde_json::to_value(s).ok()) {
                        Some(v) => FetchOutcome::Ok(v),
                        None => FetchOutcome::None,
                    }
                }
            })
            .await
            .unwrap_or(FetchOutcome::None);

            let live_claude = provider == "claude";
            let _ = this.update(cx, |this, cx| {
                this.usage.in_flight.remove(&key);
                let now = now_ms();
                match outcome {
                    FetchOutcome::Ok(value) => {
                        if let Ok(mut snapshot) = serde_json::from_value::<Snapshot>(value) {
                            let state = this.usage.poll.entry(key.clone()).or_default();
                            apply_poll_success(state, &provider, now);
                            if live_claude {
                                this.usage.claude_live_at.insert(key.clone(), now);
                            }
                            snapshot.provider = key;
                            this.record_usage(snapshot, cx);
                        } else {
                            let has = this.usage.snapshots.contains_key(&key);
                            let state = this.usage.poll.entry(key.clone()).or_default();
                            apply_poll_error(state, UsageErrorKind::Transient, "Invalid usage response.".into(), None, has, now);
                            cx.notify();
                        }
                    }
                    FetchOutcome::Err(kind, message, retry_after_secs) => {
                        let has = this.usage.snapshots.get(&key).is_some_and(|s| !s.windows.is_empty());
                        let state = this.usage.poll.entry(key.clone()).or_default();
                        apply_poll_error(state, kind, message, retry_after_secs, has, now);
                        cx.notify();
                    }
                    FetchOutcome::None => {
                        let state = this.usage.poll.entry(key.clone()).or_default();
                        state.last_attempt_at = now;
                        cx.notify();
                    }
                }
            });
        })
        .detach();
    }

    fn record_usage(&mut self, mut snapshot: Snapshot, cx: &mut Context<Self>) {
        let now = now_ms();
        let key = snapshot.provider.clone();
        snapshot.keep_resets(self.usage.snapshots.get(&key));
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
                        self.limit_reached(&key, format!("{name}: {} limit reached{reset}", w.label.to_lowercase()), snapshot.banked_resets(now), cx);
                    } else {
                        self.toast(format!("{name}: {:.0}% of the {} limit left{reset}", left, w.label.to_lowercase()), cx);
                    }
                    break;
                }
            }
        }
        self.usage.snapshots.insert(key.clone(), snapshot);
        self.save_usage_cache();
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
        let now = now_ms();
        // Status-line limits may be cached by Claude Code: a recent live API read for this account wins.
        let recent_live = provider == "claude" && self.usage.claude_live_at.get(&key).is_some_and(|&at| now - at < CLAUDE_POLL_INTERVAL_MS);
        if !windows.is_empty() && !recent_live {
            if let Some(state) = self.usage.poll.get_mut(&key) {
                state.last_error = None;
                state.error_kind = None;
            }
            self.record_usage(Snapshot { provider: key, windows, at: now, reset_credits: None, resets: Vec::new() }, cx);
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
                "agy" => format!("winfer has no usage numbers for {name} yet. Sign in to Antigravity (run agy once), then ask again."),
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
        let banked = snapshot.banked_resets(now);
        if banked > 0 {
            let expiry = snapshot.live_resets(now).first().and_then(|r| r.expires_at).map_or(String::new(), |e| format!(" (the next expires {})", at_text(e, now)));
            lines.push(format!("Banked rate limit resets: {banked}{expiry}. Only the user can use one, from winfer's usage popover."));
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
    /// Each signed-in account's overall usage left, for Home: (label, percent).
    pub(super) fn usage_left_rows(&self, cx: &App) -> Vec<(String, f64)> {
        let now = now_ms();
        let s = self.state.read(cx);
        let several = |provider: &str| self.usage.accounts.iter().filter(|a| a.provider == provider && a.signed_in).count() > 1;
        self.usage
            .accounts
            .iter()
            .filter(|a| a.signed_in)
            .filter_map(|a| {
                let left = winfer_core::usage::overall_left(self.usage.snapshots.get(&winfer_core::usage::usage_key(&a.id)), now)?;
                let name = s.provider_name(&a.provider);
                Some((if several(&a.provider) { format!("{name} · {}", a.label) } else { name }, left))
            })
            .collect()
    }

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
        for provider in ["claude", "codex", "agy"] {
            let pooled = pooled(provider, &self.usage.snapshots, &self.usage.accounts, now);
            let Some(left) = overall_left(pooled.as_ref().map(|p| &p.0), now) else { continue };
            let color = level_color(left, &c);
            let label_color = if left < 40.0 { color } else { c.muted };
            let toggle = provider.to_string();
            row = row.child(
                div().id(SharedString::from(format!("usage-{provider}"))).flex().items_center().gap_1p5().rounded_md().px_1p5().py_1().cursor_pointer().hover(move |d| d.bg(c.hover))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.usage.popover = if this.usage.popover.as_ref() == Some(&toggle) { None } else { Some(toggle.clone()) };
                        cx.notify();
                    }))
                    .child(usage_ring(provider, left, color, 22.0, c))
                    .child(div().text_size(px(11.0)).text_color(label_color).child(format!("{:.0}%", left.round()))),
            );
        }
        row
    }

    /// Banked rate limit resets for one agent's accounts (`keys`): Codex's with a button to use
    /// each; Claude shows its own only to Claude's apps, so winfer hands over to Claude Code.
    /// `remote`: another machine's numbers, to look at only.
    pub(super) fn render_banked_resets(&self, provider: &str, keys: &[String], remote: bool, c: Colors, cx: &mut Context<Self>) -> Option<Div> {
        let claude = provider == "claude";
        if !claude && provider != "codex" {
            return None;
        }
        // Claude's are redeemed here and now, so there's nothing to say about another machine's.
        if claude && remote {
            return None;
        }
        let now = now_ms();
        // A removed account's cached reading mustn't get a button.
        let keys: Vec<&String> = keys.iter().filter(|k| !k.contains(':') || self.usage.accounts.iter().any(|a| usage_key(&a.id) == **k)).collect();
        let multi = keys.len() > 1;
        let button = |id: String, label: SharedString, busy: bool| {
            div().id(SharedString::from(id)).flex_shrink_0().h(px(24.0)).flex().items_center().px_2p5().rounded_md().border_1().border_color(c.edge).text_size(px(11.5)).text_color(c.fg)
                .when(!busy, |d| d.cursor_pointer().hover(move |d| d.bg(c.hover)))
                .when(busy, |d| d.opacity(0.6))
                .child(if busy { "Using…".into() } else { label })
        };
        let row = |title: String, detail: String| {
            div().flex().items_center().gap_2()
                .child(div().flex_1().min_w_0().flex().flex_col()
                    .child(div().text_size(px(12.0)).text_color(c.fg).child(title))
                    .when(!detail.is_empty(), |d| d.child(div().text_size(px(11.0)).text_color(c.subtle).child(detail))))
        };
        let mut card = div().flex().flex_col().gap_2().rounded_lg().border_1().border_color(c.edge).bg(c.panel).p_3()
            .child(div().text_size(px(11.0)).font_weight(FontWeight::SEMIBOLD).text_color(c.subtle).child("BANKED RESETS"));
        if claude {
            let mut buttons = div().flex().flex_wrap().gap_2();
            for &key in &keys {
                if self.usage.accounts.iter().any(|a| usage_key(&a.id) == *key && !a.signed_in) {
                    continue;
                }
                let label = if multi { account_name(key, &self.usage.accounts) } else { "Open /limit-reset".into() };
                let open = key.clone();
                buttons = buttons.child(button(format!("claude-reset-{key}"), label.into(), false)
                    .on_click(cx.listener(move |this, _, window, cx| this.execute(super::commands::Command::ClaudeLimitReset(open.clone()), window, cx))));
            }
            let hint = if multi { "Claude only shows these to its own apps. Open /limit-reset in Claude Code for:" } else { "Claude only shows these to its own apps. Check and use them in Claude Code." };
            return Some(card.child(div().text_size(px(11.0)).text_color(c.subtle).child(hint)).child(buttons));
        }
        let mut shown = false;
        for &key in &keys {
            let account = multi.then(|| account_name(key, &self.usage.accounts));
            let Some(snapshot) = self.usage.snapshots.get(key).filter(|s| s.reset_credits.is_some()) else { continue };
            shown = true;
            if let Some(account) = account {
                card = card.child(div().text_size(px(11.0)).font_weight(FontWeight::MEDIUM).text_color(c.muted).child(account));
            }
            let banked = snapshot.banked_resets(now);
            if banked == 0 {
                card = card.child(div().text_size(px(11.5)).text_color(c.subtle).child("None banked."));
                continue;
            }
            let busy = self.usage.redeeming.contains(key);
            let live = snapshot.live_resets(now);
            // The list couldn't be read: one button, and Codex picks which.
            let entries: Vec<(Option<String>, String, String)> = if live.is_empty() {
                vec![(None, format!("{banked} banked reset{}", if banked == 1 { "" } else { "s" }), String::new())]
            } else {
                live.iter().map(|r| {
                    let expiry = r.expires_at.map(|e| format!("Use by {} (in {})", short_when(e, now), duration(e - now)));
                    let detail = [r.description.clone(), expiry].into_iter().flatten().collect::<Vec<_>>().join(" · ");
                    (Some(r.id.clone()), r.title.clone().unwrap_or_else(|| "Limit reset".into()), detail)
                }).collect()
            };
            for (idx, (credit, title, detail)) in entries.into_iter().enumerate() {
                let mut line = row(title, detail);
                if !remote {
                    let key = key.clone();
                    let id = format!("codex-reset-{key}-{idx}");
                    line = line.child(button(id, "Use".into(), busy).when(!busy, |d| d.on_click(cx.listener(move |this, _, window, cx| {
                        this.execute(super::commands::Command::RedeemReset { key: key.clone(), credit: credit.clone() }, window, cx)
                    }))));
                }
                card = card.child(line);
            }
        }
        shown.then_some(card)
    }

    pub(super) fn render_usage_popover(&self, c: &Colors, cx: &mut Context<Self>) -> Option<AnyElement> {
        let provider = self.usage.popover.clone()?;
        let c = *c;
        let now = now_ms();
        let name = self.name_of(&provider, cx);
        // Several accounts count toward the agent's limits: the bars are their average.
        let (snapshot, multi) = pooled(&provider, &self.usage.snapshots, &self.usage.accounts, now)?;
        let mut list = div().flex().flex_col().gap_2p5();
        for (group_title, items) in group_windows(&snapshot.windows) {
            let mut card = div().flex().flex_col().rounded_lg().border_1().border_color(c.edge).bg(c.panel).p_3();
            if let Some(title) = group_title {
                card = card.child(div().pb_2().text_size(px(12.0)).font_weight(FontWeight::MEDIUM).text_color(c.fg).child(title.to_string()));
            }
            for (idx, (label, w)) in items.into_iter().enumerate() {
                if idx > 0 {
                    card = card.child(div().my_2p5().h(px(1.0)).w_full().bg(c.border));
                }
                let left = left_now(w, now);
                let color = level_color(left, &c);
                let reset = w.resets_at.filter(|r| *r > now).map(|r| format!("{} {}", if multi { "first resets" } else { "resets" }, at_text(r, now)));
                // Only warn when the pace runs it out before it resets.
                let out = pooled_runs_out_at(&provider, &w.kind, &self.usage.snapshots, &self.usage.accounts, &self.usage.samples, now).filter(|o| w.resets_at.is_none_or(|r| *o < r)).map(|o| format!("Runs out in {} at this pace", duration(o - now)));
                card = card.child(
                    div().flex().flex_col().gap_1p5()
                        .child(div().flex().items_baseline().gap_2()
                            .child(div().flex_1().min_w_0().truncate().text_size(px(12.0)).text_color(c.muted).child(label.to_string()))
                            .child(div().text_size(px(17.0)).font_weight(FontWeight::SEMIBOLD).text_color(color).child(format!("{left:.0}%")))
                            .child(div().text_size(px(11.0)).text_color(c.subtle).child("left")))
                        .child(div().h(px(4.0)).w_full().rounded_full().bg(c.active).child(div().h_full().rounded_full().bg(color).w(relative((left / 100.0).clamp(0.0, 1.0) as f32))))
                        .when_some(reset, |d, t| d.child(div().text_size(px(11.0)).text_color(c.subtle).child(t)))
                        .when_some(out, |d, t| d.child(div().flex().items_center().gap_1p5().mt_0p5().rounded_md().bg(Hsla { a: 0.1, ..c.warning }).px_2().py_1().text_size(px(11.0)).text_color(c.warning)
                            .child(Icon::new(IconName::TriangleAlert).size(px(11.0))).child(t))),
                );
            }
            list = list.child(card);
        }
        let accounts: Vec<&Account> = self.usage.accounts.iter().filter(|a| a.provider == provider).collect();
        if accounts.len() > 1 {
            let mut rows = div().flex().flex_col().gap_2().pt_1()
                .child(div().text_size(px(11.0)).font_weight(FontWeight::SEMIBOLD).text_color(c.subtle).child("ACCOUNTS"));
            for a in accounts {
                let key = usage_key(&a.id);
                let snapshot = self.usage.snapshots.get(&key);
                let left = overall_left(snapshot, now);
                let poll_state = self.usage.poll.get(&key);
                let busy_acct = self.usage.in_flight.contains(&key);
                let color = left.map_or(c.subtle, |l| level_color(l, &c));
                let (status_text, status_color, status_tip) = if !a.signed_in {
                    ("not signed in".to_string(), c.warning, None)
                } else if let Some(l) = left {
                    (format!("{:.0}% left", l.round()), c.subtle, None)
                } else if busy_acct {
                    ("refreshing…".to_string(), c.subtle, None)
                } else if let Some(kind) = poll_state.and_then(|p| p.error_kind) {
                    (error_short_label(kind).to_string(), c.warning, poll_state.and_then(|p| p.last_error.clone()))
                } else {
                    ("no reading".to_string(), c.subtle, None)
                };
                let status_el = div()
                    .id(SharedString::from(format!("usage-acct-status-{}", a.id)))
                    .flex_shrink_0()
                    .text_size(px(11.0))
                    .text_color(status_color)
                    .when_some(status_tip, |d, tip| d.tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx)))
                    .child(status_text);
                let mut card = div().flex().flex_col().gap_1().rounded_lg().border_1().border_color(c.edge).bg(c.panel).p_2p5()
                    .child(div().flex().items_center().gap_2()
                        .child(div().size(px(6.0)).flex_shrink_0().rounded_full().bg(color))
                        .child(div().flex().flex_1().min_w_0().flex_col()
                            .child(div().truncate().text_size(px(12.5)).font_weight(FontWeight::MEDIUM).text_color(c.fg).child(a.label.clone()))
                            .when_some(a.email.clone(), |d, e| d.child(div().truncate().text_size(px(11.0)).text_color(c.subtle).child(e))))
                        .child(status_el));
                // Every window of the account (5-hour, weekly, …) with when it resets.
                for w in snapshot.map_or(&[][..], |s| s.windows.as_slice()) {
                    let wl = left_now(w, now);
                    let wc = level_color(wl, &c);
                    let reset = match w.resets_at {
                        Some(r) if r > now => format!("resets {}", at_text(r, now)),
                        Some(_) => "reset — full again".to_string(),
                        None => String::new(),
                    };
                    card = card.child(div().flex().flex_col().pl(px(14.0)).text_size(px(11.0))
                        .child(div().flex().items_baseline().gap_2()
                            .child(div().w(px(46.0)).flex_shrink_0().truncate().text_color(c.muted).child(w.label.clone()))
                            .child(div().flex_1().h(px(3.0)).rounded_full().bg(c.active).child(div().h_full().rounded_full().bg(wc).w(relative((wl / 100.0).clamp(0.0, 1.0) as f32))))
                            .child(div().w(px(62.0)).flex_shrink_0().text_right().font_family(crate::fonts::mono(cx)).text_color(wc).child(format!("{wl:.0}% left"))))
                        .when(!reset.is_empty(), |d| d.child(div().pl(px(54.0)).text_size(px(10.5)).text_color(c.subtle).child(reset))));
                }
                rows = rows.child(card);
            }
            list = list.child(rows);
        }
        let keys = group_limit_keys(&self.usage.snapshots, &self.usage.accounts).into_iter().find(|(p, _)| *p == provider).map(|(_, keys)| keys).unwrap_or_default();
        if let Some(resets) = self.render_banked_resets(&provider, &keys, false, c, cx) {
            list = list.child(resets);
        }
        let busy = self.usage.is_refreshing();
        let refresh = div().id("usage-refresh").flex().items_center().gap_1p5().h(px(26.0)).px_2().rounded_md().text_size(px(11.5)).text_color(c.muted)
            .when(!busy, |d| d.cursor_pointer().hover(move |d| d.bg(c.hover).text_color(c.fg)).on_click(cx.listener(|this, _, _, cx| this.refresh_usage_manual(cx))))
            .when(busy, |d| d.opacity(0.6))
            .child(Icon::new(IconName::RefreshCw).size(px(12.0)).with_animation("usage-refresh-spin", Animation::new(Duration::from_millis(900)).repeat(), move |i, t| if busy { i.transform(Transformation::rotate(percentage(t))) } else { i }))
            .child(if busy { "Refreshing…" } else { "Refresh" });
        let settings = div().id("usage-settings").flex().items_center().gap_1p5().pt_2().text_size(px(12.0)).text_color(c.accent).cursor_pointer()
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
                    div().id("usage-popover").absolute().top(px(40.0)).right(px(120.0)).w(px(340.0)).max_h(relative(0.94)).overflow_y_scroll().flex().flex_col().gap_3().rounded(px(12.0)).border_1().border_color(c.edge).bg(c.raised).shadow_2xl().p_4()
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
        self.usage.snapshots.insert("claude".into(), Snapshot { provider: "claude".into(), windows: vec![window("session", "5-hour", 78.0, 2.5), window("week", "Weekly", 36.0, 60.0)], at: now, reset_credits: None, resets: Vec::new() });
        let reset = |id: &str, days: f64| winfer_core::usage::BankedReset { id: id.into(), title: Some("Full reset (Weekly + 5 hr)".into()), description: None, expires_at: Some(now + days * 86_400_000.0) };
        self.usage.snapshots.insert("codex".into(), Snapshot { provider: "codex".into(), windows: vec![window("session", "5-hour", 22.0, 3.0), window("week", "Weekly", 91.0, 30.0)], at: now, reset_credits: Some(2), resets: vec![reset("demo-1", 4.0), reset("demo-2", 21.0)] });
        self.usage.samples.insert(format!("claude:session:{}", (now + 2.5 * 3_600_000.0) as i64), vec![(now - 3_600_000.0, 50.0), (now, 78.0)]);
        // Three Claude Code accounts with their own readings.
        let account = |id: &str, label: &str, email: Option<&str>| Account { id: id.into(), provider: "claude".into(), label: label.into(), enabled: true, signed_in: true, email: email.map(str::to_string), exhausted_until: None, dir: String::new() };
        self.usage.accounts = vec![account("a1", "Default", Some("dane@example.com")), account("a2", "Account 2", Some("work@example.org")), account("a3", "Account 3", None)];
        for (id, session, week) in [("a1", 100.0, 41.0), ("a2", 99.0, 12.0), ("a3", 35.0, 60.0)] {
            self.usage.snapshots.insert(usage_key(id), Snapshot { provider: "claude".into(), windows: vec![window("session", "5-hour", session, 6.3), window("week", "Weekly", week, 70.0)], at: now, reset_credits: None, resets: Vec::new() });
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
        arc(0.0, 1.0, if fraction == 0.0 { Hsla { a: 0.45, ..color } } else { c.border_strong });
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
    use super::{
        Account, CLAUDE_POLL_INTERVAL_MS, PollState, RATE_LIMIT_BASE_MS, RATE_LIMIT_MAX_MS, Snapshot, TRANSIENT_NO_DATA_RETRY_MS, TRANSIENT_WITH_DATA_RETRY_MS, UsageErrorKind, UsageWindow,
        apply_poll_error, apply_poll_success, duration, group_windows, left_now, level_color, overall_left, overall_summary, pooled, prune_samples, runs_out_at, should_auto_poll, start_account, theme,
        tightest, usage_key,
    };
    use std::collections::HashMap;

    fn window(kind: &str, used: f64, resets: Option<f64>) -> UsageWindow {
        UsageWindow { kind: kind.into(), label: kind.into(), used_percent: used, resets_at: resets }
    }

    #[test]
    fn pooled_averages_counted_accounts_resetting_with_the_first() {
        let acc = |id: &str, enabled: bool| Account { id: id.into(), provider: "claude".into(), label: id.into(), enabled, signed_in: true, email: None, exhausted_until: None, dir: String::new() };
        let snap = |used: [f64; 2], resets: f64, at: f64| Snapshot { provider: "claude".into(), windows: vec![window("session", used[0], Some(resets)), window("week", used[1], None)], at, reset_credits: None, resets: Vec::new() };
        let snapshots: HashMap<String, Snapshot> = [
            ("claude".to_string(), snap([100.0, 0.0], 5000.0, 10.0)),
            ("claude:a2".to_string(), snap([26.0, 26.0], 9000.0, 20.0)),
            ("claude:a3".to_string(), snap([0.0, 23.0], 7000.0, 30.0)),
            ("claude:off".to_string(), snap([100.0, 100.0], 3000.0, 1.0)),
        ].into();
        let list = vec![acc("claude:default", true), acc("claude:a2", true), acc("claude:a3", true), acc("claude:off", false)];
        let (p, multi) = pooled("claude", &snapshots, &list, 100.0).unwrap();
        assert!(multi);
        assert_eq!(p.at, 10.0);
        assert!((left_now(&p.windows[0], 100.0) - 58.0).abs() < 1e-9);
        assert_eq!(p.windows[0].resets_at, Some(5000.0));
        assert!((left_now(&p.windows[1], 100.0) - 251.0 / 3.0).abs() < 1e-9);
        // One account: its own reading.
        let (single, multi) = pooled("claude", &snapshots, &[], 100.0).unwrap();
        assert!(!multi);
        assert_eq!(single.windows, snapshots["claude"].windows);
    }

    #[test]
    fn durations() {
        assert_eq!(duration(60_000.0), "1m");
        assert_eq!(duration(108.0 * 60_000.0), "1h 48m");
        assert_eq!(duration(3.0 * 1440.0 * 60_000.0), "3d");
    }

    #[test]
    fn reset_windows_start_over_and_tightest_wins() {
        let s = Snapshot { provider: "claude".into(), windows: vec![window("session", 90.0, Some(1000.0)), window("week", 50.0, None)], at: 0.0, reset_credits: None, resets: Vec::new() };
        assert_eq!(left_now(&s.windows[0], 2000.0), 100.0);
        assert_eq!(tightest(Some(&s), 2000.0).unwrap().kind, "week");
        assert_eq!(tightest(Some(&s), 500.0).unwrap().kind, "session");
    }

    #[test]
    fn credits_only_once_the_plan_is_out() {
        let s = Snapshot { provider: "x".into(), windows: vec![window("week", 100.0, None), window("month-credit", 20.0, None)], at: 0.0, reset_credits: None, resets: Vec::new() };
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

    #[test]
    fn groups_pair_five_hour_and_weekly() {
        let mk = |kind: &str, label: &str| UsageWindow { kind: kind.into(), label: label.into(), used_percent: 10.0, resets_at: None };

        // Antigravity: two model groups, each pairing 5-hour before Weekly.
        let agy = vec![
            mk("week-0", "Weekly (Gemini Models)"),
            mk("session-0", "5-hour (Gemini Models)"),
            mk("week-1", "Weekly (Claude and GPT models)"),
            mk("session-1", "5-hour (Claude and GPT models)"),
        ];
        let grouped = group_windows(&agy);
        assert_eq!(grouped.len(), 2);
        assert_eq!(grouped[0].0, Some("Gemini Models"));
        assert_eq!(grouped[0].1.iter().map(|(l, _)| *l).collect::<Vec<_>>(), vec!["5-hour", "Weekly"]);
        assert_eq!(grouped[1].0, Some("Claude and GPT models"));
        assert_eq!(grouped[1].1.iter().map(|(l, _)| *l).collect::<Vec<_>>(), vec!["5-hour", "Weekly"]);

        // Claude: main 5-hour + Weekly grouped together, per-model weekly extras stay standalone.
        let claude = vec![mk("session", "5-hour"), mk("week", "Weekly"), mk("week-opus", "Weekly (Opus)")];
        let grouped = group_windows(&claude);
        assert_eq!(grouped.len(), 2);
        assert_eq!(grouped[0].0, None);
        assert_eq!(grouped[0].1.iter().map(|(l, _)| *l).collect::<Vec<_>>(), vec!["5-hour", "Weekly"]);
        assert_eq!(grouped[1].0, None);
        assert_eq!(grouped[1].1.iter().map(|(l, _)| *l).collect::<Vec<_>>(), vec!["Weekly (Opus)"]);

        // Codex: main 5-hour + Weekly grouped together, credits stay separate.
        let codex = vec![mk("session", "5-hour"), mk("week", "Weekly"), mk("month-credit", "Monthly credits")];
        let grouped = group_windows(&codex);
        assert_eq!(grouped.len(), 2);
        assert_eq!(grouped[0].1.len(), 2);
        assert_eq!(grouped[1].1.iter().map(|(l, _)| *l).collect::<Vec<_>>(), vec!["Monthly credits"]);
    }

    #[test]
    fn level_color_shifts_when_low() {
        let c = theme::colors("dark");
        assert_eq!(level_color(100.0, &c), c.success);
        assert_eq!(level_color(60.0, &c), c.success);
        assert_ne!(level_color(33.0, &c), c.success);
        assert_eq!(level_color(30.0, &c), c.warning);
        assert_ne!(level_color(20.0, &c), c.warning);
        assert_ne!(level_color(20.0, &c), c.danger);
        assert_eq!(level_color(10.0, &c), c.danger);
        assert_eq!(level_color(0.0, &c), c.danger);
    }

    #[test]
    fn overall_averages_independent_model_groups() {
        let mk = |kind: &str, label: &str, used: f64, resets: Option<f64>| UsageWindow {
            kind: kind.into(),
            label: label.into(),
            used_percent: used,
            resets_at: resets,
        };
        let now = 1_000.0;

        // Reported Antigravity case: Gemini (31% 5h, 51% weekly) + Claude/GPT (0% 5h, 41% weekly) -> avg(31, 0) = 15.5% (rounds to 16%).
        let agy = Snapshot {
            provider: "agy".into(),
            windows: vec![
                mk("session-0", "5-hour (Gemini Models)", 69.0, Some(5_000.0)),
                mk("week-0", "Weekly (Gemini Models)", 49.0, Some(50_000.0)),
                mk("session-1", "5-hour (Claude and GPT models)", 100.0, Some(8_000.0)),
                mk("week-1", "Weekly (Claude and GPT models)", 59.0, Some(40_000.0)),
            ],
            at: now,
            reset_credits: None, resets: Vec::new(),
        };
        let (left, reset) = overall_summary(Some(&agy), now).unwrap();
        assert!((left - 15.5).abs() < 1e-9);
        assert_eq!(left.round(), 16.0);
        assert_eq!(reset, Some(5_000.0));

        // When Gemini's 5-hour window resets, Gemini becomes governed by its 51% weekly limit -> avg(51, 0) = 25.5%.
        let (left_after_reset, reset_after) = overall_summary(Some(&agy), 6_000.0).unwrap();
        assert!((left_after_reset - 25.5).abs() < 1e-9);
        assert_eq!(reset_after, Some(8_000.0));

        // Both groups exhausted -> 0%.
        let agy_out = Snapshot {
            provider: "agy".into(),
            windows: vec![
                mk("session-0", "5-hour (Gemini Models)", 100.0, Some(5_000.0)),
                mk("week-0", "Weekly (Gemini Models)", 49.0, Some(50_000.0)),
                mk("session-1", "5-hour (Claude and GPT models)", 100.0, Some(8_000.0)),
                mk("week-1", "Weekly (Claude and GPT models)", 59.0, Some(40_000.0)),
            ],
            at: now,
            reset_credits: None, resets: Vec::new(),
        };
        assert_eq!(overall_left(Some(&agy_out), now), Some(0.0));

        // Legacy Antigravity fallback: summary group + standalone gemini bucket -> avg(min(session, week), gemini).
        let agy_legacy = Snapshot {
            provider: "agy".into(),
            windows: vec![
                mk("session", "5-hour", 100.0, Some(8_000.0)),
                mk("week", "Weekly", 59.0, Some(40_000.0)),
                mk("gemini", "Gemini models", 69.0, Some(5_000.0)),
            ],
            at: now,
            reset_credits: None, resets: Vec::new(),
        };
        let (legacy_left, legacy_reset) = overall_summary(Some(&agy_legacy), now).unwrap();
        assert!((legacy_left - 15.5).abs() < 1e-9);
        assert_eq!(legacy_reset, Some(5_000.0));

        // Claude: global 0% 5-hour + 60% weekly + 80% Weekly (Opus) must still read 0%.
        let claude = Snapshot {
            provider: "claude".into(),
            windows: vec![
                mk("session", "5-hour", 100.0, Some(5_000.0)),
                mk("week", "Weekly", 40.0, Some(50_000.0)),
                mk("week-opus", "Weekly (Opus)", 20.0, Some(50_000.0)),
            ],
            at: now,
            reset_credits: None, resets: Vec::new(),
        };
        assert_eq!(overall_summary(Some(&claude), now), Some((0.0, Some(5_000.0))));

        // Codex: exhausted plan falls back to monthly credits.
        let codex = Snapshot {
            provider: "codex".into(),
            windows: vec![
                mk("session", "5-hour", 100.0, Some(5_000.0)),
                mk("week", "Weekly", 40.0, Some(50_000.0)),
                mk("month-credit", "Monthly credit", 20.0, Some(90_000.0)),
            ],
            at: now,
            reset_credits: None, resets: Vec::new(),
        };
        assert_eq!(overall_summary(Some(&codex), now), Some((80.0, Some(90_000.0))));
    }

    #[test]
    fn claude_polling_throttles_and_backs_off_on_rate_limits() {
        let mut st = PollState::default();
        assert!(should_auto_poll("claude", None, 1_000.0));
        apply_poll_success(&mut st, "claude", 1_000.0);
        assert!(!should_auto_poll("claude", Some(&st), 1_000.0 + 60_000.0));
        assert!(should_auto_poll("claude", Some(&st), 1_000.0 + CLAUDE_POLL_INTERVAL_MS));

        // 429 backs off exponentially (2m -> 4m -> 8m -> 15m cap), honoring larger Retry-After.
        apply_poll_error(&mut st, UsageErrorKind::RateLimited, "429".into(), None, false, 10_000.0);
        assert_eq!(st.rate_limit_streak, 1);
        assert_eq!(st.next_allowed_at, 10_000.0 + RATE_LIMIT_BASE_MS);
        assert!(!should_auto_poll("claude", Some(&st), 10_000.0 + 60_000.0));

        apply_poll_error(&mut st, UsageErrorKind::RateLimited, "429".into(), Some(300), false, 20_000.0);
        assert_eq!(st.rate_limit_streak, 2);
        // 300s (5m) > 2nd backoff step (4m), so Retry-After wins.
        assert_eq!(st.next_allowed_at, 20_000.0 + 300_000.0);

        for _ in 0..5 {
            apply_poll_error(&mut st, UsageErrorKind::RateLimited, "429".into(), None, false, 30_000.0);
        }
        assert_eq!(st.next_allowed_at, 30_000.0 + RATE_LIMIT_MAX_MS);

        // Transient failure retries quickly when no snapshot exists, slower when one exists.
        apply_poll_error(&mut st, UsageErrorKind::Transient, "net".into(), None, false, 40_000.0);
        assert_eq!(st.rate_limit_streak, 0);
        assert_eq!(st.next_allowed_at, 40_000.0 + TRANSIENT_NO_DATA_RETRY_MS);
        apply_poll_error(&mut st, UsageErrorKind::Transient, "net".into(), None, true, 50_000.0);
        assert_eq!(st.next_allowed_at, 50_000.0 + TRANSIENT_WITH_DATA_RETRY_MS);

        // Success clears error state and streak.
        apply_poll_success(&mut st, "claude", 60_000.0);
        assert_eq!(st.rate_limit_streak, 0);
        assert!(st.last_error.is_none());
        assert!(st.error_kind.is_none());
    }

    #[test]
    fn prunes_expired_and_empty_samples() {
        let mut samples: HashMap<String, Vec<(f64, f64)>> = [
            ("claude:session:1000".into(), vec![(500.0, 20.0)]),
            ("claude:session:5000".into(), vec![(500.0, 20.0)]),
            ("claude:week:".into(), vec![(500.0, 10.0)]),
            ("claude:empty:5000".into(), vec![]),
        ]
        .into();
        prune_samples(&mut samples, 2000.0);
        assert!(!samples.contains_key("claude:session:1000"));
        assert!(!samples.contains_key("claude:empty:5000"));
        assert!(samples.contains_key("claude:session:5000"));
        assert!(samples.contains_key("claude:week:"));
    }
}
