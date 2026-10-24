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

/// `list_chats` for agents: saved winfer code chats, newest first. Chats in `cwd` (or a folder
/// inside it) unless `all`.
pub fn chats_text(cwd: &str, all: bool, limit: usize) -> String {
    let cwd = cwd.trim_end_matches('/');
    let mine = |s: &Saved| s.cwd == cwd || s.cwd.starts_with(&format!("{cwd}/"));
    let list: Vec<Saved> = list().into_iter().filter(|s| all || cwd.is_empty() || mine(s)).take(limit.clamp(1, 100)).collect();
    if list.is_empty() {
        return if all { "No saved winfer code chats.".into() } else { "No saved winfer code chats in this folder. Pass all: true to list chats from every folder.".into() };
    }
    let rows: Vec<String> = list
        .iter()
        .map(|s| {
            let title = if s.title.trim().is_empty() { "(untitled)" } else { s.title.trim() };
            format!("{} · {} · {} · {} · {}", s.id, ago(s.updated), s.model, s.cwd, title)
        })
        .collect();
    format!("Saved winfer code chats, newest first (id · updated · model · folder · title). Read one with read_chat.\n{}", rows.join("\n"))
}

/// Most of a chat `read_chat` returns at once.
const READ_MAX_CHARS: usize = 60_000;

/// `read_chat` for agents: a chat's transcript (the latest `id` when it is "latest"), tool calls
/// shortened, the newest messages kept when it is long.
pub fn chat_text(id: &str, cwd: &str, tools: bool) -> Result<String, String> {
    let id = if id.is_empty() || id == "latest" {
        let cwd = cwd.trim_end_matches('/');
        list().into_iter().find(|s| s.cwd == cwd).or_else(|| list().into_iter().next()).map(|s| s.id).ok_or("No saved winfer code chats.")?
    } else {
        id.to_string()
    };
    let s = load(&id).ok_or_else(|| format!("No chat {id}. Use list_chats to see them."))?;
    let mut parts: Vec<String> = s
        .transcript
        .iter()
        .filter(|e| tools || e.role != "tool")
        .map(|e| {
            let text = if e.role == "tool" { short(&e.text, 300) } else { e.text.trim().to_string() };
            format!("[{}]\n{text}", e.role)
        })
        .collect();
    let mut cut = false;
    while parts.len() > 1 && parts.iter().map(|p| p.len() + 2).sum::<usize>() > READ_MAX_CHARS {
        parts.remove(0);
        cut = true;
    }
    let head = format!("Chat {} · {} · {} · {} · {}", s.id, s.title.trim(), s.model, s.cwd, ago(s.updated));
    let note = if cut { "\n(earlier messages left out: too long)" } else { "" };
    Ok(format!("{head}{note}\n\n{}", parts.join("\n\n")))
}

fn short(text: &str, max: usize) -> String {
    let text = text.trim();
    match text.char_indices().nth(max) {
        Some((i, _)) => format!("{}…", &text[..i]),
        None => text.to_string(),
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chats_for_agents() {
        let entry = |role: &str, text: &str| Entry { role: role.into(), text: text.into(), ..Default::default() };
        let id = new_id();
        save(&Saved {
            id: id.clone(),
            model: "claude/opus".into(),
            cwd: "/tmp/chat-proj".into(),
            title: "Fix login".into(),
            updated: now(),
            transcript: vec![entry("user", "fix the login"), entry("tool", &"x".repeat(1000)), entry("assistant", "Done.")],
            ..Default::default()
        });
        assert!(chats_text("/tmp/chat-proj", false, 20).contains(&format!("{id} · just now · claude/opus · /tmp/chat-proj · Fix login")));
        assert!(!chats_text("/tmp/elsewhere", false, 20).contains(&id));
        assert!(chats_text("/tmp/elsewhere", true, 100).contains(&id));

        let text = chat_text("latest", "/tmp/chat-proj", true).unwrap();
        assert!(text.contains("[user]\nfix the login") && text.contains("[assistant]\nDone.") && text.contains('…'));
        assert!(!chat_text(&id, "", false).unwrap().contains("[tool]"));
        assert!(chat_text("nope", "", true).is_err());
    }
}
