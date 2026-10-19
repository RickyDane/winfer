//! Saved `winfer code` sessions (any backend), for `/resume` and `--resume`. The backend's own
//! session id is kept so the conversation continues where it was; the transcript is kept so it
//! can be shown again.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Saved {
    pub id: String,
    /// `backend/model`
    pub model: String,
    pub backend_session: Option<String>,
    pub cwd: String,
    pub title: String,
    pub updated: u64,
    pub transcript: Vec<Entry>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Entry {
    /// "user" | "assistant" | "tool" | "notice"
    pub role: String,
    pub text: String,
    /// Tools: "running" | "done" | "failed" (a background task's own status once it has one).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    /// Tools: a background shell or subagent.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub background: bool,
}

fn dir() -> PathBuf {
    super::config::dir().join("sessions")
}

pub fn new_id() -> String {
    format!("{:012x}", rand::random::<u64>() & 0xffff_ffff_ffff)
}

pub fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

pub fn save(s: &Saved) {
    let _ = std::fs::create_dir_all(dir());
    if let Ok(text) = serde_json::to_string(s) {
        let _ = std::fs::write(dir().join(format!("{}.json", s.id)), text);
    }
}

pub fn load(id: &str) -> Option<Saved> {
    let id: String = id.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').collect();
    serde_json::from_str(&std::fs::read_to_string(dir().join(format!("{id}.json"))).ok()?).ok()
}

/// Sessions, newest first (without transcripts).
pub fn list() -> Vec<Saved> {
    let mut out: Vec<Saved> = std::fs::read_dir(dir())
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| serde_json::from_str::<Saved>(&std::fs::read_to_string(e.path()).ok()?).ok())
        .map(|mut s| {
            s.transcript.clear();
            s
        })
        .collect();
    out.sort_by(|a, b| b.updated.cmp(&a.updated));
    out
}

/// "3m ago", "2h ago", "5d ago".
pub fn ago(ts: u64) -> String {
    let d = now().saturating_sub(ts);
    match d {
        0..60 => "just now".into(),
        60..3600 => format!("{}m ago", d / 60),
        3600..86_400 => format!("{}h ago", d / 3600),
        _ => format!("{}d ago", d / 86_400),
    }
}
