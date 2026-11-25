//! Session progress and race timings (the state parts of `features/progress/progress.ts` and
//! `features/orchestration/runTimes.ts`).

use super::{AgentStatus, AppState, Pane, PaneTask};
use std::time::SystemTime;

pub const MAX_SESSION_TITLES: usize = 500;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Progress {
    /// What the agent was asked to do (its start prompt or the last message it got).
    pub task: Option<String>,
    /// Short task title from the LLM, for compact displays.
    pub title: Option<String>,
    /// 0–100, or `None` before the first estimate.
    pub percent: Option<u32>,
    /// What the agent is doing right now, per the LLM.
    pub step: Option<String>,
    pub updated_at: Option<SystemTime>,
    pub estimating: bool,
    pub error: Option<String>,
    /// Output the last estimate was based on, so unchanged panes aren't re-asked.
    pub last_output: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Estimate {
    pub percent: u32,
    pub step: Option<String>,
    pub title: Option<String>,
}

fn clipped(value: &serde_json::Value) -> Option<String> {
    let text = value.as_str()?.trim();
    (!text.is_empty()).then(|| text.chars().take(80).collect())
}

/// Pulls {percent, step, title} out of a model answer, tolerating prose around the JSON.
pub fn parse_estimate(answer: &str) -> Option<Estimate> {
    let json: serde_json::Value = serde_json::from_str(answer.get(answer.find('{')?..=answer.rfind('}')?)?).ok()?;
    let percent = match &json["percent"] {
        serde_json::Value::String(s) => s.trim().parse::<f64>().ok()?,
        v => v.as_f64()?,
    };
    if !percent.is_finite() {
        return None;
    }
    Some(Estimate { percent: percent.round().clamp(0.0, 100.0) as u32, step: clipped(&json["step"]), title: clipped(&json["title"]) })
}

const QUOTES: &[char] = &['"', '\'', '“', '”', '‘', '’', '`', '*'];
const TRAILING: &[char] = &['.', '!', '?', ':', ';', ','];

/// A model's title answer, without quotes, a "Title:" label or trailing punctuation.
pub fn clean_title(answer: &str) -> Option<String> {
    let line = answer.trim().lines().next().unwrap_or_default();
    let lower = line.to_lowercase();
    let unlabeled = ["title", "titel"].iter().find_map(|label| {
        let rest = lower.strip_prefix(label)?;
        let after = rest.trim_start();
        after.starts_with(':').then(|| line[line.len() - after.len() + 1..].trim_start())
    });
    let text = unlabeled.unwrap_or(line).trim_matches(QUOTES).trim_end_matches(TRAILING);
    let title = text.split_whitespace().collect::<Vec<_>>().join(" ");
    (!title.is_empty() && title.split(' ').count() <= 10).then(|| title.chars().take(60).collect())
}

const FILLER: [&str; 34] = [
    "i'd like you to", "i want you to", "i need you to", "i noticed that", "it seems that", "i see that", "thank you", "kannst du",
    "can you", "could you", "would you", "will you", "i noticed", "it seems", "i see", "alright", "thanks", "please", "great", "okay",
    "thank", "bitte", "let's", "lets", "also", "then", "next", "hey", "and", "now", "pls", "hi", "ok", "so",
];

/// The filler word `text` starts with, if any, plus the separators after it.
fn strip_filler(text: &str) -> Option<&str> {
    let lower = text.to_lowercase();
    let filler = FILLER.iter().find(|f| {
        lower.starts_with(**f) && !lower[f.len()..].chars().next().is_some_and(|c| c.is_alphanumeric() || c == '_')
    })?;
    Some(text.get(filler.len()..)?.trim_start_matches(|c: char| c.is_whitespace() || ",:;!.-".contains(c)))
}

fn first_sentence(task: &str) -> &str {
    let task = task.trim();
    let mut chars = task.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        if c == '\n' {
            return task[..i].trim();
        }
        if matches!(c, '.' | '!' | '?') && chars.peek().is_some_and(|(_, next)| next.is_whitespace()) {
            return task[..=i].trim();
        }
    }
    task
}

/// Without an LLM title: the task's first sentence, trimmed of filler and cut to a few words.
pub fn task_headline(task: &str) -> String {
    let first = first_sentence(task);
    let mut rest = first;
    while let Some(next) = strip_filler(rest) {
        rest = next;
    }
    let stripped = rest.trim_end_matches(TRAILING).trim();
    let stripped = if stripped.is_empty() { first } else { stripped };
    let words: Vec<&str> = stripped.split_whitespace().collect();
    let short = if words.len() > 8 { format!("{}…", words[..8].join(" ")) } else { stripped.to_string() };
    let mut chars = short.chars();
    chars.next().map_or(String::new(), |c| c.to_uppercase().chain(chars).collect())
}

/// What to call the session's work: the LLM's title, else the task's headline.
pub fn progress_title(p: Option<&Progress>) -> Option<String> {
    let p = p?;
    p.title.clone().or_else(|| p.task.as_deref().map(task_headline))
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RunTime {
    /// First time the agent started working (unix ms).
    pub started_at: u64,
    /// Last time it stopped working; `None` while it hasn't yet.
    pub finished_at: Option<u64>,
    /// Time spent working in finished stretches.
    pub work_ms: u64,
    /// Start of the current working stretch, or `None` when not working.
    pub since: Option<u64>,
}

impl RunTime {
    /// Time spent working so far, counting the current stretch.
    pub fn active_ms(&self, now: u64) -> u64 {
        self.work_ms + self.since.map_or(0, |s| now.saturating_sub(s))
    }
}

/// Applies a status change to a pane's timing.
pub fn next_run_time(t: Option<RunTime>, was: AgentStatus, status: AgentStatus, now: u64) -> Option<RunTime> {
    if status == AgentStatus::Working {
        let Some(t) = t else { return Some(RunTime { started_at: now, finished_at: None, work_ms: 0, since: Some(now) }) };
        return Some(if t.since.is_some() { t } else { RunTime { since: Some(now), ..t } });
    }
    match t {
        Some(RunTime { since: Some(since), .. }) if was == AgentStatus::Working => {
            let t = t.unwrap();
            Some(RunTime { work_ms: t.work_ms + now.saturating_sub(since), since: None, finished_at: Some(now), ..t })
        }
        other => other,
    }
}

impl AppState {
    pub fn progress_of(&self, id: &str) -> Progress {
        self.progress.get(id).cloned().unwrap_or_default()
    }

    /// Changes a pane's progress; task and title also go to the saved state.
    pub fn patch_progress(&mut self, id: &str, patch: impl FnOnce(&mut Progress)) {
        let entry = self.progress.entry(id.to_string()).or_default();
        let before = (entry.task.clone(), entry.title.clone());
        patch(entry);
        if before != (entry.task.clone(), entry.title.clone()) {
            self.save_task(id);
        }
    }

    fn save_task(&mut self, id: &str) {
        let Some(p) = self.progress.get(id) else { return };
        let task = PaneTask { task: p.task.clone(), title: p.title.clone() };
        if self.pane_tasks.get(id) != Some(&task) {
            self.pane_tasks.insert(id.to_string(), task);
            self.sync_session_titles();
        }
    }

    /// Ties titles to agent sessions: a pane's title is saved under its session id, and a pane
    /// that resumes a titled session gets that title back.
    pub fn sync_session_titles(&mut self) {
        let panes: Vec<(String, String)> = self.all_panes().filter_map(|p| Some((p.id.clone(), p.session_id.clone()?))).collect();
        for (id, session) in panes {
            let title = self.progress.get(&id).and_then(|p| p.title.clone());
            let saved = self.session_titles.get(&session).cloned();
            match (title, saved) {
                (Some(title), saved) if saved.as_ref() != Some(&title) => {
                    self.session_titles.insert(session.clone(), title);
                    if self.session_titles.len() > MAX_SESSION_TITLES {
                        let open: Vec<String> = self.all_panes().filter_map(|p| p.session_id.clone()).collect();
                        if let Some(old) = self.session_titles.keys().find(|k| !open.contains(k)).cloned() {
                            self.session_titles.remove(&old);
                        }
                    }
                }
                (None, Some(saved)) => {
                    self.progress.entry(id.clone()).or_default().title = Some(saved);
                    self.save_task(&id);
                }
                _ => {}
            }
        }
    }

    /// Brings back the tasks and titles saved before the last restart.
    pub fn restore_tasks(&mut self) {
        for (id, saved) in &self.pane_tasks {
            self.progress.entry(id.clone()).or_insert_with(|| Progress { task: saved.task.clone(), title: saved.title.clone(), ..Default::default() });
        }
        self.sync_session_titles();
    }

    /// The agent got a new task: progress starts over. Short inputs ("y", "continue", menu picks)
    /// don't replace the task. The title stays: a follow-up usually continues the same work.
    pub fn set_task(&mut self, id: &str, text: &str) {
        let task = text.trim();
        if task.chars().count() < 12 {
            return;
        }
        self.patch_progress(id, |p| {
            p.task = Some(task.to_string());
            p.percent = Some(0);
            p.step = None;
            p.updated_at = None;
            p.error = None;
            p.last_output = None;
        });
    }

    /// Called when an agent finishes: done means done, no need to ask the LLM.
    pub fn mark_finished(&mut self, id: &str) {
        let tracked = self.progress.get(id).is_some_and(|p| p.task.is_some() || p.percent.is_some());
        if tracked {
            self.patch_progress(id, |p| {
                p.percent = Some(100);
                p.step = Some("Finished".into());
                p.updated_at = Some(SystemTime::now());
            });
        }
    }

    pub fn forget_progress(&mut self, id: &str) {
        self.progress.remove(id);
        self.pane_tasks.remove(id);
        self.run_times.remove(id);
    }

    /// Done with its task: hook agents say so; the others go quiet (Claude also goes idle at start).
    pub fn is_finished(&self, pane: &Pane) -> bool {
        matches!(pane.status, AgentStatus::Done | AgentStatus::Error)
            || (pane.status == AgentStatus::Idle && !self.has_hooks(pane) && self.run_times.contains_key(&pane.id))
    }
}

#[cfg(test)]
mod tests {
    use super::{AgentStatus as S, RunTime, clean_title, next_run_time, parse_estimate, task_headline};

    #[test]
    fn estimates_tolerate_prose_and_clamp() {
        let e = parse_estimate("Sure: {\"percent\": 120.4, \"step\": \" Writing \", \"title\": \"\"} done").unwrap();
        assert_eq!((e.percent, e.step.as_deref(), e.title), (100, Some("Writing"), None));
        assert_eq!(parse_estimate("{\"percent\": \"40\"}").unwrap().percent, 40);
        assert_eq!(parse_estimate("no json"), None);
        assert_eq!(parse_estimate("{\"percent\": \"x\"}"), None);
    }

    #[test]
    fn titles_lose_labels_quotes_and_punctuation() {
        assert_eq!(clean_title("Title: \"Fix login redirect.\"\nmore").as_deref(), Some("Fix login redirect"));
        assert_eq!(clean_title("“Dark mode for settings”").as_deref(), Some("Dark mode for settings"));
        assert_eq!(clean_title("one two three four five six seven eight nine ten eleven"), None);
        assert_eq!(clean_title("  "), None);
    }

    #[test]
    fn headlines_drop_filler() {
        assert_eq!(task_headline("Hey, can you please fix the login page. It breaks."), "Fix the login page");
        assert_eq!(task_headline("so"), "So");
        assert_eq!(task_headline("one two three four five six seven eight nine"), "One two three four five six seven eight…");
    }

    #[test]
    fn run_times_count_working_stretches() {
        let t = next_run_time(None, S::Idle, S::Working, 100);
        assert_eq!(t, Some(RunTime { started_at: 100, finished_at: None, work_ms: 0, since: Some(100) }));
        let t = next_run_time(t, S::Working, S::NeedsInput, 150).unwrap();
        assert_eq!((t.work_ms, t.since, t.finished_at), (50, None, Some(150)));
        let t = next_run_time(Some(t), S::NeedsInput, S::Working, 200).unwrap();
        assert_eq!(t.active_ms(230), 80);
        assert_eq!(next_run_time(None, S::Idle, S::Done, 300), None);
    }
}
