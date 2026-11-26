//! Review on the server: when an agent finishes, its work becomes a review that paired desktops
//! see (`reviews`, pushed on change). They merge it (through this server's merge queue), run its
//! tests, send it back to the agent, discard or dismiss it with `review.*` requests; all of it
//! runs here, in the agent's worktree on this machine.

use crate::host::Host;
use parking_lot::Mutex;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tauri::Manager;
use winfer_backend::{llm, review as review_backend, store, worktree};
use winfer_core::model::{AgentStatus, Pane};
use winfer_core::review::{DiffStats, TestRun, commit_message, conflict_note, diff_stats, last_output, resolve_prompt, summary_prompt, wrong_script};
use winfer_core::terminal::SESSIONS;
use winfer_core::{backend, connect};

/// How long an agent must stay finished before its work is collected (hooks fire per turn).
const SETTLE: Duration = Duration::from_millis(1500);

#[derive(Clone)]
enum Load<T> {
    Idle,
    Off,
    Loading,
    Done(T),
    Error(String),
}

impl<T> Load<T> {
    fn state(&self) -> &'static str {
        match self {
            Load::Idle => "idle",
            Load::Off => "off",
            Load::Loading => "loading",
            Load::Done(_) => "done",
            Load::Error(_) => "error",
        }
    }
    fn error(&self) -> Option<&str> {
        if let Load::Error(e) = self { Some(e) } else { None }
    }
}

#[derive(Clone)]
struct Review {
    id: String,
    pane_id: String,
    workspace_id: String,
    provider: String,
    title: String,
    prompt: Option<String>,
    finished_at: u64,
    worktree: Option<worktree::WorktreeInfo>,
    dir: String,
    stats: Option<DiffStats>,
    last_output: String,
    summary: Load<String>,
    tests: Load<Option<TestRun>>,
    test_command: Option<String>,
    conflict: Option<(bool, String)>,
    /// The agent's dev server, and a screenshot of it (JPEG data URL) from this server's browser.
    dev_url: Option<String>,
    screenshot: Load<String>,
}

impl Review {
    fn json(&self) -> Value {
        let summary = if let Load::Done(t) = &self.summary { Some(t.as_str()) } else { None };
        let tests = if let Load::Done(t) = &self.tests { json!(t) } else { Value::Null };
        json!({
            "id": self.id, "paneId": self.pane_id, "workspaceId": self.workspace_id, "provider": self.provider,
            "title": self.title, "prompt": self.prompt, "finishedAt": self.finished_at,
            "branch": self.worktree.as_ref().map(|w| &w.branch), "base": self.worktree.as_ref().map(|w| &w.base), "dir": self.dir,
            "stats": self.stats, "lastOutput": self.last_output,
            "summary": {"state": self.summary.state(), "text": summary, "error": self.summary.error()},
            "tests": {"state": self.tests.state(), "run": tests, "error": self.tests.error()},
            "testCommand": self.test_command,
            "conflict": self.conflict.as_ref().map(|(base, text)| json!({"base": base, "text": text})),
            // The image itself is fetched with `review.screenshot` (too big to push every change).
            "screenshot": {"state": self.screenshot.state(), "url": self.dev_url, "error": self.screenshot.error()},
        })
    }
}

#[derive(Clone)]
struct QueueItem {
    review_id: String,
    branch: String,
    base: String,
    /// waiting, syncing, testing, merging, merged, conflict, failed
    state: &'static str,
    detail: Option<String>,
    files: Vec<String>,
    tests: bool,
}

impl QueueItem {
    fn pending(&self) -> bool {
        matches!(self.state, "waiting" | "syncing" | "testing" | "merging")
    }
}

#[derive(Default)]
pub struct Reviews {
    items: Mutex<Vec<Review>>,
    queue: Mutex<Vec<QueueItem>>,
    running: AtomicBool,
    /// Agents that finished, collected once they stay finished.
    finished: Mutex<HashMap<String, Instant>>,
    last_sent: Mutex<String>,
    conflicts_at: Mutex<Option<Instant>>,
}

static REVIEWS: std::sync::LazyLock<Reviews> = std::sync::LazyLock::new(Reviews::default);

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

fn id() -> String {
    winfer_core::model::new_id("rev")
}

fn bridged(info: &winfer_core::model::WorktreeInfo) -> Option<worktree::WorktreeInfo> {
    serde_json::to_value(info).ok().and_then(|v| serde_json::from_value(v).ok())
}

fn with_review<R>(id: &str, f: impl FnOnce(&mut Review) -> R) -> Option<R> {
    REVIEWS.items.lock().iter_mut().find(|r| r.id == id).map(f)
}

fn patch_queue(id: &str, state: &'static str, detail: Option<String>, files: Vec<String>) {
    if let Some(item) = REVIEWS.queue.lock().iter_mut().find(|i| i.review_id == id) {
        item.state = state;
        item.detail = detail;
        item.files = files;
    }
}

/// An agent finished a turn: its work is collected if it stays finished.
pub fn finished(pane_id: &str) {
    REVIEWS.finished.lock().insert(pane_id.to_string(), Instant::now());
}

/// What paired machines get: the open reviews and the merge queue.
pub fn message() -> Value {
    let items: Vec<Value> = REVIEWS.items.lock().iter().map(Review::json).collect();
    let queue: Vec<Value> = REVIEWS.queue.lock().iter().map(|i| json!({"reviewId": i.review_id, "branch": i.branch, "base": i.base, "state": i.state, "detail": i.detail, "files": i.files})).collect();
    json!({"t": "reviews", "items": items, "queue": queue})
}

/// Once a second: collects settled agents, refreshes conflicts now and then, pushes changes.
pub fn tick(host: &'static Host) {
    let ready: Vec<String> = {
        let mut finished = REVIEWS.finished.lock();
        let ready: Vec<String> = finished.iter().filter(|(_, at)| at.elapsed() >= SETTLE).map(|(id, _)| id.clone()).collect();
        finished.retain(|id, _| !ready.contains(id));
        ready
    };
    for pane_id in ready {
        let pane = host.with_state(|s| s.pane(&pane_id).cloned());
        if let Some(pane) = pane.filter(|p| p.status != AgentStatus::Working && p.provider != "shell") {
            std::thread::spawn(move || collect(host, pane));
        }
    }
    let stale = REVIEWS.conflicts_at.lock().is_none_or(|t| t.elapsed() > Duration::from_secs(60));
    if stale && REVIEWS.items.lock().iter().any(|r| r.worktree.is_some()) {
        *REVIEWS.conflicts_at.lock() = Some(Instant::now());
        std::thread::spawn(refresh_conflicts);
    }
    let msg = message();
    let text = msg.to_string();
    let mut last = REVIEWS.last_sent.lock();
    if *last != text {
        *last = text;
        connect::broadcast(&msg);
    }
}

fn collect(host: &'static Host, pane: Pane) {
    let info = pane.worktree.as_ref().and_then(bridged);
    let mut diff = String::new();
    let mut stats = None;
    if let Some(info) = &info {
        let Ok(d) = worktree::diff(info) else { return };
        let v = serde_json::to_value(d).unwrap_or_default();
        let computed = diff_stats(&v);
        // A turn that changed nothing (a question, a plan) isn't work to review.
        if computed.files.is_empty() && computed.ahead == 0 {
            return;
        }
        diff = v["diff"].as_str().unwrap_or_default().to_string();
        stats = Some(computed);
    }
    let screen = SESSIONS.get(&pane.id).map(|s| s.read_text(60)).unwrap_or_default();
    let (title, prompt) = host.with_state(|s| (s.pane_label(&pane), pane.prompt.clone().or_else(|| s.pane_tasks.get(&pane.id).and_then(|t| t.task.clone()))));
    let dir = info.as_ref().map_or_else(|| pane.cwd.clone(), |i| i.path.clone());
    let existing = REVIEWS.items.lock().iter().find(|r| r.pane_id == pane.id).map(|r| r.id.clone());
    let review = Review {
        id: existing.unwrap_or_else(id),
        pane_id: pane.id.clone(),
        workspace_id: pane.workspace_id.clone(),
        provider: pane.provider.clone(),
        title,
        prompt,
        finished_at: now_ms(),
        worktree: info,
        dir: dir.clone(),
        stats,
        last_output: last_output(&screen),
        summary: Load::Idle,
        tests: Load::Idle,
        test_command: review_backend::test_command(std::path::Path::new(&dir)),
        conflict: None,
        dev_url: pane.dev_url.clone(),
        screenshot: if pane.dev_url.is_some() { Load::Loading } else { Load::Off },
    };
    let id = review.id.clone();
    {
        let mut items = REVIEWS.items.lock();
        items.retain(|r| r.id != id);
        items.insert(0, review.clone());
    }
    *REVIEWS.conflicts_at.lock() = None;
    if let Some(url) = review.dev_url.clone() {
        let (id, ws) = (review.id.clone(), review.workspace_id.clone());
        std::thread::spawn(move || screenshot(&id, ws, url));
    }
    summarize(host, review, diff);
}

/// The agent's running app, as the desktop's reviews show it: only with a browser already here
/// (a review doesn't download one).
fn screenshot(id: &str, workspace: String, url: String) {
    let app = backend::app();
    let has_browser = winfer_backend::browser::browser_status(app.clone()).ok().is_some_and(|s| !json!(s)["executable"].is_null());
    if !has_browser {
        if std::env::var_os("WINFER_CONNECT_TRACE").is_some() {
            eprintln!("[review] no browser here: no screenshot of {url}");
        }
        with_review(id, |r| r.screenshot = Load::Off);
        return;
    }
    let shot = tauri::async_runtime::block_on(winfer_backend::browser::browser_screenshot(app.clone(), app.state::<winfer_backend::browser::BrowserManager>(), workspace, url.clone()));
    if std::env::var_os("WINFER_CONNECT_TRACE").is_some() {
        eprintln!("[review] screenshot of {url}: {}", shot.as_ref().map_or_else(|e| format!("failed: {e}"), |d| format!("{} bytes", d.len())));
    }
    with_review(id, |r| {
        r.screenshot = match shot {
            Ok(data) => Load::Done(data),
            Err(e) => Load::Error(e.to_string()),
        }
    });
}

/// The agent CLI that writes summaries here: `WINFERD_REVIEW_SUMMARY` (codex, claude or off),
/// else the first one installed.
fn writer(host: &Host) -> Option<String> {
    let choice = std::env::var("WINFERD_REVIEW_SUMMARY").ok().filter(|c| !c.is_empty());
    if choice.as_deref() == Some("off") {
        return None;
    }
    let installed: Vec<String> = host.with_state(|s| s.providers.iter().filter(|p| p.installed && matches!(p.id.as_str(), "codex" | "claude")).map(|p| p.id.clone()).collect());
    installed.iter().find(|p| Some(*p) == choice.as_ref()).or(installed.first()).cloned()
}

fn summarize(host: &Host, review: Review, diff: String) {
    let Some(provider) = writer(host) else {
        with_review(&review.id, |r| r.summary = Load::Off);
        return;
    };
    with_review(&review.id, |r| r.summary = Load::Loading);
    let (system, user) = summary_prompt(review.prompt.as_deref(), review.stats.as_ref(), &diff, &review.last_output);
    let task = review.prompt.clone().unwrap_or_default();
    let ask = || -> Result<String, String> {
        let req = serde_json::from_value(json!({"provider": provider, "model": "", "system": system, "user": user, "fast": true})).map_err(|e| e.to_string())?;
        tauri::async_runtime::block_on(llm::llm_compose(req))
    };
    let result = ask().and_then(|text| if wrong_script(&text, &task) { ask() } else { Ok(text) });
    with_review(&review.id, |r| {
        r.summary = match result {
            Ok(t) => Load::Done(t.trim().to_string()),
            Err(e) => Load::Error(e),
        }
    });
}

fn run_tests(dir: &str) -> Result<Option<TestRun>, String> {
    let run = tauri::async_runtime::block_on(review_backend::review_run_tests(dir.to_string()))?;
    serde_json::to_value(run).and_then(serde_json::from_value).map_err(|e| e.to_string())
}

fn refresh_conflicts() {
    let infos: Vec<worktree::WorktreeInfo> = REVIEWS.items.lock().iter().filter_map(|r| r.worktree.clone()).collect();
    if infos.is_empty() {
        return;
    }
    let report = serde_json::to_value(worktree::conflicts(&infos)).ok();
    for r in REVIEWS.items.lock().iter_mut() {
        r.conflict = conflict_note(report.as_ref(), &r.dir);
    }
}

// ---- requests ----

/// A `review.*` request from a paired machine; returns its answer.
pub fn on_request(host: &'static Host, msg: &Value) -> Value {
    let kind = msg["t"].as_str().unwrap_or_default();
    let id = msg["reviewId"].as_str().unwrap_or_default().to_string();
    let no = |e: &str| json!({"t": "ack", "ok": false, "error": e});
    let ok = json!({"t": "ack", "ok": true});
    if kind == "review.clear" {
        REVIEWS.queue.lock().retain(QueueItem::pending);
        return ok;
    }
    if kind == "review.unqueue" {
        REVIEWS.queue.lock().retain(|i| i.review_id != id || (i.pending() && i.state != "waiting"));
        return ok;
    }
    if kind == "review.history" {
        // This server's merges (History), newest first.
        let query = msg["query"].as_str().unwrap_or_default().to_string();
        return match winfer_backend::store::history_search(winfer_core::backend::app().state(), query, None) {
            Ok(records) => json!({"t": "review.history", "records": records}),
            Err(e) => no(&e),
        };
    }
    let Some(review) = REVIEWS.items.lock().iter().find(|r| r.id == id).cloned() else { return no("That review is gone.") };
    let pane = host.with_state(|s| s.pane(&review.pane_id).cloned());
    match kind {
        "review.screenshot" => match &review.screenshot {
            Load::Done(data) => json!({"t": "review.screenshot", "reviewId": id, "data": data, "url": review.dev_url}),
            _ => no("No screenshot of this review."),
        },
        "review.diff" => {
            let Some(info) = &review.worktree else { return no("Only work in a worktree has changes to show.") };
            match worktree::diff(info) {
                Ok(d) => {
                    let mut v = serde_json::to_value(d).unwrap_or_default();
                    v["t"] = json!("review.diff");
                    v["reviewId"] = json!(id);
                    v
                }
                Err(e) => no(&e.to_string()),
            }
        }
        "review.merge" => {
            let Some(info) = &review.worktree else { return no("Only work in a worktree can be merged.") };
            let mut queue = REVIEWS.queue.lock();
            if queue.iter().any(|i| i.review_id == id && i.pending()) {
                return ok;
            }
            queue.retain(|i| i.review_id != id);
            queue.push(QueueItem { review_id: id, branch: info.branch.clone(), base: info.base.clone(), state: "waiting", detail: None, files: Vec::new(), tests: msg["tests"] == true });
            drop(queue);
            run_queue(host);
            ok
        }
        "review.tests" => {
            if matches!(review.tests, Load::Loading) {
                return ok;
            }
            with_review(&id, |r| r.tests = Load::Loading);
            std::thread::spawn(move || {
                let result = run_tests(&review.dir);
                with_review(&id, |r| {
                    r.tests = match result {
                        Ok(v) => Load::Done(v),
                        Err(e) => Load::Error(e),
                    }
                });
            });
            ok
        }
        "review.send" | "review.resolve" => {
            let Some(pane) = pane else { return no("That agent's pane is gone.") };
            let text = if kind == "review.resolve" {
                let item = REVIEWS.queue.lock().iter().find(|i| i.review_id == id).cloned();
                let Some(item) = item else { return no("That merge isn't in the queue.") };
                resolve_prompt(&item.branch, &item.base, &item.files)
            } else {
                msg["text"].as_str().map(str::trim).unwrap_or_default().to_string()
            };
            if text.is_empty() {
                return no("Nothing to send.");
            }
            host.type_into(&pane, &text);
            host.log(&pane, format!("Sent back: {}", text.chars().take(80).collect::<String>()));
            drop_review(&id);
            ok
        }
        "review.discard" => {
            let Some(info) = review.worktree.clone() else { return no("Only a worktree can be discarded.") };
            if let Some(pane) = &pane {
                winfer_core::terminal::close(&pane.id);
                host.log(pane, format!("Discarded {}", info.branch));
                host.with_state(|s| s.remove_pane(&pane.id));
            }
            drop_review(&id);
            match worktree::remove(&info, true) {
                Ok(()) => ok,
                Err(e) => no(&format!("Couldn't remove the worktree: {e}")),
            }
        }
        "review.dismiss" => {
            drop_review(&id);
            ok
        }
        _ => no("A winfer server can't do that yet."),
    }
}

fn drop_review(id: &str) {
    REVIEWS.items.lock().retain(|r| r.id != id);
    REVIEWS.queue.lock().retain(|i| i.review_id != id || !i.pending());
}

// ---- merge queue ----

fn run_queue(host: &'static Host) {
    if REVIEWS.running.swap(true, Ordering::SeqCst) {
        return;
    }
    std::thread::spawn(move || {
        loop {
            let next = REVIEWS.queue.lock().iter().find(|i| i.state == "waiting").cloned();
            let Some(next) = next else { break };
            if let Err(e) = merge(host, &next) {
                patch_queue(&next.review_id, "failed", Some(e), Vec::new());
            }
        }
        REVIEWS.running.store(false, Ordering::SeqCst);
        // One queued while this one stopped looking.
        if REVIEWS.queue.lock().iter().any(|i| i.state == "waiting") {
            run_queue(host);
        }
    });
}

/// Commit what the agent left, bring in the base, test (if asked and not already green), merge.
fn merge(host: &Host, item: &QueueItem) -> Result<(), String> {
    let id = &item.review_id;
    let Some(review) = REVIEWS.items.lock().iter().find(|r| &r.id == id).cloned() else {
        return Err("The review was closed.".into());
    };
    let pane = host.with_state(|s| s.pane(&review.pane_id).cloned());
    let (Some(info), Some(pane)) = (review.worktree.clone(), pane) else { return Err("The agent's pane or worktree is gone.".into()) };
    let name = host.with_state(|s| s.provider_name(&pane.provider));
    patch_queue(id, "syncing", Some(format!("Bringing in {}…", info.base)), Vec::new());
    let summary = if let Load::Done(t) = &review.summary { Some(t.as_str()) } else { None };
    let message = commit_message(summary, review.prompt.as_deref()).unwrap_or_else(|| format!("Changes by {name}"));
    worktree::commit_all(&info, &message)?;
    let outcome = serde_json::to_value(worktree::sync_with_base(&info)?).unwrap_or_default();
    let kind = outcome["kind"].as_str().unwrap_or_default();
    if kind == "conflicts" {
        let files: Vec<String> = serde_json::from_value(outcome["files"].clone()).unwrap_or_default();
        patch_queue(id, "conflict", Some(format!("Conflicts with {}", info.base)), files);
        return Ok(());
    }
    let synced = kind == "synced";
    let commits = outcome["commits"].as_u64().unwrap_or(0);
    let mut tests_line = if let Load::Done(Some(t)) = &review.tests { Some(format!("{} {}", t.command, if t.ok { "passed" } else { "failed" })) } else { None };
    // Tests that already passed on this exact branch needn't run again.
    let fresh = !synced && matches!(&review.tests, Load::Done(Some(t)) if t.ok);
    if item.tests && !fresh && let Some(command) = review.test_command.clone().or_else(|| review_backend::test_command(std::path::Path::new(&review.dir))) {
        let with = if synced { format!(" with {}'s {commits} new commits", info.base) } else { String::new() };
        patch_queue(id, "testing", Some(format!("Running {command}{with}…")), Vec::new());
        let run = run_tests(&review.dir).inspect_err(|e| {
            with_review(id, |r| r.tests = Load::Error(e.clone()));
        });
        let run = run.map_err(|e| format!("Tests couldn't run: {e}"))?;
        with_review(id, |r| r.tests = Load::Done(run.clone()));
        if let Some(t) = &run {
            tests_line = Some(format!("{} {}", t.command, if t.ok { "passed" } else { "failed" }));
            if !t.ok {
                let after = if synced { format!(" after bringing in {}", info.base) } else { String::new() };
                patch_queue(id, "failed", Some(format!("{} failed{after}", t.command)), Vec::new());
                return Ok(());
            }
        }
    }
    patch_queue(id, "merging", Some(format!("Merging into {}…", info.base)), Vec::new());
    worktree::merge(&info)?;
    record(host, &review, &pane, &info, tests_line);
    host.log(&pane, format!("Merged {} into {} (queue)", info.branch, info.base));
    REVIEWS.items.lock().retain(|r| &r.id != id);
    patch_queue(id, "merged", None, Vec::new());
    Ok(())
}

/// The merge in this server's history.
fn record(host: &Host, review: &Review, pane: &Pane, info: &worktree::WorktreeInfo, tests: Option<String>) {
    let workspace = host.with_state(|s| s.workspaces.iter().find(|w| w.id == pane.workspace_id).map_or(String::new(), |w| w.name.clone()));
    let stats = review.stats.clone().unwrap_or_default();
    let summary = if let Load::Done(t) = &review.summary { Some(t.clone()) } else { None };
    let merge = json!({
        "id": winfer_core::model::new_id("merge"), "mergedAt": now_ms(), "workspaceId": pane.workspace_id, "workspace": workspace,
        "repo": info.repo, "branch": info.branch, "base": info.base, "commitSha": null, "provider": pane.provider,
        "title": review.title, "summary": summary, "task": review.prompt, "files": stats.files, "adds": stats.adds,
        "dels": stats.dels, "tests": tests, "cost": null, "tokens": null,
    });
    if let Ok(record) = serde_json::from_value(merge) {
        let _ = store::history_add(backend::app().state(), record);
    }
}
