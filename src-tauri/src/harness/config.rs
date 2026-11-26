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
    /// Ultracode multi-agent mode (Claude Code's `ultracode` flag, Codex's multi-agent + ultra effort, Antigravity's `/swarm`).
    pub ultracode: bool,
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
    /// Start Codex lean (without its bundled extras, extra instructions and unpicked skills) and
    /// with a cap on each tool result, when a Codex model runs.
    pub lean_codex: bool,
    /// With `lean_codex`, a short system prompt in place of Codex's built-in one.
    pub lean_codex_prompt: bool,
    /// When every account is out of usage, wait for the limit to reset, then continue the session
    /// and send the messages that came in meanwhile (Claude Code and Codex).
    pub auto_resume: bool,
    /// The model subagents run on, per backend id ("chatgpt", "claude", "codex"); a missing or
    /// empty entry means the main model.
    pub subagents: std::collections::BTreeMap<String, SubagentModel>,
    /// A local OpenAI-compatible server (`local/<model>`), see local_llm.rs.
    pub local: LocalLlm,
}

/// Where a local model server listens (Settings → winfer code → Local models).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LocalLlm {
    /// Offer its models in the picker.
    pub enabled: bool,
    /// Host name or IP, optionally with a scheme (`https://box.lan`).
    pub host: String,
    /// 0 = none (the scheme's default).
    pub port: u16,
    /// The API's path on the server.
    pub path: String,
    /// Sent as a bearer token when set.
    pub api_key: String,
    /// Tokens; 0 = what the server reports for the model.
    pub context_window: u64,
    /// The thinking levels to offer for its models, as their chat templates name them (Qwen3.8:
    /// low, medium, xhigh); "none" turns thinking off. Empty: only the model default.
    pub efforts: Vec<String>,
    /// Also pass the level to the chat template (`chat_template_kwargs`), which is where
    /// llama.cpp, oMLX and most MLX servers let a model's template see it.
    pub template_effort: bool,
    /// Thinking-token caps per level, sent as `thinking_budget` (oMLX enforces it); a level
    /// without one thinks as long as the model wants.
    pub thinking_budgets: std::collections::BTreeMap<String, u64>,
    /// Give the models a `web_search` tool when the server searches the web (oMLX).
    pub web_search: bool,
}

impl Default for LocalLlm {
    fn default() -> Self {
        LocalLlm {
            enabled: false,
            host: "127.0.0.1".into(), port: 8000, path: "/v1".into(), api_key: String::new(),
            context_window: 0,
            efforts: ["none", "low", "medium", "xhigh"].map(String::from).into(),
            template_effort: true,
            thinking_budgets: [("low".to_string(), 1024), ("medium".to_string(), 4096)].into(),
            web_search: true,
        }
    }
}

/// The model (and effort) a backend's subagents run on while the main model orchestrates.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SubagentModel {
    /// The backend's own model id (no `backend/` prefix); empty = the main model.
    pub model: String,
    pub effort: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            model: String::new(),
            effort: None,
            ultracode: false,
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
            lean_codex: true,
            lean_codex_prompt: false,
            auto_resume: true,
            subagents: Default::default(),
            local: LocalLlm::default(),
        }
    }
}

pub fn dir() -> PathBuf {
    // Tests never touch the user's sessions, config or checkpoints.
    if cfg!(test) {
        return std::env::temp_dir().join(format!("winfer-harness-test-{}", std::process::id()));
    }
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

    /// The subagent model set for a backend (by id), if any: `(model, effort)`.
    pub fn subagent_for(&self, backend: &str) -> (Option<String>, Option<String>) {
        match self.subagents.get(backend).filter(|s| !s.model.trim().is_empty()) {
            Some(s) => (Some(s.model.trim().to_string()), s.effort.clone().filter(|e| !e.is_empty() && e != "default")),
            None => (None, None),
        }
    }

    /// Sets (or with `model` None/empty, clears) a backend's subagent model.
    pub fn set_subagent(&mut self, backend: &str, model: Option<String>, effort: Option<String>) {
        match model.filter(|m| !m.trim().is_empty()) {
            Some(model) => {
                let effort = effort.filter(|e| !e.is_empty() && e != "default");
                self.subagents.insert(backend.to_string(), SubagentModel { model, effort });
            }
            None => {
                self.subagents.remove(backend);
            }
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
