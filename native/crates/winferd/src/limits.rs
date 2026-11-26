//! Usage and limits on the server: its own sign-ins polled like the desktop polls its own, and
//! `usage.get` answered the way the desktop answers its phone (plus, with `raw`, the readings and
//! token history themselves, which a paired desktop's Usage page shows for this machine).

use crate::host::Host;
use parking_lot::Mutex;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::LazyLock;
use std::time::{Duration, Instant};
use winfer_backend::usage_history;
use winfer_core::backend;
use winfer_core::usage::{Poller, Range, UsageWindow, due_targets, fetch, limits_json, load_accounts, now_ms, tokens_json};

static POLLER: std::sync::LazyLock<Mutex<Poller>> = std::sync::LazyLock::new(Mutex::default);
/// The token history (90 days of hourly rows) and when it was read.
static ROWS: Mutex<Option<(Instant, Vec<Value>)>> = Mutex::new(None);
/// One poll at a time (the timer's or a requested one).
static POLLING: Mutex<()> = Mutex::new(());

const CLAUDE_STAGGER: Duration = Duration::from_millis(350);

/// Loads the saved readings, then polls every minute (each account when it's due).
pub fn start() {
    POLLER.lock().load();
    std::thread::Builder::new()
        .name("winferd-usage".into())
        .spawn(|| {
            std::thread::sleep(Duration::from_secs(3));
            loop {
                poll(false);
                std::thread::sleep(Duration::from_secs(60));
            }
        })
        .expect("usage thread");
}

/// Polls the accounts that are due (all of them when `manual`).
pub fn poll(manual: bool) {
    let _one = POLLING.lock();
    let (accounts, cached) = load_accounts();
    let targets = {
        let mut p = POLLER.lock();
        p.set_accounts(accounts, cached);
        due_targets(&p.accounts, &p.poll, manual, now_ms())
    };
    let mut claudes = 0;
    for (provider, account) in targets {
        if provider == "claude" {
            if claudes > 0 {
                std::thread::sleep(CLAUDE_STAGGER);
            }
            claudes += 1;
        }
        let outcome = fetch(&provider, account.clone());
        POLLER.lock().apply(&provider, account.as_deref(), outcome, now_ms());
    }
    POLLER.lock().save();
}

/// A `usage://report` from an agent's status line here.
/// Each agent pane's context as its last report gave it (`source`: its transcript), for
/// phones reading its thread.
static CONTEXTS: LazyLock<Mutex<HashMap<String, Value>>> = LazyLock::new(Default::default);

/// The transcript of `pane` (provider, source), from its reports or, for Codex, its session.
pub fn transcript(pane: &winfer_core::model::Pane) -> Option<(String, String)> {
    if let Some(c) = CONTEXTS.lock().get(&pane.id)
        && let Some(source) = c["source"].as_str()
    {
        return Some((c["provider"].as_str().unwrap_or(&pane.provider).to_string(), source.to_string()));
    }
    if pane.provider != "codex" {
        return None;
    }
    let found = tauri::async_runtime::block_on(winfer_backend::usage::usage_codex_context(pane.session_id.clone(), pane.cwd.clone()))?;
    let c = serde_json::to_value(found).ok()?;
    Some(("codex".to_string(), c["source"].as_str()?.to_string()))
}

pub fn report(host: &Host, payload: &Value) {
    if let Some(pane) = payload["paneId"].as_str()
        && payload["report"]["context"].is_object()
    {
        CONTEXTS.lock().insert(pane.to_string(), payload["report"]["context"].clone());
    }
    let provider = payload["provider"].as_str().unwrap_or_default().to_string();
    let account = payload["paneId"].as_str().and_then(|id| host.with_state(|s| s.pane(id).and_then(|p| p.account_id.clone()))).filter(|a| a.split(':').next() == Some(provider.as_str()));
    let windows: Vec<UsageWindow> = serde_json::from_value(payload["report"]["windows"].clone()).unwrap_or_default();
    let mut p = POLLER.lock();
    p.report(&provider, account.as_deref(), windows, now_ms());
    p.save();
}

fn rows(fresh: bool) -> Vec<Value> {
    if !fresh && let Some((_, rows)) = ROWS.lock().as_ref().filter(|(at, _)| at.elapsed() < Duration::from_secs(60)) {
        return rows.clone();
    }
    let found = tauri::async_runtime::block_on(usage_history::usage_history(backend::app().clone(), 90));
    let rows: Vec<Value> = found.ok().and_then(|r| serde_json::to_value(r).ok()).and_then(|v| v.as_array().cloned()).unwrap_or_default();
    *ROWS.lock() = Some((Instant::now(), rows.clone()));
    rows
}

/// `usage.get {range?, refresh?, raw?}` → `usage {providers, now, tokens[, raw]}`.
pub fn answer(host: &Host, msg: &Value) -> Value {
    if msg["refresh"] == true {
        poll(true);
    }
    let range = msg["range"].as_str().and_then(Range::parse).unwrap_or(Range::Week);
    let rows = rows(msg["refresh"] == true);
    let now = now_ms();
    let name = |p: &str| host.with_state(|s| s.provider_name(p));
    let p = POLLER.lock();
    let mut usage = limits_json(&p.snapshots, &p.accounts, &p.samples, &name, now);
    usage["tokens"] = tokens_json(&rows, range, now, &name);
    if msg["raw"] == true {
        usage["raw"] = json!({"snapshots": p.snapshots, "samples": p.samples, "accounts": p.accounts, "rows": rows});
    }
    usage["t"] = json!("usage");
    usage
}
