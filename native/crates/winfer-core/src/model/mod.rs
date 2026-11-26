//! The app's state (port of `src/stores/workspaces.ts`). The saved JSON matches the Tauri app's
//! snapshot, so `data_export`/`data_import` move a setup between the two.

pub use crate::layout;
pub mod progress;

use layout::{GRID, LayoutNode, PRESETS, build_preset, insert_dwindle, leaves, remove_leaf};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use winfer_backend::providers::Provider;

pub const DEFAULT_BROWSER_WIDTH: f64 = 420.0;
pub const DEFAULT_SIDEBAR_WIDTH: f64 = 256.0;
pub const DEFAULT_TERMINAL_FONT_SIZE: f64 = 12.5;
pub const MIN_TERMINAL_FONT_SIZE: f64 = 8.0;
pub const MAX_TERMINAL_FONT_SIZE: f64 = 28.0;
const MAX_RECENT_SESSIONS: usize = 200;

pub const DEFAULT_SYSTEM_PROMPT: &str = "Use your tools (MCP, browser, skills, subagents, shell) wherever they're faster or more reliable than by hand, and to verify your work. Prefer winfer's integrated tools over external ones. Open and test web pages only in winfer's browser (its browser tools or open_url), never via `open`/`xdg-open`/`start` or other browser automation. Keep tool output small, it fills your context: search with line numbers before reading, read only the lines you need (your file-read tool with an offset and limit rather than printing whole files with cat or sed), don't re-read a file you haven't changed, and trim long command output to the part you need. Change existing files with your edit tool (only the changed lines), not by rewriting whole files with Write or shell heredocs. Run dev servers, watchers, loops and other long-running commands with winfer's bg tool, not by backgrounding them in your shell (&, nohup, run_in_background, a kept-open terminal session): wait on them with bg read until and stop them when done.";

/// MCP tools agents rarely use, off by default (each one costs context in every request).
pub const OPTIONAL_TOOLS: [&str; 6] = ["browser_drag", "browser_drop", "browser_emulate_media", "browser_navigate_back", "kb_explain", "kb_save"];

/// The plugin that provides an optional tool; such tools are shown only while it's installed and enabled.
pub fn optional_tool_plugin(name: &str) -> Option<&'static str> {
    match name {
        "kb_explain" | "kb_save" => Some("knowledge-base"),
        _ => None,
    }
}

/// An agent and whether its CLI is installed.
#[derive(Clone, Debug)]
pub struct AgentProvider {
    pub info: Provider,
    pub installed: bool,
}

impl std::ops::Deref for AgentProvider {
    type Target = Provider;
    fn deref(&self) -> &Provider {
        &self.info
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AgentStatus {
    #[default]
    Idle,
    Working,
    NeedsInput,
    Done,
    Error,
}

impl AgentStatus {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "idle" => Self::Idle,
            "working" => Self::Working,
            "needs-input" => Self::NeedsInput,
            "done" => Self::Done,
            "error" => Self::Error,
            _ => return None,
        })
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WorktreeInfo {
    pub repo: String,
    pub path: String,
    pub branch: String,
    pub base: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Pane {
    pub id: String,
    pub provider: String,
    pub title: String,
    pub cwd: String,
    #[serde(default)]
    pub worktree: Option<WorktreeInfo>,
    #[serde(default)]
    pub race_id: Option<String>,
    #[serde(default)]
    pub plain: bool,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub effort: Option<String>,
    #[serde(default)]
    pub agent_title: Option<String>,
    #[serde(default)]
    pub custom_title: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub archived: bool,
    #[serde(default)]
    pub account_id: Option<String>,
    /// Another machine's pane shown here (Winfer Connect); its process runs there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote: Option<RemoteRef>,
    #[serde(skip)]
    pub workspace_id: String,
    #[serde(skip)]
    pub status: AgentStatus,
    #[serde(skip)]
    pub wants_worktree: bool,
    #[serde(skip)]
    pub resume: bool,
    #[serde(skip)]
    pub unseen: bool,
    #[serde(skip)]
    pub dev_url: Option<String>,
    #[serde(skip)]
    pub prompt: Option<String>,
    /// Images pasted on the welcome screen; a winfer code pane sends them with `prompt` as its first message.
    #[serde(skip)]
    pub attachments: Vec<crate::code::state::Attachment>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Workspace {
    pub id: String,
    pub name: String,
    pub root_dir: String,
    pub panes: Vec<Pane>,
    #[serde(default)]
    pub layout: Option<LayoutNode>,
    #[serde(default)]
    pub zoomed_pane_id: Option<String>,
    #[serde(default)]
    pub preset_index: usize,
    #[serde(default = "yes")]
    pub use_worktrees: bool,
    /// Set on a workspace that holds tiles of a paired machine's workspace: listed under that
    /// machine in the sidebar, not with the local workspaces. Kept in the ui state
    /// (`remoteWorkspaces`): the workspaces table has no column for it.
    #[serde(skip)]
    pub remote: Option<RemoteWorkspace>,
}

/// A paired machine's workspace that a local workspace shows tiles of.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteWorkspace {
    pub peer: String,
    /// The workspace's id on that machine.
    pub workspace: String,
}

fn yes() -> bool {
    true
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Overlay {
    #[default]
    None,
    Race,
    AbTest,
    Broadcast,
    Recipes,
    Model,
    Review,
    Progress,
    History,
    Branch,
    NewPane,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum View {
    Workspace,
    Overview,
    Archive,
    Page(String),
}

impl View {
    fn parse(s: &str) -> Self {
        match s {
            "overview" => Self::Overview,
            "archive" => Self::Archive,
            s => s.strip_prefix("page:").map_or(Self::Workspace, |p| Self::Page(p.to_string())),
        }
    }

    fn name(&self) -> String {
        match self {
            Self::Workspace => "workspace".into(),
            Self::Overview => "overview".into(),
            Self::Archive => "archive".into(),
            Self::Page(p) => format!("page:{p}"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SidePanel {
    Browser,
    Device,
    Terminal,
    Viewer,
}

impl SidePanel {
    fn parse(s: &str) -> Self {
        match s {
            "device" => Self::Device,
            "terminal" => Self::Terminal,
            "viewer" => Self::Viewer,
            _ => Self::Browser,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Browser => "browser",
            Self::Device => "device",
            Self::Terminal => "terminal",
            Self::Viewer => "viewer",
        }
    }
}

/// Where a remote tile's pane lives: the paired machine and its pane id there.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteRef {
    pub peer: String,
    pub pane: String,
    /// The machine's name when the tile was opened (shown while it's offline).
    pub machine: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct PaneTask {
    pub task: Option<String>,
    pub title: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct OverviewLayout {
    pub layout: Option<LayoutNode>,
    pub zoomed_pane_id: Option<String>,
    pub preset_index: usize,
}

/// Files open in the side panel's viewer. `activeByProject` remembers each project's shown file.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ViewerDocs {
    #[serde(default)]
    pub docs: Vec<String>,
    #[serde(default)]
    pub active: Option<String>,
    #[serde(default)]
    pub active_by_project: HashMap<String, String>,
}

/// Saved settings. Settings this build doesn't use yet stay in `rest`, so saving never drops them.
#[derive(Clone, Debug, Default)]
pub struct AppState {
    pub hydrated: bool,
    pub providers: Vec<AgentProvider>,
    pub workspaces: Vec<Workspace>,
    pub active_workspace_id: String,
    pub focused_pane_id: Option<String>,
    pub sidebar_open: bool,
    pub expanded_workspaces: Vec<String>,
    pub browser_open: bool,
    pub palette_query: Option<String>,
    pub cheat_sheet_open: bool,
    pub diff_pane_id: Option<String>,
    pub settings_open: bool,
    pub settings_page: String,
    pub overlay: Overlay,
    pub new_pane_provider: Option<String>,
    pub agent_models: HashMap<String, String>,
    /// A private note per pane, kept for the pane's life.
    pub pane_notes: HashMap<String, String>,
    pub auto_approve: Vec<String>,
    pub system_prompt: Option<String>,
    pub enabled_tools: Vec<String>,
    pub default_provider: Option<String>,
    pub editor: Option<String>,
    pub shortcuts: HashMap<String, String>,
    pub browser_width: f64,
    pub sidebar_width: f64,
    pub side_panel: Option<SidePanel>,
    pub viewer: ViewerDocs,
    pub theme: String,
    /// "system": the theme follows the system's light/dark setting, using `theme_picks`.
    pub theme_mode: String,
    /// The theme picked for "dark" and "light".
    pub theme_picks: HashMap<String, String>,
    pub view: Option<View>,
    pub terminal_font_size: f64,
    pub overview: OverviewLayout,
    pub show_pane_cost: bool,
    pub page_tab_icons_only: bool,
    pub search_pane_id: Option<String>,
    pub recent_sessions: Vec<(String, String)>,
    pub pane_tasks: HashMap<String, PaneTask>,
    pub session_titles: HashMap<String, String>,
    /// Panes still starting: "worktree" or "process".
    pub starting: HashMap<String, &'static str>,
    /// The sidebar's "Latest" feed, newest first.
    pub activity: Vec<ActivityEvent>,
    pub progress: HashMap<String, progress::Progress>,
    /// Each pane's last agent reply, from its "done" hook (Claude Code, Codex).
    pub replies: HashMap<String, String>,
    /// Each pane's latest status message (the hook's or notify_user's).
    pub status_messages: HashMap<String, String>,
    /// Race and A/B agents' working time, in memory only.
    pub run_times: HashMap<String, progress::RunTime>,
    pub rest: serde_json::Map<String, Value>,
}

/// `ui` fields the typed state above owns; everything else round-trips through `rest`.
const OWNED_UI: [&str; 27] = [
    "viewer", "remoteWorkspaces",
    "activeWorkspaceId", "sidebarOpen", "expandedWorkspaces", "browserOpen", "agentModels", "autoApprove",
    "systemPrompt", "enabledTools", "defaultProvider", "editor", "shortcuts", "browserWidth", "sidebarWidth", "sidePanel",
    "theme", "view", "terminalFontSize", "overviewLayout", "showPaneCost", "pageTabIconsOnly",
    "recentSessions", "paneTasks", "sessionTitles", "activeView", "remoteTiles",
];

#[derive(Clone, Debug)]
pub struct ActivityEvent {
    pub id: u64,
    pub at: std::time::SystemTime,
    pub workspace_id: String,
    pub provider: Option<String>,
    pub text: String,
}

const MAX_ACTIVITY: usize = 300;

pub fn new_id(prefix: &str) -> String {
    let n: u32 = rand::random();
    format!("{prefix}-{n:08x}")
}

fn new_workspace(name: &str, root: &str) -> Workspace {
    Workspace { id: new_id("ws"), name: name.into(), root_dir: root.into(), use_worktrees: true, ..Default::default() }
}

fn first_pane(ws: Option<&Workspace>) -> Option<String> {
    ws?.layout.as_ref().map(|l| leaves(l)[0].clone())
}

fn home() -> String {
    dirs::home_dir().map(|p| p.display().to_string()).unwrap_or_default()
}

/// Key of `recent_sessions`: the agent and the folder it ran in ("~" expanded).
pub fn session_key(provider: &str, path: &str) -> String {
    let home = home();
    let full = if !home.is_empty() && (path == "~" || path.starts_with("~/")) { format!("{home}{}", &path[1..]) } else { path.to_string() };
    let trimmed = if full.len() > 1 { full.trim_end_matches('/') } else { &full };
    format!("{provider}:{}", if trimmed.is_empty() { "/" } else { trimmed })
}

/// Cleans a terminal title set by an agent: drops spinner glyphs and titles that only repeat the
/// CLI's name or a program path.
pub fn clean_agent_title(raw: &str, names: &[&str]) -> Option<String> {
    let start = raw.char_indices().find(|(_, c)| c.is_alphanumeric()).map(|(i, _)| i)?;
    let title: String = raw[start..].split_whitespace().collect::<Vec<_>>().join(" ").chars().take(80).collect();
    if title.is_empty() || names.iter().any(|n| n.eq_ignore_ascii_case(&title)) {
        return None;
    }
    let trimmed = raw.trim();
    let after_prefix = trimmed.split_once(": ").map_or(trimmed, |(_, r)| r);
    let is_path = !after_prefix.contains(char::is_whitespace)
        && (after_prefix.starts_with('/') || after_prefix.starts_with('\\') || after_prefix.as_bytes().get(1) == Some(&b':'));
    (!is_path && !trimmed.to_lowercase().ends_with(".exe")).then_some(title)
}

#[derive(Default, Clone)]
pub struct AddPane {
    pub split_from: Option<String>,
    pub worktree: Option<bool>,
    pub prompt: Option<String>,
    pub attachments: Vec<crate::code::state::Attachment>,
    pub race_id: Option<String>,
    pub plain: bool,
    pub custom_title: Option<String>,
    pub workspace_id: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub existing_worktree: Option<WorktreeInfo>,
    pub cwd: Option<String>,
    pub resume_session: Option<String>,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            sidebar_open: true,
            browser_open: true,
            settings_page: "general".into(),
            browser_width: DEFAULT_BROWSER_WIDTH,
            sidebar_width: DEFAULT_SIDEBAR_WIDTH,
            side_panel: Some(SidePanel::Browser),
            theme: "dark".into(),
            theme_mode: "manual".into(),
            view: Some(View::Workspace),
            terminal_font_size: DEFAULT_TERMINAL_FONT_SIZE,
            overview: OverviewLayout { preset_index: GRID, ..Default::default() },
            ..Default::default()
        }
    }

    pub fn view(&self) -> View {
        self.view.clone().unwrap_or(View::Workspace)
    }

    pub fn side_panel(&self) -> SidePanel {
        self.side_panel.unwrap_or(SidePanel::Browser)
    }

    pub fn hydrate(&mut self, snapshot: Value) {
        let mut workspaces: Vec<Workspace> = snapshot["workspaces"]
            .as_array()
            .map(|a| a.iter().filter_map(|w| serde_json::from_value(w.clone()).ok()).collect())
            .unwrap_or_default();
        self.hydrated = true;
        if workspaces.is_empty() {
            let home = new_workspace("home", "~");
            self.active_workspace_id = home.id.clone();
            self.expanded_workspaces = vec![home.id.clone()];
            self.workspaces = vec![home];
            return;
        }
        // Panes resume their own session by id. Without one, only one restored pane per provider
        // and folder resumes the folder's last session.
        let mut resumed = std::collections::HashSet::new();
        for w in &mut workspaces {
            for p in &mut w.panes {
                let key = format!("{}:{}", p.provider, p.cwd);
                p.resume = p.session_id.is_some() || !resumed.contains(&key);
                if p.session_id.is_none() {
                    resumed.insert(key);
                }
                p.workspace_id = w.id.clone();
            }
        }
        let ui = snapshot["ui"].as_object().cloned().unwrap_or_default();
        let str_of = |k: &str| ui.get(k).and_then(Value::as_str).map(str::to_string);
        let bool_of = |k: &str, d: bool| ui.get(k).and_then(Value::as_bool).unwrap_or(d);
        let num_of = |k: &str, d: f64| ui.get(k).and_then(Value::as_f64).unwrap_or(d);
        let list_of = |k: &str| -> Vec<String> { ui.get(k).and_then(|v| serde_json::from_value(v.clone()).ok()).unwrap_or_default() };
        let map_of = |k: &str| -> HashMap<String, String> { ui.get(k).and_then(|v| serde_json::from_value(v.clone()).ok()).unwrap_or_default() };

        let active = str_of("activeWorkspaceId")
            .and_then(|id| workspaces.iter().position(|w| w.id == id))
            .unwrap_or(0);
        self.active_workspace_id = workspaces[active].id.clone();
        self.focused_pane_id = first_pane(Some(&workspaces[active]));
        self.sidebar_open = bool_of("sidebarOpen", true);
        self.expanded_workspaces = match ui.get("expandedWorkspaces") {
            Some(_) => list_of("expandedWorkspaces").into_iter().filter(|id| workspaces.iter().any(|w| &w.id == id)).collect(),
            None => vec![self.active_workspace_id.clone()],
        };
        self.browser_open = bool_of("browserOpen", true);
        self.agent_models = map_of("agentModels");
        self.pane_notes = map_of("paneNotes");
        self.auto_approve = list_of("autoApprove");
        self.system_prompt = str_of("systemPrompt");
        self.enabled_tools = list_of("enabledTools");
        self.default_provider = str_of("defaultProvider");
        self.editor = str_of("editor");
        self.shortcuts = map_of("shortcuts");
        self.browser_width = num_of("browserWidth", DEFAULT_BROWSER_WIDTH);
        self.sidebar_width = num_of("sidebarWidth", DEFAULT_SIDEBAR_WIDTH);
        self.side_panel = Some(SidePanel::parse(&str_of("sidePanel").unwrap_or_default()));
        self.viewer = ui.get("viewer").and_then(|v| serde_json::from_value(v.clone()).ok()).unwrap_or_default();
        self.theme = str_of("theme").unwrap_or_else(|| "dark".into());
        self.theme_mode = if str_of("themeMode").as_deref() == Some("system") { "system".into() } else { "manual".into() };
        self.theme_picks = map_of("themePicks");
        self.view = Some(View::parse(&str_of("view").unwrap_or_default()));
        self.terminal_font_size = num_of("terminalFontSize", DEFAULT_TERMINAL_FONT_SIZE);
        self.overview.layout = ui.get("overviewLayout").and_then(|v| serde_json::from_value(v.clone()).ok());
        self.show_pane_cost = bool_of("showPaneCost", false);
        self.page_tab_icons_only = bool_of("pageTabIconsOnly", false);
        // serde_json keeps object order here only if the map preserves it; newest-last isn't
        // guaranteed after a round trip through the backend, which only matters for pruning.
        self.recent_sessions = ui
            .get("recentSessions")
            .and_then(Value::as_object)
            .map(|m| m.iter().filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_string()))).collect())
            .unwrap_or_default();
        self.pane_tasks = ui.get("paneTasks").and_then(|v| serde_json::from_value(v.clone()).ok()).unwrap_or_default();
        self.session_titles = map_of("sessionTitles");
        // Tiles of another machine's panes: the panes table has no column for that, and without
        // it a tile would come back as an agent of this machine's own.
        let remote: HashMap<String, RemoteRef> = ui.get("remoteTiles").and_then(|v| serde_json::from_value(v.clone()).ok()).unwrap_or_default();
        for p in workspaces.iter_mut().flat_map(|w| w.panes.iter_mut()) {
            if let Some(r) = remote.get(&p.id) {
                p.remote = Some(r.clone());
            }
        }
        let remote: HashMap<String, RemoteWorkspace> = ui.get("remoteWorkspaces").and_then(|v| serde_json::from_value(v.clone()).ok()).unwrap_or_default();
        for w in &mut workspaces {
            if let Some(r) = remote.get(&w.id) {
                w.remote = Some(r.clone());
            }
        }
        self.rest = ui.into_iter().filter(|(k, _)| !OWNED_UI.contains(&k.as_str())).collect();
        self.workspaces = workspaces;
        self.sync_overview();
        self.restore_tasks();
    }

    /// The `ui` settings of a fresh install. (`hydrate` stops early without a workspace, so it
    /// gets an empty one.)
    pub fn default_ui() -> Value {
        let mut state = AppState::default();
        state.hydrate(serde_json::json!({ "workspaces": [Workspace { id: "w".into(), ..Default::default() }], "ui": {} }));
        state.snapshot()["ui"].clone()
    }

    pub fn snapshot(&self) -> Value {
        let open: Vec<&str> = self.all_panes().map(|p| p.id.as_str()).collect();
        let tasks: HashMap<_, _> = self.pane_tasks.iter().filter(|(id, _)| open.contains(&id.as_str())).collect();
        let recent: serde_json::Map<String, Value> = self.recent_sessions.iter().map(|(k, v)| (k.clone(), Value::String(v.clone()))).collect();
        let mut ui = self.rest.clone();
        let owned = serde_json::json!({
            "activeWorkspaceId": self.active_workspace_id, "sidebarOpen": self.sidebar_open,
            "expandedWorkspaces": self.expanded_workspaces, "browserOpen": self.browser_open,
            "agentModels": self.agent_models, "paneNotes": self.pane_notes.iter().filter(|(id, _)| open.contains(&id.as_str())).collect::<HashMap<_, _>>(), "autoApprove": self.auto_approve,
            "systemPrompt": self.system_prompt, "enabledTools": self.enabled_tools,
            "defaultProvider": self.default_provider, "editor": self.editor, "shortcuts": self.shortcuts,
            "browserWidth": self.browser_width, "sidebarWidth": self.sidebar_width,
            "sidePanel": self.side_panel().name(), "viewer": self.viewer, "theme": self.theme, "themeMode": self.theme_mode, "themePicks": self.theme_picks, "view": self.view().name(),
            "terminalFontSize": self.terminal_font_size, "overviewLayout": self.overview.layout,
            "showPaneCost": self.show_pane_cost,
            "pageTabIconsOnly": self.page_tab_icons_only, "recentSessions": recent, "paneTasks": tasks,
            "sessionTitles": self.session_titles,
            "remoteTiles": self.all_panes().filter_map(|p| Some((p.id.clone(), p.remote.clone()?))).collect::<HashMap<_, _>>(),
            "remoteWorkspaces": self.workspaces.iter().filter_map(|w| Some((w.id.clone(), w.remote.clone()?))).collect::<HashMap<_, _>>(),
        });
        ui.extend(owned.as_object().unwrap().clone());
        serde_json::json!({ "workspaces": self.workspaces, "ui": ui })
    }

    pub fn all_panes(&self) -> impl Iterator<Item = &Pane> {
        self.workspaces.iter().flat_map(|w| &w.panes)
    }

    pub fn pane(&self, id: &str) -> Option<&Pane> {
        self.all_panes().find(|p| p.id == id)
    }

    pub fn pane_mut(&mut self, id: &str) -> Option<&mut Pane> {
        self.workspaces.iter_mut().flat_map(|w| &mut w.panes).find(|p| p.id == id)
    }

    pub fn active(&self) -> Option<&Workspace> {
        self.workspaces.iter().find(|w| w.id == self.active_workspace_id)
    }

    pub fn active_mut(&mut self) -> Option<&mut Workspace> {
        let id = self.active_workspace_id.clone();
        self.workspaces.iter_mut().find(|w| w.id == id)
    }

    pub fn provider_name(&self, id: &str) -> String {
        self.providers.iter().find(|p| p.id == id).map_or_else(
            || if id == "winfer" { "WinferCode".to_string() } else { id.to_string() },
            |p| p.name.clone(),
        )
    }

    /// Stores the discovered providers with `winfer` (WinferCode) always first and branded as `WinferCode`.
    pub fn set_providers(&mut self, mut providers: Vec<AgentProvider>) {
        if let Some(pos) = providers.iter().position(|p| p.id == "winfer") {
            providers[pos].info.name = "WinferCode".to_string();
            if pos > 0 {
                let winfer = providers.remove(pos);
                providers.insert(0, winfer);
            }
        }
        self.providers = providers;
    }

    pub fn has_hooks(&self, pane: &Pane) -> bool {
        self.providers.iter().find(|p| p.id == pane.provider).is_some_and(|p| p.status_hooks)
    }

    pub fn system_prompt_of(&self) -> String {
        self.system_prompt.clone().unwrap_or_else(|| DEFAULT_SYSTEM_PROMPT.to_string())
    }

    pub fn hidden_tools(&self) -> Vec<String> {
        OPTIONAL_TOOLS.iter().filter(|t| !self.enabled_tools.iter().any(|e| e == *t)).map(|t| t.to_string()).collect()
    }

    /// The agent new panes open with: the chosen default if installed, else WinferCode, else Claude Code, else the shell.
    pub fn set_pane_note(&mut self, pane: &str, note: &str) {
        if note.is_empty() {
            self.pane_notes.remove(pane);
        } else {
            self.pane_notes.insert(pane.to_string(), note.to_string());
        }
    }

    pub fn default_provider_id(&self) -> String {
        let installed = |id: &str| self.providers.iter().any(|p| p.id == id && p.installed);
        match &self.default_provider {
            Some(p) if installed(p) => p.clone(),
            _ if installed("winfer") => "winfer".into(),
            _ if installed("claude") => "claude".into(),
            _ => "shell".into(),
        }
    }

    /// What to call a pane in lists.
    pub fn pane_label(&self, pane: &Pane) -> String {
        let task = self.pane_tasks.get(&pane.id);
        let session = pane.session_id.as_ref().and_then(|s| self.session_titles.get(s));
        let title = task.and_then(|t| t.title.as_ref());
        pane.custom_title
            .clone()
            .or_else(|| session.cloned())
            .or_else(|| title.cloned())
            .or_else(|| pane.agent_title.clone())
            .or_else(|| {
                let fallback = task.and_then(|t| t.task.as_ref()).or(pane.prompt.as_ref())?;
                Some(fallback.lines().next().unwrap_or("").chars().take(80).collect())
            })
            .unwrap_or_else(|| self.provider_name(&pane.provider))
    }

    pub fn log_activity(&mut self, workspace: &str, text: String, provider: Option<String>) {
        let id = self.activity.first().map_or(1, |e| e.id + 1);
        self.activity.insert(0, ActivityEvent { id, at: std::time::SystemTime::now(), workspace_id: workspace.to_string(), provider, text });
        self.activity.truncate(MAX_ACTIVITY);
    }

    pub fn set_active_workspace(&mut self, id: &str) {
        self.active_workspace_id = id.to_string();
        self.focused_pane_id = first_pane(self.active());
        if !self.expanded_workspaces.iter().any(|w| w == id) {
            self.expanded_workspaces.push(id.to_string());
        }
    }

    pub fn create_workspace(&mut self, name: &str, root: &str) -> String {
        let ws = new_workspace(name, root);
        let id = ws.id.clone();
        self.workspaces.push(ws);
        self.active_workspace_id = id.clone();
        self.focused_pane_id = None;
        self.expanded_workspaces.push(id.clone());
        id
    }

    pub fn remove_workspace(&mut self, id: &str) {
        self.workspaces.retain(|w| w.id != id);
        self.expanded_workspaces.retain(|w| w != id);
        if self.workspaces.iter().all(|w| w.remote.is_some()) {
            self.workspaces.insert(0, new_workspace("home", "~"));
        }
        if self.active_workspace_id == id {
            let next = self.workspaces.iter().find(|w| w.remote.is_none()).map(|w| w.id.clone()).unwrap_or_default();
            self.active_workspace_id = next;
            self.focused_pane_id = first_pane(self.active());
        }
        self.sync_overview();
    }

    /// The workspace for the folder `root` (the one already there, if any), made without
    /// switching to it: another machine or an agent asked, not the user here.
    pub fn workspace_for_folder(&mut self, root: &str, name: &str) -> String {
        if let Some(w) = self.workspaces.iter().find(|w| w.remote.is_none() && w.root_dir == root) {
            return w.id.clone();
        }
        let ws = new_workspace(name, root);
        let id = ws.id.clone();
        self.workspaces.push(ws);
        self.sync_overview();
        id
    }

    /// This machine's workspace with that id or name (any case).
    pub fn workspace_named(&self, key: &str) -> Option<&Workspace> {
        let key = key.trim();
        self.workspaces.iter().filter(|w| w.remote.is_none()).find(|w| w.id == key).or_else(|| self.workspaces.iter().filter(|w| w.remote.is_none()).find(|w| w.name.eq_ignore_ascii_case(key)))
    }

    /// The local workspace showing tiles of `peer`'s workspace `workspace`, made (and switched
    /// to) when there's none yet; `name` is what it's called.
    pub fn remote_workspace(&mut self, peer: &str, workspace: &str, name: &str) -> String {
        let want = RemoteWorkspace { peer: peer.to_string(), workspace: workspace.to_string() };
        if let Some(w) = self.workspaces.iter_mut().find(|w| w.remote.as_ref() == Some(&want)) {
            w.name = name.to_string();
            let id = w.id.clone();
            self.set_active_workspace(&id);
            return id;
        }
        let mut ws = new_workspace(name, "");
        ws.use_worktrees = false;
        ws.remote = Some(want);
        let id = ws.id.clone();
        self.workspaces.push(ws);
        self.active_workspace_id = id.clone();
        self.focused_pane_id = None;
        id
    }

    /// A machine's workspace whose last tile closed goes away.
    fn drop_empty_remote_workspaces(&mut self) {
        let empty: Vec<String> = self.workspaces.iter().filter(|w| w.remote.is_some() && w.panes.iter().all(|p| p.archived)).map(|w| w.id.clone()).collect();
        for id in empty {
            self.remove_workspace(&id);
        }
    }

    pub fn toggle_workspace_expanded(&mut self, id: &str) {
        match self.expanded_workspaces.iter().position(|w| w == id) {
            Some(i) => {
                self.expanded_workspaces.remove(i);
            }
            None => self.expanded_workspaces.push(id.to_string()),
        }
    }

    pub fn focus_pane(&mut self, id: &str) {
        if self.overview.zoomed_pane_id.as_deref().is_some_and(|z| z != id) {
            self.overview.zoomed_pane_id = Some(id.to_string());
        }
        for w in &mut self.workspaces {
            if w.zoomed_pane_id.as_deref().is_some_and(|z| z != id) && w.panes.iter().any(|p| p.id == id) {
                w.zoomed_pane_id = Some(id.to_string());
            }
        }
        if let Some(ws_id) = self.workspaces.iter().find(|w| w.panes.iter().any(|p| p.id == id)).map(|w| w.id.clone())
            && self.active_workspace_id != ws_id
        {
            self.active_workspace_id = ws_id.clone();
            if !self.expanded_workspaces.iter().any(|w| w == &ws_id) {
                self.expanded_workspaces.push(ws_id);
            }
        }
        self.focused_pane_id = Some(id.to_string());
        if let Some(p) = self.pane_mut(id) {
            p.unseen = false;
        }
    }

    pub fn add_pane(&mut self, provider: &str, opts: AddPane) -> String {
        if let Some(h) = crate::hooks::get() {
            h.pane_opened(provider, opts.effort.as_deref());
        }
        let workspace_id = opts.workspace_id.clone().unwrap_or_else(|| self.active_workspace_id.clone());
        let background = workspace_id != self.active_workspace_id;
        let id = new_id("p");
        let model = opts.model.clone().or_else(|| self.agent_models.get(provider).cloned());
        let focused = self.focused_pane_id.clone();
        if !background {
            self.focused_pane_id = Some(id.clone());
            if self.view() == View::Archive {
                self.view = Some(View::Workspace);
            }
        }
        if let Some(w) = self.workspaces.iter_mut().find(|w| w.id == workspace_id) {
            let wants = opts.existing_worktree.is_none() && provider != "shell" && opts.worktree.unwrap_or(w.use_worktrees);
            let cwd = opts.existing_worktree.as_ref().map(|t| t.path.clone()).or(opts.cwd.clone()).unwrap_or_else(|| w.root_dir.clone());
            w.panes.push(Pane {
                id: id.clone(),
                workspace_id: w.id.clone(),
                provider: provider.to_string(),
                title: w.name.clone(),
                cwd,
                worktree: opts.existing_worktree,
                wants_worktree: wants,
                resume: opts.resume_session.is_some(),
                session_id: opts.resume_session,
                custom_title: opts.custom_title,
                race_id: opts.race_id,
                plain: opts.plain,
                prompt: opts.prompt,
                attachments: opts.attachments,
                model,
                effort: opts.effort,
                ..Default::default()
            });
            let target = opts.split_from.or(if background { None } else { focused });
            w.layout = Some(insert_dwindle(w.layout.as_ref(), &id, target.as_deref()));
            w.zoomed_pane_id = None;
        }
        self.sync_overview();
        id
    }

    fn after_leaving_tiles(&mut self, id: &str) {
        if self.focused_pane_id.as_deref() == Some(id) {
            self.focused_pane_id = self.active().and_then(|w| w.layout.as_ref()).map(|l| leaves(l).last().unwrap().clone());
        }
    }

    pub fn remove_pane(&mut self, id: &str) {
        for w in &mut self.workspaces {
            if w.panes.iter().any(|p| p.id == id) {
                w.panes.retain(|p| p.id != id);
                w.layout = remove_leaf(w.layout.as_ref(), id);
                if w.zoomed_pane_id.as_deref() == Some(id) {
                    w.zoomed_pane_id = None;
                }
            }
        }
        self.after_leaving_tiles(id);
        self.drop_empty_remote_workspaces();
        self.sync_overview();
    }

    pub fn archive_pane(&mut self, id: &str) {
        for w in &mut self.workspaces {
            if let Some(p) = w.panes.iter_mut().find(|p| p.id == id && !p.archived) {
                p.archived = true;
                w.layout = remove_leaf(w.layout.as_ref(), id);
                if w.zoomed_pane_id.as_deref() == Some(id) {
                    w.zoomed_pane_id = None;
                }
            }
        }
        if self.view() != View::Archive {
            self.after_leaving_tiles(id);
        }
        self.sync_overview();
    }

    pub fn unarchive_pane(&mut self, id: &str) {
        let active = self.active_workspace_id.clone();
        let in_tiles = self.view() == View::Workspace;
        let focused = self.focused_pane_id.clone();
        for w in &mut self.workspaces {
            if let Some(p) = w.panes.iter_mut().find(|p| p.id == id && p.archived) {
                p.archived = false;
                let target = if w.id == active && in_tiles { focused.as_deref() } else { None };
                w.layout = Some(insert_dwindle(w.layout.as_ref(), id, target));
                w.zoomed_pane_id = None;
            }
        }
        self.sync_overview();
    }

    pub fn rename_pane(&mut self, id: &str, name: &str) {
        let clean: String = name.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(80).collect();
        if let Some(p) = self.pane_mut(id) {
            p.custom_title = (!clean.is_empty()).then_some(clean);
        }
    }

    pub fn set_pane_status(&mut self, id: &str, status: AgentStatus, window_focused: bool) {
        let visible = self.focused_pane_id.as_deref() == Some(id) && window_focused;
        let was = self.pane(id).filter(|p| p.race_id.is_some() && p.status != status).map(|p| p.status);
        if let Some(was) = was {
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
            match progress::next_run_time(self.run_times.get(id).copied(), was, status, now) {
                Some(t) => self.run_times.insert(id.to_string(), t),
                None => self.run_times.remove(id),
            };
        }
        if let Some(p) = self.pane_mut(id) {
            p.status = status;
            p.unseen = !visible && matches!(status, AgentStatus::NeedsInput | AgentStatus::Done | AgentStatus::Error);
        }
    }

    pub fn set_pane_session(&mut self, id: &str, session: &str) {
        let Some(pane) = self.pane(id).filter(|p| p.provider != "shell") else { return };
        let key = session_key(&pane.provider, pane.worktree.as_ref().map_or(&pane.cwd, |t| &t.path));
        if let Some(p) = self.pane_mut(id) {
            p.session_id = Some(session.to_string());
        }
        self.recent_sessions.retain(|(k, _)| *k != key);
        self.recent_sessions.push((key, session.to_string()));
        if self.recent_sessions.len() > MAX_RECENT_SESSIONS {
            let drop = self.recent_sessions.len() - MAX_RECENT_SESSIONS;
            self.recent_sessions.drain(..drop);
        }
    }

    pub fn recent_session(&self, provider: &str, path: &str) -> Option<String> {
        let key = session_key(provider, path);
        let id = self.recent_sessions.iter().find(|(k, _)| *k == key)?.1.clone();
        (!self.all_panes().any(|p| p.session_id.as_deref() == Some(&id))).then_some(id)
    }

    pub fn set_view(&mut self, view: View) {
        let focused_archived = self.focused_pane_id.as_deref().and_then(|id| self.pane(id)).is_some_and(|p| p.archived);
        if view != View::Archive && focused_archived {
            self.focused_pane_id = first_pane(self.active());
        }
        self.view = Some(view);
    }

    /// Picks a theme for its light or dark mode. Following the system stays on, and the theme
    /// only changes when it is the one for the system's current mode; otherwise it takes over.
    pub fn pick_theme(&mut self, id: &str, system_dark: bool) {
        let def = crate::theme::meta(id);
        self.theme_picks.insert(def.mode.clone(), def.id.clone());
        if self.theme_mode != "system" || def.mode == if system_dark { "dark" } else { "light" } {
            self.theme = def.id.clone();
        }
    }

    /// The theme to show when following the system: the pick for its mode, else the current
    /// theme or its counterpart.
    pub fn theme_for_system(&self, system_dark: bool) -> String {
        let want = if system_dark { "dark" } else { "light" };
        if let Some(pick) = self.theme_picks.get(want).filter(|p| crate::theme::meta(p).mode == want) {
            return pick.clone();
        }
        let current = crate::theme::meta(&self.theme);
        if current.mode == want { current.id.clone() } else { current.pair.clone() }
    }

    pub fn set_terminal_font_size(&mut self, size: f64) {
        self.terminal_font_size = (size.clamp(MIN_TERMINAL_FONT_SIZE, MAX_TERMINAL_FONT_SIZE) * 2.0).round() / 2.0;
    }

    /// Every session, grouped by workspace, each workspace in its own tile order.
    pub fn overview_order(&self) -> Vec<String> {
        self.workspaces
            .iter()
            .flat_map(|w| match &w.layout {
                Some(l) => leaves(l),
                None => w.panes.iter().filter(|p| !p.archived).map(|p| p.id.clone()).collect(),
            })
            .collect()
    }

    /// Keeps the overview's tiles in step with the sessions: closed ones leave (others keep their
    /// sizes); when sessions are added it re-tiles with its preset.
    pub fn sync_overview(&mut self) {
        let ids = self.overview_order();
        let current = self.overview.layout.as_ref().map(leaves).unwrap_or_default();
        if ids.len() == current.len() && ids.iter().all(|id| current.contains(id)) {
            return;
        }
        let added = ids.iter().any(|id| !current.contains(id));
        self.overview.layout = if added {
            build_preset(PRESETS[self.overview.preset_index], &ids)
        } else {
            current.iter().filter(|id| !ids.contains(id)).fold(self.overview.layout.clone(), |tree, id| remove_leaf(tree.as_ref(), id))
        };
        if self.overview.zoomed_pane_id.as_ref().is_some_and(|z| !ids.contains(z)) {
            self.overview.zoomed_pane_id = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleans_titles() {
        assert_eq!(clean_agent_title("✳ Fix the bug", &["Claude Code"]).as_deref(), Some("Fix the bug"));
        assert_eq!(clean_agent_title("Claude Code", &["Claude Code"]), None);
        assert_eq!(clean_agent_title("C:\\WINDOWS\\system32\\cmd.exe", &[]), None);
        assert_eq!(clean_agent_title("Administrator: C:\\x\\cmd.exe", &[]), None);
        assert_eq!(clean_agent_title("⠂ ⠂", &[]), None);
    }

    #[test]
    fn other_machines_tiles_get_their_own_workspace() {
        let mut s = AppState::new();
        s.hydrate(serde_json::json!({ "workspaces": [], "ui": null }));
        let home = s.active_workspace_id.clone();
        let ws = s.remote_workspace("peer", "w1", "web · server");
        assert_eq!(s.active_workspace_id, ws);
        let tile = s.add_pane("claude", AddPane { workspace_id: Some(ws.clone()), worktree: Some(false), ..Default::default() });
        assert_eq!(s.remote_workspace("peer", "w1", "web · server"), ws, "the same machine workspace is reused");
        assert_eq!(s.workspaces.len(), 2);
        // Kept across a restart.
        let snap = s.snapshot();
        let mut again = AppState::new();
        again.hydrate(snap);
        assert_eq!(again.workspaces.iter().find(|w| w.id == ws).and_then(|w| w.remote.clone()), Some(RemoteWorkspace { peer: "peer".into(), workspace: "w1".into() }));
        // The last tile closing takes it away, back to a local workspace.
        s.remove_pane(&tile);
        assert_eq!(s.workspaces.len(), 1);
        assert_eq!(s.active_workspace_id, home);
    }

    #[test]
    fn empty_snapshot_makes_home() {
        let mut s = AppState::new();
        s.hydrate(serde_json::json!({ "workspaces": [], "ui": null }));
        assert_eq!(s.workspaces.len(), 1);
        assert_eq!(s.workspaces[0].name, "home");
    }

    #[test]
    fn snapshot_round_trips_and_keeps_unknown_settings() {
        let mut s = AppState::new();
        s.hydrate(serde_json::json!({
            "workspaces": [{ "id": "w", "name": "n", "rootDir": "/r", "presetIndex": 0, "useWorktrees": true, "zoomedPaneId": null,
                "layout": { "kind": "leaf", "paneId": "p" },
                "panes": [{ "id": "p", "provider": "claude", "title": "t", "cwd": "/c", "sessionId": "abc" }] }],
            "ui": { "theme": "dracula", "voice": { "readback": true } },
        }));
        assert_eq!(s.theme, "dracula");
        assert!(s.workspaces[0].panes[0].resume);
        let out = s.snapshot();
        assert_eq!(out["ui"]["voice"]["readback"], true);
        assert_eq!(out["workspaces"][0]["rootDir"], "/r");
        assert_eq!(out["workspaces"][0]["panes"][0]["sessionId"], "abc");
        assert!(out["workspaces"][0]["panes"][0].get("status").is_none());
    }

    #[test]
    fn remote_tiles_survive_the_store() {
        let mut s = AppState::new();
        s.hydrate(serde_json::json!({ "workspaces": [], "ui": null }));
        let id = s.add_pane("claude", AddPane { cwd: Some(String::new()), ..Default::default() });
        s.pane_mut(&id).unwrap().remote = Some(RemoteRef { peer: "srv".into(), pane: "p-1".into(), machine: "box".into() });
        let mut out = s.snapshot();
        // The store's panes table keeps no `remote`.
        for p in out["workspaces"][0]["panes"].as_array_mut().unwrap() {
            p.as_object_mut().unwrap().remove("remote");
        }
        let mut back = AppState::new();
        back.hydrate(out);
        assert_eq!(back.pane(&id).unwrap().remote.as_ref().map(|r| r.pane.as_str()), Some("p-1"));
    }

    #[test]
    fn panes_add_and_remove() {
        let mut s = AppState::new();
        s.hydrate(serde_json::json!({ "workspaces": [], "ui": null }));
        let a = s.add_pane("shell", AddPane::default());
        let b = s.add_pane("shell", AddPane::default());
        assert_eq!(s.focused_pane_id.as_ref(), Some(&b));
        assert_eq!(s.overview.layout.as_ref().map(leaves).unwrap().len(), 2);
        s.remove_pane(&b);
        assert_eq!(s.focused_pane_id.as_ref(), Some(&a));
        s.archive_pane(&a);
        assert!(s.active().unwrap().layout.is_none());
        s.unarchive_pane(&a);
        assert!(s.active().unwrap().layout.is_some());
    }

    #[test]
    fn session_keys_expand_home() {
        let home = home();
        assert_eq!(session_key("claude", "~/x/"), format!("claude:{home}/x"));
    }

    #[test]
    fn focus_pane_switches_to_panes_workspace() {
        let mut s = AppState::new();
        s.hydrate(serde_json::json!({ "workspaces": [], "ui": null }));
        let ws_a = s.active_workspace_id.clone();
        let pane_a = s.add_pane("claude", AddPane::default());
        let ws_b = s.create_workspace("other", "/tmp/other");
        let pane_b = s.add_pane("claude", AddPane::default());
        s.expanded_workspaces.retain(|w| w != &ws_a);
        s.set_view(View::Overview);
        assert_eq!(s.active_workspace_id, ws_b);
        assert_eq!(s.focused_pane_id.as_deref(), Some(pane_b.as_str()));

        s.focus_pane(&pane_a);
        assert_eq!(s.active_workspace_id, ws_a);
        assert_eq!(s.focused_pane_id.as_deref(), Some(pane_a.as_str()));
        assert_eq!(s.view(), View::Overview);
        assert!(s.expanded_workspaces.contains(&ws_a));

        s.focus_pane(&pane_b);
        assert_eq!(s.active_workspace_id, ws_b);
        assert_eq!(s.focused_pane_id.as_deref(), Some(pane_b.as_str()));
    }
}

#[cfg(test)]
mod default_ui_tests {
    #[test]
    fn a_fresh_install_has_the_real_defaults() {
        let ui = super::AppState::default_ui();
        assert_eq!(ui["theme"], "dark");
        assert!(ui["sidebarWidth"].as_f64().is_some_and(|w| w > 0.0));
        assert!(ui["terminalFontSize"].as_f64().is_some_and(|w| w > 0.0));
    }
}
