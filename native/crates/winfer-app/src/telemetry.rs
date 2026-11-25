//! Anonymous usage data, sent to PostHog.
//!
//! What is sent: a random install id (not tied to the user or the machine), the app version,
//! OS and architecture, and these events:
//! - `app_started` (startup time, whether the previous run exited uncleanly) and `app_exited`
//!   (session length, memory high-water mark);
//! - `heartbeat` every hour (uptime, memory);
//! - `pane_opened` (agent provider, from a fixed list), `commands_used` (counts every 10 minutes per
//!   command name, never its arguments), `worktree_created`, `plugin_toggled` (store plugins by id, others as
//!   "local");
//! - `panic` (source file and line only, no message) and `tokens_processed` (token counts per
//!   provider and model per hour, from the usage history).
//!
//! - `workspace_size` (open workspaces and panes, when it changes, at most every 10 minutes),
//!   `settings_drift` once per launch (names of the preferences changed from the defaults; values
//!   only for fixed choices like the theme), `memory_milestone` after 1, 4, 8, 24 and 72 hours up,
//!   `effort_used` (winfer code turns per backend and reasoning effort, every 10 minutes);
//! - `tool_used`: the agents' tool calls per tool and hour (name, calls, failures, average
//!   time), from Claude Code's and Codex's logs; never inputs or results.
//! - `$ai_generation` and `$ai_span` (PostHog AI Observability) for live voice: model, token
//!   counts, latency, status and tool names, grouped by connection (`$ai_session_id`) and spoken
//!   turn (`$ai_trace_id`). Never the transcripts (`$ai_input` / `$ai_output_choices`).
//!
//! Never sent: prompts, transcripts, terminal output, file paths, folder or repo names, URLs, messages.
//!
//! On by default; Settings → Diagnostics turns it off, a first-run toast says it is on, and
//! `DO_NOT_TRACK=1` or `WINFER_TELEMETRY=0` turn it off for the process. Debug builds only send
//! with `WINFER_TELEMETRY=1`. Without a PostHog key at build time (`WINFER_POSTHOG_KEY`) nothing
//! is ever sent.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tauri::Manager;
use winfer_backend::{diagnostics, tool_stats, usage_history};

const DEFAULT_HOST: &str = "https://us.i.posthog.com";
const FILE: &str = "telemetry.json";
/// Events not sent yet (a panic, or a failed send at quit) go out at the next start.
const PENDING_FILE: &str = "telemetry-pending.json";
const FLUSH_EVERY: Duration = Duration::from_secs(60);
const HEARTBEAT_EVERY: Duration = Duration::from_secs(60 * 60);
/// Token use is checked this often; an hour goes out once it has been over for [`TOKENS_GRACE_MS`].
const TOKENS_EVERY: Duration = Duration::from_secs(10 * 60);
const TOKENS_GRACE_MS: i64 = 10 * 60 * 1000;
const MAX_QUEUE: usize = 500;
const HOUR_MS: i64 = 3_600_000;

/// Providers reported by name; anything else (custom commands) is "custom".
const KNOWN_PROVIDERS: &[&str] = &["winfer", "claude", "codex", "agy", "gemini", "grok", "opencode", "cursor", "pi", "muse", "shell"];

#[derive(Serialize, Deserialize, Default, Clone)]
#[serde(rename_all = "camelCase", default)]
struct Saved {
    install_id: String,
    disabled: bool,
    notice_shown: bool,
    /// Token use is reported up to here (unix ms, an hour's start).
    tokens_through: i64,
    /// Agents' tool calls are reported up to here (unix ms, an hour's start).
    tools_through: i64,
}

struct Telemetry {
    dir: PathBuf,
    saved: Mutex<Saved>,
    queue: Mutex<Vec<Value>>,
    commands: Mutex<std::collections::BTreeMap<String, u32>>,
    /// Turns sent in winfer code per (backend, effort).
    efforts: Mutex<std::collections::BTreeMap<(String, String), u32>>,
    /// Latest workspace size, the session's largest, and the last one sent.
    size: Mutex<(Option<WorkspaceSize>, WorkspaceSize, Option<WorkspaceSize>)>,
    /// The build has a key and the environment doesn't forbid it.
    allowed: bool,
    enabled: AtomicBool,
}

static T: OnceLock<Telemetry> = OnceLock::new();
static PROCESS_START: OnceLock<Instant> = OnceLock::new();

/// Call first thing in `main`: startup time counts from here.
pub fn mark_process_start() {
    PROCESS_START.get_or_init(Instant::now);
}

fn api_key() -> Option<&'static str> {
    option_env!("WINFER_POSTHOG_KEY").filter(|k| !k.is_empty())
}

fn host() -> &'static str {
    option_env!("WINFER_POSTHOG_HOST").filter(|h| !h.is_empty()).unwrap_or(DEFAULT_HOST).trim_end_matches('/')
}

fn env_allows() -> bool {
    let var = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    if var("DO_NOT_TRACK").is_some_and(|v| v != "0") {
        return false;
    }
    match var("WINFER_TELEMETRY").as_deref() {
        Some("0" | "false" | "off") => false,
        Some(_) => true,
        None => !cfg!(debug_assertions),
    }
}

fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64)
}

/// Loads the saved state, the events left from last run, and starts sending.
/// The core's telemetry hooks (`winfer_core::hooks`).
struct CoreHooks;

impl winfer_core::hooks::Hooks for CoreHooks {
    fn turn_sent(&self, model: &str, effort: &str, ultracode: bool) {
        turn_sent(model, effort, ultracode);
    }
    fn pane_opened(&self, provider: &str, effort: Option<&str>) {
        pane_opened(provider, effort);
    }
    fn worktree_created(&self, provider: &str) {
        track("worktree_created", serde_json::json!({ "provider": provider_label(provider) }));
    }
    fn started(&self, data_dir: &Path) {
        init(data_dir);
    }
    fn stopping(&self) {
        shutdown();
    }
    fn window_focused(&self) -> bool {
        use tauri::Manager;
        crate::backend::app().get_webview_window("main").and_then(|w| w.is_focused().ok()).unwrap_or(true)
    }
    fn orchestrated(&self, pane_id: &str) -> bool {
        crate::voice::orchestrator_workspace(pane_id).is_some()
    }
}

/// Before `backend::start`, which calls `init` through them.
pub fn install_hooks() {
    winfer_core::hooks::install(Box::new(CoreHooks));
}

pub fn init(data_dir: &Path) {
    let first_run = !data_dir.join(FILE).exists();
    let mut saved: Saved = std::fs::read_to_string(data_dir.join(FILE)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    if saved.install_id.is_empty() {
        saved.install_id = format!("{:032x}", rand::random::<u128>());
    }
    if first_run || saved.tokens_through == 0 {
        // Only use from now on, not the history already on disk.
        saved.tokens_through = now_ms() / HOUR_MS * HOUR_MS;
    }
    if saved.tools_through == 0 {
        saved.tools_through = now_ms() / HOUR_MS * HOUR_MS;
    }
    let pending: Vec<Value> = std::fs::read_to_string(data_dir.join(PENDING_FILE)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    let _ = std::fs::remove_file(data_dir.join(PENDING_FILE));
    let enabled = !saved.disabled;
    let t = Telemetry { dir: data_dir.to_path_buf(), saved: Mutex::new(saved), queue: Mutex::new(pending), commands: Mutex::default(), efforts: Mutex::default(), size: Mutex::default(), allowed: api_key().is_some() && env_allows(), enabled: AtomicBool::new(enabled) };
    if T.set(t).is_err() {
        return;
    }
    save();
    install_panic_hook();
    std::thread::Builder::new().name("telemetry".into()).spawn(run).ok();
}

fn save() {
    let Some(t) = T.get() else { return };
    let saved = t.saved.lock().unwrap_or_else(PoisonError::into_inner).clone();
    if let Ok(text) = serde_json::to_string_pretty(&saved) {
        let _ = std::fs::write(t.dir.join(FILE), text);
    }
}

/// Whether the user has it on (Settings).
pub fn enabled() -> bool {
    T.get().is_some_and(|t| t.enabled.load(Ordering::Relaxed))
}

/// Whether events actually leave the machine (on, and allowed by build and environment).
fn active() -> bool {
    T.get().is_some_and(|t| t.allowed && t.enabled.load(Ordering::Relaxed))
}

/// Whether this build can send at all (Settings explains when it can't).
pub fn available() -> bool {
    T.get().is_some_and(|t| t.allowed)
}

pub fn set_enabled(on: bool) {
    let Some(t) = T.get() else { return };
    if !on {
        // Opting out also drops what wasn't sent yet.
        t.queue.lock().unwrap_or_else(PoisonError::into_inner).clear();
    }
    t.enabled.store(on, Ordering::Relaxed);
    t.saved.lock().unwrap_or_else(PoisonError::into_inner).disabled = !on;
    save();
}

/// True once: the first time this is asked, so the UI shows the notice that usage data is on.
pub fn take_first_run_notice() -> bool {
    let Some(t) = T.get() else { return false };
    if !t.allowed {
        return false;
    }
    let show = {
        let mut saved = t.saved.lock().unwrap_or_else(PoisonError::into_inner);
        let show = !saved.notice_shown;
        saved.notice_shown = true;
        show
    };
    if show {
        save();
    }
    show
}

/// Queues an event (a no-op while off).
pub fn track(event: &str, props: Value) {
    let Some(t) = T.get() else { return };
    if !active() {
        return;
    }
    let distinct_id = t.saved.lock().unwrap_or_else(PoisonError::into_inner).install_id.clone();
    let entry = event_entry(event, props, distinct_id);
    let mut queue = t.queue.lock().unwrap_or_else(PoisonError::into_inner);
    if queue.len() < MAX_QUEUE {
        queue.push(entry);
    }
}

/// One event in PostHog's batch format, with the properties every event carries.
fn event_entry(event: &str, props: Value, distinct_id: String) -> Value {
    let mut properties = match props {
        Value::Object(m) => m,
        _ => Map::new(),
    };
    properties.insert("app_version".into(), json!(env!("CARGO_PKG_VERSION")));
    properties.insert("os".into(), json!(std::env::consts::OS));
    properties.insert("arch".into(), json!(std::env::consts::ARCH));
    properties.insert("$lib".into(), json!("winfer-native"));
    // Anonymous: no person profiles, no location from the IP.
    properties.insert("$process_person_profile".into(), json!(false));
    properties.insert("$geoip_disable".into(), json!(true));
    json!({ "event": event, "distinct_id": distinct_id, "properties": properties, "timestamp": iso_utc(now_ms()) })
}

/// A provider id as reported: known ones by name, custom ones as "custom".
pub fn provider_label(provider: &str) -> &'static str {
    KNOWN_PROVIDERS.iter().find(|p| **p == provider).copied().unwrap_or("custom")
}

pub fn pane_opened(provider: &str, effort: Option<&str>) {
    track("pane_opened", json!({ "provider": provider_label(provider), "effort": effort.map(plain_label) }));
}

/// A short identifier as is (`high`, `Bash`); anything else as "other".
fn plain_label(name: &str) -> String {
    let plain = !name.is_empty() && name.len() <= 40 && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.');
    if plain { name.to_string() } else { "other".to_string() }
}

/// A winfer code turn was sent at this reasoning effort (`model` is `backend/model`).
pub fn turn_sent(model: &str, effort: &str, ultracode: bool) {
    let Some(t) = T.get() else { return };
    if !active() {
        return;
    }
    let backend = provider_label(model.split('/').next().unwrap_or_default()).to_string();
    let effort = if ultracode { "ultracode".to_string() } else { plain_label(effort) };
    *t.efforts.lock().unwrap_or_else(PoisonError::into_inner).entry((backend, effort)).or_default() += 1;
}

fn report_efforts() {
    let Some(t) = T.get() else { return };
    let counts = std::mem::take(&mut *t.efforts.lock().unwrap_or_else(PoisonError::into_inner));
    for ((backend, effort), turns) in counts {
        track("effort_used", json!({ "backend": backend, "effort": effort, "turns": turns }));
    }
}

/// How many workspaces and panes are open.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WorkspaceSize {
    pub workspaces: usize,
    pub panes: usize,
    /// In the fullest workspace.
    pub max_panes: usize,
}

/// The UI saved its state: the open workspaces and panes now.
pub fn workspace_size(size: WorkspaceSize) {
    let Some(t) = T.get() else { return };
    let mut s = t.size.lock().unwrap_or_else(PoisonError::into_inner);
    s.0 = Some(size);
    s.1 = WorkspaceSize { workspaces: s.1.workspaces.max(size.workspaces), panes: s.1.panes.max(size.panes), max_panes: s.1.max_panes.max(size.max_panes) };
}

fn bucket(n: usize) -> &'static str {
    match n {
        0 => "0",
        1 => "1",
        2..=4 => "2-4",
        5..=9 => "5-9",
        _ => "10+",
    }
}

/// Sent when it changed since the last one, at most every 10 minutes.
fn report_workspace_size() {
    let Some(t) = T.get() else { return };
    let size = {
        let mut s = t.size.lock().unwrap_or_else(PoisonError::into_inner);
        match s.0 {
            Some(now) if s.2 != Some(now) => {
                s.2 = Some(now);
                Some((now, s.1))
            }
            _ => None,
        }
    };
    if let Some((now, max)) = size {
        track("workspace_size", json!({
            "workspaces": now.workspaces, "panes": now.panes, "max_panes_per_workspace": now.max_panes,
            "panes_bucket": bucket(now.panes), "workspaces_bucket": bucket(now.workspaces),
            "session_max_panes": max.panes, "session_max_workspaces": max.workspaces,
        }));
    }
}

/// Which settings differ from the defaults (names; values only for the ones in [`SETTING_VALUES`]).
/// Sent once per launch.
pub fn settings_drift(current: &Value, defaults: &Value) {
    let changed = changed_settings(current, defaults);
    let mut props = Map::new();
    for key in SETTING_VALUES {
        if let Some(v) = current.get(*key).filter(|v| v.is_boolean() || v.is_number() || v.is_string()) {
            let v = match v.as_str() {
                Some(s) if *key == "defaultProvider" => json!(provider_label(s)),
                Some(s) => json!(plain_label(s)),
                None => v.clone(),
            };
            props.insert(format!("setting_{key}"), v);
        }
    }
    props.insert("changed_count".into(), json!(changed.len()));
    props.insert("changed".into(), json!(changed));
    track("settings_drift", Value::Object(props));
}

/// Settings whose value is sent: choices from a fixed set, numbers and switches.
const SETTING_VALUES: &[&str] = &["theme", "themeMode", "defaultProvider", "autoApprove", "showPaneCost", "terminalFontSize", "pageTabIconsOnly"];

/// Settings compared with the defaults: preferences, not window state the app saves as it's used
/// (panel widths, the overview layout, the file viewer).
const SETTING_KEYS: &[&str] = &[
    "theme", "themeMode", "themePicks", "defaultProvider", "autoApprove", "showPaneCost", "terminalFontSize",
    "pageTabIconsOnly", "editor", "shortcuts", "systemPrompt", "enabledTools", "agentModels",
];

fn changed_settings(current: &Value, defaults: &Value) -> Vec<&'static str> {
    SETTING_KEYS.iter().copied().filter(|k| current.get(*k).unwrap_or(&Value::Null) != defaults.get(*k).unwrap_or(&Value::Null)).collect()
}

/// Counts a command by its name without its arguments (`Spawn { provider: .. }` → `Spawn`).
/// The counts go out every 10 minutes as one `commands_used` event.
pub fn command_used(cmd: &impl std::fmt::Debug) {
    let Some(t) = T.get() else { return };
    if !active() {
        return;
    }
    let name = command_name(&format!("{cmd:?}"));
    if !name.is_empty() {
        *t.commands.lock().unwrap_or_else(PoisonError::into_inner).entry(name).or_default() += 1;
    }
}

fn command_name(debug: &str) -> String {
    debug.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect()
}

fn report_commands() {
    let Some(t) = T.get() else { return };
    let counts = std::mem::take(&mut *t.commands.lock().unwrap_or_else(PoisonError::into_inner));
    if !counts.is_empty() {
        track("commands_used", json!({ "counts": counts }));
    }
}

/// A random id for AI Observability sessions, traces and spans.
pub fn new_ai_id() -> String {
    let n = rand::random::<u128>();
    let h = format!("{n:032x}");
    format!("{}-{}-{}-{}-{}", &h[..8], &h[8..12], &h[12..16], &h[16..20], &h[20..])
}

/// One live-voice model response, without its input or output.
pub struct AiGeneration<'a> {
    pub session_id: &'a str,
    pub trace_id: &'a str,
    pub model: &'a str,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub latency_secs: f64,
    pub error: bool,
}

pub fn ai_generation(g: AiGeneration) {
    track("$ai_generation", json!({
        "$ai_session_id": g.session_id, "$ai_trace_id": g.trace_id, "$ai_span_id": new_ai_id(),
        "$ai_provider": "openai", "$ai_model": g.model,
        "$ai_input_tokens": g.input_tokens, "$ai_output_tokens": g.output_tokens,
        "$ai_latency": g.latency_secs, "$ai_is_error": g.error, "feature": "live_voice",
    }));
}

/// A tool the live-voice model ran (winfer's own tools, by name).
pub fn ai_span(session_id: &str, trace_id: &str, name: &str, latency_secs: f64) {
    track("$ai_span", json!({
        "$ai_session_id": session_id, "$ai_trace_id": trace_id, "$ai_span_id": new_ai_id(),
        "$ai_span_name": name, "$ai_latency": latency_secs, "feature": "live_voice",
    }));
}

/// The UI is up: reports the startup time and how the last run ended.
pub fn app_ready() {
    if !active() {
        return;
    }
    let startup_ms = PROCESS_START.get().map(|s| s.elapsed().as_millis() as u64);
    std::thread::spawn(move || {
        let info = diagnostics::diagnostics_info(crate::backend::app().clone());
        track("app_started", json!({ "startup_ms": startup_ms, "previous_unexpected_exit": info.previous_unexpected_exit, "system_memory_mb": mb(info.memory.system_total_bytes) }));
    });
}

/// At quit: the session's end, then a last send (kept for next time if it fails).
pub fn shutdown() {
    let Some(t) = T.get() else { return };
    if active() {
        let mem = diagnostics::memory_snapshot();
        let session_secs = PROCESS_START.get().map(|s| s.elapsed().as_secs());
        report_commands();
        report_efforts();
        let max = T.get().map(|t| t.size.lock().unwrap_or_else(PoisonError::into_inner).1).unwrap_or_default();
        track("app_exited", json!({ "session_secs": session_secs, "peak_rss_mb": mb(mem.peak_rss_bytes), "session_max_panes": max.panes, "session_max_workspaces": max.workspaces }));
    }
    if !flush(Duration::from_secs(3)) {
        write_pending(&t.queue.lock().unwrap_or_else(PoisonError::into_inner));
    }
}

fn mb(bytes: Option<u64>) -> Option<u64> {
    bytes.map(|b| b / (1024 * 1024))
}

fn write_pending(events: &[Value]) {
    let Some(t) = T.get() else { return };
    if events.is_empty() || !active() {
        return;
    }
    if let Ok(text) = serde_json::to_string(events) {
        let _ = std::fs::write(t.dir.join(PENDING_FILE), text);
    }
}

/// Sends what's queued. False if it couldn't (the events stay queued).
fn flush(timeout: Duration) -> bool {
    let (Some(t), Some(key)) = (T.get(), api_key()) else { return true };
    if !active() {
        t.queue.lock().unwrap_or_else(PoisonError::into_inner).clear();
        return true;
    }
    let batch: Vec<Value> = std::mem::take(&mut *t.queue.lock().unwrap_or_else(PoisonError::into_inner));
    if batch.is_empty() {
        return true;
    }
    let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(timeout)).build().into();
    let body = json!({ "api_key": key, "batch": batch });
    match agent.post(&format!("{}/batch/", host())).header("User-Agent", "winfer").send_json(&body) {
        Ok(_) => true,
        Err(e) => {
            diagnostics::debug("telemetry", &format!("send failed: {e}"));
            let mut queue = t.queue.lock().unwrap_or_else(PoisonError::into_inner);
            let mut kept = batch;
            kept.append(&mut queue);
            kept.truncate(MAX_QUEUE);
            *queue = kept;
            false
        }
    }
}

/// Hours of uptime at which memory is reported, to catch growth that only long sessions show.
const MEMORY_MILESTONES_H: [u64; 5] = [1, 4, 8, 24, 72];
static NEXT_MILESTONE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

fn report_memory_milestones() {
    let Some(up) = PROCESS_START.get().map(|s| s.elapsed().as_secs() / 3600) else { return };
    let next = NEXT_MILESTONE.load(Ordering::Relaxed);
    // Only the latest milestone passed (a sleeping laptop may skip several).
    let Some(passed) = MEMORY_MILESTONES_H.iter().rposition(|h| up >= *h).filter(|i| *i >= next) else { return };
    NEXT_MILESTONE.store(passed + 1, Ordering::Relaxed);
    let mem = diagnostics::memory_snapshot();
    let panes = T.get().and_then(|t| t.size.lock().unwrap_or_else(PoisonError::into_inner).0).map(|s| s.panes);
    track("memory_milestone", json!({
        "hours": MEMORY_MILESTONES_H[passed], "rss_mb": mb(mem.rss_bytes), "peak_rss_mb": mb(mem.peak_rss_bytes), "panes": panes,
        "pty_sessions": diagnostics::diagnostics_info(crate::backend::app().clone()).pty_sessions,
    }));
}

fn run() {
    let mut last_heartbeat = Instant::now();
    // Token use and the heartbeat go out once the app has been up for a while.
    // The first check a couple of minutes after start.
    let mut last_tokens = Instant::now() - TOKENS_EVERY + Duration::from_secs(2 * 60);
    loop {
        std::thread::sleep(FLUSH_EVERY);
        if last_tokens.elapsed() >= TOKENS_EVERY {
            last_tokens = Instant::now();
            report_tokens();
            report_tools();
            report_commands();
            report_efforts();
            report_workspace_size();
            report_memory_milestones();
        }
        if last_heartbeat.elapsed() >= HEARTBEAT_EVERY {
            last_heartbeat = Instant::now();
            let mem = diagnostics::memory_snapshot();
            let uptime_min = PROCESS_START.get().map(|s| s.elapsed().as_secs() / 60);
            track("heartbeat", json!({ "uptime_min": uptime_min, "rss_mb": mb(mem.rss_bytes), "peak_rss_mb": mb(mem.peak_rss_bytes) }));
        }
        flush(Duration::from_secs(15));
    }
}

/// Tokens per provider and model for each finished hour not reported yet (once it has been over
/// for a few minutes, so the agents' last log lines are in). The logs are read again first.
fn report_tokens() {
    let Some(t) = T.get() else { return };
    let since = t.saved.lock().unwrap_or_else(PoisonError::into_inner).tokens_through;
    let until = (now_ms() - TOKENS_GRACE_MS) / HOUR_MS * HOUR_MS;
    if until <= since {
        return;
    }
    if active() {
        let app = crate::backend::app();
        let history = app.state::<usage_history::UsageHistory>();
        history.scan_if_stale(Duration::from_secs(5 * 60));
        let Ok(buckets) = history.buckets(since, &usage_history::Prices::load()) else { return };
        for (hour, provider, model, [input, output, cache_read, cache_write]) in token_rows(&buckets, until) {
            track("tokens_processed", json!({
                "hour": iso_utc(hour), "provider": provider, "model": model,
                "input": input, "output": output, "cache_read": cache_read, "cache_write": cache_write,
                "total": input + output + cache_read + cache_write,
            }));
        }
    }
    t.saved.lock().unwrap_or_else(PoisonError::into_inner).tokens_through = until;
    save();
}

/// The agents' tool calls (Claude Code, Codex) per tool for each finished hour not reported
/// yet: calls, failures and average time. Tool names only, never inputs or results.
fn report_tools() {
    let Some(t) = T.get() else { return };
    let since = t.saved.lock().unwrap_or_else(PoisonError::into_inner).tools_through;
    let until = (now_ms() - TOKENS_GRACE_MS) / HOUR_MS * HOUR_MS;
    if until <= since {
        return;
    }
    if active() {
        // `analyze` counts from a start on; the window is the difference of two.
        let (from, after) = (tool_stats::analyze(since), tool_stats::analyze(until));
        for row in tool_rows(&from.tools, &after.tools) {
            track("tool_used", json!({
                "hour": iso_utc(since), "hours": (until - since) / HOUR_MS, "tool": row.name,
                "calls": row.calls, "failed": row.failed, "avg_ms": row.duration_ms.checked_div(row.timed),
            }));
        }
    }
    t.saved.lock().unwrap_or_else(PoisonError::into_inner).tools_through = until;
    save();
}

const MAX_TOOLS: usize = 50;

/// Per tool, `from` minus `after`: the calls in between. Names that don't look like a plain
/// tool name are counted as "other".
fn tool_rows(from: &[tool_stats::Stat], after: &[tool_stats::Stat]) -> Vec<tool_stats::Stat> {
    let mut out: Vec<tool_stats::Stat> = Vec::new();
    for s in from {
        let later = after.iter().find(|a| a.name == s.name);
        let sub = |f: fn(&tool_stats::Stat) -> u64| f(s).saturating_sub(later.map_or(0, f));
        let calls = sub(|x| x.calls);
        if calls == 0 {
            continue;
        }
        let name = tool_name(&s.name);
        let row = match out.iter_mut().find(|r| r.name == name) {
            Some(r) => r,
            None => {
                out.push(tool_stats::Stat { name, ..Default::default() });
                out.last_mut().unwrap()
            }
        };
        row.calls += calls;
        row.failed += sub(|x| x.failed);
        row.duration_ms += sub(|x| x.duration_ms);
        row.timed += sub(|x| x.timed);
    }
    out.sort_by(|a, b| b.calls.cmp(&a.calls));
    out.truncate(MAX_TOOLS);
    out
}

fn tool_name(name: &str) -> String {
    let plain = !name.is_empty() && name.len() <= 40 && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    if plain { name.to_string() } else { "other".to_string() }
}

type TokenRow = (i64, String, String, [u64; 4]);

/// The buckets of hours before `until`, with the provider as reported.
fn token_rows(buckets: &[usage_history::Bucket], until: i64) -> Vec<TokenRow> {
    buckets
        .iter()
        .filter(|b| b.hour < until)
        .filter(|b| b.input + b.output + b.cache_read + b.cache_write > 0)
        .map(|b| (b.hour, provider_label(&b.provider).to_string(), b.model.clone(), [b.input, b.output, b.cache_read, b.cache_write]))
        .collect()
}

/// Reports a panic's place in the source (never its message), kept for the next start.
fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        // Only try_lock here: the panicking thread may hold a lock already.
        if active()
            && let Some(t) = T.get()
            && let Ok(saved) = t.saved.try_lock()
            && let Ok(mut queue) = t.queue.try_lock()
        {
            let location = info.location().map_or_else(|| "?".to_string(), |l| format!("{}:{}", short_path(l.file()), l.line()));
            let thread = if std::thread::current().name() == Some("main") { "main" } else { "other" };
            queue.push(event_entry("panic", json!({ "location": location, "thread": thread }), saved.install_id.clone()));
            write_pending(&queue);
        }
        previous(info);
    }));
}

/// The last three parts of a source path: drops the home folder in registry and git checkouts.
fn short_path(file: &str) -> String {
    let parts: Vec<&str> = file.split(['/', '\\']).filter(|p| !p.is_empty() && *p != "..").collect();
    parts[parts.len().saturating_sub(3)..].join("/")
}

/// Unix ms as an RFC 3339 UTC timestamp (`2026-10-20T19:43:24Z`).
fn iso_utc(ms: i64) -> String {
    let secs = ms.max(0) / 1000;
    let (days, rem) = (secs / 86_400, secs % 86_400);
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_timestamps() {
        assert_eq!(iso_utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(iso_utc(1_792_000_000_000), "2026-10-14T17:46:40Z");
    }

    #[test]
    fn strips_command_arguments() {
        assert_eq!(command_name("Spawn { provider: Some(\"/Users/x\") }"), "Spawn");
        assert_eq!(command_name("Settings(Some(\"diagnostics\"))"), "Settings");
        assert_eq!(command_name("ToggleBrowser"), "ToggleBrowser");
    }

    #[test]
    fn keeps_only_the_end_of_source_paths() {
        assert_eq!(short_path("/Users/someone/.cargo/registry/src/index/gpui-0.2/src/window.rs"), "gpui-0.2/src/window.rs");
        assert_eq!(short_path("../src-tauri/src/pty.rs"), "src-tauri/src/pty.rs");
        assert_eq!(short_path("src\\ui\\root.rs"), "src/ui/root.rs");
    }

    #[test]
    fn ai_ids_look_like_uuids() {
        let id = new_ai_id();
        assert_eq!(id.len(), 36);
        assert_eq!(id.matches('-').count(), 4);
    }

    #[test]
    fn tool_rows_are_the_difference_by_tool() {
        let st = |name: &str, calls, failed, ms| tool_stats::Stat { name: name.into(), calls, failed, duration_ms: ms, timed: calls, ..Default::default() };
        let rows = tool_rows(&[st("Bash", 10, 2, 1000), st("Read", 3, 0, 30), st("weird name/x", 1, 0, 5)], &[st("Bash", 4, 1, 400), st("Read", 3, 0, 30)]);
        assert_eq!(rows.iter().map(|r| (r.name.as_str(), r.calls, r.failed, r.duration_ms)).collect::<Vec<_>>(), vec![("Bash", 6, 1, 600), ("other", 1, 0, 5)]);
    }

    #[test]
    fn settings_drift_names_changed_preferences_only() {
        let defaults = json!({ "theme": "dark", "autoApprove": false, "sidebarWidth": 260, "activeWorkspaceId": "a" });
        let current = json!({ "theme": "light", "autoApprove": false, "sidebarWidth": 300, "activeWorkspaceId": "b", "systemPrompt": "secret" });
        assert_eq!(changed_settings(&current, &defaults), vec!["theme", "systemPrompt"]);
    }

    #[test]
    fn buckets_and_labels() {
        assert_eq!([bucket(0), bucket(1), bucket(3), bucket(7), bucket(12)], ["0", "1", "2-4", "5-9", "10+"]);
        assert_eq!(plain_label("xhigh"), "xhigh");
        assert_eq!(plain_label("my effort/x"), "other");
    }

    #[test]
    fn labels_custom_providers() {
        assert_eq!(provider_label("claude"), "claude");
        assert_eq!(provider_label("my-secret-tool"), "custom");
    }

    #[test]
    fn token_rows_skip_the_open_hour_and_empty_rows() {
        let b = |hour, provider: &str, input| usage_history::Bucket { hour, provider: provider.into(), model: "m".into(), input, output: 1, cache_read: 0, cache_write: 0, cost: 0.0, priced: true };
        let rows = token_rows(&[b(0, "claude", 10), b(HOUR_MS, "mine", 5), b(2 * HOUR_MS, "codex", 7)], 2 * HOUR_MS);
        assert_eq!(rows, vec![(0, "claude".into(), "m".into(), [10, 1, 0, 0]), (HOUR_MS, "custom".into(), "m".into(), [5, 1, 0, 0])]);
    }
}
