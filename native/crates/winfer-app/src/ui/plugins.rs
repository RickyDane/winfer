//! Plugin host (port of `features/plugins`): plugins' `main` modules run in a wry webview
//! (assets/plugin-host), which also shows their pages over the main area. This side keeps what
//! they registered (palette tools, pages, agent tools, panel buttons), answers their API calls
//! and runs the Command Bus commands they send.

use super::commands::{Command, Direction, PaneRef, ViewTarget};
use super::review::small_button;
use super::*;
use crate::model::layout::{Preset, SplitDir};
use crate::model::Overlay;
use crate::request::describe_request;
use base64::Engine as _;
use serde::Deserialize;
use serde_json::{Value, json};
use std::borrow::Cow;
use winfer_backend::{browser, plugins as backend_plugins, vault};
use wry::{Rect, WebView, WebViewBuilder, dpi};

const HOST_HTML: &str = include_str!("../../assets/plugin-host/index.html");
const HOST_JS: &str = include_str!("../../assets/plugin-host/host.js");
/// Pages pinned before the user has chosen any.
const DEFAULT_PINNED: usize = 3;

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct PaletteArgs {
    pub title: String,
    #[serde(default)]
    pub args: Value,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct PluginTool {
    pub id: String,
    pub tool_id: String,
    pub plugin: String,
    pub code: bool,
    pub commands: Option<Vec<Value>>,
    #[serde(default)]
    pub palette: Vec<PaletteArgs>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct PageInfo {
    pub key: String,
    pub title: String,
    pub icon: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct AgentToolInfo {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct PanelActionInfo {
    pub key: String,
    pub plugin_id: String,
    pub panel: String,
    pub icon: Option<String>,
    pub label: Option<String>,
    pub active: bool,
}

/// A plugin's `api.confirm` question.
pub(super) struct Approval {
    id: u64,
    plugin: String,
    pane_id: Option<String>,
    title: String,
    message: String,
    ok_label: String,
    warning: bool,
}

/// What a JS-side request answers to once it comes back.
enum Waiting {
    /// An agent's MCP call of a plugin tool.
    Mcp(String),
    /// A palette entry or panel button: failures become toasts.
    Toast(String),
}

#[derive(Default)]
pub(super) struct PluginHost {
    webview: Option<WebView>,
    ready: bool,
    /// `plugins_list` as JSON, for Settings.
    pub infos: Vec<Value>,
    /// Errors raised while activating or running a plugin's code, by plugin id.
    pub runtime_errors: HashMap<String, String>,
    pub tools: Vec<PluginTool>,
    pub pages: Vec<PageInfo>,
    pub agent_tools: Vec<AgentToolInfo>,
    pub panel_actions: Vec<PanelActionInfo>,
    pub approvals: Vec<Approval>,
    approval_serial: u64,
    waiting: HashMap<u64, Waiting>,
    request_serial: u64,
    sent_state: String,
    sent_theme: String,
    previous: HashMap<String, (AgentStatus, String, String)>,
    mounted: Option<String>,
    page_bounds: Bounds<Pixels>,
    shown: Option<bool>,
    apps_open: bool,
}

/// The URL the host's custom protocol serves `path` at (wry maps it per platform).
fn host_url(path: &str) -> String {
    if cfg!(any(target_os = "windows", target_os = "android")) { format!("http://winfer.localhost/{path}") } else { format!("winfer://localhost/{path}") }
}

fn respond(mime: &str, status: u16, body: Vec<u8>) -> wry::http::Response<Cow<'static, [u8]>> {
    wry::http::Response::builder().status(status).header("Content-Type", mime).header("Access-Control-Allow-Origin", "*").body(Cow::Owned(body)).unwrap()
}

fn serve(path: &str) -> wry::http::Response<Cow<'static, [u8]>> {
    match path.trim_start_matches('/') {
        "" | "index.html" => respond("text/html", 200, HOST_HTML.as_bytes().to_vec()),
        "host.js" => respond("text/javascript", 200, HOST_JS.as_bytes().to_vec()),
        other => match other.strip_prefix("plugin/").and_then(|p| p.strip_suffix(".js")) {
            Some(id) => match backend_plugins::plugins_source(id.to_string()) {
                Ok(source) => respond("text/javascript", 200, format!("{source}\n//# sourceURL=winfer-plugin://{id}").into_bytes()),
                Err(e) => respond("text/plain", 404, e.into_bytes()),
            },
            None => respond("text/plain", 404, Vec::new()),
        },
    }
}

/// Replaces `{{name}}` in every string of `value` with the argument (empty if missing).
pub fn fill_template(value: &Value, args: &Value) -> Value {
    static PLACEHOLDER: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| regex::Regex::new(r"\{\{\s*([\w.-]+)\s*\}\}").unwrap());
    match value {
        Value::String(s) => Value::String(
            PLACEHOLDER
                .replace_all(s, |c: &regex::Captures| match &args[&c[1]] {
                    Value::Null => String::new(),
                    Value::String(s) => s.clone(),
                    v @ (Value::Object(_) | Value::Array(_)) => v.to_string(),
                    v => v.to_string(),
                })
                .into_owned(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(|v| fill_template(v, args)).collect()),
        Value::Object(map) => Value::Object(map.iter().map(|(k, v)| (k.clone(), fill_template(v, args))).collect()),
        other => other.clone(),
    }
}

/// Matches a search against a plugin's name, id, description and tags (every word must hit).
pub fn matches_search(query: &str, fields: &[&str]) -> bool {
    let haystack = fields.join(" ").to_lowercase();
    query.to_lowercase().split_whitespace().all(|w| haystack.contains(w))
}

/// Pinned pages that are currently registered, in pin order.
fn pinned_pages<'a>(pages: &'a [PageInfo], pinned: Option<&Vec<String>>) -> Vec<&'a PageInfo> {
    let keys: Vec<String> = pinned.cloned().unwrap_or_else(|| pages.iter().take(DEFAULT_PINNED).map(|p| p.key.clone()).collect());
    keys.iter().filter_map(|k| pages.iter().find(|p| p.key == *k)).collect()
}

fn pane_ref(v: &Value) -> Option<PaneRef> {
    if v.is_null() || v == "focused" {
        return Some(PaneRef::Focused);
    }
    if let Some(id) = v["id"].as_str() {
        return Some(PaneRef::Id(id.to_string()));
    }
    v["index"].as_u64().map(|n| PaneRef::Index(n as usize))
}

fn direction(v: &Value) -> Option<Direction> {
    Some(match v.as_str()? {
        "left" => Direction::Left,
        "right" => Direction::Right,
        "up" => Direction::Up,
        "down" => Direction::Down,
        _ => return None,
    })
}

fn split_dir(v: &Value) -> Option<SplitDir> {
    Some(match v.as_str()? {
        "row" => SplitDir::Row,
        "col" | "column" => SplitDir::Col,
        _ => return None,
    })
}

fn overlay(name: &str) -> Option<Overlay> {
    Some(match name {
        "race" => Overlay::Race,
        "abtest" => Overlay::AbTest,
        "broadcast" => Overlay::Broadcast,
        "recipes" => Overlay::Recipes,
        "model" => Overlay::Model,
        "review" => Overlay::Review,
        "progress" => Overlay::Progress,
        "history" => Overlay::History,
        "branch" => Overlay::Branch,
        "newPane" => Overlay::NewPane,
        _ => return None,
    })
}

fn side_panel(name: &str) -> Option<SidePanel> {
    Some(match name {
        "browser" => SidePanel::Browser,
        "device" => SidePanel::Device,
        "terminal" => SidePanel::Terminal,
        "viewer" => SidePanel::Viewer,
        _ => return None,
    })
}

fn view(name: &str) -> Option<ViewTarget> {
    Some(match name {
        "toggle" => ViewTarget::Toggle,
        "workspace" => ViewTarget::Set(View::Workspace),
        "overview" => ViewTarget::Set(View::Overview),
        "archive" => ViewTarget::Set(View::Archive),
        page => ViewTarget::Set(View::Page(page.strip_prefix("page:")?.to_string())),
    })
}

fn preset(name: &str) -> Option<Option<Preset>> {
    Some(Some(match name {
        "next" => return Some(None),
        "auto" => Preset::Auto,
        "main-stack" => Preset::MainStack,
        "grid" => Preset::Grid,
        "columns" => Preset::Columns,
        "rows" => Preset::Rows,
        _ => return None,
    }))
}

/// A Command Bus command as the Tauri app writes it (`src/commands/bus.ts`).
#[derive(Debug, PartialEq)]
pub enum BusCommand {
    Native(Command),
    Race { counts: Vec<(String, usize)>, prompt: String },
    AbTest { provider: String, prompt: String, model: Option<String> },
    PluginRun { plugin: String, command: String, args: Value },
}

pub fn parse_bus(c: &Value) -> Result<BusCommand, String> {
    use BusCommand::Native as N;
    use Command as C;
    let s = |k: &str| c[k].as_str().map(str::to_string);
    let target = || pane_ref(&c["target"]).ok_or("bad target");
    let kind = c["type"].as_str().unwrap_or_default();
    let bad = || format!("bad {kind} command");
    Ok(match kind {
        "pane.spawn" => N(C::Spawn {
            provider: s("provider").filter(|p| p != "default"),
            count: c["count"].as_u64().unwrap_or(1) as usize,
            prompt: s("prompt"),
            worktree: c["worktree"].as_bool(),
            model: s("model"),
        }),
        "pane.setModel" => N(C::SetModel { target: target()?, model: s("model") }),
        "pane.split" => N(C::Split { dir: split_dir(&c["dir"]).ok_or_else(bad)? }),
        "pane.close" => N(C::Close(target()?)),
        "pane.focus" => N(C::Focus(target()?)),
        "pane.focusDir" => N(C::FocusDir(direction(&c["dir"]).ok_or_else(bad)?)),
        "pane.swapDir" => N(C::SwapDir(direction(&c["dir"]).ok_or_else(bad)?)),
        "pane.resize" => N(C::Resize { target: target()?, axis: split_dir(&c["axis"]).ok_or_else(bad)?, delta: c["delta"].as_f64().ok_or_else(bad)? }),
        "pane.zoom" => N(C::Zoom(target()?)),
        "pane.archive" => N(C::Archive { target: target()?, restore: c["restore"] == true }),
        "pane.interruptAll" => N(C::InterruptAll),
        "pane.closeAll" => N(C::CloseAll),
        "pane.interrupt" => N(C::Interrupt(target()?)),
        "pane.approve" => N(C::Approve(target()?)),
        "pane.broadcast" => N(C::Broadcast {
            text: s("text").ok_or_else(bad)?,
            submit: c["submit"] != false,
            pane_ids: c["paneIds"].as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect()),
        }),
        "race.start" => BusCommand::Race {
            counts: c["counts"].as_object().ok_or_else(bad)?.iter().map(|(k, v)| (k.clone(), v.as_u64().unwrap_or(0) as usize)).collect(),
            prompt: s("prompt").ok_or_else(bad)?,
        },
        "abtest.start" => BusCommand::AbTest { provider: s("provider").ok_or_else(bad)?, prompt: s("prompt").ok_or_else(bad)?, model: s("model").filter(|m| !m.is_empty()) },
        "pane.search" => N(C::Search),
        "pane.sendText" => N(C::SendText { target: target()?, text: s("text").ok_or_else(bad)?, submit: c["submit"] == true }),
        "pane.showChanges" => N(C::ShowChanges(target()?)),
        "pane.mergeWorktree" => N(C::MergeWorktree(target()?)),
        "pane.discardWorktree" => N(C::DiscardWorktree(target()?)),
        "editor.open" => N(C::EditorOpen { target: if c["target"] == "workspace" { None } else { Some(target()?) }, editor: s("editor") }),
        "pane.nextAttention" => N(C::NextAttention),
        "layout.equalize" => N(C::LayoutEqualize),
        "layout.preset" => N(C::LayoutPreset(preset(c["name"].as_str().unwrap_or_default()).ok_or_else(bad)?)),
        "workspace.switch" => match (c["target"]["id"].as_str(), c["target"]["index"].as_u64()) {
            (Some(id), _) => N(C::WorkspaceSwitch(id.to_string())),
            (_, Some(n)) => N(C::WorkspaceSwitchIndex(n as usize)),
            _ => return Err(bad()),
        },
        "workspace.create" => N(C::WorkspaceCreate),
        "workspace.remove" => N(C::WorkspaceRemove(s("id").ok_or_else(bad)?)),
        "workspace.toggleWorktrees" => N(C::ToggleWorktrees),
        "browser.open" => N(C::BrowserOpen { url: s("url").ok_or_else(bad)?, new_tab: c["newTab"] == true }),
        "browser.newTab" => N(C::BrowserNewTab),
        "browser.closeTab" => N(C::BrowserCloseTab),
        "browser.history" => N(C::BrowserHistory(c["delta"].as_i64().unwrap_or(0))),
        "browser.focusUrl" => N(C::BrowserFocusUrl),
        "ui.toggleSidebar" => N(C::ToggleSidebar),
        "ui.toggleBrowser" => N(C::ToggleBrowser),
        "ui.sidePanel" => N(C::SidePanel { panel: side_panel(c["panel"].as_str().unwrap_or_default()).ok_or_else(bad)?, show: c["show"] == true }),
        "ui.palette" => N(C::Palette(s("query"))),
        "ui.cheatSheet" => N(C::CheatSheet),
        "ui.settings" => N(C::Settings(s("page"))),
        "ui.theme" => N(C::Theme(s("theme").ok_or_else(bad)?)),
        "ui.overlay" => N(C::Overlay { overlay: overlay(c["overlay"].as_str().unwrap_or_default()).ok_or_else(bad)?, provider: s("provider") }),
        "terminal.zoom" => N(C::TerminalZoom(c["delta"].as_i64().unwrap_or(0) as i32)),
        "ui.view" => N(C::View(view(c["view"].as_str().unwrap_or_default()).ok_or_else(bad)?)),
        "recipe.load" => N(C::RecipeLoad(s("id").ok_or_else(bad)?)),
        "plugin.run" => BusCommand::PluginRun { plugin: s("plugin").ok_or_else(bad)?, command: s("command").ok_or_else(bad)?, args: c.get("args").cloned().unwrap_or(json!({})) },
        other => return Err(format!("“{other}” isn't a command winfer knows")),
    })
}

fn css_vars(theme_id: &str) -> Value {
    let def = theme::theme_by_id(theme_id);
    let u = &def.ui;
    let soft = format!("{}{}", u.accent, if def.mode == "light" { "22" } else { "26" });
    json!({
        "--color-app": u.app, "--color-panel": u.panel, "--color-panel-raised": u.panel_raised, "--color-hover": u.hover,
        "--color-active": u.active, "--color-border": u.border, "--color-border-strong": u.border_strong, "--color-fg": u.fg,
        "--color-fg-muted": u.fg_muted, "--color-fg-subtle": u.fg_subtle, "--color-accent": u.accent, "--color-accent-soft": soft,
        "--color-on-accent": u.on_accent, "--color-success": u.success, "--color-warning": u.warning, "--color-danger": u.danger,
        "--color-claude": u.claude,
    })
}

impl Root {
    fn send_to_host(&self, msg: Value) {
        if let Some(webview) = self.plugins.webview.as_ref().filter(|_| self.plugins.ready) {
            let _ = webview.evaluate_script(&format!("window.__winfer.recv({msg})"));
        }
    }

    fn reply_to_host(&self, id: u64, result: Result<Value, String>) {
        if let Some(webview) = &self.plugins.webview {
            let (ok, value) = match result {
                Ok(v) => (true, v),
                Err(e) => (false, Value::String(e)),
            };
            let _ = webview.evaluate_script(&format!("window.__winfer.reply({id}, {ok}, {value})"));
        }
    }

    /// Rescans the plugin folder and (re)starts the enabled plugins.
    pub(super) fn reload_plugins(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let infos = backend::background(|| backend_plugins::plugins_list().map(|l| serde_json::to_value(l).unwrap_or_default())).await;
            let infos: Vec<Value> = infos.ok().and_then(Result::ok).and_then(|v| v.as_array().cloned()).unwrap_or_default();
            let found = tauri::async_runtime::spawn(winfer_backend::providers::providers_list()).await;
            let _ = this.update(cx, |this, cx| {
                this.plugins.infos = infos;
                this.plugins.runtime_errors.clear();
                if let Ok(found) = found {
                    let providers = found.into_iter().map(|p| crate::model::AgentProvider { info: p.provider, installed: p.installed }).collect();
                    this.update_state(cx, |s| s.providers = providers);
                }
                this.load_into_host();
                cx.notify();
            });
        })
        .detach();
    }

    fn enabled_plugins(&self) -> Vec<Value> {
        self.plugins.infos.iter().filter(|p| p["enabled"] == true && p["error"].is_null() && p["manifest"].is_object()).cloned().collect()
    }

    fn load_into_host(&mut self) {
        let plugins: Vec<Value> = self.enabled_plugins().into_iter().map(|mut p| {
            let id = p["id"].as_str().unwrap_or_default().to_string();
            p["moduleUrl"] = Value::String(host_url(&format!("plugin/{id}.js")));
            p
        }).collect();
        if plugins.is_empty() {
            self.plugins.tools.clear();
            self.plugins.pages.clear();
            self.plugins.agent_tools.clear();
            self.plugins.panel_actions.clear();
        }
        self.send_to_host(json!({ "type": "load", "plugins": plugins }));
    }

    /// Starts the webview once there's a plugin to run, and keeps its state, theme and page in sync.
    pub(super) fn sync_plugins(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.plugins.webview.is_none() && !self.enabled_plugins().is_empty() {
            self.start_host(window, cx);
        }
        if !self.plugins.ready {
            return;
        }
        let theme_id = self.state.read(cx).theme.clone();
        if theme_id != self.plugins.sent_theme {
            let mode = theme::theme_by_id(&theme_id).mode.clone();
            self.send_to_host(json!({ "type": "theme", "id": theme_id, "mode": mode, "vars": css_vars(&theme_id) }));
            self.plugins.sent_theme = theme_id;
        }
        self.push_plugin_state(cx);
        let page = match self.state.read(cx).view() {
            View::Page(key) if self.plugins.pages.iter().any(|p| p.key == key) => Some(key),
            _ => None,
        };
        if page != self.plugins.mounted {
            self.send_to_host(json!({ "type": "mount", "key": page }));
            self.plugins.mounted = page.clone();
        }
        let show = page.is_some() && !self.modal_layer_open(cx) && self.plugins.page_bounds.size.width > px(0.0);
        if let Some(webview) = &self.plugins.webview {
            if show {
                let b = self.plugins.page_bounds;
                let _ = webview.set_bounds(Rect {
                    position: dpi::LogicalPosition::new(f64::from(b.origin.x / px(1.0)), f64::from(b.origin.y / px(1.0))).into(),
                    size: dpi::LogicalSize::new(f64::from(b.size.width / px(1.0)), f64::from(b.size.height / px(1.0))).into(),
                });
            }
            if self.plugins.shown != Some(show) {
                let _ = webview.set_visible(show);
                self.plugins.shown = Some(show);
            }
        }
    }

    /// Whether a GPUI layer covers the main area: the native webview would hide it.
    fn modal_layer_open(&self, cx: &App) -> bool {
        let s = self.state.read(cx);
        s.palette_query.is_some() || s.cheat_sheet_open || s.settings_open || s.overlay != Overlay::None || s.diff_pane_id.is_some()
            || !self.questions.is_empty() || !self.plugins.approvals.is_empty() || self.orchestration.has_layer() || self.plugins.apps_open
    }

    fn start_host(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (tx, rx) = async_channel::unbounded::<String>();
        let built = WebViewBuilder::new()
            .with_custom_protocol("winfer".into(), |_, request| serve(request.uri().path()))
            .with_ipc_handler(move |request| {
                let _ = tx.try_send(request.into_body());
            })
            .with_url(host_url("index.html"))
            .with_visible(false)
            .with_devtools(cfg!(debug_assertions))
            .with_bounds(Rect { position: dpi::LogicalPosition::new(0.0, 0.0).into(), size: dpi::LogicalSize::new(1.0, 1.0).into() })
            .build_as_child(window);
        match built {
            Ok(webview) => self.plugins.webview = Some(webview),
            Err(err) => {
                self.toast(format!("Plugins can't run: {err}"), cx);
                return;
            }
        }
        self.plugins.shown = Some(false);
        cx.spawn_in(window, async move |this, cx| {
            while let Ok(text) = rx.recv().await {
                let Ok(msg) = serde_json::from_str::<Value>(&text) else { continue };
                if this.update_in(cx, |this, window, cx| this.on_host_message(msg, window, cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    fn on_host_message(&mut self, msg: Value, window: &mut Window, cx: &mut Context<Self>) {
        match msg["kind"].as_str().unwrap_or_default() {
            "ready" => {
                self.plugins.ready = true;
                self.plugins.sent_state.clear();
                self.plugins.sent_theme.clear();
                self.plugins.mounted = None;
                self.load_into_host();
            }
            "registry" => {
                let take = |key: &str| msg[key].as_array().cloned().unwrap_or_default();
                self.plugins.tools = take("tools").into_iter().filter_map(|v| serde_json::from_value(v).ok()).collect();
                self.plugins.pages = take("pages").into_iter().filter_map(|v| serde_json::from_value(v).ok()).collect();
                self.plugins.agent_tools = take("agentTools").into_iter().filter_map(|v| serde_json::from_value(v).ok()).collect();
                self.plugins.panel_actions = take("panelActions").into_iter().filter_map(|v| serde_json::from_value(v).ok()).collect();
            }
            "error" => {
                let id = msg["plugin"].as_str().unwrap_or_default().to_string();
                match msg["error"].as_str() {
                    Some(e) => self.plugins.runtime_errors.insert(id, e.to_string()),
                    None => self.plugins.runtime_errors.remove(&id),
                };
            }
            "result" => {
                let Some(waiting) = msg["reqId"].as_u64().and_then(|id| self.plugins.waiting.remove(&id)) else { return };
                let ok = msg["ok"] == true;
                let text = match &msg["value"] {
                    Value::String(s) => s.clone(),
                    Value::Null => String::new(),
                    v => v.to_string(),
                };
                match waiting {
                    Waiting::Mcp(call) => agent_api::respond(&call, ok, text),
                    Waiting::Toast(plugin) if !ok => self.toast(format!("Plugin “{plugin}” failed: {text}"), cx),
                    Waiting::Toast(_) => {}
                }
            }
            "call" => {
                let id = msg["id"].as_u64().unwrap_or(0);
                self.host_call(id, msg["method"].as_str().unwrap_or_default().to_string(), msg["args"].clone(), window, cx);
            }
            _ => {}
        }
        cx.notify();
    }

    fn ask_host(&mut self, msg: Value, waiting: Waiting) {
        self.plugins.request_serial += 1;
        let id = self.plugins.request_serial;
        self.plugins.waiting.insert(id, waiting);
        let mut msg = msg;
        msg["reqId"] = json!(id);
        self.send_to_host(msg);
    }

    /// Answers a plugin's API call; slow ones reply when done.
    fn host_call(&mut self, id: u64, method: String, a: Value, window: &mut Window, cx: &mut Context<Self>) {
        let s = |k: &str| a[k].as_str().unwrap_or_default().to_string();
        let blocking = |this: &Self, cx: &mut Context<Self>, f: Box<dyn FnOnce() -> Result<Value, String> + Send>| {
            let _ = this;
            cx.spawn(async move |this, cx| {
                let result = backend::background(f).await.unwrap_or_else(|e| Err(e.to_string()));
                let _ = this.update(cx, |this, _| this.reply_to_host(id, result));
            })
            .detach();
        };
        match method.as_str() {
            "appExe" => self.reply_to_host(id, backend_plugins::plugins_app_exe().map(Value::String)),
            "toast" => {
                self.toast(s("text"), cx);
                self.reply_to_host(id, Ok(Value::Null));
            }
            "execute" => {
                let result = self.run_bus(&a["command"], window, cx).map(|_| Value::Null);
                if let Err(e) = &result {
                    self.toast(format!("A plugin sent a command winfer couldn't run: {e}"), cx);
                }
                self.reply_to_host(id, result);
            }
            "readPane" => {
                let lines = a["lines"].as_u64().unwrap_or(40) as usize;
                let text = SESSIONS.get(&s("paneId")).map(|session| Value::String(session.read_text(lines)));
                self.reply_to_host(id, Ok(text.unwrap_or(Value::Null)));
            }
            "paneRequest" => {
                let request = SESSIONS.get(&s("paneId")).map(|session| serde_json::to_value(describe_request(&session.read_text(40), None)).unwrap_or_default());
                self.reply_to_host(id, Ok(request.unwrap_or(Value::Null)));
            }
            "readFile" => {
                let path = s("path");
                blocking(self, cx, Box::new(move || {
                    let path = winfer_backend::editors::expand_home(&path);
                    if !path.is_absolute() {
                        return Err(format!("{} isn't an absolute path", path.display()));
                    }
                    let bytes = backend_plugins::read_file_capped(&path)?;
                    Ok(Value::String(String::from_utf8_lossy(&bytes).into_owned()))
                }));
            }
            "writeFile" => {
                let (path, data) = (s("path"), s("data"));
                blocking(self, cx, Box::new(move || {
                    let bytes = base64::engine::general_purpose::STANDARD.decode(data).map_err(|e| e.to_string())?;
                    let path = winfer_backend::editors::expand_home(&path);
                    if !path.is_absolute() {
                        return Err(format!("{} isn't an absolute path", path.display()));
                    }
                    if let Some(dir) = path.parent() {
                        std::fs::create_dir_all(dir).map_err(|e| format!("can't create {}: {e}", dir.display()))?;
                    }
                    std::fs::write(&path, bytes).map_err(|e| format!("can't write {}: {e}", path.display()))?;
                    Ok(Value::String(path.to_string_lossy().into_owned()))
                }));
            }
            "revealFile" => {
                let path = winfer_backend::editors::expand_home(&s("path"));
                blocking(self, cx, Box::new(move || reveal(&path).map(|_| Value::Null)));
            }
            "usage" => {
                let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_millis() as f64);
                let plans: Vec<String> = match a["provider"].as_str() {
                    Some(p) => vec![p.to_string()],
                    None => vec!["claude".into(), "codex".into()],
                };
                let found: Vec<Value> = plans.iter().filter_map(|p| self.usage.snapshots.get(p)).filter(|s| !s.windows.is_empty()).map(|s| json!({
                    "provider": s.provider, "at": s.at,
                    "windows": s.windows.iter().map(|w| json!({ "kind": w.kind, "label": w.label, "usedPercent": usage::used_now(w, now), "resetsAt": w.resets_at })).collect::<Vec<_>>(),
                })).collect();
                self.reply_to_host(id, Ok(Value::Array(found)));
            }
            "confirm" => {
                self.plugins.approval_serial += 1;
                let approval = Approval {
                    id,
                    plugin: s("plugin"),
                    pane_id: a["paneId"].as_str().map(str::to_string),
                    title: s("title"),
                    message: s("message"),
                    ok_label: s("okLabel"),
                    warning: a["kind"] != "info",
                };
                self.plugins.approvals.push(approval);
            }
            "vault.list" => {
                let plugin = s("plugin");
                blocking(self, cx, Box::new(move || to_json_result(vault::vault_list(plugin))));
            }
            "vault.set" => {
                let (plugin, key) = (s("plugin"), s("key"));
                let value = a["value"].as_str().map(str::to_string);
                let meta = a["meta"].as_object().cloned();
                blocking(self, cx, Box::new(move || to_json_result(vault::vault_set(plugin, key, value, meta))));
            }
            "vault.delete" => {
                let (plugin, key) = (s("plugin"), s("key"));
                blocking(self, cx, Box::new(move || vault::vault_delete(plugin, key).map(|_| Value::Null)));
            }
            "vault.reveal" => {
                let (plugin, key) = (s("plugin"), s("key"));
                blocking(self, cx, Box::new(move || vault::vault_reveal(plugin, key).map(Value::String)));
            }
            "vault.exec" => {
                let (plugin, command, cwd) = (s("plugin"), s("command"), s("cwd"));
                let keys: Vec<String> = a["keys"].as_array().map(|k| k.iter().filter_map(|v| v.as_str().map(str::to_string)).collect()).unwrap_or_default();
                let timeout = a["timeoutSec"].as_u64();
                cx.spawn(async move |this, cx| {
                    let result = tauri::async_runtime::spawn(vault::vault_exec(plugin, command, cwd, keys, timeout)).await;
                    let result = result.map_err(|e| e.to_string()).and_then(|r| to_json_result(r));
                    let _ = this.update(cx, |this, _| this.reply_to_host(id, result));
                })
                .detach();
            }
            "vault.fill" => {
                let (plugin, key, selector, workspace) = (s("plugin"), s("key"), s("selector"), s("workspaceId"));
                let tab = a["tab"].as_str().map(str::to_string);
                let target = self.browser.read(cx).fill_target(&workspace, tab.as_deref());
                let Some((target_id, url)) = target else {
                    let error = match tab {
                        Some(t) => format!("No browser tab matches “{t}”."),
                        None => "The workspace has no browser tab open.".to_string(),
                    };
                    return self.reply_to_host(id, Err(error));
                };
                cx.spawn(async move |this, cx| {
                    let result = tauri::async_runtime::spawn(async move {
                        let app = backend::app().clone();
                        browser::browser_fill_secret(app.clone(), app.state(), workspace, target_id, selector, plugin, key).await.map_err(|e| e.to_string())
                    })
                    .await;
                    let result = result.map_err(|e| e.to_string()).and_then(|r| r).map(|_| Value::String(url));
                    let _ = this.update(cx, |this, _| this.reply_to_host(id, result));
                })
                .detach();
            }
            other => self.reply_to_host(id, Err(format!("unknown plugin API call {other}"))),
        }
    }

    /// Runs a Command Bus command written the Tauri app's way (plugins and declarative tools).
    pub(super) fn run_bus(&mut self, command: &Value, window: &mut Window, cx: &mut Context<Self>) -> Result<(), String> {
        match parse_bus(command)? {
            BusCommand::Native(cmd) => self.execute(cmd, window, cx),
            BusCommand::Race { counts, prompt } => self.start_race(counts, prompt, cx),
            BusCommand::AbTest { provider, prompt, model } => self.start_ab_test(provider, prompt, model, cx),
            BusCommand::PluginRun { plugin, command, args } => self.run_plugin_command(plugin, command, args),
        }
        Ok(())
    }

    pub(super) fn run_plugin_command(&mut self, plugin: String, command: String, args: Value) {
        self.ask_host(json!({ "type": "command", "plugin": plugin, "command": command, "args": args }), Waiting::Toast(plugin));
    }

    /// Runs a plugin tool's palette entry.
    pub(super) fn run_plugin_tool(&mut self, tool_id: &str, args: Value, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tool) = self.plugins.tools.iter().find(|t| t.id == tool_id).cloned() else { return };
        if tool.code {
            return self.run_plugin_command(tool.plugin, format!("tool:{}", tool.tool_id), args);
        }
        for command in tool.commands.unwrap_or_default() {
            if let Err(e) = self.run_bus(&fill_template(&command, &args), window, cx) {
                self.toast(format!("Plugin “{}” failed: {e}", tool.plugin), cx);
                break;
            }
        }
    }

    pub(super) fn plugin_palette_entries(&self) -> Vec<(String, Command)> {
        let mut entries = Vec::new();
        for tool in &self.plugins.tools {
            for e in &tool.palette {
                entries.push((e.title.clone(), Command::PluginTool { id: tool.id.clone(), args: e.args.clone() }));
            }
        }
        for page in &self.plugins.pages {
            entries.push((format!("Open {}", page.title), Command::View(ViewTarget::Set(View::Page(page.key.clone())))));
        }
        entries
    }

    /// Recipes from every enabled plugin; invalid ones are skipped.
    pub(super) fn plugin_recipes(&self) -> Vec<Value> {
        let mut out = Vec::new();
        for p in self.enabled_plugins() {
            let id = p["id"].as_str().unwrap_or_default();
            for r in p["manifest"]["recipes"].as_array().into_iter().flatten() {
                let (Some(rid), Some(name), Some(panes)) = (r["id"].as_str(), r["name"].as_str(), r["panes"].as_array()) else { continue };
                let preset = r["preset"].as_str().filter(|p| preset(p).is_some_and(|x| x.is_some())).unwrap_or("auto");
                let panes: Vec<Value> = panes.iter().filter(|p| p.is_object()).map(|p| {
                    let provider = p["provider"].as_str().map_or_else(|| p["provider"].to_string(), str::to_string);
                    json!({ "provider": provider, "prompt": p["prompt"].as_str() })
                }).collect();
                out.push(json!({ "id": format!("plugin:{id}:{rid}"), "name": name, "panes": panes, "preset": preset, "url": r["url"].as_str() }));
            }
        }
        out
    }

    /// What `tools/list` adds for agents (MCP tool definitions).
    pub(super) fn plugin_agent_tool_list(&self) -> Value {
        Value::Array(self.plugins.agent_tools.iter().map(|t| json!({ "name": t.name, "description": t.description, "inputSchema": t.input_schema })).collect())
    }

    /// Runs an agent's call of a plugin tool; false if no plugin offers `name`.
    pub(super) fn run_plugin_agent_tool(&mut self, call: &str, name: &str, args: Value, pane_id: &str, cx: &App) -> bool {
        if !self.plugins.agent_tools.iter().any(|t| t.name == name) {
            return false;
        }
        let caller = self.describe_caller(pane_id, cx);
        self.ask_host(json!({ "type": "agentTool", "name": name, "args": args, "caller": caller }), Waiting::Mcp(call.to_string()));
        true
    }

    fn describe_caller(&self, pane_id: &str, cx: &App) -> Value {
        let s = self.state.read(cx);
        let Some(ws) = s.workspaces.iter().find(|w| w.panes.iter().any(|p| p.id == pane_id)) else { return Value::Null };
        let pane = ws.panes.iter().find(|p| p.id == pane_id).unwrap();
        let position = ws.layout.as_ref().map(leaves).unwrap_or_default().iter().position(|id| id == pane_id).map_or(0, |i| i + 1);
        let at = if position > 0 { format!("pane {position}") } else { "a pane".into() };
        json!({
            "paneId": pane_id, "workspaceId": ws.id, "workspaceName": ws.name,
            "cwd": pane.worktree.as_ref().map_or(&pane.cwd, |t| &t.path), "provider": pane.provider,
            "label": format!("{at} ({})", s.provider_name(&pane.provider)),
        })
    }

    /// Gives the host a fresh snapshot of workspaces and feeds pane events to plugins.
    fn push_plugin_state(&mut self, cx: &App) {
        let s = self.state.read(cx);
        let workspaces: Vec<Value> = s.workspaces.iter().map(|w| {
            let mut v = serde_json::to_value(w).unwrap_or_default();
            if let Some(panes) = v["panes"].as_array_mut() {
                for (p, pane) in panes.iter_mut().zip(&w.panes) {
                    p["workspaceId"] = json!(w.id);
                    p["status"] = json!(agent_api::status_name(pane.status));
                }
            }
            v
        }).collect();
        let mut positions = serde_json::Map::new();
        for w in &s.workspaces {
            for (i, id) in w.layout.as_ref().map(leaves).unwrap_or_default().iter().enumerate() {
                positions.insert(id.clone(), json!(i + 1));
            }
        }
        let providers: serde_json::Map<String, Value> = s.providers.iter().map(|p| (p.id.clone(), json!(p.name))).collect();
        let msg = json!({
            "type": "state",
            "state": { "workspaces": workspaces, "activeWorkspaceId": s.active_workspace_id, "focusedPaneId": s.focused_pane_id },
            "providers": providers, "positions": positions, "replies": s.replies,
        });
        let text = msg.to_string();
        if text == self.plugins.sent_state {
            return;
        }
        let current: HashMap<String, (AgentStatus, String, String)> = s.all_panes().map(|p| (p.id.clone(), (p.status, p.workspace_id.clone(), p.provider.clone()))).collect();
        let hydrated = s.hydrated;
        self.send_to_host(msg);
        self.plugins.sent_state = text;
        // Panes restored at launch aren't news.
        if hydrated && !self.plugins.previous.is_empty() {
            let mut events = Vec::new();
            for (id, (status, ws, provider)) in &current {
                let base = json!({ "paneId": id, "workspaceId": ws, "provider": provider });
                match self.plugins.previous.get(id) {
                    None => events.push(json!({ "type": "pane.opened", "paneId": id, "workspaceId": ws, "provider": provider })),
                    Some((before, _, _)) if before != status => {
                        let mut e = base;
                        e["type"] = json!("pane.status");
                        e["status"] = json!(agent_api::status_name(*status));
                        e["previous"] = json!(agent_api::status_name(*before));
                        events.push(e);
                    }
                    _ => {}
                }
            }
            for (id, (_, ws, provider)) in &self.plugins.previous {
                if !current.contains_key(id) {
                    events.push(json!({ "type": "pane.closed", "paneId": id, "workspaceId": ws, "provider": provider }));
                }
            }
            for event in events {
                self.send_to_host(json!({ "type": "event", "event": event }));
            }
        }
        if hydrated {
            self.plugins.previous = current;
        }
    }

    fn answer_approval(&mut self, ok: bool, cx: &mut Context<Self>) {
        if self.plugins.approvals.is_empty() {
            return;
        }
        let approval = self.plugins.approvals.remove(0);
        self.reply_to_host(approval.id, Ok(Value::Bool(ok)));
        cx.notify();
    }

    /// Panes with a plugin question waiting.
    pub(super) fn plugin_approval_panes(&self) -> Vec<String> {
        self.plugins.approvals.iter().filter_map(|a| a.pane_id.clone()).collect()
    }

    /// Answers a question by its id, from the phone.
    pub(super) fn answer_approval_id(&mut self, id: u64, ok: bool, cx: &mut Context<Self>) {
        if let Some(i) = self.plugins.approvals.iter().position(|a| a.id == id) {
            self.plugins.approvals.remove(i);
            self.reply_to_host(id, Ok(Value::Bool(ok)));
            cx.notify();
        }
    }

    /// The oldest plugin question an agent in `pane` is waiting on, for the phone bridge.
    pub(super) fn approval_for(&self, pane: &str) -> Option<(u64, String, String)> {
        self.plugins.approvals.iter().find(|a| a.pane_id.as_deref() == Some(pane)).map(|a| (a.id, a.title.clone(), a.message.clone()))
    }

    /// The main area while a plugin page is shown: the webview is placed over it.
    pub(super) fn render_plugin_page(&mut self, key: &str, c: &Colors, cx: &mut Context<Self>) -> Div {
        let c = *c;
        let known = self.plugins.pages.iter().any(|p| p.key == key);
        let measure = cx.entity();
        div().relative().size_full().rounded_lg().bg(c.panel)
            .child(canvas(move |bounds, _, cx| measure.update(cx, |this, _| this.plugins.page_bounds = bounds), |_, _, _, _| {}).absolute().size_full())
            .when(!known, |d| d.flex().items_center().justify_center().text_xs().text_color(c.subtle).child(if self.plugins.ready { "This page's plugin is off or gone." } else { "Loading plugins…" }))
    }

    fn page_icon(icon: Option<&str>) -> IconName {
        match icon.unwrap_or_default() {
            "bot" => IconName::Bot,
            "globe" => IconName::Globe,
            "terminal" => IconName::SquareTerminal,
            "file" => IconName::FileText,
            "star" => IconName::Star,
            "database" | "database-search" => IconName::Database,
            "key" | "key-round" | "lock" | "shield" => IconName::Lock,
            "send" => IconName::Send,
            "camera" | "video" | "record" => IconName::Camera,
            "stop" => IconName::Square,
            _ => IconName::Puzzle,
        }
    }

    fn pinned_keys(&self, cx: &App) -> Option<Vec<String>> {
        self.state.read(cx).rest.get("pinnedPages").and_then(|v| serde_json::from_value(v.clone()).ok())
    }

    fn toggle_pin(&mut self, key: &str, cx: &mut Context<Self>) {
        let mut pins = self.pinned_keys(cx).unwrap_or_else(|| self.plugins.pages.iter().take(DEFAULT_PINNED).map(|p| p.key.clone()).collect());
        if let Some(i) = pins.iter().position(|k| k == key) {
            pins.remove(i);
        } else {
            pins.push(key.to_string());
        }
        self.update_state(cx, |s| {
            s.rest.insert("pinnedPages".into(), json!(pins));
        });
    }

    /// Title bar tabs: pinned plugin pages, then the Apps menu with every page.
    pub(super) fn render_page_tabs(&self, c: Colors, cx: &mut Context<Self>) -> Vec<AnyElement> {
        if self.plugins.pages.is_empty() {
            return Vec::new();
        }
        let s = self.state.read(cx);
        let current = match s.view() {
            View::Page(k) => Some(k),
            _ => None,
        };
        let icons_only = s.page_tab_icons_only;
        let pins = self.pinned_keys(cx);
        let pinned = pinned_pages(&self.plugins.pages, pins.as_ref());
        let tab = |id: SharedString, active: bool| {
            div().id(id).flex().items_center().gap_1p5().rounded_md().px_2p5().py_1().text_xs().cursor_pointer()
                .text_color(if active { c.fg } else { c.muted }).when(active, |d| d.bg(c.active)).hover(move |d| d.bg(c.hover).text_color(c.fg))
        };
        let mut out: Vec<AnyElement> = pinned.iter().map(|page| {
            let key = page.key.clone();
            tab(SharedString::from(format!("page-{}", page.key)), current.as_ref() == Some(&page.key))
                .child(Icon::new(Self::page_icon(page.icon.as_deref())).size(px(12.0)))
                .when(!icons_only, |d| d.child(div().max_w(px(128.0)).truncate().child(page.title.clone())))
                .on_click(cx.listener(move |this, _, window, cx| this.execute(Command::View(ViewTarget::Set(View::Page(key.clone()))), window, cx)))
                .into_any_element()
        }).collect();
        let unpinned: Vec<&PageInfo> = self.plugins.pages.iter().filter(|p| !pinned.iter().any(|x| x.key == p.key)).collect();
        let shown = unpinned.iter().find(|p| current.as_ref() == Some(&p.key));
        let label = shown.map_or("Apps".to_string(), |p| p.title.clone());
        out.push(
            tab("page-apps".into(), shown.is_some() || self.plugins.apps_open)
                .child(Icon::new(shown.map_or(IconName::LayoutDashboard, |p| Self::page_icon(p.icon.as_deref()))).size(px(12.0)))
                .when(!icons_only, |d| d.child(label))
                .when(shown.is_none() && !unpinned.is_empty(), |d| d.child(div().text_size(px(10.0)).text_color(c.subtle).child(unpinned.len().to_string())))
                .child(Icon::new(IconName::ChevronDown).size(px(11.0)).text_color(c.subtle))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.plugins.apps_open = !this.plugins.apps_open;
                    cx.notify();
                }))
                .into_any_element(),
        );
        out
    }

    /// The Apps menu: every plugin page, with a star to pin it to the title bar.
    pub(super) fn render_apps_menu(&self, c: &Colors, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.plugins.apps_open {
            return None;
        }
        let c = *c;
        let pins = self.pinned_keys(cx);
        let pinned: Vec<String> = pinned_pages(&self.plugins.pages, pins.as_ref()).iter().map(|p| p.key.clone()).collect();
        let current = match self.state.read(cx).view() {
            View::Page(k) => Some(k),
            _ => None,
        };
        let mut menu = div().id("apps-menu").absolute().top(px(40.0)).left(relative(0.5)).w(px(256.0)).ml(px(-128.0)).rounded_lg().bg(c.raised).p_1().flex().flex_col()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation());
        let mut ordered: Vec<&PageInfo> = pinned.iter().filter_map(|k| self.plugins.pages.iter().find(|p| p.key == *k)).collect();
        ordered.extend(self.plugins.pages.iter().filter(|p| !pinned.contains(&p.key)));
        for (i, page) in ordered.into_iter().enumerate() {
            let (open, pin) = (page.key.clone(), page.key.clone());
            let is_pinned = pinned.contains(&page.key);
            let selected = current.as_ref() == Some(&page.key);
            menu = menu.child(
                div().flex().items_center().rounded_md().when(selected, |d| d.bg(c.active)).hover(move |d| d.bg(c.hover))
                    .child(div().id(("app-open", i)).flex_1().min_w_0().flex().items_center().gap_2().px_2().py_1p5().text_xs().cursor_pointer()
                        .text_color(if selected { c.fg } else { c.muted })
                        .child(Icon::new(Self::page_icon(page.icon.as_deref())).size(px(12.0)))
                        .child(div().truncate().child(page.title.clone()))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.plugins.apps_open = false;
                            this.execute(Command::View(ViewTarget::Set(View::Page(open.clone()))), window, cx);
                        })))
                    .child(div().id(("app-pin", i)).mr_1().rounded_sm().p_1().cursor_pointer().text_color(if is_pinned { c.warning } else { c.subtle })
                        .child(crate::ui::code_pickers::star_icon(if is_pinned { c.warning } else { c.subtle }, 12.0))
                        .on_click(cx.listener(move |this, _, _, cx| this.toggle_pin(&pin, cx)))),
            );
        }
        menu = menu.child(div().my_1().h(px(1.0)).bg(c.border)).child(
            div().id("apps-manage").flex().items_center().gap_2().rounded_md().px_2().py_1p5().text_xs().text_color(c.muted).cursor_pointer().hover(move |d| d.bg(c.hover).text_color(c.fg))
                .child(Icon::new(IconName::Settings).size(px(12.0)))
                .child("Manage plugins…")
                .on_click(cx.listener(|this, _, window, cx| {
                    this.plugins.apps_open = false;
                    this.execute(Command::Settings(Some("plugins".into())), window, cx);
                })),
        );
        Some(
            div().id("apps-backdrop").absolute().size_full()
                .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                    this.plugins.apps_open = false;
                    cx.notify();
                }))
                .child(menu)
                .into_any_element(),
        )
    }

    /// Buttons plugins add to a side panel's toolbar.
    pub(super) fn render_panel_actions(&self, panel: &str, c: Colors, cx: &mut Context<Self>) -> Vec<AnyElement> {
        self.plugins.panel_actions.iter().filter(|a| a.panel == panel).enumerate().map(|(i, action)| {
            let (key, plugin) = (action.key.clone(), action.plugin_id.clone());
            div().id(("panel-action", i)).flex().flex_shrink_0().items_center().gap_1().rounded_sm().p_1().cursor_pointer()
                .text_color(if action.active { c.danger } else { c.subtle }).hover(move |d| d.bg(c.hover))
                .child(Icon::new(Self::page_icon(action.icon.as_deref())).size(px(14.0)))
                .when_some(action.label.clone(), |d, l| d.child(div().text_size(px(10.0)).child(l)))
                .on_click(cx.listener(move |this, _, _, _| this.ask_host(json!({ "type": "panelAction", "key": key }), Waiting::Toast(plugin.clone()))))
                .into_any_element()
        }).collect()
    }

    /// The in-app dialog for plugins' `api.confirm` questions.
    pub(super) fn render_approval(&self, c: &Colors, cx: &mut Context<Self>) -> Option<AnyElement> {
        let approval = self.plugins.approvals.first()?;
        let c = *c;
        let waiting = self.plugins.approvals.len() - 1;
        let name = self.plugins.infos.iter().find(|p| p["id"] == approval.plugin.as_str()).and_then(|p| p["manifest"]["name"].as_str()).unwrap_or(&approval.plugin).to_string();
        let subtitle = format!("{name}{}", if approval.pane_id.is_some() { " · you can also answer from your phone" } else { "" });
        Some(
            div().id("approval-backdrop").absolute().size_full().flex().items_center().justify_center().bg(hsla(0.0, 0.0, 0.0, 0.4)).occlude().child(
                div().w(px(460.0)).max_h(relative(0.7)).flex().flex_col().rounded_xl().bg(c.raised)
                    .child(div().flex().items_start().gap_3().px_5().py_3()
                        .child(Icon::new(if approval.warning { IconName::TriangleAlert } else { IconName::Info }).size(px(16.0)).text_color(if approval.warning { c.warning } else { c.accent }))
                        .child(div().min_w_0().flex_1().child(div().text_sm().text_color(c.fg).child(approval.title.clone())).child(div().text_xs().text_color(c.subtle).child(subtitle))))
                    .child(div().id("approval-message").overflow_y_scroll().px_5().py_4().text_size(px(13.0)).text_color(c.fg).child(approval.message.clone()))
                    .child(div().flex().items_center().gap_2().px_5().py_3()
                        .when(waiting > 0, |d| d.child(div().text_xs().text_color(c.subtle).child(format!("{waiting} more waiting"))))
                        .child(div().flex_1())
                        .child(small_button(c, "approval-cancel", "Cancel").on_click(cx.listener(|this, _, _, cx| this.answer_approval(false, cx))))
                        .child(orchestration::accent_button(c, "approval-ok", approval.ok_label.clone(), true).on_click(cx.listener(|this, _, _, cx| this.answer_approval(true, cx))))),
            )
            .into_any_element(),
        )
    }

    /// Escape on the approval dialog cancels it.
    pub(super) fn close_plugin_layer(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.plugins.approvals.is_empty() {
            self.answer_approval(false, cx);
            return true;
        }
        if self.plugins.apps_open {
            self.plugins.apps_open = false;
            cx.notify();
            return true;
        }
        false
    }
}

fn to_json_result<T: serde::Serialize>(r: Result<T, String>) -> Result<Value, String> {
    r.and_then(|v| serde_json::to_value(v).map_err(|e| e.to_string()))
}

pub(super) fn reveal(path: &std::path::Path) -> Result<(), String> {
    let mut command = if cfg!(target_os = "macos") {
        let mut c = std::process::Command::new("open");
        c.arg("-R").arg(path);
        c
    } else if cfg!(target_os = "windows") {
        let mut c = std::process::Command::new("explorer");
        c.arg(format!("/select,{}", path.display()));
        c
    } else {
        let mut c = std::process::Command::new("xdg-open");
        c.arg(path.parent().unwrap_or(path));
        c
    };
    command.spawn().map(|_| ()).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::{BusCommand, fill_template, matches_search, parse_bus};
    use crate::ui::root::commands::{Command, PaneRef};
    use serde_json::json;

    #[test]
    fn templates_fill_every_string() {
        let filled = fill_template(&json!({ "type": "pane.sendText", "text": "run {{ script }} on {{env}}", "n": 2, "list": ["{{env}}"] }), &json!({ "script": "test", "env": "ci" }));
        assert_eq!(filled, json!({ "type": "pane.sendText", "text": "run test on ci", "n": 2, "list": ["ci"] }));
        assert_eq!(fill_template(&json!("{{missing}}!"), &json!({})), json!("!"));
    }

    #[test]
    fn search_needs_every_word() {
        assert!(matches_search("kno base", &["Knowledge base", "kb"]));
        assert!(!matches_search("graph db", &["Knowledge base"]));
    }

    #[test]
    fn bus_commands_parse_like_the_tauri_app() {
        assert_eq!(parse_bus(&json!({ "type": "pane.sendText", "target": { "index": 2 }, "text": "hi", "submit": true })).unwrap(), BusCommand::Native(Command::SendText { target: PaneRef::Index(2), text: "hi".into(), submit: true }));
        assert_eq!(parse_bus(&json!({ "type": "pane.interrupt", "target": "focused" })).unwrap(), BusCommand::Native(Command::Interrupt(PaneRef::Focused)));
        assert!(matches!(parse_bus(&json!({ "type": "race.start", "counts": { "claude": 2 }, "prompt": "x" })).unwrap(), BusCommand::Race { .. }));
        assert!(parse_bus(&json!({ "type": "nope" })).is_err());
    }
}
