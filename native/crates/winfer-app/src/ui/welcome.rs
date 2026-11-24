//! The empty workspace (Graphite "Welcome"): a task box with backend, model and effort pickers,
//! the installed agents to switch between, and a few next steps.

use super::commands::Command;
use super::dialogs::ModelInfo as ProviderModelInfo;
use super::*;
use crate::code::names::{backend_of, model_name};
use crate::model::AddPane;
use crate::ui::code_pickers::{effort_glyph, effort_hint, effort_label, effort_levels, mark};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::tooltip::Tooltip;
use winfer_backend::{harness, providers};

#[derive(Clone, Copy, PartialEq, Eq)]
enum WelcomePicker {
    Model,
    Effort,
}

#[derive(Clone, Debug)]
struct WelcomeModel {
    /// Value passed to `pane.model` (`"agy/gemini-4-neon"` for winfer, `"gemini-4-neon"` for agy).
    id: String,
    /// Display label.
    label: String,
    /// Optional backend key for icon display (used for `winfer` code which spans multiple backends).
    backend: Option<String>,
    /// Supported reasoning efforts for this model (`Some([])` = none, `None` = backend default).
    efforts: Option<Vec<String>>,
}

#[derive(Default)]
pub(super) struct WelcomeUi {
    input: Option<Entity<InputState>>,
    search: Option<Entity<InputState>>,
    /// Selected backend provider id (`None` = workspace/app default provider).
    provider: Option<String>,
    /// Selected model for the active backend (`None` = backend default).
    model: Option<String>,
    /// Selected effort level (`None` = model default).
    effort: Option<String>,
    open: Option<WelcomePicker>,
    /// Cached `winfer code` / harness model catalog (with per-model `efforts`).
    pub(super) harness_models: Option<Vec<crate::code::protocol::ModelInfo>>,
    loading_harness: bool,
    /// Providers currently fetching `providers_models`.
    loading_providers: std::collections::HashSet<String>,
    /// Saved harness default model & effort, reread when the harness config changes (chats save their last model there).
    pub(super) harness_defaults: Option<(Option<String>, Option<String>)>,
    harness_defaults_at: Option<std::time::SystemTime>,
    /// Images pasted into the task box (with their decoded thumbnails); winfer code sends them with the first message.
    attachments: Vec<(crate::code::state::Attachment, std::sync::Arc<Image>)>,
}

pub(super) const HARNESS_BACKENDS: [&str; 7] = ["claude", "codex", "agy", "cursor", "opencode", "pi", "muse"];

/// The model a new pane of `provider` starts with when none is picked: the agent's saved model
/// (what `AppState::add_pane` passes as `--model`), else for winfer code its own config default.
pub(super) fn default_model(provider: &str, agent_models: &std::collections::HashMap<String, String>, harness_model: Option<&str>) -> Option<String> {
    agent_models.get(provider).cloned().or_else(|| (provider == "winfer").then(|| harness_model.map(str::to_string)).flatten())
}

pub(super) fn backend_default_efforts(provider: &str) -> Option<Vec<String>> {
    match provider {
        "winfer" => Some(["low", "medium", "high", "xhigh", "max"].map(String::from).into()),
        "claude" => Some(["low", "medium", "high", "xhigh", "max"].map(String::from).into()),
        "codex" => Some(["low", "medium", "high", "xhigh"].map(String::from).into()),
        "muse" => Some(["low", "medium", "high"].map(String::from).into()),
        "pi" => Some(["minimal", "low", "medium", "high", "xhigh"].map(String::from).into()),
        _ => None,
    }
}

impl Root {
    pub(super) fn close_welcome_picker(&mut self, cx: &mut Context<Self>) -> bool {
        if self.welcome.open.take().is_some() {
            cx.notify();
            true
        } else {
            false
        }
    }

    fn welcome_input(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Entity<InputState> {
        if let Some(input) = &self.welcome.input {
            return input.clone();
        }
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Describe a task — Enter starts an agent on it"));
        cx.subscribe_in(&input, window, |this, _, event: &InputEvent, window, cx| match event {
            InputEvent::PressEnter { .. } => this.start_welcome_task(window, cx),
            InputEvent::Change => cx.notify(),
            _ => {}
        })
        .detach();
        self.welcome.input = Some(input.clone());
        input
    }

    fn welcome_search(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Entity<InputState> {
        if let Some(s) = &self.welcome.search {
            return s.clone();
        }
        let s = cx.new(|cx| InputState::new(window, cx).placeholder("Filter models…"));
        cx.subscribe_in(&s, window, |_, _, event: &InputEvent, _, cx| {
            if matches!(event, InputEvent::Change) {
                cx.notify();
            }
        })
        .detach();
        self.welcome.search = Some(s.clone());
        s
    }

    fn ensure_welcome_models(&mut self, provider: &str, cx: &mut Context<Self>) {
        let at = std::fs::metadata(harness::config::dir().join("config.json")).and_then(|m| m.modified()).ok();
        if self.welcome.harness_defaults.is_none() || at != self.welcome.harness_defaults_at {
            let cfg = harness::config::Config::load();
            self.welcome.harness_defaults = Some(((!cfg.model.is_empty()).then_some(cfg.model), cfg.effort));
            self.welcome.harness_defaults_at = at;
        }
        if (provider == "winfer" || HARNESS_BACKENDS.contains(&provider)) && self.welcome.harness_models.is_none() && !self.welcome.loading_harness {
            self.welcome.loading_harness = true;
            cx.spawn(async move |this, cx| {
                let list = tauri::async_runtime::spawn_blocking(harness::backend::all_models).await.unwrap_or_default();
                let mapped: Vec<crate::code::protocol::ModelInfo> = list
                    .into_iter()
                    .map(|m| crate::code::protocol::ModelInfo { spec: m.spec, label: m.label, detail: m.detail, efforts: m.efforts })
                    .collect();
                let _ = this.update(cx, |this, cx| {
                    this.welcome.loading_harness = false;
                    this.welcome.harness_models = Some(mapped);
                    cx.notify();
                });
            })
            .detach();
        }
        let has_model_arg = self.state.read(cx).providers.iter().any(|p| p.id == provider && p.model_arg.is_some());
        if provider != "winfer" && has_model_arg && !self.models.contains_key(provider) && !self.welcome.loading_providers.contains(provider) {
            let id = provider.to_string();
            self.welcome.loading_providers.insert(id.clone());
            cx.spawn(async move |this, cx| {
                let p = id.clone();
                let found = tauri::async_runtime::spawn(async move { providers::providers_models(p).await.ok().and_then(|m| serde_json::to_value(m).ok()) }).await.ok().flatten();
                let list: Vec<ProviderModelInfo> = found.and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default();
                let _ = this.update(cx, |this, cx| {
                    this.welcome.loading_providers.remove(&id);
                    this.models.insert(id, list);
                    cx.notify();
                });
            })
            .detach();
        }
    }

    fn welcome_provider(&self, cx: &App) -> String {
        let s = self.state.read(cx);
        if let Some(p) = &self.welcome.provider {
            if s.providers.iter().any(|x| x.id == *p && x.installed) {
                return p.clone();
            }
        }
        if s.providers.iter().any(|x| x.id == "winfer" && x.installed) {
            return "winfer".to_string();
        }
        s.default_provider_id()
    }

    fn welcome_models_for(&self, provider: &str) -> (Vec<WelcomeModel>, bool) {
        if provider == "winfer" {
            let loading = self.welcome.harness_models.is_none();
            let list = self
                .welcome
                .harness_models
                .as_deref()
                .unwrap_or(&[])
                .iter()
                .map(|m| WelcomeModel {
                    id: m.spec.clone(),
                    label: model_name(&m.spec, None),
                    backend: backend_of(&m.spec).map(|b| b.0.to_string()),
                    efforts: m.efforts.clone(),
                })
                .collect();
            return (list, loading);
        }
        if HARNESS_BACKENDS.contains(&provider) {
            let prefix = format!("{provider}/");
            if let Some(hm) = &self.welcome.harness_models {
                let matched: Vec<WelcomeModel> = hm
                    .iter()
                    .filter_map(|m| {
                        let slug = m.spec.strip_prefix(&prefix)?;
                        Some(WelcomeModel { id: slug.to_string(), label: m.label.clone(), backend: None, efforts: m.efforts.clone() })
                    })
                    .collect();
                if !matched.is_empty() {
                    return (matched, false);
                }
            }
        }
        let loading = (HARNESS_BACKENDS.contains(&provider) && self.welcome.harness_models.is_none()) || self.welcome.loading_providers.contains(provider);
        let list = self
            .models
            .get(provider)
            .map(|l| l.iter().map(|m| WelcomeModel { id: m.id.clone(), label: m.label.clone(), backend: None, efforts: None }).collect())
            .unwrap_or_default();
        (list, loading)
    }

    fn welcome_efforts_for(&self, provider: &str, models: &[WelcomeModel], selected_model: Option<&str>) -> Vec<String> {
        let lookup = selected_model.or_else(|| {
            if provider == "winfer" {
                self.welcome.harness_defaults.as_ref().and_then(|(m, _)| m.as_deref())
            } else {
                None
            }
        });
        if let Some(sel) = lookup {
            if let Some(found) = models.iter().find(|m| m.id == sel) {
                return match &found.efforts {
                    Some(v) if v.is_empty() => Vec::new(),
                    Some(v) => effort_levels(Some(v.clone())),
                    None => match backend_default_efforts(provider) {
                        Some(v) => effort_levels(Some(v)),
                        None => Vec::new(),
                    },
                };
            }
        }
        // No explicit model picked: offer efforts if the backend supports a standalone effort flag.
        match backend_default_efforts(provider) {
            Some(v) => effort_levels(Some(v)),
            None => Vec::new(),
        }
    }

    /// True while the empty-workspace welcome screen is what's showing (no dialog or panel on top).
    pub(super) fn welcome_showing(&self, cx: &App) -> bool {
        let s = self.state.read(cx);
        matches!(s.view(), crate::model::View::Workspace)
            && s.active().is_some_and(|w| w.layout.is_none())
            && !s.settings_open
            && s.overlay == crate::model::Overlay::None
            && self.palette_input.is_none()
            && self.questions.is_empty()
            && self.new_pane.is_none()
            && self.model_dialog.is_none()
            && self.diff_dialog.is_none()
            && self.search.is_none()
            && self.history.is_none()
    }

    /// Cmd/Ctrl+V anywhere on the welcome screen: attaches clipboard images even when the task box isn't focused.
    /// Returns `true` if the keystroke was consumed.
    pub(super) fn paste_welcome_images_from_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) -> bool {
        let m = event.keystroke.modifiers;
        let primary = if cfg!(target_os = "macos") { m.platform && !m.control } else { m.control && !m.platform };
        primary && !m.alt && !m.shift && event.keystroke.key.eq_ignore_ascii_case("v") && self.welcome_showing(cx) && self.paste_welcome_images(cx)
    }

    /// Attaches clipboard images to the task box. Returns `true` if any were attached (so the text isn't pasted too).
    pub(super) fn paste_welcome_images(&mut self, cx: &mut Context<Self>) -> bool {
        if self.welcome_provider(cx) != "winfer" {
            return false;
        }
        let Some(item) = cx.read_from_clipboard() else { return false };
        let mut added = false;
        for entry in &item.entries {
            let ClipboardEntry::Image(img) = entry else { continue };
            let count = self.welcome.attachments.len();
            let Some(att) = crate::ui::code_view::attachment_from_clipboard_image(img, count) else { continue };
            let Some(thumb) = crate::ui::code_view::data_image(&format!("data:{};base64,{}", att.mime, att.data)) else { continue };
            self.welcome.attachments.push((att, thumb));
            added = true;
        }
        if added {
            cx.notify();
        }
        added
    }

    /// Attaches image files dropped onto the task box (non-images are ignored).
    fn drop_welcome_images(&mut self, paths: &[std::path::PathBuf], cx: &mut Context<Self>) {
        for path in paths {
            let Some(att) = crate::ui::code_view::attachment_from_path(path) else { continue };
            if !att.mime.starts_with("image/") {
                continue;
            }
            let Some(thumb) = crate::ui::code_view::data_image(&format!("data:{};base64,{}", att.mime, att.data)) else { continue };
            self.welcome.attachments.push((att, thumb));
        }
        cx.notify();
    }

    /// Starts the selected backend on what's typed in the task box with the chosen model and effort.
    fn start_welcome_task(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let provider = self.welcome_provider(cx);
        let task = self.welcome.input.as_ref().map(|i| i.read(cx).value().trim().to_string()).unwrap_or_default();
        let model = self.welcome.model.clone();
        let effort = self.welcome.effort.clone().filter(|e| !e.is_empty() && e != "default");
        self.welcome.open = None;
        if let Some(input) = self.welcome.input.clone() {
            input.update(cx, |i, cx| i.set_value("", window, cx));
        }
        let prompt = (!task.is_empty()).then_some(task);
        // Only winfer code sessions can take images with their first message.
        let attachments: Vec<_> = std::mem::take(&mut self.welcome.attachments).into_iter().map(|(a, _)| a).filter(|_| provider == "winfer").collect();
        let name = self.state.read(cx).provider_name(&provider);
        self.update_state(cx, |s| {
            s.add_pane(&provider, AddPane { prompt, attachments, model, effort, ..Default::default() });
            let ws = s.active_workspace_id.clone();
            s.log_activity(&ws, format!("Opened {name}"), Some(provider.clone()));
        });
    }

    pub(super) fn render_welcome(&mut self, c: Colors, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let input = self.welcome_input(window, cx);
        let selected_provider = self.welcome_provider(cx);
        self.ensure_welcome_models(&selected_provider, cx);
        let (models, models_loading) = self.welcome_models_for(&selected_provider);
        let s = self.state.read(cx);
        let workspace = s.active().map(|w| w.name.clone()).unwrap_or_else(|| "this workspace".into());
        let selected_name = s.provider_name(&selected_provider);
        let provider_supports_model = s.providers.iter().any(|p| p.id == selected_provider && p.model_arg.is_some());
        let harness_model = self.welcome.harness_defaults.as_ref().and_then(|(m, _)| m.as_deref());
        let saved_agent_model = default_model(&selected_provider, &s.agent_models, harness_model);
        let saved_effort = if selected_provider == "winfer" {
            self.welcome.harness_defaults.as_ref().and_then(|(_, e)| e.clone())
        } else {
            None
        };
        let agents: Vec<(String, String)> = s.providers.iter().filter(|p| p.id != "shell" && p.installed).map(|p| (p.id.clone(), p.name.clone())).collect();
        let missing = s.providers.iter().filter(|p| p.id != "shell" && !p.installed).count();

        let effective_model = self.welcome.model.as_deref().or(saved_agent_model.as_deref());
        let model_label = match effective_model {
            Some(id) => models.iter().find(|m| m.id == id).map(|m| m.label.clone()).unwrap_or_else(|| {
                if selected_provider == "winfer" { model_name(id, None) } else { id.to_string() }
            }),
            None => "Default model".to_string(),
        };
        let model_icon_provider = if selected_provider == "winfer" {
            effective_model.and_then(backend_of).map(|b| b.0).unwrap_or("winfer")
        } else {
            selected_provider.as_str()
        };
        let efforts = self.welcome_efforts_for(&selected_provider, &models, effective_model);
        let active_effort = self
            .welcome
            .effort
            .as_deref()
            .or(saved_effort.as_deref())
            .filter(|e| !e.is_empty() && efforts.iter().any(|x| x == e))
            .unwrap_or("default")
            .to_string();

        let show_model_picker = provider_supports_model || !models.is_empty();
        let show_effort_picker = !efforts.is_empty();
        let open = self.welcome.open;

        // Model popover dropdown
        let model_popover = (open == Some(WelcomePicker::Model)).then(|| {
            let show_search = models.len() > 8;
            let search = show_search.then(|| self.welcome_search(window, cx));
            let query = search.as_ref().map(|s| s.read(cx).value().trim().to_lowercase()).unwrap_or_default();
            let filtered: Vec<&WelcomeModel> = models
                .iter()
                .filter(|m| query.is_empty() || m.label.to_lowercase().contains(&query) || m.id.to_lowercase().contains(&query))
                .take(80)
                .collect();
            let is_default = self.welcome.model.is_none();
            let default_desc = saved_agent_model
                .as_deref()
                .map(|m| format!("Default ({})", if selected_provider == "winfer" { model_name(m, None) } else { m.to_string() }))
                .unwrap_or_else(|| "Default model".to_string());
            let mut list = div().id("welcome-model-list").flex().flex_col().gap_px().max_h(px(240.0)).overflow_y_scroll().p_1p5();
            list = list.child(
                div().id("wm-default").flex().items_center().gap_2().h(px(30.0)).px_2p5().rounded_md().cursor_pointer().text_size(px(12.5))
                    .when(is_default, |d| d.bg(Hsla { a: 0.14, ..c.accent }).text_color(c.fg))
                    .when(!is_default, |d| d.text_color(c.muted).hover(move |d| d.bg(c.hover).text_color(c.fg)))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.welcome.model = None;
                        this.welcome.effort = None;
                        this.welcome.open = None;
                        cx.notify();
                    }))
                    .child(div().flex_1().truncate().child(default_desc))
                    .when(is_default, |d| d.child(Icon::new(IconName::Check).size(px(13.0)).text_color(c.accent))),
            );
            if models_loading && filtered.is_empty() {
                list = list.child(div().px_2p5().py_2().text_size(px(11.5)).text_color(c.subtle).child(format!("Loading {selected_name} models…")));
            }
            for (i, m) in filtered.into_iter().enumerate() {
                let on = self.welcome.model.as_deref() == Some(m.id.as_str());
                let pick_id = m.id.clone();
                let pick_efforts = m.efforts.clone();
                let icon_id = m.backend.as_deref().unwrap_or(&selected_provider).to_string();
                let show_icon = m.backend.is_some();
                list = list.child(
                    div().id(("wm-opt", i)).flex().items_center().gap_2().h(px(30.0)).px_2p5().rounded_md().cursor_pointer().text_size(px(12.5))
                        .when(on, |d| d.bg(Hsla { a: 0.14, ..c.accent }).text_color(c.fg))
                        .when(!on, |d| d.text_color(c.muted).hover(move |d| d.bg(c.hover).text_color(c.fg)))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.welcome.model = Some(pick_id.clone());
                            // Drop effort if the newly picked model doesn't support it.
                            if let Some(cur) = &this.welcome.effort {
                                let keep = match &pick_efforts {
                                    Some(v) => v.iter().any(|x| x == cur),
                                    None => true,
                                };
                                if !keep {
                                    this.welcome.effort = None;
                                }
                            }
                            this.welcome.open = None;
                            cx.notify();
                        }))
                        .when(show_icon, |d| d.child(mark(c, &icon_id, 13.0, 1.0)))
                        .child(div().flex_1().min_w_0().truncate().child(m.label.clone()))
                        .when(on, |d| d.child(Icon::new(IconName::Check).size(px(13.0)).text_color(c.accent))),
                );
            }
            let panel = div().id("welcome-model-pop").w(px(290.0)).rounded_xl().bg(c.raised).shadow_lg().occlude()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .when_some(search, |d, s| {
                    d.child(div().flex().items_center().gap_2().px_2p5().py_1p5()
                        .child(Icon::new(IconName::Search).size(px(12.5)).text_color(c.subtle))
                        .child(div().flex_1().min_w_0().child(Input::new(&s).appearance(false))))
                })
                .child(list);
            deferred(anchored().snap_to_window_with_margin(px(8.0)).child(div().pt_1p5().child(panel))).with_priority(10)
        });

        // Effort popover dropdown
        let effort_popover = (open == Some(WelcomePicker::Effort) && show_effort_picker).then(|| {
            let mut list = div().flex().flex_col().gap_px().p_1p5();
            for (i, lvl) in efforts.iter().enumerate() {
                let on = active_effort == *lvl;
                let pick = lvl.clone();
                let hint = effort_hint(lvl);
                list = list.child(
                    div().id(("we-opt", i)).flex().items_center().gap_2().h(px(32.0)).px_2p5().rounded_md().cursor_pointer().text_size(px(12.5))
                        .when(on, |d| d.bg(Hsla { a: 0.14, ..c.accent }).text_color(c.fg))
                        .when(!on, |d| d.text_color(c.muted).hover(move |d| d.bg(c.hover).text_color(c.fg)))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.welcome.effort = if pick == "default" { None } else { Some(pick.clone()) };
                            this.welcome.open = None;
                            cx.notify();
                        }))
                        .child(effort_glyph(if on { c.accent } else { c.muted }, lvl, 15.0))
                        .child(div().flex_1().min_w_0().flex().items_center().gap_1p5()
                            .child(effort_label(lvl))
                            .when(!hint.is_empty(), |d| d.child(div().truncate().text_size(px(10.5)).text_color(c.subtle).child(hint))))
                        .when(on, |d| d.child(Icon::new(IconName::Check).size(px(13.0)).text_color(c.accent))),
                );
            }
            let panel = div().id("welcome-effort-pop").w(px(260.0)).rounded_xl().bg(c.raised).shadow_lg().occlude()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(list);
            deferred(anchored().snap_to_window_with_margin(px(8.0)).child(div().pt_1p5().child(panel))).with_priority(10)
        });

        let picker_row = (show_model_picker || show_effort_picker).then(|| {
            div().flex().items_center().gap_1p5().px_2p5().pt_1p5().pb(px(1.0))
                .when(show_model_picker, |d| {
                    d.child(
                        div().relative()
                            .child(
                                div().id("welcome-model-btn").flex().items_center().gap_1p5().h(px(26.0)).px_2().rounded_md().cursor_pointer().text_size(px(12.0))
                                    .text_color(if open == Some(WelcomePicker::Model) { c.fg } else { c.muted })
                                    .when(open == Some(WelcomePicker::Model), |d| d.bg(c.hover))
                                    .hover(move |d| d.bg(c.hover).text_color(c.fg))
                                    .tooltip(move |window, cx| Tooltip::new("Choose model for this agent").build(window, cx))
                                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.welcome.open = if this.welcome.open == Some(WelcomePicker::Model) { None } else { Some(WelcomePicker::Model) };
                                        if this.welcome.open == Some(WelcomePicker::Model) {
                                            if let Some(s) = &this.welcome.search {
                                                s.update(cx, |st, cx| st.set_value("", window, cx));
                                            }
                                        }
                                        cx.notify();
                                    }))
                                    .child(mark(c, model_icon_provider, 13.0, 1.0))
                                    .child(div().max_w(px(200.0)).truncate().child(model_label.clone()))
                                    .child(Icon::new(IconName::ChevronDown).size(px(11.0)).text_color(c.subtle)),
                            )
                            .children(model_popover),
                    )
                })
                .when(show_effort_picker, |d| {
                    let eff_tip = format!("Reasoning effort: {}", effort_label(&active_effort));
                    d.child(
                        div().relative()
                            .child(
                                div().id("welcome-effort-btn").flex().items_center().gap_1p5().h(px(26.0)).px_2().rounded_md().cursor_pointer().text_size(px(12.0))
                                    .text_color(if open == Some(WelcomePicker::Effort) { c.fg } else { c.muted })
                                    .when(open == Some(WelcomePicker::Effort), |d| d.bg(c.hover))
                                    .hover(move |d| d.bg(c.hover).text_color(c.fg))
                                    .tooltip(move |window, cx| Tooltip::new(eff_tip.clone()).build(window, cx))
                                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.welcome.open = if this.welcome.open == Some(WelcomePicker::Effort) { None } else { Some(WelcomePicker::Effort) };
                                        cx.notify();
                                    }))
                                    .child(effort_glyph(c.muted, &active_effort, 14.0))
                                    .child(effort_label(&active_effort))
                                    .child(Icon::new(IconName::ChevronDown).size(px(11.0)).text_color(c.subtle)),
                            )
                            .children(effort_popover),
                    )
                })
        });

        let card_shadow = if c.light {
            vec![
                BoxShadow { color: hsla(0.0, 0.0, 0.0, 0.07), offset: point(px(0.0), px(2.0)), blur_radius: px(10.0), spread_radius: px(-2.0), inset: false },
                BoxShadow { color: hsla(0.0, 0.0, 0.0, 0.04), offset: point(px(0.0), px(1.0)), blur_radius: px(3.0), spread_radius: px(0.0), inset: false },
            ]
        } else {
            vec![
                BoxShadow { color: hsla(0.0, 0.0, 0.0, 0.36), offset: point(px(0.0), px(4.0)), blur_radius: px(14.0), spread_radius: px(-2.0), inset: false },
                BoxShadow { color: hsla(0.0, 0.0, 0.0, 0.22), offset: point(px(0.0), px(1.0)), blur_radius: px(3.0), spread_radius: px(0.0), inset: false },
            ]
        };

        let thumbs = (selected_provider == "winfer" && !self.welcome.attachments.is_empty()).then(|| {
            let mut row = div().flex().flex_wrap().gap_2().px_3p5().pt_2p5();
            for (i, (a, image)) in self.welcome.attachments.iter().enumerate() {
                row = row.child(
                    div().id(("welcome-att", i)).relative().size(px(48.0)).flex_shrink_0().rounded_md().overflow_hidden().bg(c.hover).border_1().border_color(c.border)
                        .tooltip({
                            let name = a.name.clone();
                            move |window, cx| Tooltip::new(name.clone()).build(window, cx)
                        })
                        .child(gpui_kit::img(image.clone()).size(px(48.0)).object_fit(ObjectFit::Cover))
                        .child(
                            div().id(("welcome-att-x", i)).absolute().top(px(2.0)).right(px(2.0)).size(px(16.0)).flex().items_center().justify_center().rounded_full()
                                .bg(Hsla { a: 0.85, ..c.panel }).text_color(c.fg).cursor_pointer().hover(move |d| d.bg(c.panel))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    if i < this.welcome.attachments.len() {
                                        this.welcome.attachments.remove(i);
                                    }
                                    cx.notify();
                                }))
                                .child(Icon::new(IconName::X).size(px(10.0))),
                        ),
                );
            }
            row
        });

        let animate_send = self.state.read(cx).rest.get("codeWeb").and_then(|v| v["sendAnimation"].as_bool()).unwrap_or(true);
        let has_task = !input.read(cx).value().trim().is_empty() || (selected_provider == "winfer" && !self.welcome.attachments.is_empty());
        let start_btn = crate::ui::code_view::nebula_send(c, false, has_task, animate_send, IconName::ArrowUp, 32.0)
            .tooltip(move |window, cx| Tooltip::new("Start (Enter)").build(window, cx))
            .on_click(cx.listener(|this, _, window, cx| this.start_welcome_task(window, cx)));

        let task = div().id("welcome-task").w_full().flex().flex_col().rounded_xl().bg(c.raised).shadow(card_shadow.clone()).border_1().border_color(c.edge)
            .when(selected_provider == "winfer", |d| {
                d.drag_over::<ExternalPaths>(move |d, _, _, _| d.border_color(c.accent))
                    .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| {
                        this.drop_welcome_images(paths.paths(), cx);
                        cx.stop_propagation();
                    }))
            })
            .children(thumbs)
            .child(
                div().w_full().flex().items_center().gap_2().h(px(46.0)).px_3p5()
                    .child(mark(c, &selected_provider, 16.0, 1.0))
                    .child(div().flex_1().min_w_0().child(Input::new(&input).appearance(false))),
            )
            .child(
                // Bottom bar: pickers on the left, send button in the bottom-right corner.
                div().w_full().flex().items_end().pb_1p5().pr_1p5()
                    .child(div().flex_1().min_w_0().children(picker_row))
                    .child(start_btn),
            );

        let mut picks = div().flex().items_stretch().justify_center().gap_2p5().w_full().pt_2();
        let mut others = div().flex().flex_wrap().items_stretch().justify_center().gap_2();
        let has_others = agents.iter().any(|(id, _)| id != "winfer");
        for (id, name) in agents {
            let on = id == selected_provider;
            let big = id == "winfer";
            let pick = id.clone();
            let btn = div().id(SharedString::from(format!("welcome-{id}"))).flex().items_center().justify_center().gap_2().h(px(36.0)).rounded_lg().font_weight(FontWeight::MEDIUM)
                .when(big, |d| d.min_w(px(136.0)).px_4().pt(px(2.0)).text_size(px(13.5)))
                .when(!big, |d| d.min_w(px(80.0)).px_3().text_size(px(13.0)))
                .border_1()
                .when(on, |d| d.bg(Hsla { a: 0.14, ..c.accent }).border_color(Hsla { a: 0.4, ..c.accent }).text_color(c.fg))
                .when(!on, |d| d.bg(c.raised).border_color(c.edge).text_color(c.muted).hover(move |d| d.bg(c.hover).text_color(c.fg)))
                .cursor_pointer()
                .on_click(cx.listener(move |this, e: &ClickEvent, window, cx| {
                    if this.welcome.provider.as_deref() != Some(pick.as_str()) {
                        this.welcome.provider = Some(pick.clone());
                        this.welcome.model = None;
                        this.welcome.effort = None;
                        this.welcome.open = None;
                        this.ensure_welcome_models(&pick, cx);
                        cx.notify();
                    }
                    if e.click_count() == 2 {
                        this.start_welcome_task(window, cx);
                    }
                }))
                .child(mark(c, &id, if big { 18.0 } else { 15.0 }, 1.0))
                .child(if big { "WinferCode".to_string() } else { name });
            if big {
                // Paint the badge as a sibling after `btn` so `btn`'s border doesn't draw over it.
                let wrapped = div()
                    .relative()
                    .child(btn)
                    .child(
                        div()
                            .absolute()
                            .top(px(-9.0))
                            .left_0()
                            .right_0()
                            .flex()
                            .justify_center()
                            .child(crate::ui::code_pickers::recommended_badge(c, true)),
                    );
                picks = picks.child(wrapped);
                if has_others {
                    picks = picks.child(
                        // A full-height line with "or" sitting on top of it (panel-coloured
                        // backing), so the line shows above and below the text.
                        div().relative().h(px(36.0)).w(px(24.0)).flex().items_center().justify_center().text_size(px(12.0)).text_color(c.subtle)
                            .child(div().absolute().top_0().bottom_0().left(px(11.5)).w(px(1.0)).bg(c.edge))
                            .child(div().relative().bg(c.panel).py(px(1.0)).child("or")),
                    );
                }
            } else {
                others = others.child(btn);
            }
        }
        picks = picks.child(others);
        let more = (missing > 0).then(|| div().id("welcome-more-agents").flex().items_center().text_size(px(12.0)).text_color(c.subtle).cursor_pointer().hover(move |d| d.text_color(c.fg))
            .on_click(cx.listener(|this, _, window, cx| {
                this.execute(Command::Settings(Some("agents".into())), window, cx);
                this.scroll_settings_to_install(cx);
            }))
            .child(format!("+ {missing} more agents you can install")));

        div().size_full().flex().items_center().justify_center().rounded_lg().bg(c.panel)
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                if this.welcome.open.take().is_some() {
                    cx.notify();
                }
            }))
            .child(
                div().w(px(620.0)).max_w(relative(0.92)).flex().flex_col().items_center().gap_5()
                    .child(div().flex().flex_col().items_center().gap_1()
                        .child(div().text_size(px(20.0)).font_weight(FontWeight::SEMIBOLD).text_color(c.fg).child(format!("What should agents do in {workspace}?")))
                        .child(div().text_size(px(13.0)).text_color(c.muted).child("One line starts an agent in its own worktree. Or pick an agent below.")))
                    .child(task)
                    .child(picks)
                    .children(more),
            )
    }
}
