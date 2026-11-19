//! The settings window (port of `src/features/settings/SettingsModal.tsx`).

use super::commands::{BINDINGS, Binding, chord_for, event_chord, format_chord};
use super::*;
use crate::model::OPTIONAL_TOOLS;
use crate::theme::THEMES;
use gpui_kit::component::input::{Editor, EditorState, Input, InputEvent, InputState};
use serde_json::json;
use winfer_backend::{diagnostics, editors, provider_updates, providers, transfer};

/// A settings page: id, label, icon and a one-line description for its header.
type Page = (&'static str, &'static str, IconName, &'static str);

/// The pages in five groups (the Graphite settings).
const GROUPS: [(&str, &[Page]); 5] = [
    ("Workspace", &[
        ("general", "General", IconName::SlidersHorizontal, "Terminals, your code editor and keyboard shortcuts."),
        ("theme", "Appearance", IconName::Palette, "Light or dark, and the theme for each."),
    ]),
    ("Agents", &[
        ("agents", "Agents", IconName::Bot, "New panes, models, versions, permissions, review and progress."),
        ("winfer-code", "winfer code", IconName::SquareTerminal, "winfer's own agent: interface, model and context."),
    ]),
    ("AI & voice", &[
        ("accounts", "Accounts", IconName::Users, "Sign-ins and the accounts winfer switches between."),
        ("ai", "AI", IconName::Sparkles, "The model winfer uses for summaries, titles and progress."),
        ("voice", "Voice", IconName::Mic, "Talk to your agents, and have them talk back."),
    ]),
    ("Extensions", &[
        ("plugins", "Plugins", IconName::Puzzle, "Panels, tools and agents others built for winfer."),
        ("skills", "Skills", IconName::WandSparkles, "Reusable instructions every agent can load."),
    ]),
    ("System", &[
        ("usage", "Usage & limits", IconName::ChartColumn, "Plan limits, spend and warnings."),
        ("mobile", "Mobile", IconName::Smartphone, "Watch and steer agents from your phone."),
        ("data", "Backup & transfer", IconName::ArchiveRestore, "Move your setup to another machine."),
        ("updates", "Updates", IconName::Download, "winfer's release channel and updates."),
        ("diagnostics", "Diagnostics", IconName::Activity, "Memory, logs and crash reports."),
    ]),
];

fn page_info(id: &str) -> (&'static str, Page) {
    GROUPS.iter().find_map(|(g, pages)| pages.iter().find(|p| p.0 == id).map(|p| (*g, *p))).unwrap_or((GROUPS[0].0, GROUPS[0].1[0]))
}

/// What the search finds: page, setting and words that lead to it (the setting's hint or synonyms).
const SEARCH_INDEX: &[(&str, &str, &str)] = &[
    ("winfer-code", "Use TUI style", ""),
    ("winfer-code", "Text size", ""),
    ("winfer-code", "Model", ""),
    ("winfer-code", "Reasoning effort", ""),
    ("winfer-code", "Permission mode", ""),
    ("winfer-code", "Auto-approve in panes", ""),
    ("winfer-code", "Plan sidebar", ""),
    ("winfer-code", "Lean setup", ""),
    ("winfer-code", "Short Codex prompt", ""),
    ("winfer-code", "Codex CLI panes too", ""),
    ("winfer-code", "Skills", ""),
    ("winfer-code", "Context compaction", ""),
    ("winfer-code", "Micro-compact at", ""),
    ("winfer-code", "Full compact at", ""),
    ("winfer-code", "Tools", ""),
    ("winfer-code", "Always-allowed commands", ""),
    ("winfer-code", "MCP servers", "Extra servers for every backend, as JSON: { \\"),
    ("mobile", "Phone", ""),
    ("mobile", "Pair a phone", ""),
    ("mobile", "Relay", ""),
    ("mobile", "Relay URL", ""),
    ("general", "Terminals", ""),
    ("general", "Text size", "Also ⌘= / ⌘- / ⌘0, or ⌘ + scroll over a terminal."),
    ("general", "Code editor", ""),
    ("general", "Open projects and worktrees in", ""),
    ("general", "Keyboard", ""),
    ("general", "Keyboard shortcuts", "See every shortcut and change any of them."),
    ("agents", "New panes", ""),
    ("agents", "Default agent", "⌘T, split and “open a new window” start this agent."),
    ("agents", "Always open new agents in their own worktree", ""),
    ("agents", "Models", "The model each agent starts with in new panes."),
    ("agents", "Versions", "Updating changes the CLI on disk; running panes keep their version until restarted."),
    ("agents", "Check for agent updates", ""),
    ("agents", "Auto-approve", "Agents run tools and edits without asking. Only for sandboxes or work you can undo."),
    ("agents", "System prompt", "Given to every agent when it starts (or restarts). Leave empty for none."),
    ("agents", "Instructions for every agent", ""),
    ("agents", "Agent tools", "Tools agents rarely use are left out, since every tool costs context in each request."),
    ("agents", "Review", ""),
    ("agents", "Run tests when an agent finishes", ""),
    ("agents", "Screenshot the dev server", "Shows the agent's running app on its review card."),
    ("agents", "Session progress", "Uses the LLM under AI. Agents whose output hasn't changed aren't re-checked."),
    ("agents", "Estimate how far working agents are", ""),
    ("agents", "Check every", ""),
    ("data", "Export", "Saves settings, workspaces, plugins with their data, and usage history into one file to import on another machine."),
    ("data", "Include secrets", "ChatGPT sign-in, the voice OpenAI key and plugin vault values, encrypted with a passphrase."),
    ("data", "Export file", ""),
    ("data", "Import", "Replaces this machine's data with an export, then restarts."),
    ("data", "Import and restart", ""),
    ("diagnostics", "Memory", "winfer's own resident memory, sampled every 5 seconds into the log. A steady climb before a crash points at a leak; the timeline in the export shows it."),
    ("diagnostics", "Now / session peak", ""),
    ("diagnostics", "Terminals", "Each live pane also holds up to 2 MB of replay scrollback in the backend, plus its terminal buffer."),
    ("diagnostics", "Last 2 minutes", ""),
    ("diagnostics", "Logs", "winfer.log keeps the app log (rotated at 5 MB); crash.log holds panics with backtraces. Both live in the folder below."),
    ("diagnostics", "Log folder", ""),
    ("diagnostics", "Sizes", ""),
    ("diagnostics", "Verbose logging", "Also records debug lines (more detail, faster-growing log). Kept across restarts."),
    ("diagnostics", "Copy summary", "Version, memory and the crash-log tail — paste it into a bug report."),
    ("diagnostics", "Export for a bug report", "A zip with the summary, both logs and the memory timeline. No settings, chats or secrets."),
    ("diagnostics", "Clear logs", ""),
    ("diagnostics", "Latest crash", "The end of crash.log. Full backtraces are in the file and in the export."),
    ("voice", "Live voice", ""),
    ("voice", "Connection", ""),
    ("voice", "OpenAI API key", ""),
    ("voice", "Model (API key)", ""),
    ("voice", "System prompt", "How the voice agent behaves and talks. winfer adds the current panes, workspaces and browser tabs after it; changes apply right away, also mid-conversation."),
    ("voice", "Voice", "API key · ChatGPT plan. Applies to the next conversation."),
    ("voice", "Input", ""),
    ("voice", "Microphone", ""),
    ("voice", "Read aloud", ""),
    ("voice", "Speak agent updates aloud", "“Codex needs your input”, “Claude Code finished”."),
    ("ai", "LLM", "Writes progress estimates, review summaries and other short texts in winfer."),
    ("ai", "Provider", ""),
    ("ai", "Model", ""),
    ("ai", "ChatGPT account", "Runs the LLM below on your ChatGPT plan (Plus or Pro); no API key needed. Until you sign in, the Codex CLI's own login is used instead if Codex is installed. Set a weekly limit or disconnect winfer in ChatGPT's settings."),
    ("theme", "Light or dark", "theme mode sun moon appearance"),
    ("theme", "Themes", "Winfer Dark Light Tokyo Night Catppuccin Dracula Nord Gruvbox Solarized GitHub Rosé Pine One Dark"),
    ("accounts", "Claude account", "sign in plan login"),
    ("accounts", "ChatGPT account", "sign in plan login openai"),
    ("accounts", "Multiple accounts", "switch account when one runs out of usage"),
    ("plugins", "Discover plugins", "store catalog install"),
    ("plugins", "Installed plugins", "remove update"),
    ("plugins", "Hide names on plugin tabs", ""),
    ("skills", "Discover skills", "store catalog install"),
    ("skills", "Installed skills", ""),
    ("usage", "Usage limits", "5-hour weekly rings title bar"),
    ("usage", "Spend", "cost tokens API prices"),
    ("usage", "Show task cost on panes", "cost header"),
    ("updates", "Update channel", "Stable Nebula"),
    ("updates", "Check for updates", "install download version"),
    ("updates", "What's new", "release notes changelog"),
    ("agents", "Auto-approve", "permissions yolo approve without asking"),
    ("agents", "Versions", "update agent CLI"),
    ("agents", "Session progress", "estimate how far agents are"),
    ("winfer-code", "Permission mode", "approve ask yolo plan"),
];

/// Settings whose label, words or page match every word of `query`, best first.
fn search_settings(query: &str) -> Vec<(&'static str, &'static str, &'static str)> {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    if words.is_empty() {
        return Vec::new();
    }
    let pages = GROUPS.iter().flat_map(|(_, p)| p.iter()).map(|p| (p.0, p.1, p.3));
    let mut hits: Vec<(u8, (&str, &str, &str))> = pages.map(|(id, label, desc)| (id, label, desc))
        .chain(SEARCH_INDEX.iter().copied())
        .filter_map(|(page, label, words_of)| {
            let page_label = page_info(page).1.1;
            let hay = format!("{label} {words_of} {page_label}").to_lowercase();
            words.iter().all(|w| hay.contains(w.as_str())).then(|| (if label.to_lowercase().starts_with(&words[0]) { 0 } else { 1 }, (page, label, words_of)))
        })
        .collect();
    hits.sort_by_key(|h| h.0);
    let mut seen = std::collections::HashSet::new();
    hits.into_iter().map(|h| h.1).filter(|(page, label, _)| seen.insert((*page, *label))).collect()
}

pub struct SettingsUi {
    open_select: Option<String>,
    system_prompt: Entity<EditorState>,
    editors: Vec<(String, String)>,
    versions: HashMap<String, Value>,
    checking: bool,
    updating: Vec<String>,
    with_secrets: bool,
    passphrase: Entity<InputState>,
    confirm: Entity<InputState>,
    import_passphrase: Entity<InputState>,
    importing: Option<(String, Value)>,
    export_result: Option<Result<String, String>>,
    import_result: Option<Result<String, String>>,
    busy: bool,
    diagnostics: Option<Value>,
    verbose: bool,
    diagnostics_result: Option<Result<String, String>>,
    diagnostics_polled: Option<std::time::Instant>,
    search: Entity<InputState>,
}

fn mb(bytes: f64) -> String {
    format!("{:.0} MB", bytes / 1048576.0)
}

pub(super) fn card(c: Colors, title: &str, description: Option<&str>) -> Div {
    let header = div()
        .px_4()
        .pt_3()
        .pb_2()
        .flex()
        .flex_col()
        .gap_0p5()
        .child(div().text_size(px(12.5)).font_weight(FontWeight::SEMIBOLD).text_color(c.fg).child(title.to_string()))
        .when_some(description.map(str::to_string), |d, t| d.child(div().text_xs().text_color(c.subtle).child(t)));
    div().flex().flex_col().rounded(px(10.0)).border_1().border_color(c.border).bg(c.panel).child(header)
}

pub(super) fn row(c: Colors, label: impl Into<SharedString>, hint: Option<String>, control: impl IntoElement) -> Div {
    div()
        .flex()
        .items_center()
        .gap_6()
        .px_4()
        .py_3()
        .border_t_1()
        .border_color(c.border)
        .child(
            div().flex_1().min_w_0().flex().flex_col().gap_0p5().child(div().text_size(px(13.0)).font_weight(FontWeight::MEDIUM).text_color(c.fg).child(label.into()))
                .when_some(hint, |d, t| d.child(div().text_xs().text_color(c.subtle).child(t))),
        )
        .child(div().flex().items_center().gap_2().flex_shrink_0().child(control))
}

pub(super) fn stacked_row(c: Colors, label: impl Into<SharedString>, hint: Option<String>, control: impl IntoElement) -> Div {
    div()
        .flex()
        .flex_col()
        .gap_2()
        .px_4()
        .py_2p5()
        .border_t_1()
        .border_color(c.border)
        .child(div().text_size(px(13.0)).text_color(c.fg).child(label.into()))
        .when_some(hint, |d, t| d.child(div().text_xs().text_color(c.subtle).child(t)))
        .child(control)
}

fn result_line(c: Colors, result: &Option<Result<String, String>>) -> Option<Div> {
    let r = result.as_ref()?;
    let (text, color) = match r {
        Ok(t) => (t.clone(), c.success),
        Err(t) => (t.clone(), c.danger),
    };
    Some(div().px_1().text_xs().text_color(color).child(text))
}

pub(super) fn plain_button(c: Colors, id: impl Into<ElementId>, label: impl Into<SharedString>) -> Stateful<Div> {
    div().id(id).h(px(28.0)).flex().items_center().gap_1p5().rounded_md().border_1().border_color(c.border_strong).px_2p5().text_xs().text_color(c.fg).cursor_pointer().hover(move |d| d.bg(c.hover)).child(label.into())
}

impl Root {
    fn settings_ui(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.settings.is_some() {
            return;
        }
        let saved = self.state.read(cx).system_prompt_of();
        let system_prompt = cx.new(|cx| {
            let mut state = EditorState::new(window, cx);
            state.set_value(saved, window, cx);
            state
        });
        let password = |placeholder: &'static str, window: &mut Window, cx: &mut Context<Self>| cx.new(|cx| InputState::new(window, cx).masked(true).placeholder(placeholder));
        let passphrase = password("Passphrase", window, cx);
        let confirm = password("Repeat", window, cx);
        let import_passphrase = password("Passphrase", window, cx);
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search settings"));
        for input in [&passphrase, &confirm, &import_passphrase, &search] {
            cx.subscribe(input, |_, _, _: &InputEvent, cx| cx.notify()).detach();
        }
        self.settings = Some(SettingsUi {
            open_select: None,
            system_prompt,
            editors: Vec::new(),
            versions: HashMap::new(),
            checking: false,
            updating: Vec::new(),
            with_secrets: false,
            passphrase,
            confirm,
            import_passphrase,
            importing: None,
            export_result: None,
            import_result: None,
            busy: false,
            diagnostics: None,
            verbose: false,
            diagnostics_result: None,
            diagnostics_polled: None,
            search,
        });
        cx.spawn(async move |this, cx| {
            let list = tauri::async_runtime::spawn(async { serde_json::to_value(editors::editors_list().await).unwrap_or_default() }).await.unwrap_or_default();
            let parsed: Vec<(String, String)> = list.as_array().map(|a| a.iter().filter_map(|e| Some((e["id"].as_str()?.to_string(), e["name"].as_str()?.to_string()))).collect()).unwrap_or_default();
            let _ = this.update(cx, |this, cx| {
                if let Some(s) = this.settings.as_mut() {
                    s.editors = parsed;
                }
                cx.notify();
            });
        })
        .detach();
        self.check_versions(cx);
        self.load_all_models(cx);
        self.load_diagnostics(cx);
    }

    pub(super) fn load_all_models(&mut self, cx: &mut Context<Self>) {
        let ids: Vec<String> = self.state.read(cx).providers.iter().filter(|p| p.installed && p.model_arg.is_some() && !self.models.contains_key(&p.id)).map(|p| p.id.clone()).collect();
        for provider in ids {
            cx.spawn(async move |this, cx| {
                let id = provider.clone();
                let found = tauri::async_runtime::spawn(async move { providers::providers_models(id).await.ok().and_then(|m| serde_json::to_value(m).ok()) }).await.ok().flatten();
                let list: Vec<dialogs::ModelInfo> = found.and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default();
                let _ = this.update(cx, |this, cx| {
                    this.models.insert(provider, list);
                    cx.notify();
                });
            })
            .detach();
        }
    }

    fn check_versions(&mut self, cx: &mut Context<Self>) {
        let Some(ui) = self.settings.as_mut() else { return };
        ui.checking = true;
        cx.spawn(async move |this, cx| {
            let list = tauri::async_runtime::spawn(async { serde_json::to_value(provider_updates::providers_versions().await).unwrap_or_default() }).await.unwrap_or_default();
            let _ = this.update(cx, |this, cx| {
                if let Some(s) = this.settings.as_mut() {
                    s.checking = false;
                    if let Some(a) = list.as_array() {
                        s.versions = a.iter().filter_map(|v| Some((v["id"].as_str()?.to_string(), v.clone()))).collect();
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn update_provider(&mut self, id: String, cx: &mut Context<Self>) {
        let Some(ui) = self.settings.as_mut() else { return };
        ui.updating.push(id.clone());
        cx.spawn(async move |this, cx| {
            let target = id.clone();
            let result = tauri::async_runtime::spawn(async move { provider_updates::providers_update(target).await.map(|v| serde_json::to_value(v).unwrap_or_default()) }).await.map_err(|e| e.to_string()).and_then(|r| r);
            let _ = this.update(cx, |this, cx| {
                if let Some(s) = this.settings.as_mut() {
                    s.updating.retain(|u| *u != id);
                    if let Ok(v) = &result {
                        s.versions.insert(id.clone(), v.clone());
                    }
                }
                if let Err(err) = result {
                    this.toast(format!("Update failed: {err}"), cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn load_diagnostics(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let info = backend::background(|| serde_json::to_value(diagnostics::diagnostics_info(backend::app().clone())).unwrap_or_default()).await.unwrap_or_default();
            let _ = this.update(cx, |this, cx| {
                if let Some(s) = this.settings.as_mut() {
                    s.diagnostics = Some(info);
                    s.diagnostics_polled = Some(std::time::Instant::now());
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn select_button(&self, key: &str, current: String, options: Vec<(String, String)>, c: Colors, cx: &mut Context<Self>, on_pick: impl Fn(&mut Self, String, &mut Context<Self>) + Clone + 'static) -> AnyElement {
        let open = self.settings.as_ref().is_some_and(|s| s.open_select.as_deref() == Some(key));
        let toggle_key = key.to_string();
        let trigger = div()
            .id(SharedString::from(format!("select-{key}")))
            .w(px(200.0))
            .h(px(28.0))
            .flex()
            .items_center()
            .gap_1()
            .rounded_md()
            .border_1()
            .border_color(c.border_strong)
            .bg(c.raised)
            .px_2()
            .text_xs()
            .text_color(c.fg)
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, _, cx| {
                if let Some(s) = this.settings.as_mut() {
                    s.open_select = if s.open_select.as_deref() == Some(&toggle_key) { None } else { Some(toggle_key.clone()) };
                }
                cx.notify();
            }))
            .child(div().flex_1().truncate().child(current))
            .child(Icon::new(IconName::ChevronDown).size(px(12.0)).text_color(c.subtle));
        let mut list = div().id(SharedString::from(format!("select-list-{key}"))).w(px(200.0)).max_h(px(260.0)).overflow_y_scroll().rounded_md().border_1().border_color(c.border_strong).bg(c.raised).p_1();
        if open {
            for (i, (value, label)) in options.into_iter().enumerate() {
                let (on_pick, picked) = (on_pick.clone(), value.clone());
                list = list.child(
                    div().id(SharedString::from(format!("select-option-{key}-{i}"))).rounded_sm().px_2().py_1().text_xs().text_color(c.fg).truncate().cursor_pointer().hover(move |d| d.bg(c.hover))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(s) = this.settings.as_mut() {
                                s.open_select = None;
                            }
                            on_pick(this, picked.clone(), cx);
                            cx.notify();
                        }))
                        .child(label),
                );
            }
        }
        div().relative().child(trigger).when(open, |d| d.child(deferred(anchored().snap_to_window_with_margin(px(8.0)).child(list)).with_priority(5))).into_any_element()
    }

    pub(super) fn toggle(&self, id: impl Into<ElementId>, on: bool, c: Colors, cx: &mut Context<Self>, flip: impl Fn(&mut Self, &mut Context<Self>) + 'static) -> AnyElement {
        div()
            .id(id)
            .w(px(32.0))
            .h(px(18.0))
            .rounded_full()
            .bg(if on { c.accent } else { c.active })
            .p(px(2.0))
            .cursor_pointer()
            .flex()
            .when(on, |d| d.justify_end())
            .on_click(cx.listener(move |this, _, _, cx| {
                flip(this, cx);
                cx.notify();
            }))
            .child(div().size(px(14.0)).rounded_full().bg(hsla(0.0, 0.0, 1.0, 1.0)))
            .into_any_element()
    }

    pub(super) fn rest_bool(&self, group: &str, key: &str, default: bool, cx: &App) -> bool {
        self.state.read(cx).rest.get(group).and_then(|g| g[key].as_bool()).unwrap_or(default)
    }

    pub(super) fn set_rest(&self, group: &'static str, key: &'static str, value: Value, cx: &mut Context<Self>) {
        self.update_state(cx, |s| {
            let entry = s.rest.entry(group.to_string()).or_insert_with(|| json!({}));
            if !entry.is_object() {
                *entry = json!({});
            }
            entry[key] = value;
        });
    }

    fn page_general(&mut self, c: Colors, cx: &mut Context<Self>) -> Div {
        let s = self.state.read(cx);
        let size = s.terminal_font_size;
        let chosen = s.editor.clone();
        let ui = self.settings.as_ref().unwrap();
        let editors = ui.editors.clone();
        let current_editor = editors.iter().find(|(id, _)| Some(id) == chosen.as_ref()).or(editors.first()).map_or_else(|| "None installed".to_string(), |(_, n)| n.clone());
        let step = move |id: &'static str, label: &'static str| div().id(id).w(px(26.0)).h(px(26.0)).flex().items_center().justify_center().rounded_md().border_1().border_color(c.border_strong).text_xs().cursor_pointer().hover(move |d| d.bg(c.hover)).child(label);
        let editor_select = self.select_button("editor", current_editor, editors, c, cx, |this, id, cx| this.update_state(cx, |s| s.editor = Some(id)));
        div()
            .flex()
            .flex_col()
            .gap_6()
            .child(
                card(c, "Terminals", None).child(row(
                    c,
                    "Text size",
                    Some("Also ⌘= / ⌘- / ⌘0, or ⌘ + scroll over a terminal.".to_string()),
                    div().flex().items_center().gap_2()
                        .child(step("font-down", "−").on_click(cx.listener(|this, _, _, cx| this.update_state(cx, |s| s.set_terminal_font_size(s.terminal_font_size - 0.5)))))
                        .child(div().w(px(48.0)).text_center().text_xs().text_color(c.muted).child(format!("{size}px")))
                        .child(step("font-up", "+").on_click(cx.listener(|this, _, _, cx| this.update_state(cx, |s| s.set_terminal_font_size(s.terminal_font_size + 0.5)))))
                        .child(plain_button(c, "font-reset", "Reset").on_click(cx.listener(|this, _, _, cx| this.update_state(cx, |s| s.set_terminal_font_size(crate::model::DEFAULT_TERMINAL_FONT_SIZE))))),
                )),
            )
            .child(card(c, "Code editor", None).child(row(c, "Open projects and worktrees in", None, editor_select)))
            .child(card(c, "Keyboard", None).child(row(
                c,
                "Keyboard shortcuts",
                Some("See every shortcut and change any of them.".to_string()),
                plain_button(c, "edit-shortcuts", "Edit shortcuts").on_click(cx.listener(|this, _, _, cx| {
                    this.update_state(cx, |s| {
                        s.cheat_sheet_open = true;
                        s.settings_open = false;
                    });
                })),
            )))
    }

    fn page_theme(&mut self, c: Colors, cx: &mut Context<Self>) -> Div {
        let s = self.state.read(cx);
        let current = s.theme.clone();
        let mut grid = div().flex().flex_wrap().gap_2().p_3();
        for t in THEMES.iter() {
            let id = t.id.clone();
            let selected = t.id == current;
            let mut dots = div().flex().gap_0p5().ml_auto();
            for i in 1..=5 {
                dots = dots.child(div().size_2().rounded_full().bg(crate::theme::hex(&t.ansi[i])));
            }
            grid = grid.child(
                div().id(SharedString::from(format!("theme-{id}"))).w(px(172.0)).overflow_hidden().rounded_md().border_1().border_color(if selected { c.accent } else { c.border }).cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| this.update_state(cx, |s| s.theme = id.clone())))
                    .child(
                        div().flex().items_center().gap_1().px_2().py_2().bg(crate::theme::hex(&t.ui.panel))
                            .child(div().text_size(px(11.0)).text_color(crate::theme::hex(&t.ui.fg)).child("Aa"))
                            .child(dots)
                            .child(div().h_2().w_3().rounded_sm().bg(crate::theme::hex(&t.ui.accent))),
                    )
                    .child(
                        div().flex().items_center().gap_1().border_t_1().border_color(c.border).bg(c.raised).px_2().py_1().text_size(px(11.0)).text_color(c.muted)
                            .child(div().flex_1().truncate().child(t.name.clone()))
                            .when(selected, |d| d.child(Icon::new(IconName::Check).size(px(11.0)).text_color(c.accent))),
                    ),
            );
        }
        div()
            .flex()
            .flex_col()
            .gap_6()
            .child(card(c, "Theme", Some("Also from the command palette (⌘K, “Theme:”).")).child(grid))
    }

    fn page_agents(&mut self, c: Colors, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let s = self.state.read(cx).clone();
        let provider_options: Vec<(String, String)> = s.providers.iter().map(|p| (p.id.clone(), if p.installed { p.name.clone() } else { format!("{} (not installed)", p.name) })).collect();
        let current = s.provider_name(&s.default_provider_id());
        let default_select = self.select_button("default-agent", current, provider_options, c, cx, |this, id, cx| this.update_state(cx, |s| s.default_provider = Some(id)));
        let mut panes = card(c, "New panes", None).child(row(c, "Default agent", Some("⌘T, split and “open a new window” start this agent.".to_string()), default_select));
        if let Some(active) = s.active() {
            let toggle = self.toggle("use-worktrees", active.use_worktrees, c, cx, |this, cx| this.update_state(cx, |s| {
                if let Some(w) = s.active_mut() {
                    w.use_worktrees = !w.use_worktrees;
                }
            }));
            panes = panes.child(row(c, "Always open new agents in their own worktree", Some(format!("Applies to the workspace “{}”.", active.name)), toggle));
        }

        let agents: Vec<_> = s.providers.iter().filter(|p| p.installed && p.model_arg.is_some()).cloned().collect();
        let mut models = card(c, "Models", Some("The model each agent starts with in new panes."));
        for p in &agents {
            let list = self.models.get(&p.id).cloned().unwrap_or_default();
            let mut options = vec![(String::new(), "CLI default".to_string())];
            options.extend(list.iter().map(|m| (m.id.clone(), m.label.clone())));
            let chosen = s.agent_models.get(&p.id).cloned().unwrap_or_default();
            let label = options.iter().find(|(id, _)| *id == chosen).map_or_else(|| if chosen.is_empty() { "CLI default".to_string() } else { chosen.clone() }, |(_, l)| l.clone());
            let id = p.id.clone();
            let control = self.select_button(&format!("model-{}", p.id), label, options, c, cx, move |this, value, cx| {
                let id = id.clone();
                this.update_state(cx, |s| {
                    if value.is_empty() {
                        s.agent_models.remove(&id);
                    } else {
                        s.agent_models.insert(id, value);
                    }
                });
            });
            models = models.child(row(c, p.name.clone(), None, control));
        }

        let ui = self.settings.as_ref().unwrap();
        let (versions, checking, updating) = (ui.versions.clone(), ui.checking, ui.updating.clone());
        let mut version_card = card(c, "Versions", Some("Updating changes the CLI on disk; running panes keep their version until restarted."));
        for p in s.providers.iter().filter(|p| p.installed && p.command.is_some()) {
            let v = versions.get(&p.id);
            let current = v.and_then(|v| v["current"].as_str()).unwrap_or_default();
            let (outdated, latest, updatable) = v.map_or((false, None, false), |v| (v["outdated"] == true, v["latest"].as_str().map(str::to_string), v["updatable"] == true));
            let hint = match v {
                None if checking => Some("Checking…".to_string()),
                None => None,
                Some(_) if outdated => latest.clone().map(|l| format!("{l} is available.")),
                Some(_) if latest.is_some() => Some("Up to date.".to_string()),
                Some(_) => None,
            };
            let busy = updating.contains(&p.id);
            let id = p.id.clone();
            let control = div().when(busy, |d| d.child(div().text_xs().text_color(c.subtle).child("Updating…"))).when(!busy && updatable && (outdated || latest.is_none()), |d| {
                d.child(plain_button(c, SharedString::from(format!("update-{id}")), if outdated { format!("Update to {}", latest.clone().unwrap_or_default()) } else { "Update".to_string() }).on_click(cx.listener(move |this, _, _, cx| this.update_provider(id.clone(), cx))))
            });
            version_card = version_card.child(row(c, format!("{} {current}", p.name), hint, control));
        }
        version_card = version_card.child(row(c, "Check for agent updates", None, plain_button(c, "check-versions", if checking { "Checking…" } else { "Check now" }).on_click(cx.listener(|this, _, _, cx| this.check_versions(cx)))));

        let mut auto = card(c, "Auto-approve", Some("Agents run tools and edits without asking. Only for sandboxes or work you can undo."));
        for p in s.providers.iter().filter(|p| p.installed && !p.auto_approve_args.is_empty()) {
            let (id, on) = (p.id.clone(), s.auto_approve.contains(&p.id));
            let toggle = self.toggle(SharedString::from(format!("auto-{id}")), on, c, cx, move |this, cx| {
                let id = id.clone();
                this.update_state(cx, |s| {
                    if !s.auto_approve.iter().any(|x| *x == id) {
                        s.auto_approve.push(id);
                    } else {
                        s.auto_approve.retain(|x| *x != id);
                    }
                });
            });
            auto = auto.child(row(c, format!("Start {} in auto-approve mode", p.name), None, toggle));
        }

        let prompt = self.settings.as_ref().unwrap().system_prompt.clone();
        let custom = s.system_prompt.is_some();
        let reset_prompt = cx.listener(|this, _: &ClickEvent, window, cx| {
            this.update_state(cx, |s| s.system_prompt = None);
            let text = this.state.read(cx).system_prompt_of();
            if let Some(ui) = this.settings.as_ref() {
                ui.system_prompt.update(cx, |e, cx| e.set_value(text, window, cx));
            }
        });
        let save_prompt = cx.listener(|this, _: &ClickEvent, _, cx| {
            let Some(ui) = this.settings.as_ref() else { return };
            let text = ui.system_prompt.read(cx).value().trim().to_string();
            this.update_state(cx, |s| s.system_prompt = if text == crate::model::DEFAULT_SYSTEM_PROMPT { None } else { Some(text.clone()) });
            this.toast("System prompt saved", cx);
        });
        let _ = window;
        let prompt_card = card(c, "System prompt", Some("Given to every agent when it starts (or restarts). Leave empty for none.")).child(stacked_row(
            c,
            "Instructions for every agent",
            None,
            div().flex().flex_col().gap_2().child(div().h(px(120.0)).child(Editor::new(&prompt))).child(
                div().flex().gap_2().justify_end().when(custom, |d| d.child(plain_button(c, "prompt-reset", "Reset to default").on_click(reset_prompt))).child(plain_button(c, "prompt-save", "Save").on_click(save_prompt)),
            ),
        ));

        let mut tools = card(c, "Agent tools", Some("Tools agents rarely use are left out, since every tool costs context in each request."));
        for name in OPTIONAL_TOOLS {
            let on = s.enabled_tools.iter().any(|t| t == name);
            let toggle = self.toggle(SharedString::from(format!("tool-{name}")), on, c, cx, move |this, cx| {
                this.update_state(cx, |s| {
                    if s.enabled_tools.iter().any(|t| t == name) {
                        s.enabled_tools.retain(|t| t != name);
                    } else {
                        s.enabled_tools.push(name.to_string());
                    }
                });
            });
            tools = tools.child(row(c, name, None, toggle));
        }

        let auto_tests = self.rest_bool("review", "autoTests", false, cx);
        let shots = self.rest_bool("review", "screenshots", true, cx);
        let tests_toggle = self.toggle("review-tests", auto_tests, c, cx, move |this, cx| this.set_rest("review", "autoTests", Value::Bool(!auto_tests), cx));
        let shots_toggle = self.toggle("review-shots", shots, c, cx, move |this, cx| this.set_rest("review", "screenshots", Value::Bool(!shots), cx));
        let review = card(c, "Review", None)
            .child(row(c, "Run tests when an agent finishes", None, tests_toggle))
            .child(row(c, "Screenshot the dev server", Some("Shows the agent's running app on its review card.".to_string()), shots_toggle));

        let progress_on = self.rest_bool("progress", "enabled", true, cx);
        let interval = s.rest.get("progress").and_then(|p| p["intervalSec"].as_u64()).unwrap_or(60);
        let options: Vec<(String, String)> = [(30, "30 seconds"), (60, "minute"), (120, "2 minutes"), (300, "5 minutes")].iter().map(|(v, l)| (v.to_string(), l.to_string())).collect();
        let label = options.iter().find(|(v, _)| *v == interval.to_string()).map_or_else(String::new, |(_, l)| l.clone());
        let on_toggle = self.toggle("progress-on", progress_on, c, cx, move |this, cx| this.set_rest("progress", "enabled", Value::Bool(!progress_on), cx));
        let interval_select = self.select_button("progress-interval", label, options, c, cx, |this, v, cx| this.set_rest("progress", "intervalSec", json!(v.parse::<u64>().unwrap_or(60)), cx));
        let progress = card(c, "Session progress", Some("Uses the LLM under AI. Agents whose output hasn't changed aren't re-checked."))
            .child(row(c, "Estimate how far working agents are", None, on_toggle))
            .when(progress_on, |d| d.child(row(c, "Check every", None, interval_select)));

        div().flex().flex_col().gap_6().child(panes).when(!agents.is_empty(), |d| d.child(models)).child(version_card).child(auto).child(prompt_card).child(tools).child(review).child(progress)
    }

    fn page_data(&mut self, c: Colors, cx: &mut Context<Self>) -> Div {
        let ui = self.settings.as_ref().unwrap();
        let (with_secrets, busy) = (ui.with_secrets, ui.busy);
        let (pass, conf) = (ui.passphrase.read(cx).value().to_string(), ui.confirm.read(cx).value().to_string());
        let mismatch = with_secrets && pass != conf;
        let can_export = !busy && (!with_secrets || (pass.len() >= 8 && !mismatch));
        let importing = ui.importing.clone();
        let (passphrase, confirm, import_pass) = (ui.passphrase.clone(), ui.confirm.clone(), ui.import_passphrase.clone());
        let export_line = result_line(c, &ui.export_result);
        let import_line = result_line(c, &ui.import_result);
        let secrets_toggle = self.toggle("export-secrets", with_secrets, c, cx, |this, _| {
            if let Some(s) = this.settings.as_mut() {
                s.with_secrets = !s.with_secrets;
            }
        });
        let mut export = card(c, "Export", Some("Saves settings, workspaces, plugins with their data, and usage history into one file to import on another machine."))
            .child(row(c, "Include secrets", Some("ChatGPT sign-in, the voice OpenAI key and plugin vault values, encrypted with a passphrase.".to_string()), secrets_toggle));
        if with_secrets {
            export = export.child(row(c, "Passphrase", Some(if mismatch && !conf.is_empty() { "The passphrases don't match.".to_string() } else { "At least 8 characters. It can't be recovered.".to_string() }), div().flex().gap_2().child(div().w(px(130.0)).child(Input::new(&passphrase))).child(div().w(px(130.0)).child(Input::new(&confirm)))));
        }
        let export_button = plain_button(c, "export", if busy { "Working…" } else { "Export…" }).when(!can_export, |d| d.opacity(0.5)).on_click(cx.listener(move |this, _, _, cx| {
            if can_export {
                this.run_export(cx);
            }
        }));
        export = export.child(row(c, "Export file", None, export_button));

        let choose = plain_button(c, "import-choose", if importing.is_some() { "Change…" } else { "Choose…" }).on_click(cx.listener(|this, _, _, cx| this.pick_import(cx)));
        let name = importing.as_ref().map_or("Export file".to_string(), |(p, _)| p.rsplit(['/', '\\']).next().unwrap_or(p).to_string());
        let mut import = card(c, "Import", Some("Replaces this machine's data with an export, then restarts.")).child(row(c, name, None, choose));
        if importing.as_ref().is_some_and(|(_, m)| m["hasSecrets"] == true) {
            import = import.child(row(c, "Passphrase", Some("Leave empty to import without the secrets.".to_string()), div().w(px(180.0)).child(Input::new(&import_pass))));
        }
        if importing.is_some() {
            import = import.child(row(c, "Import and restart", None, plain_button(c, "import-run", "Import").on_click(cx.listener(|this, _, window, cx| this.run_import(window, cx)))));
        }
        div().flex().flex_col().gap_4().child(export).children(export_line).child(import).children(import_line)
    }

    fn run_export(&mut self, cx: &mut Context<Self>) {
        let Some(ui) = self.settings.as_mut() else { return };
        let secret = ui.with_secrets.then(|| ui.passphrase.read(cx).value().to_string());
        let dir = dirs::home_dir().unwrap_or_default();
        let date = chrono_date();
        let target = cx.prompt_for_new_path(&dir, Some(&format!("winfer-{date}.winfer")));
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(path))) = target.await else { return };
            let path = path.display().to_string();
            let _ = this.update(cx, |this, cx| {
                if let Some(s) = this.settings.as_mut() {
                    s.busy = true;
                    s.export_result = None;
                }
                cx.notify();
            });
            let shown = path.clone();
            let result = tauri::async_runtime::spawn(async move { transfer::data_export(backend::app().clone(), path, Default::default(), secret).await }).await.map_err(|e| e.to_string()).and_then(|r| r);
            let _ = this.update(cx, |this, cx| {
                if let Some(s) = this.settings.as_mut() {
                    s.busy = false;
                    s.export_result = Some(result.map(|_| format!("Exported to {shown}")).map_err(|e| format!("Export failed: {e}")));
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn pick_import(&mut self, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(PathPromptOptions { files: true, directories: false, multiple: false, prompt: Some("Import".into()) });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = picked.await else { return };
            let Some(path) = paths.first().map(|p| p.display().to_string()) else { return };
            let target = path.clone();
            let manifest = backend::background(move || transfer::data_import_inspect(target).map(|m| serde_json::to_value(m).unwrap_or_default())).await.map_err(|e| e.to_string()).and_then(|r| r);
            let _ = this.update(cx, |this, cx| {
                if let Some(s) = this.settings.as_mut() {
                    match manifest {
                        Ok(m) => {
                            s.importing = Some((path, m));
                            s.import_result = None;
                        }
                        Err(err) => {
                            s.importing = None;
                            s.import_result = Some(Err(err));
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn run_import(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ui) = self.settings.as_ref() else { return };
        let Some((path, manifest)) = ui.importing.clone() else { return };
        let typed = ui.import_passphrase.read(cx).value().to_string();
        let secret = (manifest["hasSecrets"] == true && !typed.is_empty()).then_some(typed);
        let detail = "Importing replaces this machine's settings, workspaces, plugins and usage history, then restarts winfer.".to_string();
        self.confirm("Import winfer data?", detail, "Import and restart", window, cx, move |this, cx| {
            cx.spawn(async move |this, cx| {
                let result = tauri::async_runtime::spawn(async move { transfer::data_import_stage(backend::app().clone(), path, secret).await }).await.map_err(|e| e.to_string()).and_then(|r| r);
                let _ = this.update(cx, |this, cx| match result {
                    Ok(_) => {
                        // An import staged for the next start mustn't be overwritten by this one.
                        this.suspend_saving = true;
                        cx.restart();
                    }
                    Err(err) => {
                        if let Some(s) = this.settings.as_mut() {
                            s.import_result = Some(Err(format!("Import failed: {err}")));
                        }
                        cx.notify();
                    }
                });
            })
            .detach();
            let _ = this;
        });
    }

    fn page_diagnostics(&mut self, c: Colors, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let _ = window;
        // Memory moves: re-read every five seconds while the page is open.
        if self.settings.as_ref().is_some_and(|s| s.diagnostics_polled.is_none_or(|t| t.elapsed() > Duration::from_secs(5))) {
            if let Some(s) = self.settings.as_mut() {
                s.diagnostics_polled = Some(std::time::Instant::now());
            }
            self.load_diagnostics(cx);
        }
        let ui = self.settings.as_ref().unwrap();
        let Some(info) = ui.diagnostics.clone() else {
            return card(c, "Diagnostics", None).child(row(c, "Loading…", None, div()));
        };
        let verbose = ui.verbose;
        let result = result_line(c, &ui.diagnostics_result);
        let num = |k: &str| info[k].as_f64().unwrap_or(0.0);
        let mem = |k: &str| info["memory"][k].as_f64().unwrap_or(0.0);
        let summary = format!(
            "winfer {} · {} {} · up {} min\nmemory: {} now, {} session peak\npanes: {} terminal sessions, {} scrollback\nlogs: {} ({} log, {} crash log)\n{}",
            info["version"].as_str().unwrap_or_default(), info["os"].as_str().unwrap_or_default(), info["arch"].as_str().unwrap_or_default(), (num("uptimeSecs") / 60.0).floor(),
            mb(mem("rssBytes")), mb(num("sessionPeakRssBytes")), num("ptySessions"), mb(num("scrollbackBytes")),
            info["logDir"].as_str().unwrap_or_default(), mb(num("logBytes")), mb(num("crashBytes")),
            if info["previousUnexpectedExit"] == true { "previous run ended without a clean exit (crash or OOM kill)" } else { "previous run exited cleanly" },
        );
        let crash = info["crashTail"].as_str().unwrap_or("").to_string();
        let copy_text = format!("{summary}\n\ncrash log tail:\n{}", if crash.is_empty() { "(empty)" } else { &crash });
        let verbose_toggle = self.toggle("verbose-log", verbose, c, cx, |this, _| {
            if let Some(s) = this.settings.as_mut() {
                s.verbose = !s.verbose;
                diagnostics::diagnostics_set_verbose(s.verbose);
            }
        });
        let recent: Vec<String> = info["recentMemory"].as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect()).unwrap_or_default();
        let values: Vec<f32> = recent.iter().map(|line| line.split("MB").next().and_then(|h| h.rsplit(|ch: char| !ch.is_ascii_digit()).next()).and_then(|n| n.parse().ok()).unwrap_or(0.0)).collect();
        let peak = values.iter().cloned().fold(1.0, f32::max);
        let log_dir = info["logDir"].as_str().unwrap_or_default().to_string();
        let reveal_dir = log_dir.clone();
        let dumps = info["dumpCount"].as_u64().unwrap_or(0);
        let crash_bytes = num("crashBytes");
        let unexpected = info["previousUnexpectedExit"] == true;
        let mut memory = card(c, "Memory", Some("winfer's own resident memory, sampled every 5 seconds into the log. A steady climb before a crash points at a leak; the timeline in the export shows it."))
            .child(row(c, "Now / session peak", Some(format!("{} of {} system memory free", mb(mem("systemAvailableBytes")), mb(mem("systemTotalBytes")))), div().text_xs().text_color(c.fg).child(format!("{} / {}", mb(mem("rssBytes")), mb(num("sessionPeakRssBytes"))))))
            .child(row(c, "Terminals", Some("Each live pane also holds up to 2 MB of replay scrollback in the backend, plus its terminal buffer.".to_string()), div().text_xs().text_color(c.fg).child(format!("{} sessions · {}", num("ptySessions"), mb(num("scrollbackBytes"))))));
        if values.len() > 1 {
            memory = memory.child(row(c, "Last 2 minutes", recent.last().cloned(), div().h(px(32.0)).flex().items_end().gap(px(2.0)).children(values.iter().map(|v| div().w(px(6.0)).h(px(32.0 * (v / peak).max(0.25))).rounded_sm().bg(Hsla { a: 0.7, ..c.accent })))));
        }
        div()
            .flex()
            .flex_col()
            .gap_4()
            .when(unexpected, |d| d.child(div().rounded_lg().border_1().border_color(Hsla { a: 0.4, ..c.warning }).bg(Hsla { a: 0.1, ..c.warning }).px_3().py_2().text_xs().text_color(c.fg)
                .child("The previous run ended without a clean exit — a crash or an out-of-memory kill. Its last memory samples are at the end of winfer.log; export the diagnostics below and attach them to a bug report.")))
            .child(memory)
            .child(card(c, "Logs", Some("winfer.log keeps the app log (rotated at 5 MB); crash.log holds panics with backtraces. Both live in the folder below."))
                .child(row(c, "Log folder", Some(log_dir), plain_button(c, "diag-open", "Open…").on_click(cx.listener(move |_, _, _, _| sidebar::reveal_path(&reveal_dir)))))
                .child(row(c, "Sizes", (dumps > 0).then(|| format!("{dumps} minidump(s) kept in the log folder — attach the newest .dmp to a crash report.")), div().text_xs().text_color(c.fg).child(format!("{} log · {} crash", mb(num("logBytes")), mb(crash_bytes)))))
                .child(row(c, "Verbose logging", Some("Also records debug lines (more detail, faster-growing log). Kept across restarts.".to_string()), verbose_toggle))
                .child(row(c, "Copy summary", Some("Version, memory and the crash-log tail — paste it into a bug report.".to_string()), plain_button(c, "diag-copy", "Copy").on_click(cx.listener(move |this, _, _, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(copy_text.clone()));
                    this.toast("Diagnostics summary copied", cx);
                }))))
                .child(row(c, "Export for a bug report", Some("A zip with the summary, both logs and the memory timeline. No settings, chats or secrets.".to_string()), plain_button(c, "diag-export", "Export…").on_click(cx.listener(|this, _, _, cx| this.export_diagnostics(cx)))))
                .child(row(c, "Clear logs", None, plain_button(c, "diag-clear", "Clear").on_click(cx.listener(|this, _, window, cx| {
                    this.confirm("Clear logs?", "Delete the current log and crash log? Minidumps are kept.".to_string(), "Clear", window, cx, |this, cx| {
                        let _ = diagnostics::diagnostics_clear();
                        this.load_diagnostics(cx);
                    });
                })))))
            .children(result)
            .when(!crash.is_empty(), |d| d.child(card(c, "Latest crash", Some("The end of crash.log. Full backtraces are in the file and in the export.")).child(div().px_4().pb_3().max_h(px(256.0)).overflow_hidden().text_size(px(11.0)).font_family(crate::fonts::mono(cx)).text_color(c.muted).child(crash))))
    }

    fn export_diagnostics(&mut self, cx: &mut Context<Self>) {
        let dir = dirs::home_dir().unwrap_or_default();
        let target = cx.prompt_for_new_path(&dir, Some(&format!("winfer-diagnostics-{}.zip", chrono_date())));
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(path))) = target.await else { return };
            let path = path.display().to_string();
            let result = tauri::async_runtime::spawn(async move { diagnostics::diagnostics_export(backend::app().clone(), path).await }).await.map_err(|e| e.to_string()).and_then(|r| r);
            let _ = this.update(cx, |this, cx| {
                if let Some(s) = this.settings.as_mut() {
                    s.diagnostics_result = Some(result.map_err(|e| format!("Export failed: {e}")));
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn render_settings(&mut self, c: &Colors, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.state.read(cx).settings_open {
            self.settings = None;
            return None;
        }
        self.settings_ui(window, cx);
        let c = *c;
        let page = self.state.read(cx).settings_page.clone();
        let (group, (id, label, icon, description)) = page_info(&page);
        let search = self.settings.as_ref().map(|s| s.search.clone()).expect("settings ui");
        let query = search.read(cx).value().trim().to_string();
        let hits = search_settings(&query);
        let searching = !query.is_empty();
        let mono = crate::fonts::mono(cx);

        // Navigation: search on top, then the pages in their groups.
        let mut list = div().id("settings-nav").flex_1().min_h_0().overflow_y_scroll().flex().flex_col().px_2().pb_2();
        for (g, pages) in GROUPS {
            list = list.child(div().px_2p5().pt_3().pb_1().text_size(px(10.5)).font_weight(FontWeight::SEMIBOLD).text_color(c.subtle).child(g.to_uppercase()));
            for (pid, plabel, picon, _) in pages.iter().copied() {
                let selected = !searching && pid == id;
                let matches = hits.iter().filter(|h| h.0 == pid).count();
                let dot = pid == "updates" && self.updates.available.is_some();
                list = list.child(
                    div().id(pid).h(px(30.0)).flex_shrink_0().flex().items_center().gap_2p5().rounded_md().px_2p5().text_size(px(13.0)).cursor_pointer()
                        .text_color(if selected { c.fg } else { c.muted })
                        .when(selected, |d| d.bg(Hsla { a: 0.14, ..c.accent }))
                        .when(searching && matches == 0, |d| d.opacity(0.35))
                        .hover(move |d| d.bg(c.hover).text_color(c.fg))
                        .on_click(cx.listener(move |this, _, window, cx| this.open_settings_page(pid, window, cx)))
                        .child(Icon::new(picon).size(px(15.0)).when(selected, |i| i.text_color(c.accent)))
                        .child(div().flex_1().truncate().child(plabel))
                        .when(searching && matches > 0, |d| d.child(div().font_family(mono.clone()).text_size(px(10.5)).text_color(c.subtle).child(matches.to_string())))
                        .when(dot && !searching, |d| d.child(div().size(px(6.0)).rounded_full().bg(c.accent))),
                );
            }
        }
        let search_box = div().flex().items_center().gap_2().h(px(32.0)).pl_2p5().pr_1().rounded_md().bg(c.app).border_1().border_color(c.border_strong).text_color(c.subtle)
            .child(Icon::new(IconName::Search).size(px(13.0)))
            .child(div().flex_1().min_w_0().child(Input::new(&search).appearance(false).text_size(px(12.5))))
            .when(searching, |d| d.child(div().id("settings-search-clear").rounded_sm().p_1().cursor_pointer().hover(move |d| d.bg(c.hover).text_color(c.fg))
                .on_click(cx.listener(|this, _, window, cx| {
                    if let Some(s) = this.settings.as_ref() {
                        s.search.update(cx, |i, cx| i.set_value("", window, cx));
                    }
                    cx.notify();
                }))
                .child(Icon::new(IconName::X).size(px(12.0)))));
        let version = self.updates.version.clone();
        let update = self.updates.available.as_ref().map(|a| a.version.clone());
        let foot = div().flex().items_center().justify_between().gap_2().border_t_1().border_color(c.border).px_3p5().py_2p5().text_size(px(11.0)).text_color(c.subtle)
            .child(div().font_family(mono.clone()).child(format!("winfer {version}")))
            .when_some(update, |d, v| d.child(div().id("settings-update-pill").flex().items_center().gap_1().rounded_full().px_2().py_0p5().bg(Hsla { a: 0.14, ..c.accent }).text_color(c.accent).cursor_pointer()
                .tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(format!("winfer {v} is available")).build(window, cx))
                .on_click(cx.listener(|this, _, window, cx| this.open_settings_page("updates", window, cx)))
                .child(Icon::new(IconName::Download).size(px(11.0))).child("Update")));
        let nav = div().w(px(232.0)).flex_shrink_0().flex().flex_col().border_r_1().border_color(c.border).bg(c.panel)
            .child(div().flex().flex_col().gap_3().px_3().pt_3p5().pb_1()
                .child(div().flex().items_center().gap_2().px_1().text_size(px(14.0)).font_weight(FontWeight::SEMIBOLD).text_color(c.fg)
                    .child(Icon::new(IconName::Settings).size(px(15.0)).text_color(c.accent)).child("Settings"))
                .child(search_box))
            .child(list)
            .child(foot);

        let (head, content): (Div, AnyElement) = if searching {
            let head = div().flex().flex_col().gap_0p5()
                .child(div().text_size(px(11.0)).text_color(c.subtle).child("SEARCH"))
                .child(div().text_size(px(18.0)).font_weight(FontWeight::SEMIBOLD).text_color(c.fg).child(format!("Results for “{query}”")));
            let body = if hits.is_empty() {
                div().flex().flex_col().items_center().gap_1p5().pt(px(80.0)).text_color(c.subtle)
                    .child(Icon::new(IconName::Search).size(px(22.0)))
                    .child(div().text_size(px(13.0)).text_color(c.fg).child(format!("No settings match “{query}”")))
                    .child(div().text_size(px(12.0)).child("Try “theme”, “model” or “approve”."))
            } else {
                let mut rows = div().flex().flex_col().gap_1();
                for (n, (pid, plabel, words)) in hits.into_iter().enumerate() {
                    let (pg, (_, page_label, page_icon, _)) = page_info(pid);
                    let crumb = if plabel == page_label { pg.to_string() } else { format!("{pg} › {page_label}") };
                    rows = rows.child(
                        div().id(SharedString::from(format!("hit-{n}"))).flex().items_center().gap_3().rounded_lg().px_3().py_2p5().cursor_pointer().border_1().border_color(gpui_kit::transparent_black())
                            .hover(move |d| d.bg(c.panel).border_color(c.border))
                            .on_click(cx.listener(move |this, _, window, cx| this.open_settings_page(pid, window, cx)))
                            .child(div().size(px(28.0)).flex_shrink_0().flex().items_center().justify_center().rounded_lg().bg(c.hover).text_color(c.muted).child(Icon::new(page_icon).size(px(14.0))))
                            .child(div().flex_1().min_w_0().flex().flex_col()
                                .child(div().text_size(px(13.0)).text_color(c.fg).child(plabel))
                                .when(!words.is_empty(), |d| d.child(div().truncate().text_size(px(12.0)).text_color(c.subtle).child(words))))
                            .child(div().flex_shrink_0().text_size(px(11.0)).text_color(c.subtle).child(crumb)),
                    );
                }
                rows
            };
            (head, body.into_any_element())
        } else {
            let head = div().flex().items_start().gap_3p5()
                .child(div().size(px(38.0)).flex_shrink_0().flex().items_center().justify_center().rounded_lg().bg(Hsla { a: 0.14, ..c.accent }).text_color(c.accent).child(Icon::new(icon).size(px(18.0))))
                .child(div().flex().flex_col().gap_0p5().min_w_0()
                    .child(div().text_size(px(11.0)).text_color(c.subtle).child(group.to_uppercase()))
                    .child(div().text_size(px(18.0)).font_weight(FontWeight::SEMIBOLD).text_color(c.fg).child(label))
                    .child(div().text_size(px(12.5)).text_color(c.muted).child(description)));
            let content: AnyElement = match id {
                "general" => self.page_general(c, cx).into_any_element(),
                "theme" => self.page_theme(c, cx).into_any_element(),
                "agents" => self.page_agents(c, window, cx).into_any_element(),
                "data" => self.page_data(c, cx).into_any_element(),
                "diagnostics" => self.page_diagnostics(c, window, cx).into_any_element(),
                "accounts" => self.page_accounts(c, window, cx).into_any_element(),
                "voice" => self.page_voice(c, window, cx).into_any_element(),
                "ai" => self.page_ai(c, window, cx).into_any_element(),
                "plugins" => self.page_plugins(c, window, cx).into_any_element(),
                "skills" => self.page_skills(c, window, cx).into_any_element(),
                "winfer-code" => self.page_harness(c, window, cx).into_any_element(),
                "mobile" => self.page_mobile(c, window, cx).into_any_element(),
                "usage" => self.page_usage(c, cx).into_any_element(),
                "updates" => self.page_updates(c, cx).into_any_element(),
                _ => div().text_xs().text_color(c.subtle).child(format!("The {label} page isn't in the native build yet.")).into_any_element(),
            };
            (head, content)
        };
        Some(
            div().id("settings-backdrop").absolute().size_full().flex().items_center().justify_center().bg(hsla(0.0, 0.0, 0.0, 0.55)).occlude()
                .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| this.update_state(cx, |s| s.settings_open = false)))
                .child(
                    div().id("settings").w(px(1000.0)).max_w(relative(0.94)).h(px(700.0)).max_h(relative(0.9)).flex().overflow_hidden().rounded(px(14.0)).border_1().border_color(c.border_strong).bg(c.raised).shadow_2xl()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(nav)
                        .child(
                            div().relative().flex_1().min_w_0().flex().flex_col()
                                .child(div().flex_shrink_0().pl_8().pr(px(96.0)).pt_6().pb_4().child(head))
                                .child(
                                    div().id("settings-close").absolute().top(px(16.0)).right(px(16.0)).flex().items_center().gap_1p5().rounded_md().px_1p5().py_1().cursor_pointer().text_color(c.subtle)
                                        .hover(move |d| d.bg(c.hover).text_color(c.fg))
                                        .on_click(cx.listener(|this, _, _, cx| this.update_state(cx, |s| s.settings_open = false)))
                                        .child(Icon::new(IconName::X).size(px(15.0)))
                                        .child(div().rounded_sm().border_1().border_color(c.border).px_1().font_family(mono.clone()).text_size(px(10.0)).child("esc")),
                                )
                                .child(div().id(SharedString::from(format!("settings-page-{id}-{searching}"))).flex_1().min_h_0().overflow_y_scroll().px_8().pt_1().pb_10().child(content)),
                        ),
                )
                .into_any_element(),
        )
    }

    /// Development aid: open settings with this search typed in.
    #[cfg(debug_assertions)]
    pub(super) fn settings_ui_for_dev(&mut self, query: String, window: &mut Window, cx: &mut Context<Self>) {
        self.settings_ui(window, cx);
        if let Some(s) = self.settings.as_ref() {
            s.search.update(cx, |i, cx| i.set_value(query, window, cx));
        }
    }

    /// Opens a settings page (from the navigation or a search result) and clears the search.
    fn open_settings_page(&mut self, page: &'static str, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(s) = self.settings.as_mut() {
            s.open_select = None;
            s.search.update(cx, |i, cx| i.set_value("", window, cx));
        }
        self.update_state(cx, |s| s.settings_page = page.to_string());
    }

    /// The shortcut recording in the shortcut sheet: true if the key went to it.
    pub(super) fn record_shortcut(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) -> bool {
        let Some(id) = self.recording.clone() else { return false };
        if event.keystroke.key == "escape" {
            self.recording = None;
            cx.notify();
            return true;
        }
        let Some(mut chord) = event_chord(&event.keystroke) else { return true };
        let binding: Option<&Binding> = BINDINGS.iter().find(|b| b.id == id);
        if binding.is_some_and(|b| b.digit.is_some()) {
            let parts: Vec<&str> = chord.split('+').collect();
            if !parts.last().is_some_and(|k| k.len() == 1 && k.chars().all(|c| c.is_ascii_digit())) {
                self.toast("Numbered shortcuts end in a digit", cx);
                return true;
            }
            chord = format!("{}+#", parts[..parts.len() - 1].join("+"));
        }
        let overrides = self.state.read(cx).shortcuts.clone();
        let mut expanded: Vec<String> = vec![chord.clone()];
        if chord.contains('#') {
            expanded = (1..=9).map(|n| chord.replace('#', &n.to_string())).collect();
        }
        let clash = BINDINGS.iter().find(|b| b.id != id && chord_for(b, &overrides).is_some_and(|c| expanded.contains(&c)));
        if let Some(b) = clash {
            let title = b.title.clone();
            self.toast(format!("{} is used by “{title}”", format_chord(&chord)), cx);
            return true;
        }
        self.recording = None;
        self.update_state(cx, |s| {
            s.shortcuts.insert(id, chord);
        });
        true
    }
}

fn chrono_date() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs()) as i64;
    let days = secs / 86400 + 719468;
    let era = days.div_euclid(146097);
    let doe = days.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + if month <= 2 { 1 } else { 0 };
    format!("{year:04}-{month:02}-{day:02}")
}

#[cfg(test)]
mod settings_search_tests {
    use super::{GROUPS, SEARCH_INDEX, search_settings};

    #[test]
    fn every_index_entry_is_on_a_page() {
        let pages: Vec<&str> = GROUPS.iter().flat_map(|(_, p)| p.iter().map(|p| p.0)).collect();
        for (page, label, _) in SEARCH_INDEX {
            assert!(pages.contains(page), "{label} points to unknown page {page}");
        }
    }

    #[test]
    fn search_finds_settings_by_label_and_words() {
        assert!(search_settings("approve").iter().any(|h| h.0 == "agents"));
        assert!(search_settings("nebula").iter().any(|h| h.0 == "updates"));
        assert_eq!(search_settings("text size")[0].1, "Text size");
        assert!(search_settings("   ").is_empty());
        assert_eq!(search_settings("auto-approve").iter().filter(|h| h.0 == "agents").count(), 1);
        assert!(search_settings("zzz-nothing").is_empty());
    }
}
