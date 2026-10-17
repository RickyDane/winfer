//! `winfer code` settings and paths: `<data>/harness/config.json`.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::event::Mode;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Config {
    /// `backend/model`; empty picks the first backend that's available.
    pub model: String,
    pub effort: Option<String>,
    pub mode: Mode,
    /// Extra MCP servers for every backend: `{ name: { command, args, env } }`.
    pub mcp_servers: serde_json::Map<String, Value>,
    /// Commands always allowed (first word), for the ChatGPT backend.
    pub allowed_commands: Vec<String>,
    /// Prompt history, newest last.
    pub history: Vec<String>,
    /// Context fill (%) at which old tool output is trimmed (ChatGPT backend); 0 = off.
    pub trim_at: u8,
    /// Context fill (%) at which the conversation is summarized; 0 = off.
    pub compact_at: u8,
    /// Start Claude Code with a short prompt and only the tools coding needs.
    pub lean_bridges: bool,
    /// In lean mode, load every skill of the user's (with their settings, hooks and plugins)…
    pub lean_all_skills: bool,
    /// …or only these ([`super::claude_skills::ClaudeSkill`] ids).
    pub lean_skills: Vec<String>,
    /// Show the plan in a sidebar on the right when the terminal is wide enough.
    pub plan_sidebar: bool,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            model: String::new(),
            effort: None,
            mode: Mode::Ask,
            mcp_servers: Default::default(),
            allowed_commands: Vec::new(),
            history: Vec::new(),
            trim_at: 60,
            compact_at: 85,
            lean_bridges: true,
            lean_all_skills: true,
            lean_skills: Vec::new(),
            plan_sidebar: true,
        }
    }
}

pub fn dir() -> PathBuf {
    dirs::data_dir().unwrap_or_else(std::env::temp_dir).join("dev.winfer.app").join("harness")
}

impl Config {
    /// The skills Claude Code gets in lean mode.
    pub fn lean_skills(&self) -> super::claude_skills::LeanSkills {
        if self.lean_all_skills {
            super::claude_skills::LeanSkills::All
        } else {
            super::claude_skills::LeanSkills::Only(self.lean_skills.clone())
        }
    }

    pub fn load() -> Self {
        std::fs::read_to_string(dir().join("config.json"))
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        // Tests must never touch the user's settings.
        if cfg!(test) {
            return;
        }
        let _ = std::fs::create_dir_all(dir());
        if let Ok(text) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(dir().join("config.json"), text);
        }
    }

    /// Changes the saved settings in place (re-read first), so the app's Settings and running
    /// `winfer code` sessions don't overwrite each other's changes.
    pub fn update(f: impl FnOnce(&mut Config)) -> Config {
        let mut c = Config::load();
        f(&mut c);
        c.save();
        c
    }

    pub fn push_history(&mut self, entry: &str) {
        if entry.trim().is_empty() || self.history.last().is_some_and(|l| l == entry) {
            return;
        }
        self.history.push(entry.to_string());
        let extra = self.history.len().saturating_sub(500);
        self.history.drain(..extra);
    }
}

/// The MCP servers every backend gets: winfer's own (set by the app on panes, like pi's
/// `WINFER_PI_MCP`) plus the user's.
pub fn mcp_servers(config: &Config) -> serde_json::Map<String, Value> {
    let mut servers = std::env::var("WINFER_HARNESS_MCP")
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Map<String, Value>>(&t).ok())
        .unwrap_or_default();
    for (k, v) in &config.mcp_servers {
        servers.insert(k.clone(), v.clone());
    }
    servers
}

/// Project instructions: AGENTS.md (or CLAUDE.md) from the cwd up to the repo root, plus the
/// user's global `~/.config/winfer/AGENTS.md`.
pub fn project_instructions(cwd: &std::path::Path) -> String {
    let mut parts = Vec::new();
    if let Some(home) = dirs::home_dir() {
        if let Ok(text) = std::fs::read_to_string(home.join(".config/winfer/AGENTS.md")) {
            parts.push(text);
        }
    }
    let mut found = Vec::new();
    let mut dir = Some(cwd);
    while let Some(d) = dir {
        for name in ["AGENTS.md", "CLAUDE.md"] {
            if let Ok(text) = std::fs::read_to_string(d.join(name)) {
                found.push(format!("# {}\n\n{text}", d.join(name).display()));
                break;
            }
        }
        if d.join(".git").exists() {
            break;
        }
        dir = d.parent();
    }
    found.reverse();
    parts.extend(found);
    parts.join("\n\n")
}
