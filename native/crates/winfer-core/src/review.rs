//! The review inbox's GPUI-free parts, shared by the desktop's Review and the server's (`winferd`):
//! diff stats, the summary prompt, commit messages and conflict notes.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
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

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TestRun {
    pub command: String,
    pub ok: bool,
    pub ms: f64,
}

pub const SUMMARY_SYSTEM: &str = "You review the work a coding agent just finished, for a developer deciding whether to merge it. From the diff and the agent's last output, write 2–3 short sentences: what changed and why, then anything risky, unfinished or worth checking (failing steps, TODOs, deleted code). Name files only when it helps. Output only the summary.";

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

pub fn first_sentence(text: &str) -> &str {
    let bytes: Vec<(usize, char)> = text.char_indices().collect();
    for window in bytes.windows(2) {
        if matches!(window[0].1, '.' | '!' | '?') && window[1].1.is_whitespace() {
            return &text[..=window[0].0];
        }
    }
    text
}

pub fn shorten(text: &str, max: usize) -> String {
    if text.chars().count() > max { format!("{}…", text.chars().take(max - 3).collect::<String>()) } else { text.to_string() }
}

/// First sentence of the summary, else the task, as a commit message for changes the agent didn't commit.
pub fn commit_message(summary: Option<&str>, prompt: Option<&str>) -> Option<String> {
    match summary {
        Some(text) => Some(shorten(first_sentence(text), 100)),
        None => prompt.map(|p| format!("winfer: {}", p.chars().take(72).collect::<String>())),
    }
}

/// The agent's last words, without blank lines and prompt chrome.
pub fn last_output(text: &str) -> String {
    let lines: Vec<&str> = text.lines().map(str::trim_end).filter(|l| !l.trim().is_empty() && !l.chars().all(|c| "─━═ >│╭╮╰╯".contains(c))).collect();
    let tail = lines[lines.len().saturating_sub(25)..].join("\n");
    let chars: Vec<char> = tail.chars().collect();
    chars[chars.len().saturating_sub(1800)..].iter().collect()
}

/// The system and user prompt for a review's summary.
pub fn summary_prompt(prompt: Option<&str>, stats: Option<&DiffStats>, diff: &str, last_output: &str) -> (String, String) {
    let mut parts = Vec::new();
    if let Some(p) = prompt {
        parts.push(format!("Task: {p}"));
    }
    if let Some(s) = stats {
        let files = if s.files.is_empty() { "none".to_string() } else { s.files.join(", ") };
        parts.push(format!("Files changed: {files} (+{} −{})", s.adds, s.dels));
    }
    if !diff.is_empty() {
        let clipped: String = diff.chars().take(12_000).collect();
        parts.push(format!("Diff:\n{clipped}{}", if diff.chars().count() > 12_000 { "\n…(truncated)" } else { "" }));
    }
    parts.push(format!("Agent's last output:\n{last_output}"));
    (format!("{SUMMARY_SYSTEM} Write it in {}.", reply_language(prompt.unwrap_or_default())), parts.join("\n\n"))
}

/// What to tell an agent whose branch conflicts with its base in the merge queue.
pub fn resolve_prompt(branch: &str, base: &str, files: &[String]) -> String {
    let files = if files.is_empty() { String::new() } else { format!(" in {}", files.join(", ")) };
    format!("Merging your branch {branch} into {base} conflicts{files}. Merge {base} into your branch (git merge {base}), resolve the conflicts keeping both sides' intent, make sure the tests pass, and commit.")
}

/// Whether the worktree at `dir` conflicts with its base (true) or is only behind, and a line
/// saying so, from a `worktree_conflicts` report.
pub fn conflict_note(report: Option<&Value>, dir: &str) -> Option<(bool, String)> {
    let path = dir.trim_end_matches('/');
    let item = report?["items"].as_array()?.iter().find(|i| i["path"].as_str().is_some_and(|p| p.trim_end_matches('/') == path))?;
    let base: Vec<&str> = item["baseConflicts"].as_array()?.iter().filter_map(Value::as_str).collect();
    let behind = item["behind"].as_u64().unwrap_or(0);
    (!base.is_empty() || behind > 0).then(|| (!base.is_empty(), if base.is_empty() { format!("{behind} commits behind its base") } else { format!("Conflicts with its base in {}", base.join(", ")) }))
}

#[cfg(test)]
mod tests {
    use super::*;
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
        assert_eq!(commit_message(None, None), None);
        assert_eq!(commit_message(Some("Adds X. Also Y."), Some("task")).as_deref(), Some("Adds X."));
        assert_eq!(commit_message(None, Some("fix it")).as_deref(), Some("winfer: fix it"));
    }

    #[test]
    fn conflicts_by_path() {
        let report = json!({"items": [{"path": "/w/a/", "baseConflicts": ["x.rs"], "behind": 1}, {"path": "/w/b", "baseConflicts": [], "behind": 3}]});
        assert_eq!(conflict_note(Some(&report), "/w/a"), Some((true, "Conflicts with its base in x.rs".into())));
        assert_eq!(conflict_note(Some(&report), "/w/b/"), Some((false, "3 commits behind its base".into())));
        assert_eq!(conflict_note(Some(&report), "/w/c"), None);
    }
}
