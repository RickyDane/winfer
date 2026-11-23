//! Review inbox, merge queue and history (port of `features/review` and `features/history`).

use super::commands::{Command, PaneRef, worktree_info};
use super::*;
use crate::model::{Overlay, WorktreeInfo};
use super::dialogs::relative_time;
use base64::Engine as _;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use serde::Deserialize;
use serde_json::json;
use smallvec::smallvec;
use std::sync::Arc;
use winfer_backend::{browser, chatgpt, llm, review as review_backend, store, worktree};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct DiffStats {
    pub files: Vec<String>,
    pub adds: u64,
    pub dels: u64,
    pub ahead: u64,
    pub dirty: bool,
}

/// Files, line counts and commit state from a worktree diff.
pub fn diff_stats(d: &Value) -> DiffStats {
    let mut files: Vec<String> = Vec::new();
    let (mut adds, mut dels) = (0, 0);
    for line in d["diff"].as_str().unwrap_or_default().split('\n') {
        if let Some(rest) = line.strip_prefix("diff --git a/") {
            if let Some(name) = rest.split(" b/").nth(1).filter(|n| !files.iter().any(|f| f == n)) {
                files.push(name.to_string());
            }
        } else if line.starts_with('+') && !line.starts_with("+++") {
            adds += 1;
        } else if line.starts_with('-') && !line.starts_with("---") {
            dels += 1;
        }
    }
    for f in d["untracked"].as_array().into_iter().flatten().filter_map(Value::as_str) {
        if !files.iter().any(|x| x == f) {
            files.push(f.to_string());
        }
    }
    DiffStats { files, adds, dels, ahead: d["ahead"].as_u64().unwrap_or(0), dirty: d["dirty"] == true }
}

#[derive(Clone, Debug, Deserialize)]
pub struct TestRun {
    pub command: String,
    pub ok: bool,
    pub ms: f64,
}

#[derive(Clone)]
pub enum Load<T> {
    Idle,
    Off,
    Loading,
    Done(T),
    Error(String),
}

#[derive(Clone, Copy, PartialEq)]
pub enum ReviewStatus {
    Open,
    Merged,
    Discarded,
}

#[derive(Clone)]
pub struct Review {
    pub id: String,
    pub pane_id: String,
    pub workspace_id: String,
    pub provider: String,
    pub prompt: Option<String>,
    pub finished_at: std::time::SystemTime,
    pub branch: Option<String>,
    pub dir: String,
    pub stats: Option<DiffStats>,
    pub last_output: String,
    pub summary: Load<String>,
    pub tests: Load<Option<TestRun>>,
    pub test_command: Option<String>,
    pub screenshot: Load<Arc<RenderImage>>,
    pub screenshot_url: Option<String>,
    pub status: ReviewStatus,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum QueueState {
    Queued,
    Syncing,
    Testing,
    Merging,
    Merged,
    Conflict,
    Failed,
}

#[derive(Clone)]
pub struct QueueItem {
    pub review_id: String,
    pub pane_id: String,
    pub workspace_id: String,
    pub branch: String,
    pub base: String,
    pub state: QueueState,
    pub detail: Option<String>,
    pub files: Vec<String>,
}

impl QueueItem {
    fn pending(&self) -> bool {
        matches!(self.state, QueueState::Queued | QueueState::Syncing | QueueState::Testing | QueueState::Merging)
    }
}

#[derive(Default)]
pub struct ReviewStore {
    pub reviews: Vec<Review>,
    pub queue: Vec<QueueItem>,
    pub queue_running: bool,
    pub queue_tests: bool,
    pub pending: HashMap<String, Task<()>>,
    pub conflicts: Option<Value>,
    /// The "send back" box of each open card.
    pub replies: HashMap<String, Entity<InputState>>,
    /// The review shown in the detail column.
    pub selected: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct MergeSnapshot {
    workspace_id: Option<String>,
    workspace: String,
    repo: String,
    branch: String,
    base: String,
    provider: String,
    title: String,
    summary: Option<String>,
    task: Option<String>,
    files: Vec<String>,
    adds: u64,
    dels: u64,
    tests: Option<String>,
}

pub struct HistoryUi {
    input: Entity<InputState>,
    records: Option<Vec<Value>>,
    this_workspace: bool,
    expanded: Option<String>,
    search: Option<Task<()>>,
}

const SUMMARY_SYSTEM: &str = "You review the work a coding agent just finished, for a developer deciding whether to merge it. From the diff and the agent's last output, write 2–3 short sentences: what changed and why, then anything risky, unfinished or worth checking (failing steps, TODOs, deleted code). Name files only when it helps. Output only the summary.";

/// The language to write about a task in, named in the prompt.
pub fn reply_language(task: &str) -> &'static str {
    let lower = task.to_lowercase();
    if lower.chars().any(|c| ('\u{400}'..='\u{4ff}').contains(&c)) {
        return "the language of the task";
    }
    let german_word = ["und", "nicht", "bitte", "ist", "das", "der", "die", "mit", "für", "auf", "ich", "wir", "noch", "auch"];
    if lower.contains(['ä', 'ö', 'ü', 'ß']) || lower.split(|c: char| !c.is_alphanumeric()).any(|w| german_word.contains(&w)) {
        return "German";
    }
    "English"
}

fn is_non_latin(c: char) -> bool {
    c.is_alphabetic() && !c.is_ascii() && !('\u{c0}'..='\u{24f}').contains(&c) && !('\u{1e00}'..='\u{1eff}').contains(&c)
}

/// True when `text` uses a non-Latin script the task doesn't.
pub fn wrong_script(text: &str, task: &str) -> bool {
    text.chars().any(is_non_latin) && !task.chars().any(is_non_latin)
}

fn first_sentence(text: &str) -> &str {
    let bytes: Vec<(usize, char)> = text.char_indices().collect();
    for window in bytes.windows(2) {
        if matches!(window[0].1, '.' | '!' | '?') && window[1].1.is_whitespace() {
            return &text[..=window[0].0];
        }
    }
    text
}

fn shorten(text: &str, max: usize) -> String {
    if text.chars().count() > max { format!("{}…", text.chars().take(max - 3).collect::<String>()) } else { text.to_string() }
}

/// First sentence of the summary, as a commit message for changes the agent didn't commit.
pub fn commit_message(review: Option<&Review>) -> Option<String> {
    let review = review?;
    match &review.summary {
        Load::Done(text) => Some(shorten(first_sentence(text), 100)),
        _ => review.prompt.as_ref().map(|p| format!("winfer: {}", p.chars().take(72).collect::<String>())),
    }
}

/// The agent's last words, without blank lines and prompt chrome.
fn last_output(text: &str) -> String {
    let lines: Vec<&str> = text.lines().map(str::trim_end).filter(|l| !l.trim().is_empty() && !l.chars().all(|c| "─━═ >│╭╮╰╯".contains(c))).collect();
    let tail = lines[lines.len().saturating_sub(25)..].join("\n");
    let chars: Vec<char> = tail.chars().collect();
    chars[chars.len().saturating_sub(1800)..].iter().collect()
}

impl Root {
    /// The LLM to use: the chosen one if available, else the first that is; `None` when off.
    pub(super) fn llm_provider(&self, cx: &App) -> Option<(String, String)> {
        let s = self.state.read(cx);
        let voice = s.rest.get("voice");
        let choice = voice.and_then(|v| v["writer"].as_str()).map(str::to_string);
        if choice.as_deref() == Some("off") {
            return None;
        }
        let mut available = Vec::new();
        if serde_json::to_value(chatgpt::chatgpt_status()).is_ok_and(|v| v["signedIn"] == true) {
            available.push("chatgpt".to_string());
        }
        for id in ["codex", "claude"] {
            if s.providers.iter().any(|p| p.id == id && p.installed) {
                available.push(id.to_string());
            }
        }
        let provider = available.iter().find(|a| Some(*a) == choice.as_ref()).or(available.first())?.clone();
        let model = if Some(&provider) == choice.as_ref() { voice.and_then(|v| v["writerModel"].as_str()).unwrap_or_default().to_string() } else { String::new() };
        Some((provider, model))
    }

    /// An agent finished. Hooks fire per turn and idle detection can flap, so wait for it to stay finished.
    pub(super) fn agent_finished(&mut self, pane: &str, cx: &mut Context<Self>) {
        if crate::voice::orchestrator_workspace(pane).is_some() {
            return;
        }
        self.voice_agent_note(pane, "finished", cx);
        if self.state.read(cx).pane(pane).is_some_and(|p| p.status != AgentStatus::Done) {
            self.estimate_now(pane, cx);
        }
        let id = pane.to_string();
        let task = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(1500)).await;
            let _ = this.update(cx, |this, cx| {
                this.review.pending.remove(&id);
                if this.state.read(cx).pane(&id).is_some_and(|p| p.status != AgentStatus::Working) {
                    this.collect_review(id.clone(), cx);
                }
            });
        });
        self.review.pending.insert(pane.to_string(), task);
    }

    fn review_mut(&mut self, id: &str) -> Option<&mut Review> {
        self.review.reviews.iter_mut().find(|r| r.id == id)
    }

    /// Development aid: once the demo panes' worktrees exist, give each finished one a change
    /// and collect its review, then open Review.
    #[cfg(debug_assertions)]
    pub(super) fn demo_reviews(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(std::time::Duration::from_secs(4)).await;
            let _ = this.update(cx, |this, cx| {
                let done: Vec<(String, String)> = this.state.read(cx).all_panes()
                    .filter(|p| p.status == crate::model::AgentStatus::Done)
                    .filter_map(|p| p.worktree.as_ref().map(|w| (p.id.clone(), w.path.clone())))
                    .collect();
                for (id, path) in done {
                    let _ = std::fs::write(std::path::Path::new(&path).join("THEME_TOGGLE.md"), "# Theme toggle\n\nAdds a light/dark toggle to the header.\n");
                    this.collect_review(id, cx);
                }
                this.update_state(cx, |s| s.overlay = Overlay::Review);
            });
        })
        .detach();
    }

    fn collect_review(&mut self, pane_id: String, cx: &mut Context<Self>) {
        let Some(pane) = self.state.read(cx).pane(&pane_id).cloned().filter(|p| p.provider != "shell") else { return };
        cx.spawn(async move |this, cx| {
            let mut diff = String::new();
            let mut stats = None;
            if let Some(info) = pane.worktree.clone() {
                let result = tauri::async_runtime::spawn(async move {
                    let d = worktree::worktree_diff(worktree_info(&info)?).await?;
                    serde_json::to_value(d).map_err(|e| e.to_string())
                })
                .await;
                let Ok(Ok(v)) = result else { return };
                let computed = diff_stats(&v);
                // A turn that changed nothing (a question, a plan) isn't work to review.
                if computed.files.is_empty() && computed.ahead == 0 {
                    return;
                }
                diff = v["diff"].as_str().unwrap_or_default().to_string();
                stats = Some(computed);
            }
            let _ = this.update(cx, |this, cx| this.add_review(pane, stats, diff, cx));
        })
        .detach();
    }

    fn add_review(&mut self, pane: Pane, stats: Option<DiffStats>, diff: String, cx: &mut Context<Self>) {
        let existing = self.review.reviews.iter().find(|r| r.pane_id == pane.id && r.status == ReviewStatus::Open).map(|r| r.id.clone());
        let id = existing.unwrap_or_else(|| crate::model::new_id("rev"));
        let screen = SESSIONS.get(&pane.id).map(|s| s.read_text(60)).unwrap_or_default();
        let review = Review {
            id: id.clone(),
            pane_id: pane.id.clone(),
            workspace_id: pane.workspace_id.clone(),
            provider: pane.provider.clone(),
            prompt: pane.prompt.clone().or_else(|| self.state.read(cx).pane_tasks.get(&pane.id).and_then(|t| t.task.clone())),
            finished_at: std::time::SystemTime::now(),
            branch: pane.worktree.as_ref().map(|t| t.branch.clone()),
            dir: pane.worktree.as_ref().map_or_else(|| pane.cwd.clone(), |t| t.path.clone()),
            stats,
            last_output: last_output(&screen),
            summary: Load::Idle,
            tests: Load::Idle,
            test_command: None,
            screenshot: Load::Idle,
            screenshot_url: None,
            status: ReviewStatus::Open,
        };
        self.review.reviews.retain(|r| r.id != id);
        self.review.reviews.insert(0, review.clone());
        cx.notify();
        self.summarize(review.clone(), diff, cx);
        let auto_tests = self.state.read(cx).rest.get("review").is_some_and(|r| r["autoTests"] == true);
        let dir = review.dir.clone();
        let test_id = id.clone();
        cx.spawn(async move |this, cx| {
            let command = backend::background(move || review_backend::review_test_command(dir)).await.ok().flatten();
            let _ = this.update(cx, |this, cx| {
                if let Some(r) = this.review_mut(&test_id) {
                    r.test_command = command.clone();
                }
                if command.is_some() && auto_tests {
                    this.run_tests(test_id.clone(), cx);
                }
                cx.notify();
            });
        })
        .detach();
        let screenshots = self.state.read(cx).rest.get("review").is_none_or(|r| r["screenshots"] != false);
        if pane.dev_url.is_some() && screenshots {
            self.capture_screenshot(id, pane.workspace_id, pane.dev_url.unwrap(), cx);
        }
    }

    fn summarize(&mut self, review: Review, diff: String, cx: &mut Context<Self>) {
        let Some((provider, model)) = self.llm_provider(cx) else {
            if let Some(r) = self.review_mut(&review.id) {
                r.summary = Load::Off;
            }
            return;
        };
        if let Some(r) = self.review_mut(&review.id) {
            r.summary = Load::Loading;
        }
        let mut parts = Vec::new();
        if let Some(p) = &review.prompt {
            parts.push(format!("Task: {p}"));
        }
        if let Some(s) = &review.stats {
            let files = if s.files.is_empty() { "none".to_string() } else { s.files.join(", ") };
            parts.push(format!("Files changed: {files} (+{} −{})", s.adds, s.dels));
        }
        if !diff.is_empty() {
            let clipped: String = diff.chars().take(12_000).collect();
            parts.push(format!("Diff:\n{clipped}{}", if diff.chars().count() > 12_000 { "\n…(truncated)" } else { "" }));
        }
        parts.push(format!("Agent's last output:\n{}", review.last_output));
        let user = parts.join("\n\n");
        let task = review.prompt.clone().unwrap_or_default();
        let system = format!("{SUMMARY_SYSTEM} Write it in {}.", reply_language(&task));
        let id = review.id;
        cx.spawn(async move |this, cx| {
            let result = tauri::async_runtime::spawn(async move {
                let ask = || async {
                    let req = serde_json::from_value(json!({ "provider": provider, "model": model, "system": system, "user": user })).map_err(|e: serde_json::Error| e.to_string())?;
                    llm::llm_compose(req).await
                };
                let mut text = ask().await?;
                // Models sometimes switch languages anyway; one retry usually fixes it.
                if wrong_script(&text, &task) {
                    text = ask().await?;
                }
                Ok::<_, String>(text)
            })
            .await
            .map_err(|e| e.to_string())
            .and_then(|r| r);
            let _ = this.update(cx, |this, cx| {
                if let Some(r) = this.review_mut(&id) {
                    r.summary = match result {
                        Ok(t) => Load::Done(t.trim().to_string()),
                        Err(e) => Load::Error(e),
                    };
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn run_tests(&mut self, id: String, cx: &mut Context<Self>) {
        let Some(r) = self.review_mut(&id) else { return };
        r.tests = Load::Loading;
        let dir = r.dir.clone();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = run_tests_for(dir).await;
            let _ = this.update(cx, |this, cx| {
                if let Some(r) = this.review_mut(&id) {
                    r.tests = match result {
                        Ok(v) => Load::Done(v),
                        Err(e) => Load::Error(e),
                    };
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn capture_screenshot(&mut self, id: String, workspace: String, url: String, cx: &mut Context<Self>) {
        if let Some(r) = self.review_mut(&id) {
            r.screenshot = Load::Loading;
            r.screenshot_url = Some(url.clone());
        }
        cx.spawn(async move |this, cx| {
            let result = tauri::async_runtime::spawn(async move {
                let app = backend::app();
                let shot = browser::browser_screenshot(app.clone(), app.state::<browser::BrowserManager>(), workspace, url).await.map_err(|e| e.to_string())?;
                decode_screenshot(&shot)
            })
            .await
            .map_err(|e| e.to_string())
            .and_then(|r| r);
            let _ = this.update(cx, |this, cx| {
                if let Some(r) = this.review_mut(&id) {
                    r.screenshot = match result {
                        Ok(image) => Load::Done(image),
                        Err(e) => Load::Error(e),
                    };
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Records what to remember about a branch, before merging empties its diff.
    fn snapshot_for_history(&self, pane: &Pane, stats: Option<DiffStats>, cx: &App) -> Option<MergeSnapshot> {
        let info = pane.worktree.as_ref()?;
        let s = self.state.read(cx);
        let review = self.review.reviews.iter().find(|r| r.pane_id == pane.id && r.status == ReviewStatus::Open);
        let summary = review.and_then(|r| if let Load::Done(t) = &r.summary { Some(t.clone()) } else { None });
        let task = s.pane_tasks.get(&pane.id).and_then(|t| t.task.clone()).or_else(|| pane.prompt.clone());
        let tests = review.and_then(|r| if let Load::Done(Some(t)) = &r.tests { Some(format!("{} {}", t.command, if t.ok { "passed" } else { "failed" })) } else { None });
        let stats = stats.or_else(|| review.and_then(|r| r.stats.clone())).unwrap_or_default();
        let title = pane.custom_title.clone()
            .or_else(|| s.pane_tasks.get(&pane.id).and_then(|t| t.title.clone()))
            .or_else(|| pane.agent_title.clone())
            .or_else(|| summary.as_deref().map(|t| shorten(first_sentence(t).trim(), 120)))
            .or_else(|| task.as_ref().map(|t| t.lines().next().unwrap_or("").chars().take(120).collect()))
            .unwrap_or_else(|| info.branch.clone());
        Some(MergeSnapshot {
            workspace_id: Some(pane.workspace_id.clone()),
            workspace: s.workspaces.iter().find(|w| w.id == pane.workspace_id).map_or(String::new(), |w| w.name.clone()),
            repo: info.repo.clone(),
            branch: info.branch.clone(),
            base: info.base.clone(),
            provider: pane.provider.clone(),
            title,
            summary,
            task,
            files: stats.files,
            adds: stats.adds,
            dels: stats.dels,
            tests,
        })
    }

    pub(super) fn set_review_status_for(&mut self, pane: &str, status: ReviewStatus, cx: &mut Context<Self>) {
        for r in self.review.reviews.iter_mut().filter(|r| r.pane_id == pane && r.status == ReviewStatus::Open) {
            r.status = status;
        }
        cx.notify();
    }

    pub(super) fn record_merge_of(&self, pane: &Pane, stats: Option<DiffStats>, cx: &App) {
        if let Some(snapshot) = self.snapshot_for_history(pane, stats, cx) {
            record_merge(snapshot);
        }
    }

    pub(super) fn commit_message_for(&self, pane: &str) -> Option<String> {
        commit_message(self.review.reviews.iter().find(|r| r.pane_id == pane && r.status == ReviewStatus::Open))
    }

    // ---- merge queue ---------------------------------------------------------------------------

    fn enqueue(&mut self, ids: Vec<String>, cx: &mut Context<Self>) {
        let mut added = Vec::new();
        for id in ids {
            let Some(review) = self.review.reviews.iter().find(|r| r.id == id && r.status == ReviewStatus::Open) else { continue };
            let Some((pane, info)) = self.state.read(cx).pane(&review.pane_id).and_then(|p| p.worktree.clone().map(|t| (p.id.clone(), t))) else { continue };
            if self.review.queue.iter().any(|i| i.review_id == id && i.pending()) {
                continue;
            }
            added.push(QueueItem { review_id: id, pane_id: pane, workspace_id: review.workspace_id.clone(), branch: info.branch, base: info.base, state: QueueState::Queued, detail: None, files: Vec::new() });
        }
        if added.is_empty() {
            return;
        }
        self.review.queue.retain(|i| !added.iter().any(|a| a.review_id == i.review_id));
        self.review.queue.extend(added);
        self.run_queue(cx);
    }

    fn patch_queue(&mut self, id: &str, state: QueueState, detail: Option<String>, files: Vec<String>, cx: &mut Context<Self>) {
        if let Some(item) = self.review.queue.iter_mut().find(|i| i.review_id == id) {
            item.state = state;
            item.detail = detail;
            item.files = files;
        }
        cx.notify();
    }

    fn run_queue(&mut self, cx: &mut Context<Self>) {
        if self.review.queue_running {
            return;
        }
        let Some(next) = self.review.queue.iter().find(|i| i.state == QueueState::Queued).cloned() else { return };
        self.review.queue_running = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = process_queue_item(this.clone(), next.clone(), cx).await;
            let _ = this.update(cx, |this, cx| {
                if let Err(err) = result {
                    this.patch_queue(&next.review_id, QueueState::Failed, Some(err), Vec::new(), cx);
                }
                this.review.queue_running = false;
                this.run_queue(cx);
            });
        })
        .detach();
    }

    fn ask_agent_to_resolve(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(item) = self.review.queue.iter().find(|i| i.review_id == id).cloned() else { return };
        let files = if item.files.is_empty() { String::new() } else { format!(" in {}", item.files.join(", ")) };
        let text = format!("Merging your branch {} into {} conflicts{files}. Merge {} into your branch (git merge {}), resolve the conflicts keeping both sides' intent, make sure the tests pass, and commit.", item.branch, item.base, item.base, item.base);
        self.execute(Command::SendText { target: PaneRef::Id(item.pane_id), text, submit: true }, window, cx);
        self.review.queue.retain(|i| i.review_id != id);
        self.review.reviews.retain(|r| r.id != id); // a fresh card arrives when the agent finishes again
        cx.notify();
    }

    fn refresh_conflicts(&mut self, cx: &mut Context<Self>) {
        let mut seen = Vec::new();
        for p in self.state.read(cx).all_panes() {
            if let Some(t) = p.worktree.as_ref().filter(|t| !seen.iter().any(|x: &WorktreeInfo| x.path == t.path)) {
                seen.push(t.clone());
            }
        }
        cx.spawn(async move |this, cx| {
            let report = tauri::async_runtime::spawn(async move {
                let infos: Result<Vec<_>, String> = seen.iter().map(worktree_info).collect();
                let report = worktree::worktree_conflicts(infos?).await?;
                serde_json::to_value(report).map_err(|e| e.to_string())
            })
            .await
            .map_err(|e| e.to_string())
            .and_then(|r| r);
            let _ = this.update(cx, |this, cx| {
                this.review.conflicts = report.ok();
                cx.notify();
            });
        })
        .detach();
    }

    // ---- rendering -----------------------------------------------------------------------------

    pub(super) fn render_review(&mut self, c: &Colors, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.state.read(cx).overlay != Overlay::Review {
            self.review_opened = false;
            return None;
        }
        if !self.review_opened {
            self.review_opened = true;
            self.refresh_conflicts(cx);
        }
        let c = *c;
        let mono = crate::fonts::mono(cx);
        let open: Vec<Review> = self.review.reviews.iter().filter(|r| r.status == ReviewStatus::Open).cloned().collect();
        // Each open card gets a box to send a follow-up back to its agent.
        self.review.replies.retain(|id, _| open.iter().any(|r| &r.id == id));
        for r in &open {
            if self.review.replies.contains_key(&r.id) {
                continue;
            }
            let name = self.state.read(cx).provider_name(&r.provider);
            let input = cx.new(|cx| InputState::new(window, cx).placeholder(format!("Send back to {name}, e.g. “make the header sticky”")));
            let id = r.id.clone();
            cx.subscribe_in(&input, window, move |this, _, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    this.send_back(&id, window, cx);
                }
            })
            .detach();
            self.review.replies.insert(r.id.clone(), input);
        }
        let queue = self.review.queue.clone();
        let conflicts = self.review.conflicts.clone();
        let selected = self.review.selected.clone().filter(|id| open.iter().any(|r| &r.id == id)).or_else(|| open.first().map(|r| r.id.clone()));
        // Left: every finished task; right: the selected one in full.
        let mut list = div().flex().flex_col().gap_0p5();
        for r in &open {
            let on = selected.as_deref() == Some(&r.id);
            let s = self.state.read(cx);
            let title = s.pane(&r.pane_id).map(|p| s.pane_label(p)).or_else(|| r.prompt.clone()).unwrap_or_else(|| s.provider_name(&r.provider));
            let at = r.finished_at.duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_millis() as f64);
            let conflict = conflict_note(conflicts.as_ref(), &r.dir).is_some_and(|(base, _)| base);
            let (test_text, test_color) = match &r.tests {
                Load::Done(Some(t)) if t.ok => ("tests pass", c.success),
                Load::Done(Some(_)) => ("tests fail", c.danger),
                Load::Loading => ("testing…", c.subtle),
                _ => ("", c.subtle),
            };
            let pick = r.id.clone();
            list = list.child(
                div().id(SharedString::from(format!("review-item-{}", r.id))).flex().gap_2p5().rounded_lg().p_2p5().cursor_pointer()
                    .when(on, |d| d.bg(c.active)).when(!on, |d| d.hover(move |d| d.bg(c.hover)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.review.selected = Some(pick.clone());
                        cx.notify();
                    }))
                    .child(Self::agent_tile(&r.provider, crate::model::AgentStatus::Done, c))
                    .child(div().flex_1().min_w_0().flex().flex_col().gap_0p5()
                        .child(div().flex().items_center().gap_1p5()
                            .child(div().flex_1().min_w_0().truncate().text_size(px(12.5)).font_weight(FontWeight::MEDIUM).text_color(c.fg).child(title))
                            .child(div().flex_shrink_0().text_size(px(10.5)).text_color(c.subtle).child(relative_time(at))))
                        .when_some(r.branch.clone(), |d, b| d.child(div().truncate().font_family(mono.clone()).text_size(px(10.5)).text_color(c.subtle).child(b)))
                        .child(div().flex().items_center().gap_1p5().text_size(px(10.5))
                            .when_some(r.stats.as_ref().map(|st| (st.adds, st.dels)), |d, (a, del)| d.child(div().text_color(c.success).child(format!("+{a}"))).child(div().text_color(c.danger).child(format!("−{del}"))))
                            .when(!test_text.is_empty(), |d| d.child(div().text_color(test_color).child(test_text)))
                            .when(conflict, |d| d.child(div().text_color(c.warning).child("conflict"))))),
            );
        }
        let detail: AnyElement = match open.iter().find(|r| selected.as_deref() == Some(&r.id)) {
            Some(r) => self.render_review_card(r, conflicts.as_ref(), c, mono.clone(), cx).into_any_element(),
            None => div().size_full().flex().flex_col().items_center().justify_center().gap_1p5().text_color(c.subtle)
                .child(Icon::new(IconName::Inbox).size(px(24.0)))
                .child(div().text_size(px(13.0)).text_color(c.fg).child("Nothing to review"))
                .child(div().text_xs().child("When an agent finishes, its work shows up here to merge or send back."))
                .into_any_element(),
        };
        let cards = div().size_full().flex()
            .when(!open.is_empty(), |d| d.child(div().id("review-items").w(px(290.0)).flex_shrink_0().h_full().overflow_y_scroll().p_2().child(list)))
            .child(div().id("review-detail").flex_1().min_w_0().h_full().overflow_y_scroll().p_5().child(detail));
        let mut queue_list = div().flex().flex_col().gap_1();
        for item in &queue {
            let id = item.review_id.clone();
            let label = match item.state {
                QueueState::Queued => "Waiting",
                QueueState::Syncing => "Syncing",
                QueueState::Testing => "Testing",
                QueueState::Merging => "Merging",
                QueueState::Merged => "Merged",
                QueueState::Conflict => "Conflict",
                QueueState::Failed => "Failed",
            };
            let color = match item.state {
                QueueState::Merged => c.success,
                QueueState::Conflict | QueueState::Failed => c.danger,
                _ => c.muted,
            };
            let (retry_id, resolve_id, remove_id) = (id.clone(), id.clone(), id.clone());
            let stopped = matches!(item.state, QueueState::Conflict | QueueState::Failed);
            let conflict = item.state == QueueState::Conflict;
            let removable = !item.pending() || item.state == QueueState::Queued;
            queue_list = queue_list.child(
                div().flex().items_center().gap_2().text_xs()
                    .child(div().w(px(56.0)).text_color(color).child(label))
                    .child(div().font_family(mono.clone()).truncate().child(item.branch.clone()))
                    .child(div().flex_1().min_w_0().truncate().text_color(c.subtle).child(item.detail.clone().unwrap_or_default()))
                    .when(conflict, |d| d.child(small_button(c, SharedString::from(format!("q-resolve-{id}")), "Ask agent").on_click(cx.listener(move |this, _, window, cx| this.ask_agent_to_resolve(&resolve_id, window, cx)))))
                    .when(stopped, |d| d.child(small_button(c, SharedString::from(format!("q-retry-{id}")), "Retry").on_click(cx.listener(move |this, _, _, cx| this.enqueue(vec![retry_id.clone()], cx)))))
                    .when(removable, |d| d.child(small_button(c, SharedString::from(format!("q-remove-{id}")), "Remove").on_click(cx.listener(move |this, _, _, cx| {
                        this.review.queue.retain(|i| i.review_id != remove_id);
                        cx.notify();
                    })))),
            );
        }
        let mergeable: Vec<String> = open.iter().filter(|r| r.branch.is_some() && !queue.iter().any(|i| i.review_id == r.id && i.pending())).map(|r| r.id.clone()).collect();
        let finished = queue.iter().any(|i| !i.pending());
        let tests_first = self.review.queue_tests;
        Some(
            div().id("review-backdrop").absolute().size_full().flex().items_center().justify_center().bg(hsla(0.0, 0.0, 0.0, 0.4)).occlude()
                .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| this.update_state(cx, |s| s.overlay = Overlay::None)))
                .child(
                    div().id("review").w(px(1040.0)).max_w(relative(0.94)).h(px(700.0)).max_h(relative(0.9)).flex().flex_col().overflow_hidden().rounded(px(14.0)).border_1().border_color(c.edge).bg(c.raised).shadow_2xl()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(
                            div().flex().items_center().gap_3().px_5().py_3()
                                .child(Icon::new(IconName::Inbox).size(px(15.0)).text_color(c.accent))
                                .child(div().text_size(px(15.0)).font_weight(FontWeight::SEMIBOLD).text_color(c.fg).child("Review"))
                                .child(div().flex_1().text_size(px(12.0)).text_color(c.subtle).child(match open.len() { 0 => String::new(), 1 => "1 task finished".into(), n => format!("{n} tasks finished") }))
                                .child(
                                    div().id("queue-tests").flex().items_center().gap_1p5().text_xs().text_color(c.muted).cursor_pointer()
                                        .on_click(cx.listener(|this, _, _, cx| { this.review.queue_tests = !this.review.queue_tests; cx.notify(); }))
                                        .child(div().size(px(14.0)).flex().items_center().justify_center().rounded_sm().border_1()
                                            .when(tests_first, |d| d.bg(c.accent))
                                            .when(tests_first, |d| d.child(Icon::new(IconName::Check).size(px(10.0)).text_color(c.on_accent))))
                                        .child("Run tests first"),
                                )
                                .when(mergeable.len() > 1, |d| {
                                    let ids = mergeable.clone();
                                    d.child(div().id("merge-all").rounded_md().bg(c.accent).text_color(c.on_accent).px_2p5().py_1().text_xs().cursor_pointer()
                                        .on_click(cx.listener(move |this, _, _, cx| this.enqueue(ids.clone(), cx)))
                                        .child(format!("Merge all ({})", mergeable.len())))
                                })
                                .child(
                                    div().id("review-close").rounded_sm().p_1().cursor_pointer().hover(move |d| d.bg(c.hover))
                                        .on_click(cx.listener(|this, _, _, cx| this.update_state(cx, |s| s.overlay = Overlay::None)))
                                        .child(Icon::new(IconName::X).size(px(14.0)).text_color(c.subtle)),
                                ),
                        )
                        .when(!queue.is_empty(), |d| {
                            d.child(
                                div().px_5().py_2().flex().flex_col().gap_1()
                                    .child(div().flex().items_center().text_xs().text_color(c.subtle).child(div().flex_1().child("Merge queue")).when(finished, |d| d.child(small_button(c, "queue-clear", "Clear finished").on_click(cx.listener(|this, _, _, cx| { this.review.queue.retain(QueueItem::pending); cx.notify(); })))))
                                    .child(queue_list),
                            )
                        })
                        .child(div().flex_1().min_h_0().child(cards)),
                )
                .into_any_element(),
        )
    }

    /// Types the reply into the agent's pane and clears the card: a fresh one arrives when the agent finishes again.
    fn send_back(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(review) = self.review.reviews.iter().find(|r| r.id == id).cloned() else { return };
        let Some(input) = self.review.replies.get(id).cloned() else { return };
        let text = input.read(cx).value().trim().to_string();
        if text.is_empty() || self.state.read(cx).pane(&review.pane_id).is_none() {
            return;
        }
        let name = self.state.read(cx).provider_name(&review.provider);
        self.execute(Command::SendText { target: PaneRef::Id(review.pane_id.clone()), text: text.clone(), submit: true }, window, cx);
        let short: String = text.chars().take(80).collect();
        self.state.update(cx, |s, _| s.log_activity(&review.workspace_id, format!("Sent back to {name}: {short}"), Some(review.provider.clone())));
        self.review.reviews.retain(|x| x.id != id);
        self.review.replies.remove(id);
        cx.notify();
    }

    fn render_review_card(&self, r: &Review, conflicts: Option<&Value>, c: Colors, mono: SharedString, cx: &mut Context<Self>) -> Div {
        let s = self.state.read(cx);
        let agent = s.provider_name(&r.provider);
        let pane = s.pane(&r.pane_id).cloned();
        let elapsed = r.finished_at.duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_millis() as f64);
        let mut header = div().flex().items_center().gap_2().text_xs().pb_3()
            .child(Self::agent_tile(&r.provider, crate::model::AgentStatus::Done, c))
            .child(div().text_size(px(14.0)).font_weight(FontWeight::SEMIBOLD).text_color(c.fg).child(agent))
            .when_some(r.branch.clone(), |d, b| d.child(div().font_family(mono.clone()).text_color(c.subtle).truncate().child(b)))
            .child(div().flex_1())
            .child(div().text_color(c.subtle).child(relative_time(elapsed)));
        if let Some(stats) = &r.stats {
            header = header.child(div().text_color(c.subtle).child(format!("{} files", stats.files.len()))).child(div().text_color(c.success).child(format!("+{}", stats.adds))).child(div().text_color(c.danger).child(format!("−{}", stats.dels)));
        }
        let summary: Div = match &r.summary {
            Load::Done(t) => div().text_size(px(13.0)).text_color(c.fg).child(t.clone()),
            Load::Loading | Load::Idle => div().text_xs().text_color(c.subtle).child("Summarizing…"),
            Load::Error(e) => div().text_xs().text_color(c.danger).child(format!("Couldn't summarize: {e}")),
            Load::Off => div().text_xs().text_color(c.muted).font_family(mono.clone()).child(r.last_output.chars().rev().take(400).collect::<Vec<_>>().into_iter().rev().collect::<String>()),
        };
        let id = r.id.clone();
        let test_id = id.clone();
        let tests: AnyElement = match &r.tests {
            Load::Loading => div().text_xs().text_color(c.subtle).child("Running tests…").into_any_element(),
            Load::Done(Some(t)) => div().text_xs().text_color(if t.ok { c.success } else { c.danger }).child(format!("{} {} ({:.1}s)", t.command, if t.ok { "passed" } else { "failed" }, t.ms / 1000.0)).into_any_element(),
            Load::Done(None) => div().text_xs().text_color(c.subtle).child("No test command found").into_any_element(),
            Load::Error(e) => div().text_xs().text_color(c.danger).child(e.clone()).into_any_element(),
            _ if r.test_command.is_some() => small_button(c, SharedString::from(format!("test-{id}")), "Run tests").on_click(cx.listener(move |this, _, _, cx| this.run_tests(test_id.clone(), cx))).into_any_element(),
            _ => div().text_xs().text_color(c.subtle).child("No test command found for this project").into_any_element(),
        };
        let conflict = conflict_note(conflicts, &r.dir);
        let blocked = conflict.as_ref().is_some_and(|(base, _)| *base);
        let info = conflict.map(|(_, text)| text);
        let screenshot = if let Load::Done(image) = &r.screenshot { Some(image.clone()) } else { None };
        let (pane_id, merge_pane, discard_pane, changes_pane, focus_pane) = (r.pane_id.clone(), r.pane_id.clone(), r.pane_id.clone(), r.pane_id.clone(), r.pane_id.clone());
        let has_worktree = pane.as_ref().is_some_and(|p| p.worktree.is_some());
        let dismiss = r.id.clone();
        let queue_id = r.id.clone();
        let comment_id = SharedString::from(format!("review-open-{pane_id}"));
        let reply = self.review.replies.get(&r.id).cloned();
        let reply_id = r.id.clone();
        let can_reply = pane.is_some();
        let section = |label: &'static str| div().text_size(px(11.0)).font_weight(FontWeight::SEMIBOLD).text_color(c.subtle).child(label.to_uppercase());
        div().flex().flex_col().gap_4()
            .child(header)
            .when_some(r.prompt.clone(), |d, p| d.child(div().flex().flex_col().gap_1().child(section("Task")).child(div().text_size(px(12.5)).text_color(c.muted).child(p))))
            .child(div().flex().flex_col().gap_1p5().child(section("What it did")).child(summary))
            .child(div().flex().flex_col().gap_1p5().child(section("Checks")).child(tests)
                .when_some(info, |d, t| d.child(div().flex().items_center().gap_1p5().rounded_md().bg(Hsla { a: 0.08, ..c.warning }).px_2p5().py_1p5().text_xs().text_color(c.warning)
                    .child(Icon::new(IconName::TriangleAlert).size(px(12.0))).child(t))))
            .when_some(screenshot, |d, image| d.child(div().flex().flex_col().gap_1p5().child(section("Running app"))
                .child(img(ImageSource::Render(image)).w_full().max_w(px(520.0)).h(px(240.0)).object_fit(ObjectFit::Cover).rounded_lg().border_1().border_color(c.edge))))
            .when_some(reply, |d, input| d.child(
                div().flex().items_center().gap_1().rounded_md().border_1().border_color(c.edge).bg(c.panel).px_2()
                    .child(div().flex_1().when(!can_reply, |d| d.opacity(0.5)).child(Input::new(&input).appearance(false)))
                    .child(
                        div().id(SharedString::from(format!("send-back-{}", r.id))).rounded_sm().p_1().cursor_pointer().text_color(c.subtle).hover(move |d| d.text_color(c.fg))
                            .tooltip(crate::ui::code_view::tip("Send back"))
                            .on_click(cx.listener(move |this, _, window, cx| this.send_back(&reply_id, window, cx)))
                            .child(Icon::new(IconName::CornerDownLeft).size(px(12.0))),
                    ),
            ))
            .child(
                div().flex().items_center().gap_2()
                    .when(has_worktree, |d| {
                        d.child(div().id(SharedString::from(format!("merge-{}", r.id))).flex().items_center().gap_1p5().h(px(28.0)).px_3().rounded_md().text_size(px(12.5)).font_weight(FontWeight::MEDIUM)
                            .when(!blocked, |d| d.bg(c.accent).text_color(c.on_accent).cursor_pointer().hover(|d| d.opacity(0.9))
                                .on_click(cx.listener(move |this, _, window, cx| this.execute(Command::MergeWorktree(PaneRef::Id(merge_pane.clone())), window, cx))))
                            .when(blocked, |d| d.bg(c.hover).text_color(c.subtle).tooltip(crate::ui::code_view::tip("Resolve the conflict with its base first")))
                            .child(Icon::new(IconName::GitMerge).size(px(13.0))).child("Merge"))
                            .child(small_button(c, SharedString::from(format!("enqueue-{}", r.id)), "Queue").on_click(cx.listener(move |this, _, _, cx| this.enqueue(vec![queue_id.clone()], cx))))
                            .child(small_button(c, SharedString::from(format!("changes-{}", r.id)), "Changes").on_click(cx.listener(move |this, _, window, cx| {
                                this.update_state(cx, |s| s.overlay = Overlay::None);
                                this.execute(Command::ShowChanges(PaneRef::Id(changes_pane.clone())), window, cx);
                            })))
                            .child(small_button(c, SharedString::from(format!("discard-{}", r.id)), "Discard").on_click(cx.listener(move |this, _, window, cx| this.execute(Command::DiscardWorktree(PaneRef::Id(discard_pane.clone())), window, cx))))
                    })
                    .child(div().flex_1())
                    .child(small_button(c, comment_id, "Open pane").on_click(cx.listener(move |this, _, window, cx| {
                        this.update_state(cx, |s| s.overlay = Overlay::None);
                        this.execute(Command::Focus(PaneRef::Id(focus_pane.clone())), window, cx);
                    })))
                    .child(small_button(c, SharedString::from(format!("dismiss-{}", r.id)), "Dismiss").on_click(cx.listener(move |this, _, _, cx| {
                        this.review.reviews.retain(|x| x.id != dismiss);
                        cx.notify();
                    }))),
            )
    }

    fn sync_history(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let open = self.state.read(cx).overlay == Overlay::History;
        if !open {
            if self.history.is_some() {
                self.history = None;
                self.focused_view = None;
            }
            return;
        }
        if self.history.is_some() {
            return;
        }
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Search titles, summaries, tasks, branches, files…"));
        cx.subscribe(&input, |this, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                this.search_history(cx);
            }
        })
        .detach();
        input.read(cx).focus_handle(cx).focus(window, cx);
        self.history = Some(HistoryUi { input, records: None, this_workspace: false, expanded: None, search: None });
        self.search_history(cx);
    }

    fn search_history(&mut self, cx: &mut Context<Self>) {
        let workspace = self.state.read(cx).active_workspace_id.clone();
        let Some(ui) = self.history.as_mut() else { return };
        let query = ui.input.read(cx).value().to_string();
        let scope = ui.this_workspace.then_some(workspace);
        ui.search = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(150)).await;
            let found = backend::background(move || store::history_search(backend::app().state(), query, scope).map(|r| serde_json::to_value(r).unwrap_or_default())).await.ok().and_then(Result::ok);
            let _ = this.update(cx, |this, cx| {
                if let Some(ui) = this.history.as_mut() {
                    ui.records = Some(found.and_then(|v| v.as_array().cloned()).unwrap_or_default());
                }
                cx.notify();
            });
        }));
    }

    pub(super) fn render_history(&mut self, c: &Colors, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        self.sync_history(window, cx);
        let ui = self.history.as_ref()?;
        let c = *c;
        let input = ui.input.clone();
        let this_workspace = ui.this_workspace;
        let expanded = ui.expanded.clone();
        let records = ui.records.clone();
        let mono = crate::fonts::mono(cx);
        let mut list = div().flex().flex_col().gap_1();
        let mut totals = (0usize, 0usize, 0u64, 0u64);
        let mut last_day: Option<String> = None;
        match &records {
            None => list = list.child(div().py_4().text_center().text_xs().text_color(c.subtle).child("Loading…")),
            Some(found) if found.is_empty() => list = list.child(div().py_8().text_center().text_xs().text_color(c.subtle).child("Nothing merged yet, or nothing matches.")),
            Some(found) => {
                for r in found {
                    let files = r["files"].as_array().map_or(0, Vec::len);
                    totals = (totals.0 + 1, totals.1 + files, totals.2 + r["adds"].as_u64().unwrap_or(0), totals.3 + r["dels"].as_u64().unwrap_or(0));
                    let day = day_label(r["mergedAt"].as_f64().unwrap_or(0.0));
                    if last_day.as_ref() != Some(&day) {
                        list = list.child(div().px_3().pt_3().pb_1().text_size(px(11.0)).font_weight(FontWeight::SEMIBOLD).text_color(c.subtle).child(day.to_uppercase()));
                        last_day = Some(day);
                    }
                    let id = r["id"].as_str().unwrap_or_default().to_string();
                    let open = expanded.as_ref() == Some(&id);
                    let toggle = id.clone();
                    let delete = id.clone();
                    let meta = format!("{} · {}", r["workspace"].as_str().unwrap_or_default(), r["branch"].as_str().unwrap_or_default());
                    let (adds, dels) = (r["adds"].as_u64().unwrap_or(0), r["dels"].as_u64().unwrap_or(0));
                    let when = relative_time(r["mergedAt"].as_f64().unwrap_or(0.0));
                    let body = open.then(|| {
                        div().flex().flex_col().gap_1().pt_1().text_xs().text_color(c.muted)
                            .when_some(r["summary"].as_str().map(str::to_string), |d, t| d.child(div().text_color(c.fg).child(t)))
                            .when_some(r["task"].as_str().map(str::to_string), |d, t| d.child(div().child(format!("Task: {t}"))))
                            .when_some(r["tests"].as_str().map(str::to_string), |d, t| d.child(div().child(format!("Tests: {t}"))))
                            .child(div().font_family(mono.clone()).text_size(px(11.0)).child(r["files"].as_array().map_or(String::new(), |f| f.iter().filter_map(Value::as_str).collect::<Vec<_>>().join("  "))))
                            .child(small_button(c, SharedString::from(format!("history-del-{id}")), "Delete").on_click(cx.listener(move |this, _, _, cx| {
                                let target = delete.clone();
                                let _ = store::history_delete(backend::app().state(), target);
                                this.search_history(cx);
                            })))
                    });
                    list = list.child(
                        div().id(SharedString::from(format!("history-{id}"))).flex().flex_col().rounded_lg().px_3().py_2p5().cursor_pointer()
                            .when(open, |d| d.bg(c.panel).border_1().border_color(c.edge)).when(!open, |d| d.hover(move |d| d.bg(c.hover)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(ui) = this.history.as_mut() {
                                    ui.expanded = if ui.expanded.as_ref() == Some(&toggle) { None } else { Some(toggle.clone()) };
                                }
                                cx.notify();
                            }))
                            .child(div().flex().items_center().gap_2p5()
                                .child(div().size(px(26.0)).flex_shrink_0().flex().items_center().justify_center().rounded_md().bg(Hsla { a: 0.12, ..c.success }).text_color(c.success).child(Icon::new(IconName::GitMerge).size(px(13.0))))
                                .child(div().flex_1().min_w_0().flex().flex_col()
                                    .child(div().text_size(px(13.0)).font_weight(FontWeight::MEDIUM).text_color(c.fg).truncate().child(r["title"].as_str().unwrap_or_default().to_string()))
                                    .child(div().font_family(mono.clone()).text_size(px(11.0)).text_color(c.subtle).truncate().child(meta)))
                                .child(div().flex().flex_shrink_0().items_center().gap_1p5().text_size(px(11.0))
                                    .child(div().text_color(c.subtle).child(format!("{files} files")))
                                    .child(div().text_color(c.success).child(format!("+{adds}")))
                                    .child(div().text_color(c.danger).child(format!("−{dels}")))
                                    .child(div().w(px(56.0)).text_right().text_color(c.subtle).child(when))))
                            .children(body.map(|b| b.pl(px(36.0)))),
                    );
                }
            }
        }
        let stat = |label: &'static str, value: String, color: Hsla| {
            div().flex_1().flex().flex_col().gap_0p5().p_2p5().rounded_lg().border_1().border_color(c.edge).bg(c.panel)
                .child(div().text_size(px(17.0)).font_weight(FontWeight::SEMIBOLD).text_color(color).child(value))
                .child(div().text_size(px(11.0)).text_color(c.subtle).child(label))
        };
        let scope = |id: &'static str, label: &'static str, on: bool, this: bool, cx: &mut Context<Self>| {
            div().id(id).h(px(24.0)).px_2p5().flex().items_center().rounded(px(6.0)).text_size(px(12.0)).cursor_pointer()
                .text_color(if on { c.fg } else { c.muted }).when(on, |d| d.bg(c.active)).hover(move |d| d.text_color(c.fg))
                .on_click(cx.listener(move |this_root, _, _, cx| {
                    if let Some(ui) = this_root.history.as_mut() {
                        ui.this_workspace = this;
                    }
                    this_root.search_history(cx);
                }))
                .child(label)
        };
        Some(
            div().id("history-backdrop").absolute().size_full().flex().items_center().justify_center().bg(hsla(0.0, 0.0, 0.0, 0.4)).occlude()
                .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| this.update_state(cx, |s| s.overlay = Overlay::None)))
                .child(
                    div().id("history").w(px(820.0)).max_w(relative(0.94)).h(px(640.0)).max_h(relative(0.9)).flex().flex_col().overflow_hidden().rounded(px(14.0)).border_1().border_color(c.edge).bg(c.raised).shadow_2xl()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(div().flex().flex_col().gap_3().px_5().pt_4().pb_3()
                            .child(div().flex().items_center().gap_2()
                                .child(Icon::new(IconName::Clock).size(px(15.0)).text_color(c.accent))
                                .child(div().flex_1().text_size(px(15.0)).font_weight(FontWeight::SEMIBOLD).text_color(c.fg).child("History"))
                                .child(div().flex().gap_0p5().p(px(2.0)).rounded(px(8.0)).border_1().border_color(c.edge).bg(c.app)
                                    .child(scope("history-this", "This workspace", this_workspace, true, cx))
                                    .child(scope("history-all", "All workspaces", !this_workspace, false, cx))))
                            .child(div().flex().gap_2()
                                .child(stat("merged", totals.0.to_string(), c.fg))
                                .child(stat("files changed", totals.1.to_string(), c.fg))
                                .child(stat("lines added", format!("+{}", totals.2), c.success))
                                .child(stat("lines removed", format!("−{}", totals.3), c.danger)))
                            .child(div().flex().items_center().gap_2().h(px(34.0)).px_2p5().rounded_md().border_1().border_color(c.edge).bg(c.app)
                                .child(Icon::new(IconName::Search).size(px(13.0)).text_color(c.subtle))
                                .child(div().flex_1().child(Input::new(&input).appearance(false)))))
                        .child(div().id("history-list").flex_1().min_h_0().overflow_y_scroll().p_2().child(list)),
                )
                .into_any_element(),
        )
    }
}

/// "Today", "Yesterday", "3 days ago", … for grouping merges.
fn day_label(at_ms: f64) -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_millis() as f64);
    let days = ((now / 86_400_000.0).floor() - (at_ms / 86_400_000.0).floor()).max(0.0) as u64;
    match days {
        0 => "Today".into(),
        1 => "Yesterday".into(),
        2..=6 => format!("{days} days ago"),
        7..=13 => "Last week".into(),
        _ => format!("{} weeks ago", days / 7),
    }
}

/// Whether the review's worktree conflicts with its base (true) or is only behind, and a line
/// saying so.
fn conflict_note(report: Option<&Value>, dir: &str) -> Option<(bool, String)> {
    let path = dir.trim_end_matches('/');
    let item = report?["items"].as_array()?.iter().find(|i| i["path"].as_str().is_some_and(|p| p.trim_end_matches('/') == path))?;
    let base: Vec<&str> = item["baseConflicts"].as_array()?.iter().filter_map(Value::as_str).collect();
    let behind = item["behind"].as_u64().unwrap_or(0);
    (!base.is_empty() || behind > 0).then(|| (!base.is_empty(), if base.is_empty() { format!("{behind} commits behind its base") } else { format!("Conflicts with its base in {}", base.join(", ")) }))
}

pub(super) fn small_button(c: Colors, id: impl Into<ElementId>, label: impl Into<SharedString>) -> Stateful<Div> {
    div().id(id).rounded_md().border_1().border_color(c.edge).px_2().py_0p5().text_xs().text_color(c.muted).cursor_pointer().hover(move |d| d.bg(c.hover).text_color(c.fg)).child(label.into())
}

async fn run_tests_for(dir: String) -> Result<Option<TestRun>, String> {
    let value = tauri::async_runtime::spawn(async move { review_backend::review_run_tests(dir).await.map(|r| serde_json::to_value(r).unwrap_or_default()) }).await.map_err(|e| e.to_string())??;
    serde_json::from_value(value).map_err(|e| e.to_string())
}

fn decode_screenshot(value: &str) -> Result<Arc<RenderImage>, String> {
    let encoded = value.split_once(',').map_or(value, |(_, b)| b);
    let bytes = base64::engine::general_purpose::STANDARD.decode(encoded).map_err(|e| e.to_string())?;
    let mut rgba = image::load_from_memory(&bytes).map_err(|e| e.to_string())?.into_rgba8();
    for pixel in rgba.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }
    Ok(Arc::new(RenderImage::new(smallvec![image::Frame::new(rgba)])))
}

/// Development aid: a few merges to look at in History.
#[cfg(debug_assertions)]
pub(super) fn demo_history(workspace_id: &str, workspace: &str) {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64);
    let demos = [
        (0.1, "Dark mode toggle", "winfer/dark-mode", "claude", 6, 214, 31),
        (2.0, "Fix checkout redirect loop", "winfer/checkout-fix", "codex", 3, 48, 22),
        (26.0, "Persist session titles", "winfer/session-titles", "claude", 4, 96, 12),
        (75.0, "CSV export for invoices", "winfer/csv-export", "codex", 5, 160, 8),
    ];
    for (hours, title, branch, provider, files, adds, dels) in demos {
        let merge = json!({
            "id": crate::model::new_id("merge"), "mergedAt": now - (hours * 3_600_000.0) as i64, "workspaceId": workspace_id, "workspace": workspace,
            "repo": "/tmp/orbit-demo", "branch": branch, "base": "main", "commitSha": null, "provider": provider,
            "title": title, "summary": format!("{title}, with tests."), "task": title, "files": (0..files).map(|i| format!("src/file{i}.ts")).collect::<Vec<_>>(),
            "adds": adds, "dels": dels, "tests": "pnpm test passed", "cost": null, "tokens": null,
        });
        if let Ok(record) = serde_json::from_value(merge) {
            let _ = store::history_add(backend::app().state(), record);
        }
    }
}

fn record_merge(snapshot: MergeSnapshot) {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64);
    let merge = json!({
        "id": crate::model::new_id("merge"), "mergedAt": now, "workspaceId": snapshot.workspace_id, "workspace": snapshot.workspace,
        "repo": snapshot.repo, "branch": snapshot.branch, "base": snapshot.base, "commitSha": null, "provider": snapshot.provider,
        "title": snapshot.title, "summary": snapshot.summary, "task": snapshot.task, "files": snapshot.files, "adds": snapshot.adds,
        "dels": snapshot.dels, "tests": snapshot.tests, "cost": null, "tokens": null,
    });
    backend::background(move || {
        if let Ok(record) = serde_json::from_value(merge) {
            let _ = store::history_add(backend::app().state(), record);
        }
    });
}

async fn process_queue_item(this: WeakEntity<Root>, item: QueueItem, cx: &mut AsyncApp) -> Result<(), String> {
    use QueueState::*;
    let id = item.review_id.clone();
    let (info, review, name, pane, run_tests) = this
        .update(cx, |this, cx| {
            let review = this.review.reviews.iter().find(|r| r.id == id && r.status == ReviewStatus::Open).cloned();
            let pane = this.state.read(cx).pane(&item.pane_id).cloned();
            let name = pane.as_ref().map_or(String::new(), |p| this.state.read(cx).provider_name(&p.provider));
            (pane.as_ref().and_then(|p| p.worktree.clone()), review, name, pane, this.review.queue_tests)
        })
        .map_err(|e| e.to_string())?;
    let Some(review) = review else {
        let _ = this.update(cx, |this, cx| this.patch_queue(&id, Failed, Some("The review was closed.".to_string()), Vec::new(), cx));
        return Ok(());
    };
    let (Some(info), Some(pane)) = (info, pane) else {
        let _ = this.update(cx, |this, cx| this.patch_queue(&id, Failed, Some("The agent's pane or worktree is gone.".to_string()), Vec::new(), cx));
        return Ok(());
    };
    let _ = this.update(cx, |this, cx| this.patch_queue(&id, Syncing, Some(format!("Bringing in {}…", info.base)), Vec::new(), cx));
    let message = commit_message(Some(&review)).unwrap_or_else(|| format!("Changes by {name}"));
    let bridged = worktree_info(&info)?;
    let commit = bridged.clone();
    tauri::async_runtime::spawn(async move { worktree::worktree_commit(commit, message).await }).await.map_err(|e| e.to_string())??;
    let snapshot = this.update(cx, |this, cx| this.snapshot_for_history(&pane, review.stats.clone(), cx)).map_err(|e| e.to_string())?;
    let sync = bridged.clone();
    let outcome = tauri::async_runtime::spawn(async move { worktree::worktree_sync(sync).await.map(|v| serde_json::to_value(v).unwrap_or_default()) }).await.map_err(|e| e.to_string())??;
    let kind = outcome["kind"].as_str().unwrap_or_default().to_string();
    if kind == "conflicts" {
        let files: Vec<String> = serde_json::from_value(outcome["files"].clone()).unwrap_or_default();
        let _ = this.update(cx, |this, cx| this.patch_queue(&id, Conflict, Some(format!("Conflicts with {}", info.base)), files, cx));
        return Ok(());
    }
    let synced = kind == "synced";
    let commits = outcome["commits"].as_u64().unwrap_or(0);
    // Tests that already passed on this exact branch needn't run again.
    let fresh = !synced && matches!(&review.tests, Load::Done(Some(t)) if t.ok);
    let mut tests_line = snapshot.as_ref().and_then(|s| s.tests.clone());
    if run_tests && !fresh {
        let dir = review.dir.clone();
        let known = review.test_command.clone();
        let command = match known {
            Some(c) => Some(c),
            None => backend::background(move || review_backend::review_test_command(dir)).await.ok().flatten(),
        };
        if let Some(command) = command {
            let with = if synced { format!(" with {}'s {commits} new commits", info.base) } else { String::new() };
            let _ = this.update(cx, |this, cx| this.patch_queue(&id, Testing, Some(format!("Running {command}{with}…")), Vec::new(), cx));
            match run_tests_for(review.dir.clone()).await {
                Err(err) => {
                    let detail = format!("Tests couldn't run: {err}");
                    let _ = this.update(cx, |this, cx| {
                        if let Some(r) = this.review_mut(&id) {
                            r.tests = Load::Error(err);
                        }
                        this.patch_queue(&id, Failed, Some(detail), Vec::new(), cx);
                    });
                    return Ok(());
                }
                Ok(run) => {
                    let failed = run.as_ref().is_some_and(|t| !t.ok);
                    if let Some(t) = &run {
                        tests_line = Some(format!("{} {}", t.command, if t.ok { "passed" } else { "failed" }));
                    }
                    let failed_command = run.as_ref().map_or(String::new(), |t| t.command.clone());
                    let _ = this.update(cx, |this, _| {
                        if let Some(r) = this.review_mut(&id) {
                            r.tests = Load::Done(run);
                        }
                    });
                    if failed {
                        let after = if synced { format!(" after bringing in {}", info.base) } else { String::new() };
                        let _ = this.update(cx, |this, cx| this.patch_queue(&id, Failed, Some(format!("{failed_command} failed{after}")), Vec::new(), cx));
                        return Ok(());
                    }
                }
            }
        }
    }
    let _ = this.update(cx, |this, cx| this.patch_queue(&id, Merging, Some(format!("Merging into {}…", info.base)), Vec::new(), cx));
    let merge = bridged;
    tauri::async_runtime::spawn(async move { worktree::worktree_merge(merge).await }).await.map_err(|e| e.to_string())??;
    if let Some(mut snapshot) = snapshot {
        snapshot.tests = tests_line;
        record_merge(snapshot);
    }
    let _ = this.update(cx, |this, cx| {
        if let Some(r) = this.review_mut(&id) {
            r.status = ReviewStatus::Merged;
        }
        this.update_state(cx, |s| s.log_activity(&item.workspace_id, format!("Merged {} into {} (queue)", info.branch, info.base), Some(pane.provider.clone())));
        this.patch_queue(&id, Merged, None, Vec::new(), cx);
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{commit_message, diff_stats, first_sentence, last_output, reply_language, wrong_script};
    use serde_json::json;

    #[test]
    fn stats_count_changes() {
        let d = json!({ "diff": "diff --git a/x b/x\n--- a/x\n+++ b/x\n-old\n+new\n+more", "untracked": ["y"], "ahead": 2, "dirty": true });
        let s = diff_stats(&d);
        assert_eq!(s.files, ["x", "y"]);
        assert_eq!((s.adds, s.dels, s.ahead, s.dirty), (2, 1, 2, true));
    }

    #[test]
    fn languages_and_scripts() {
        assert_eq!(reply_language("bitte fix das"), "German");
        assert_eq!(reply_language("fix the bug"), "English");
        assert!(wrong_script("Готово", "fix it"));
        assert!(!wrong_script("Done, über", "fix it"));
    }

    #[test]
    fn sentences_and_chrome() {
        assert_eq!(first_sentence("One two. Three."), "One two.");
        assert_eq!(last_output("hello\n────\n\n> \n"), "hello");
        assert_eq!(commit_message(None), None);
    }
}
