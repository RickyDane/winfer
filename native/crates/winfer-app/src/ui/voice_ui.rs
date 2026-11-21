//! Live voice in the app: start and stop, the model's tool calls, and the HUD
//! (port of `features/voice/live/session.ts`, `LiveVoiceHUD.tsx` and the voice half of `tools.ts`).

use super::voice_tools::{self, END_CONVERSATION};
use super::*;
use crate::backend;
use crate::voice::client::{self, Down, Handle, Player, State, Up, Who};
use crate::voice::{io, protocol};
use serde_json::json;
use std::sync::Arc;
use winfer_backend::realtime;

const MAX_LINES: usize = 12;
const MAX_RESULT: usize = 6000;
const USE_BROWSER: &str = "use_browser";
const BG: &str = "run_in_background";
const WEB: &str = "web_";

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum LiveState {
    Off,
    Connecting,
    Listening,
    Thinking,
    Speaking,
}

#[derive(Clone)]
pub(super) enum Line {
    You(String),
    Assistant(String),
    Tool(String),
}

impl Root {
    pub(super) fn live_voice_on(&self) -> bool {
        self.voice.state != LiveState::Off
    }
}

pub(super) struct VoiceUi {
    state: LiveState,
    muted: bool,
    lines: Vec<Line>,
    tool: Option<String>,
    error: Option<String>,
    level: f32,
    handle: Option<Handle>,
    audio: Option<io::Audio>,
    epoch: u64,
    /// What the session tries: 0 everything, 1 without tools, 2 a bare session.
    attempt: u8,
    /// A response succeeded, so later errors aren't the setup's fault.
    working: bool,
    /// The agent browser's tools, once loaded for a workspace.
    web: Option<(String, Vec<serde_json::Value>)>,
}

impl Default for VoiceUi {
    fn default() -> Self {
        Self { state: LiveState::Off, muted: false, lines: Vec::new(), tool: None, error: None, level: 0.0, handle: None, audio: None, epoch: 0, attempt: 0, working: false, web: None }
    }
}

fn describe_attempt(a: u8) -> &'static str {
    if a >= 2 { "with a bare session" } else { "without tools" }
}

impl Root {
    pub(super) fn toggle_voice(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.voice.state != LiveState::Off {
            self.stop_voice(cx);
        } else {
            self.start_voice(0, window, cx);
        }
    }

    fn live_str(&self, key: &str, cx: &App) -> String {
        self.state.read(cx).rest.get("voice").and_then(|v| v[key].as_str()).unwrap_or_default().to_string()
    }

    fn voice_context(&self, cx: &App) -> voice_tools::Context {
        let s = self.state.read(cx);
        voice_tools::Context {
            providers: s.providers.iter().filter(|p| p.installed).map(|p| (p.id.clone(), p.name.clone())).collect(),
            panes: s.active().and_then(|w| w.layout.as_ref()).map(leaves).unwrap_or_default(),
            workspaces: s.workspaces.iter().map(|w| (w.id.clone(), w.name.clone())).collect(),
            themes: theme::THEMES.iter().map(|t| t.id.clone()).collect(),
            recipes: self.recipe_names(cx),
        }
    }

    fn voice_summary(&self, cx: &App) -> String {
        let s = self.state.read(cx);
        let Some(ws) = s.active() else { return String::new() };
        let order = ws.layout.as_ref().map(leaves).unwrap_or_default();
        let panes = order
            .iter()
            .enumerate()
            .filter_map(|(i, id)| {
                let p = ws.panes.iter().find(|p| p.id == *id)?;
                let extra = [p.worktree.as_ref().map(|t| format!("branch {}", t.branch)), (s.focused_pane_id.as_deref() == Some(id)).then(|| "focused".to_string())].into_iter().flatten().collect::<Vec<_>>().join(", ");
                Some((i + 1, s.provider_name(&p.provider), id.clone(), super::agent_api::status_name(p.status).to_string(), extra))
            })
            .collect();
        voice_tools::state_summary(&voice_tools::Summary {
            workspace: ws.name.clone(),
            root: ws.root_dir.clone(),
            panes,
            other_workspaces: s.workspaces.iter().filter(|w| w.id != ws.id).map(|w| w.name.clone()).collect(),
            agents: s.providers.iter().filter(|p| p.installed).map(|p| format!("{} ({})", p.name, p.id)).collect(),
            browser_tabs: self.browser.read(cx).tab_labels(&ws.id),
        })
    }

    /// The tools for the current app state (attempt 0 all, else none).
    fn voice_tool_list(&self, attempt: u8, cx: &App) -> Vec<serde_json::Value> {
        if attempt > 0 {
            return Vec::new();
        }
        let ctx = self.voice_context(cx);
        let mut tools = voice_tools::agent_tools();
        tools.extend(voice_tools::registry_tools(&ctx.providers));
        let active = self.state.read(cx).active_workspace_id.clone();
        match &self.voice.web {
            Some((ws, list)) if *ws == active => tools.extend(list.iter().cloned()),
            _ => tools.push(json!({ "type": "function", "name": USE_BROWSER, "description": "Load the tools to see and control web pages in winfer's browser (snapshot, click, type, fill forms, console). Call before acting on a page.", "parameters": { "type": "object", "properties": {} } })),
        }
        tools.push(json!({ "type": "function", "name": BG, "description": "Run a long-running command (dev server, watcher, build) in the workspace folder in the background.", "parameters": { "type": "object", "properties": { "command": { "type": "string" } }, "required": ["command"] } }));
        tools.push(voice_tools::end_tool());
        tools.iter().map(voice_tools::clean).collect()
    }

    fn voice_session(&self, attempt: u8, cx: &App) -> serde_json::Value {
        let prompt = Some(self.live_str("livePrompt", cx)).filter(|p| !p.trim().is_empty()).unwrap_or_else(|| super::settings_more::LIVE_ROLE.to_string());
        let instructions = format!("{}\n\n{}", prompt.trim(), self.voice_summary(cx));
        protocol::session(&instructions, &self.voice_tool_list(attempt, cx), &self.live_str("liveVoice", cx), attempt >= 2)
    }

    pub(super) fn start_voice(&mut self, attempt: u8, window: &mut Window, cx: &mut Context<Self>) {
        let auth = self.live_str("liveAuth", cx);
        let key = if auth == "chatgpt" { None } else { realtime::stored_api_key() };
        let Some(key) = key else {
            let text = if auth == "chatgpt" {
                "Live voice on a ChatGPT plan needs WebRTC, which the native build doesn't have yet. Add an OpenAI API key in Settings → Voice to use live voice."
            } else {
                "Add an OpenAI API key in Settings → Voice to use live voice."
            };
            self.toast_action(text, "Open settings", Command::Settings(Some("voice".into())), cx);
            return;
        };
        self.voice.epoch += 1;
        let epoch = self.voice.epoch;
        self.voice.state = LiveState::Connecting;
        self.voice.attempt = attempt;
        self.voice.working = false;
        self.voice.error = None;
        self.voice.muted = false;
        if attempt == 0 {
            self.voice.lines.clear();
        }
        let session = self.voice_session(attempt, cx);
        let url = protocol::url(&self.live_str("liveModel", cx));
        let device = Some(self.live_str("inputDevice", cx)).filter(|d| !d.is_empty());
        let (up_tx, up_rx) = async_channel::unbounded::<Up>();
        let (cap_tx, cap_rx) = tokio::sync::mpsc::unbounded_channel();
        let player = Arc::new(Player::default());
        let audio_player = player.clone();
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let audio = backend::background(move || io::start(device, cap_tx, audio_player)).await.map_err(|e| e.to_string()).and_then(|r| r);
            let audio = match audio {
                Ok(a) => a,
                Err(e) => {
                    let _ = this.update(cx, |this, cx| this.voice_fail(epoch, e, cx));
                    return;
                }
            };
            let started = this.update(cx, |this, cx| {
                if this.voice.epoch != epoch {
                    return false;
                }
                this.voice.audio = Some(audio);
                this.voice.handle = Some(client::start(client::Config { url, key, session }, up_tx, cap_rx, player));
                this.voice_tick(epoch, cx);
                true
            });
            if !started.unwrap_or(false) {
                return;
            }
            while let Ok(up) = up_rx.recv().await {
                let alive = this.update_in(cx, |this, window, cx| {
                    if this.voice.epoch != epoch {
                        return false;
                    }
                    this.on_voice(up, window, cx);
                    true
                });
                if !alive.unwrap_or(false) {
                    break;
                }
            }
        })
        .detach();
    }

    /// Follows the microphone level for the HUD.
    fn voice_tick(&mut self, epoch: u64, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_millis(90)).await;
                let keep = this.update(cx, |this, cx| {
                    if this.voice.epoch != epoch || this.voice.state == LiveState::Off {
                        return false;
                    }
                    let level = if this.voice.muted { 0.0 } else { this.voice.audio.as_ref().map_or(0.0, io::Audio::level) };
                    this.voice.level = this.voice.level * 0.5 + level * 0.5;
                    cx.notify();
                    true
                });
                if !keep.unwrap_or(false) {
                    break;
                }
            }
        })
        .detach();
    }

    pub(super) fn stop_voice(&mut self, cx: &mut Context<Self>) {
        self.voice.epoch += 1;
        if let Some(h) = self.voice.handle.take() {
            h.send(Down::Stop);
        }
        self.voice.audio = None;
        self.voice.state = LiveState::Off;
        self.voice.tool = None;
        self.voice.web = None;
        cx.notify();
    }

    fn voice_fail(&mut self, epoch: u64, text: String, cx: &mut Context<Self>) {
        if self.voice.epoch != epoch {
            return;
        }
        self.stop_voice(cx);
        self.voice.error = Some(text.clone());
        self.toast(text, cx);
    }

    fn on_voice(&mut self, up: Up, window: &mut Window, cx: &mut Context<Self>) {
        match up {
            Up::State(s) => {
                self.voice.state = match s {
                    State::Listening => LiveState::Listening,
                    State::Thinking => LiveState::Thinking,
                    State::Speaking => {
                        self.voice.working = true;
                        LiveState::Speaking
                    }
                };
                if s != State::Thinking {
                    self.voice.tool = None;
                }
            }
            Up::Line(who, text) => {
                self.voice.working = true;
                self.add_voice_line(if who == Who::User { Line::You(text) } else { Line::Assistant(text) });
            }
            Up::Calls(calls) => {
                for call in calls {
                    self.voice_call(call, window, cx);
                }
            }
            Up::Error { text, fatal } => {
                let epoch = self.voice.epoch;
                if fatal || self.voice.working || self.voice.attempt >= 2 {
                    self.voice_fail(epoch, text, cx);
                } else {
                    // OpenAI can fail a session's first response and go silent: try again with less.
                    let next = self.voice.attempt + 1;
                    self.stop_voice(cx);
                    self.add_voice_line(Line::Tool(format!("Retrying {}…", describe_attempt(next))));
                    self.start_voice(next, window, cx);
                }
            }
            Up::Closed => {
                if self.voice.state != LiveState::Off {
                    self.stop_voice(cx);
                }
            }
        }
        cx.notify();
    }

    fn add_voice_line(&mut self, line: Line) {
        self.voice.lines.push(line);
        let extra = self.voice.lines.len().saturating_sub(MAX_LINES);
        self.voice.lines.drain(..extra);
    }

    fn voice_result(&mut self, id: &str, name: &str, text: String, cx: &mut Context<Self>) {
        let text = if text.len() > MAX_RESULT { format!("{}\n… (truncated)", text.chars().take(MAX_RESULT).collect::<String>()) } else { text };
        if let Some(h) = &self.voice.handle {
            h.send(Down::Results(vec![(id.to_string(), text)]));
        }
        self.add_voice_line(Line::Tool(name.to_string()));
        cx.notify();
    }

    /// Runs one function call the model made and answers it.
    fn voice_call(&mut self, call: protocol::Call, window: &mut Window, cx: &mut Context<Self>) {
        let (id, name, args) = (call.id, call.name, call.args);
        self.voice.tool = Some(name.clone());
        if name == END_CONVERSATION {
            // Let a goodbye finish playing first.
            let epoch = self.voice.epoch;
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(Duration::from_millis(1500)).await;
                let _ = this.update(cx, |this, cx| {
                    if this.voice.epoch == epoch {
                        this.stop_voice(cx);
                    }
                });
            })
            .detach();
            return;
        }
        let workspace = self.state.read(cx).active_workspace_id.clone();
        if name == USE_BROWSER || name.starts_with(WEB) || name == BG {
            self.voice_async_call(id, name, args, workspace, cx);
            return;
        }
        let result = self.voice_sync_call(&name, &args, window, cx);
        self.voice_result(&id, &name, result.unwrap_or_else(|e| format!("Error: {e}")), cx);
    }

    fn voice_sync_call(&mut self, name: &str, args: &serde_json::Value, window: &mut Window, cx: &mut Context<Self>) -> Result<String, String> {
        match name {
            "list_panes" | "read_pane" | "send_to_pane" | "spawn_agent" | "usage_status" => {
                let mut args = args.clone();
                if name == "read_pane" && args["lines"].is_null() {
                    args["lines"] = json!(60);
                }
                self.run_voice_agent_tool(name, &args, cx)
            }
            "open_url" => {
                let url = args["url"].as_str().unwrap_or_default().trim().to_string();
                if url.is_empty() {
                    return Err("url is empty".into());
                }
                self.execute(Command::BrowserOpen { url: url.clone(), new_tab: args["new_tab"] == true }, window, cx);
                Ok(format!("Opening {url}."))
            }
            "open_file" => {
                let path = args["path"].as_str().unwrap_or_default().trim().to_string();
                if path.is_empty() {
                    return Err("path is empty".into());
                }
                let root = self.state.read(cx).active().map(|w| w.root_dir.clone()).unwrap_or_default();
                let full = if std::path::Path::new(&path).is_absolute() { path.clone() } else { std::path::Path::new(&root).join(&path).to_string_lossy().into_owned() };
                let line = args["line"].as_u64().map(|l| l as usize);
                self.viewer.update(cx, |v, cx| v.open(full, line, false, cx));
                Ok(format!("Opened {path}."))
            }
            registry if voice_tools::is_registry_tool(registry) => {
                if voice_tools::needs_confirmation(registry, args) {
                    return Ok("Not done: ask the user to confirm this first, then call again with confirmed: true.".into());
                }
                let commands = voice_tools::registry_commands(registry, args, &self.voice_context(cx))?;
                if commands.is_empty() {
                    return Ok("Nothing to do with those arguments.".into());
                }
                for c in commands {
                    self.execute(c, window, cx);
                }
                Ok("Done.".into())
            }
            other => Err(format!("Unknown tool {other}")),
        }
    }

    /// The agent browser's tools and background commands, which take a moment.
    fn voice_async_call(&mut self, id: String, name: String, args: serde_json::Value, workspace: String, cx: &mut Context<Self>) {
        let (cwd, epoch) = (self.state.read(cx).active().map(|w| w.root_dir.clone()).unwrap_or_default(), self.voice.epoch);
        cx.spawn(async move |this, cx| {
            let (call_name, call_args, ws) = (name.clone(), args.clone(), workspace.clone());
            let result: Result<(String, Option<Vec<serde_json::Value>>), String> = tauri::async_runtime::spawn(async move {
                let app = backend::app().clone();
                if call_name == USE_BROWSER {
                    let list = realtime::realtime_browser_tools(app, ws).await?;
                    let tools: Vec<serde_json::Value> = list
                        .iter()
                        .map(|t| json!({ "type": "function", "name": format!("{WEB}{}", t["name"].as_str().unwrap_or_default().trim_start_matches("browser_")), "description": t["description"].as_str().unwrap_or_default().lines().next().unwrap_or_default().chars().take(300).collect::<String>(), "parameters": t.get("inputSchema").cloned().unwrap_or_else(|| json!({ "type": "object", "properties": {} })) }))
                        .collect();
                    let names: Vec<String> = tools.iter().filter_map(|t| t["name"].as_str().map(str::to_string)).collect();
                    Ok((format!("Browser tools ready: {}. Take a web_snapshot to see the page and get element refs.", names.join(", ")), Some(tools)))
                } else if call_name == BG {
                    let reply = realtime::realtime_bg(call_args, cwd).await?;
                    let v = serde_json::to_value(reply).unwrap_or_default();
                    let text = v["text"].as_str().unwrap_or_default().to_string();
                    Ok((if v["ok"] == true { text } else { format!("Error: {text}") }, None))
                } else {
                    let tool = format!("browser_{}", call_name.trim_start_matches(WEB));
                    let reply = realtime::realtime_browser_call(app, ws, tool, call_args).await?;
                    let v = serde_json::to_value(reply).unwrap_or_default();
                    let text = v["text"].as_str().unwrap_or_default().to_string();
                    Ok((if v["ok"] == true { if text.is_empty() { "Done.".to_string() } else { text } } else { format!("Error: {text}") }, None))
                }
            })
            .await
            .map_err(|e| e.to_string())
            .and_then(|r| r);
            let _ = this.update(cx, |this, cx| {
                if this.voice.epoch != epoch {
                    return;
                }
                match result {
                    Ok((text, tools)) => {
                        if let Some(tools) = tools {
                            this.voice.web = Some((workspace, tools));
                            let session = this.voice_session(this.voice.attempt, cx);
                            if let Some(h) = &this.voice.handle {
                                h.send(Down::UpdateSession(session));
                            }
                        }
                        this.voice_result(&id, &name, text, cx);
                    }
                    Err(e) => this.voice_result(&id, &name, format!("Error: {e}"), cx),
                }
            });
        })
        .detach();
    }

    /// Tells the model about something that happened in winfer; it decides whether to speak.
    pub(super) fn voice_note(&mut self, text: String) {
        if self.voice.state == LiveState::Off {
            return;
        }
        if let Some(h) = &self.voice.handle {
            h.send(Down::Note(text));
        }
    }

    /// A pane's agent finished or needs the user.
    pub(super) fn voice_agent_note(&mut self, pane: &str, what: &str, cx: &App) {
        if self.voice.state == LiveState::Off {
            return;
        }
        let s = self.state.read(cx);
        let Some(ws) = s.workspaces.iter().find(|w| w.panes.iter().any(|p| p.id == pane)) else { return };
        let order = ws.layout.as_ref().map(leaves).unwrap_or_default();
        let (Some(p), Some(n)) = (ws.panes.iter().find(|p| p.id == pane), order.iter().position(|id| id == pane)) else { return };
        let note = format!("{} in pane {} {what}.", s.provider_name(&p.provider), n + 1);
        self.voice_note(note);
    }

    pub(super) fn render_voice_hud(&mut self, c: &Colors, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.voice.state == LiveState::Off {
            return None;
        }
        let c = *c;
        let (label, color) = match self.voice.state {
            LiveState::Connecting => ("Connecting…", c.muted),
            LiveState::Listening if self.voice.muted => ("Muted", c.warning),
            LiveState::Listening => ("Listening", c.success),
            LiveState::Thinking => ("Thinking…", c.accent),
            LiveState::Speaking => ("Speaking", c.accent),
            LiveState::Off => ("", c.muted),
        };
        let status = match &self.voice.tool {
            Some(t) => format!("{label} · {t}"),
            None => label.to_string(),
        };
        let level = (self.voice.level * 6.0).clamp(0.0, 1.0);
        let muted = self.voice.muted;
        let lines: Vec<AnyElement> = self.voice.lines.iter().rev().take(4).rev().map(|l| match l {
            Line::You(t) => div().flex().gap_1p5().text_size(px(12.0)).child(div().text_color(c.subtle).child("You")).child(div().flex_1().min_w_0().text_color(c.fg).whitespace_normal().child(t.clone())).into_any_element(),
            Line::Assistant(t) => div().flex().gap_1p5().text_size(px(12.0)).child(div().text_color(c.accent).child("winfer")).child(div().flex_1().min_w_0().text_color(c.fg).whitespace_normal().child(t.clone())).into_any_element(),
            Line::Tool(t) => div().text_size(px(11.0)).text_color(c.subtle).child(format!("⚙ {t}")).into_any_element(),
        }).collect();
        let icon_button = |id: &'static str, icon: IconName, on: bool| div().id(id).p_1p5().rounded_md().cursor_pointer().text_color(if on { c.warning } else { c.muted }).hover(move |d| d.bg(c.hover).text_color(c.fg)).child(Icon::new(icon).size(px(15.0)));
        Some(
            div().absolute().bottom(px(16.0)).left_0().right_0().flex().justify_center()
                .child(
                    div().id("voice-hud").w(px(380.0)).flex().flex_col().gap_1p5().rounded_xl().border_1().border_color(c.edge).bg(c.raised).shadow_lg().px_3().py_2()
                        .child(div().flex().items_center().gap_2()
                            .child(Icon::new(IconName::AudioLines).size(px(15.0)).text_color(color))
                            .child(div().flex_1().text_size(px(12.5)).text_color(c.fg).child(status))
                            .child(div().w(px(46.0)).h(px(4.0)).rounded_full().bg(c.border).child(div().h_full().w(relative(level)).rounded_full().bg(c.success)))
                            .child(icon_button("voice-mute", if muted { IconName::MicOff } else { IconName::Mic }, muted).on_click(cx.listener(|this, _, _, cx| {
                                this.voice.muted = !this.voice.muted;
                                if let Some(h) = &this.voice.handle {
                                    h.send(Down::Mute(this.voice.muted));
                                }
                                cx.notify();
                            })))
                            .child(icon_button("voice-stop", IconName::X, false).on_click(cx.listener(|this, _, _, cx| this.stop_voice(cx)))))
                        .when(!lines.is_empty(), |d| d.child(div().flex().flex_col().gap_0p5().children(lines))),
                )
                .into_any_element(),
        )
    }
}
