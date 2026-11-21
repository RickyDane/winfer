//! The settings window (port of `src/features/settings/SettingsModal.tsx`).

use super::commands::{BINDINGS, Binding, chord_for, event_chord, format_chord};
use super::*;
use crate::model::{AppState, OPTIONAL_TOOLS, optional_tool_plugin};
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

/// What the search finds: page, setting and words that lead to it (the setting's hint or
/// synonyms). Shared with the Tauri app.
#[derive(serde::Deserialize)]
struct IndexEntry {
    page: String,
    label: String,
    words: String,
}

/// The index: searchIndex.generated.json (every label and hint on the pages, made by
/// `scripts/gen-settings-index.mjs`) plus searchIndex.json (words people use for a setting that
/// its page doesn't). Entries with the same page and label are merged. Shared with the Tauri app.
static SEARCH_INDEX: std::sync::LazyLock<Vec<(&'static str, &'static str, &'static str)>> = std::sync::LazyLock::new(|| {
    let mut merged: Vec<(String, String, String)> = Vec::new();
    for json in [include_str!("../../../../../src/features/settings/searchIndex.json"), include_str!("../../../../../src/features/settings/searchIndex.generated.json")] {
        let entries: Vec<IndexEntry> = serde_json::from_str(json).expect("settings search index");
        for e in entries {
            match merged.iter_mut().find(|m| m.0 == e.page && m.1 == e.label) {
                Some(m) => {
                    m.2.push(' ');
                    m.2.push_str(&e.words);
                }
                None => merged.push((e.page, e.label, e.words)),
            }
        }
    }
    merged.into_iter().map(|(p, l, w)| (&*p.leak(), &*l.leak(), &*w.leak())).collect()
});

/// Lower-case words (letters and digits) of a text: "Auto-approve (yolo)" → auto, approve, yolo.
fn words_of(text: &str) -> Vec<String> {
    text.to_lowercase().split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).map(str::to_string).collect()
}

/// Every query word starts a word of the text: "host" finds "Host / Port", "port" not "Export".
fn matches_words(text: &str, query: &[String]) -> bool {
    let hay = words_of(text);
    query.iter().all(|q| hay.iter().any(|w| w.starts_with(q.as_str())))
}

/// Settings whose label, words or page match every word of `query`; labels starting with the
/// first word come first.
fn search_settings(query: &str) -> Vec<(&'static str, &'static str, &'static str)> {
    let words = words_of(query);
    if words.is_empty() {
        return Vec::new();
    }
    let pages = GROUPS.iter().flat_map(|(_, p)| p.iter()).map(|p| (p.0, p.1, p.3));
    let mut hits: Vec<(u8, (&str, &str, &str))> = pages.chain(SEARCH_INDEX.iter().copied())
        .filter_map(|(page, label, words_of)| {
            let page_label = page_info(page).1.1;
            matches_words(&format!("{label} {words_of} {page_label}"), &words).then(|| {
                let rank = if words_of_first(label).is_some_and(|w| w.starts_with(&words[0])) { 0 } else if matches_words(label, &words) { 1 } else { 2 };
                (rank, (page, label, words_of))
            })
        })
        .collect();
    hits.sort_by_key(|h| h.0);
    let mut seen = std::collections::HashSet::new();
    hits.into_iter().map(|h| h.1).filter(|(page, label, _)| seen.insert((*page, *label))).collect()
}

fn words_of_first(text: &str) -> Option<String> {
    words_of(text).into_iter().next()
}

pub struct SettingsUi {
    /// The page's scroll position, so a link can jump to a section (the agents to install).
    page_scroll: ScrollHandle,
    /// Where "Available to install" was last laid out (window coordinates), to scroll to it.
    install_bounds: std::rc::Rc<std::cell::Cell<Option<Bounds<Pixels>>>>,
    open_select: Option<String>,
    prompt_open: bool,
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

fn provider_tint(c: Colors, id: &str) -> Hsla {
    match id {
        "claude" => c.claude,
        "agy" => crate::theme::hex("#4f8cff"),
        "opencode" => crate::theme::hex("#f5b544"),
        "cursor" => crate::theme::hex("#a0a0ff"),
        "pi" => crate::theme::hex("#7ee787"),
        "muse" => crate::theme::hex("#fb7185"),
        "winfer" => c.accent,
        _ => c.fg,
    }
}

fn optional_tool_meta(name: &'static str) -> (&'static str, &'static str, &'static str, IconName) {
    match name {
        "browser_drag" => ("Browser", "Drag", "Drag and drop between elements", IconName::Globe),
        "browser_drop" => ("Browser", "Drop", "Drop files or data onto an element", IconName::Globe),
        "browser_emulate_media" => ("Browser", "Emulate media", "Dark/light scheme, reduced motion or print", IconName::Globe),
        "browser_navigate_back" => ("Browser", "Navigate back", "Go back in browser history", IconName::Globe),
        "kb_explain" => ("Knowledge", "Explain node", "Explain a knowledge graph node", IconName::Brain),
        "kb_save" => ("Knowledge", "Save answer", "Save an answer to graph memory", IconName::Brain),
        _ => ("Tool", name, "", IconName::Wrench),
    }
}

pub(super) fn card(c: Colors, title: &str, description: Option<&str>) -> Div {
    card_marked(c, title, description, None)
}

/// A card whose title carries a reset mark (see [`Root::reset_mark`]).
pub(super) fn card_marked(c: Colors, title: &str, description: Option<&str>, mark: Option<AnyElement>) -> Div {
    let header = div()
        .px_4()
        .pt_3()
        .pb_2()
        .flex()
        .flex_col()
        .gap_0p5()
        .child(div().flex().items_center().gap_2().text_size(px(12.5)).font_weight(FontWeight::SEMIBOLD).text_color(c.fg).child(title.to_string()).children(mark))
        .when_some(description.map(str::to_string), |d, t| d.child(div().text_xs().text_color(c.subtle).child(t)));
    div().flex().flex_col().rounded(px(10.0)).border_1().border_color(c.edge).bg(c.panel).child(header)
}

pub(super) fn row(c: Colors, label: impl Into<SharedString>, hint: Option<String>, control: impl IntoElement) -> Div {
    row_marked(c, label, None, hint, control)
}

/// A row whose label carries a reset mark (see [`Root::reset_mark`]).
pub(super) fn row_marked(c: Colors, label: impl Into<SharedString>, mark: Option<AnyElement>, hint: Option<String>, control: impl IntoElement) -> Div {
    div()
        .flex()
        .items_center()
        .gap_6()
        .px_4()
        .py_3()
        .child(
            div().flex_1().min_w_0().flex().flex_col().gap_0p5().child(div().flex().items_center().gap_2().text_size(px(13.0)).font_weight(FontWeight::MEDIUM).text_color(c.fg).child(label.into()).children(mark))
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
    div().id(id).h(px(28.0)).flex().items_center().gap_1p5().rounded_md().border_1().border_color(c.edge).px_2p5().text_xs().text_color(c.fg).cursor_pointer().hover(move |d| d.bg(c.hover)).child(label.into())
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
            page_scroll: ScrollHandle::new(),
            install_bounds: Default::default(),
            open_select: None,
            prompt_open: false,
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
        self.select_button_inner(key, current, options, Some(200.0), 200.0, false, c, cx, on_pick)
    }

    fn select_button_flex(&self, key: &str, current: String, options: Vec<(String, String)>, menu_w: f32, changed: bool, c: Colors, cx: &mut Context<Self>, on_pick: impl Fn(&mut Self, String, &mut Context<Self>) + Clone + 'static) -> AnyElement {
        self.select_button_inner(key, current, options, None, menu_w, changed, c, cx, on_pick)
    }

    #[allow(clippy::too_many_arguments)]
    fn select_button_inner(
        &self,
        key: &str,
        current: String,
        options: Vec<(String, String)>,
        trigger_w: Option<f32>,
        menu_w: f32,
        changed: bool,
        c: Colors,
        cx: &mut Context<Self>,
        on_pick: impl Fn(&mut Self, String, &mut Context<Self>) + Clone + 'static,
    ) -> AnyElement {
        let open = self.settings.as_ref().is_some_and(|s| s.open_select.as_deref() == Some(key));
        let toggle_key = key.to_string();
        let border = if open || changed { Hsla { a: 0.45, ..c.accent } } else { Hsla { a: 0.65, ..c.border } };
        let trigger = div()
            .id(SharedString::from(format!("select-{key}")))
            .when_some(trigger_w, |d, w| d.w(px(w)))
            .when(trigger_w.is_none(), |d| d.w_full())
            .h(px(28.0))
            .flex()
            .items_center()
            .gap_1p5()
            .rounded_md()
            .bg(c.raised)
            .border_1()
            .border_color(border)
            .px_2()
            .text_xs()
            .text_color(if changed || trigger_w.is_some() { c.fg } else { c.muted })
            .cursor_pointer()
            .hover(move |d| d.bg(c.hover).text_color(c.fg))
            .on_click(cx.listener(move |this, _, _, cx| {
                if let Some(s) = this.settings.as_mut() {
                    s.open_select = if s.open_select.as_deref() == Some(&toggle_key) { None } else { Some(toggle_key.clone()) };
                }
                cx.notify();
            }))
            .when(changed, |d| d.child(div().size(px(5.0)).flex_shrink_0().rounded_full().bg(c.accent)))
            .child(div().flex_1().min_w_0().truncate().child(current.clone()))
            .child(Icon::new(IconName::ChevronDown).size(px(12.0)).flex_shrink_0().text_color(c.subtle));
        let mut list = div()
            .id(SharedString::from(format!("select-list-{key}")))
            .w(px(menu_w))
            .max_h(px(260.0))
            .overflow_y_scroll()
            .rounded_lg()
            .bg(c.raised)
            .border_1()
            .border_color(c.edge)
            .shadow_lg()
            .p_1()
            // The list can hang outside the settings window; it must block the mouse itself, or a
            // click on the part outside lands on the backdrop behind and closes Settings.
            .occlude()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation());
        if open {
            for (i, (value, label)) in options.into_iter().enumerate() {
                let (on_pick, picked) = (on_pick.clone(), value.clone());
                let selected = label == current;
                list = list.child(
                    div()
                        .id(SharedString::from(format!("select-option-{key}-{i}")))
                        .flex()
                        .items_center()
                        .gap_1p5()
                        .rounded_sm()
                        .px_2()
                        .py_1()
                        .text_xs()
                        .cursor_pointer()
                        .when(selected, |d| d.bg(Hsla { a: 0.14, ..c.accent }).text_color(c.fg).font_weight(FontWeight::MEDIUM))
                        .when(!selected, |d| d.text_color(c.muted).hover(move |d| d.bg(c.hover).text_color(c.fg)))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(s) = this.settings.as_mut() {
                                s.open_select = None;
                            }
                            on_pick(this, picked.clone(), cx);
                            cx.notify();
                        }))
                        .child(div().flex_1().min_w_0().truncate().child(label))
                        .when(selected, |d| d.child(Icon::new(IconName::Check).size(px(11.0)).flex_shrink_0().text_color(c.accent))),
                );
            }
        }
        div().relative().when(trigger_w.is_none(), |d| d.w_full()).child(trigger).when(open, |d| d.child(deferred(anchored().snap_to_window_with_margin(px(8.0)).child(list)).with_priority(5))).into_any_element()
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
        let step = move |id: &'static str, label: &'static str| div().id(id).w(px(26.0)).h(px(26.0)).flex().items_center().justify_center().rounded_md().border_1().border_color(c.edge).text_xs().cursor_pointer().hover(move |d| d.bg(c.hover)).child(label);
        let editor_select = self.select_button("editor", current_editor, editors, c, cx, |this, id, cx| this.update_state(cx, |s| s.editor = Some(id)));
        div()
            .flex()
            .flex_col()
            .gap_6()
            .child(
                card(c, "Terminals", None).child(row_marked(
                    c,
                    "Text size",
                    self.reset_mark("terminal.size", c, cx),
                    Some("Also ⌘= / ⌘- / ⌘0, or ⌘ + scroll over a terminal.".to_string()),
                    div().flex().items_center().gap_2()
                        .child(step("font-down", "−").on_click(cx.listener(|this, _, _, cx| this.update_state(cx, |s| s.set_terminal_font_size(s.terminal_font_size - 0.5)))))
                        .child(div().w(px(48.0)).text_center().text_xs().text_color(c.muted).child(format!("{size}px")))
                        .child(step("font-up", "+").on_click(cx.listener(|this, _, _, cx| this.update_state(cx, |s| s.set_terminal_font_size(s.terminal_font_size + 0.5))))),
                )),
            )
            .child(card(c, "Code editor", None).child(row_marked(c, "Open projects and worktrees in", self.reset_mark("editor", c, cx), None, editor_select)))
            .child(card(c, "Keyboard", None).child(row_marked(
                c,
                "Keyboard shortcuts",
                self.reset_mark("shortcuts", c, cx),
                Some("See every shortcut and change any of them.".to_string()),
                plain_button(c, "edit-shortcuts", "Edit shortcuts").on_click(cx.listener(|this, _, _, cx| {
                    this.update_state(cx, |s| {
                        s.cheat_sheet_open = true;
                        s.settings_open = false;
                    });
                })),
            )))
    }

    /// Appearance: light, dark or follow the system, then the themes of each mode.
    fn page_theme(&mut self, c: Colors, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let s = self.state.read(cx);
        let current = s.theme.clone();
        let def = crate::theme::theme_by_id(&current);
        let system = s.theme_mode == "system";
        let dark = def.mode == "dark";
        let system_dark = matches!(window.appearance(), WindowAppearance::Dark | WindowAppearance::VibrantDark);
        // Each mode's theme: its pick, else the current theme or its counterpart.
        let effective = |m: &str| s.theme_picks.get(m).cloned().unwrap_or_else(|| if def.mode == m { def.id.clone() } else { def.pair.clone() });
        let (dark_pick, light_pick) = (effective("dark"), effective("light"));
        let mode_button = |id: &'static str, icon: IconName, label: &'static str, on: bool| {
            div().id(id).h(px(26.0)).px_3().flex().items_center().gap_1p5().rounded(px(6.0)).text_size(px(12.0)).cursor_pointer()
                .text_color(if on { c.fg } else { c.muted }).when(on, |d| d.bg(c.active))
                .hover(move |d| d.text_color(c.fg))
                .child(Icon::new(icon).size(px(13.0))).child(label)
        };
        // Light or dark picks that mode's theme and stops following the system.
        let choose = |want_dark: bool| move |this: &mut Self, _: &ClickEvent, _: &mut Window, cx: &mut Context<Self>| {
            this.update_state(cx, |s| {
                let cur = crate::theme::theme_by_id(&s.theme);
                let want = if want_dark { "dark" } else { "light" };
                let id = if cur.mode == want { cur.id.clone() } else { s.theme_picks.get(want).cloned().unwrap_or_else(|| cur.pair.clone()) };
                s.theme_mode = "manual".into();
                s.pick_theme(&id, want_dark);
            });
        };
        let follow = cx.listener(|this, _: &ClickEvent, window, cx| {
            let dark = matches!(window.appearance(), WindowAppearance::Dark | WindowAppearance::VibrantDark);
            this.update_state(cx, |s| {
                s.theme_mode = "system".into();
                s.theme = s.theme_for_system(dark);
            });
        });
        let mode = div().flex().gap_0p5().p(px(2.0)).rounded(px(8.0)).border_1().border_color(c.edge).bg(c.app)
            .child(mode_button("mode-light", IconName::Sun, "Light", !system && !dark).on_click(cx.listener(choose(false))))
            .child(mode_button("mode-dark", IconName::Moon, "Dark", !system && dark).on_click(cx.listener(choose(true))))
            .child(mode_button("mode-system", IconName::Monitor, "System", system).on_click(follow));
        let mark = self.reset_mark("theme", c, cx);
        let grid_for = |want: &str, picked: &str, cx: &mut Context<Self>| {
            let mut grid = div().flex().flex_wrap().gap_2p5().p_3();
            for t in THEMES.iter().filter(|t| t.mode == want) {
                let id = t.id.clone();
                let selected = t.id == picked;
                let mut dots = div().flex().gap_0p5();
                for i in 1..=5 {
                    dots = dots.child(div().size(px(7.0)).rounded_full().bg(crate::theme::hex(&t.ansi[i])));
                }
                // A tiny window in the theme's colors: sidebar, a pane with text, the accent.
                let preview = div().h(px(64.0)).rounded_t_lg().flex().gap_1().p_1p5().bg(crate::theme::hex(&t.ui.app))
                    .child(div().w(px(26.0)).rounded_sm().bg(crate::theme::hex(&t.ui.panel)))
                    .child(div().flex_1().flex().flex_col().gap_1().p_1p5().rounded_sm().bg(crate::theme::hex(&t.ui.panel))
                        .child(div().h(px(4.0)).w(px(44.0)).rounded_full().bg(crate::theme::hex(&t.ui.fg)))
                        .child(div().h(px(4.0)).w(px(30.0)).rounded_full().bg(crate::theme::hex(&t.ui.fg_muted)))
                        .child(div().flex_1())
                        .child(div().flex().items_center().justify_between().child(dots).child(div().h(px(6.0)).w(px(16.0)).rounded_full().bg(crate::theme::hex(&t.ui.accent)))));
                let tile_shadow = if selected {
                    vec![
                        BoxShadow { color: Hsla { a: if c.light { 0.22 } else { 0.35 }, ..c.accent }, offset: point(px(0.0), px(4.0)), blur_radius: px(14.0), spread_radius: px(-2.0), inset: false },
                        BoxShadow { color: hsla(0.0, 0.0, 0.0, if c.light { 0.08 } else { 0.3 }), offset: point(px(0.0), px(2.0)), blur_radius: px(6.0), spread_radius: px(-1.0), inset: false },
                    ]
                } else {
                    vec![
                        BoxShadow { color: hsla(0.0, 0.0, 0.0, if c.light { 0.06 } else { 0.22 }), offset: point(px(0.0), px(1.0)), blur_radius: px(4.0), spread_radius: px(-1.0), inset: false },
                    ]
                };
                grid = grid.child(
                    div().id(SharedString::from(format!("theme-{id}"))).w(px(160.0)).overflow_hidden().rounded_lg().border_1().cursor_pointer()
                        .shadow(tile_shadow)
                        .hover(move |d| d)
                        .on_click(cx.listener(move |this, _, _, cx| this.update_state(cx, |s| s.pick_theme(&id, system_dark))))
                        .child(preview)
                        .child(div().rounded_b_lg().flex().items_center().gap_1().bg(if selected { Hsla { a: 0.15, ..c.accent } } else { c.raised }).px_2().py(px(6.0)).text_size(px(11.5))
                            .when(selected, |d| d.font_weight(FontWeight::MEDIUM).text_color(c.fg))
                            .when(!selected, |d| d.text_color(c.muted))
                            .child(div().flex_1().truncate().child(t.name.clone()))
                            .when(selected, |d| d.child(Icon::new(IconName::Check).size(px(12.0)).text_color(c.accent)))),
                );
            }
            grid
        };
        let hint = if system { "Follows your system's light/dark setting, with the theme picked for each below.".to_string() } else { "Also the sun/moon button in the sidebar.".to_string() };
        let dark_grid = grid_for("dark", &dark_pick, cx);
        let light_grid = grid_for("light", &light_pick, cx);
        div()
            .flex()
            .flex_col()
            .gap_6()
            .child(card(c, "Mode", None).child(row_marked(c, "Light or dark", mark, Some(hint), mode)))
            // Only the themes of the selected mode; System shows both, one for each.
            .when(system || dark, |d| d.child(card(c, if system { "Dark theme" } else { "Dark themes" }, None).child(dark_grid)))
            .when(system || !dark, |d| d.child(card(c, if system { "Light theme" } else { "Light themes" }, Some("Also from the command palette (⌘K, “Theme:”).")).child(light_grid)))
    }

    fn page_agents(&mut self, c: Colors, window: &mut Window, cx: &mut Context<Self>) -> Div {
        use gpui_kit::component::tooltip::Tooltip;

        let s = self.state.read(cx).clone();
        let mono = crate::fonts::mono(cx);
        let default_id = s.default_provider_id();
        let card_border = Hsla { a: 0.6, ..c.border };
        let divider_color = Hsla { a: 0.45, ..c.border };
        let surface_card = || div().flex().flex_col().rounded(px(10.0)).bg(c.panel).border_1().border_color(card_border);
        let compact_row = |label: SharedString, mark: Option<AnyElement>, hint: Option<String>, control: AnyElement, first: bool| {
            div()
                .flex()
                .items_center()
                .gap_4()
                .px_3p5()
                .py_2p5()
                .when(!first, |d| d.border_t_1().border_color(divider_color))
                .child(
                    div().flex_1().min_w_0().flex().flex_col().gap(px(1.0))
                        .child(div().flex().items_center().gap_2().text_size(px(12.5)).font_weight(FontWeight::MEDIUM).text_color(c.fg).child(label).children(mark))
                        .when_some(hint, |d, t| d.child(div().text_size(px(11.5)).text_color(c.subtle).child(t))),
                )
                .child(div().flex().items_center().gap_2().flex_shrink_0().child(control))
        };

        // 1. Installed agents — compact 2-column grid of agent cockpit cards.
        let ui = self.settings.as_ref().unwrap();
        let (versions, checking, updating, prompt_open) = (ui.versions.clone(), ui.checking, ui.updating.clone(), ui.prompt_open);
        let installed: Vec<_> = s.providers.iter().filter(|p| p.installed && p.id != "shell" && (p.command.is_some() || p.model_arg.is_some() || !p.auto_approve_args.is_empty())).collect();
        let mut agent_grid = div().flex().flex_wrap().mx(px(-5.0));
        for p in &installed {
            let v = versions.get(&p.id);
            let current_ver = v.and_then(|v| v["current"].as_str()).unwrap_or_default().to_string();
            let (outdated, latest, updatable) = v.map_or((false, None, false), |v| (v["outdated"] == true, v["latest"].as_str().map(str::to_string), v["updatable"] == true));
            let busy = updating.contains(&p.id);
            let is_default = p.id == default_id;
            let tint = provider_tint(c, &p.id);
            let neutral = tint == c.fg;

            // Update action pill in header when an update can be triggered.
            let update_id = p.id.clone();
            let update_tip = if outdated {
                format!("Update {} to {}", p.name, latest.clone().unwrap_or_default())
            } else {
                format!("Check and update {}", p.name)
            };
            let update_btn = (!busy && updatable && (outdated || latest.is_none())).then(|| {
                div()
                    .id(SharedString::from(format!("update-{update_id}")))
                    .h(px(22.0))
                    .px_2()
                    .flex()
                    .items_center()
                    .gap_1()
                    .rounded_md()
                    .text_size(px(11.0))
                    .cursor_pointer()
                    .when(outdated, |d| d.bg(c.accent).text_color(c.on_accent).font_weight(FontWeight::MEDIUM))
                    .when(!outdated, |d| d.bg(c.raised).border_1().border_color(card_border).text_color(c.muted).hover(move |d| d.bg(c.hover).text_color(c.fg)))
                    .tooltip(move |window, cx| Tooltip::new(update_tip.clone()).build(window, cx))
                    .on_click(cx.listener(move |this, _, _, cx| this.update_provider(update_id.clone(), cx)))
                    .child(Icon::new(IconName::Download).size(px(10.5)))
                    .child("Update")
            });

            // Default star / badge right on the card.
            let star_id = p.id.clone();
            let star_name = p.name.clone();
            let default_badge = if is_default {
                div()
                    .id(SharedString::from(format!("default-star-{star_id}")))
                    .h(px(22.0))
                    .px_1p5()
                    .flex()
                    .items_center()
                    .gap_1()
                    .rounded_md()
                    .bg(Hsla { a: 0.14, ..c.accent })
                    .text_size(px(10.5))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(c.accent)
                    .tooltip(|window, cx| Tooltip::new("Default agent for new panes").build(window, cx))
                    .child(crate::ui::code_pickers::star_icon(c.accent, 10.5))
                    .child("Default")
                    .into_any_element()
            } else {
                div()
                    .id(SharedString::from(format!("default-star-{star_id}")))
                    .size(px(22.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_md()
                    .text_color(c.subtle)
                    .cursor_pointer()
                    .hover(move |d| d.bg(c.hover).text_color(c.fg))
                    .tooltip(move |window, cx| Tooltip::new(format!("Set {star_name} as default agent")).build(window, cx))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let id = star_id.clone();
                        this.update_state(cx, |s| s.default_provider = Some(id));
                    }))
                    .child(Icon::new(IconName::Star).size(px(12.0)))
                    .into_any_element()
            };

            let status_el = if busy {
                Some(div().text_color(c.accent).child("Updating…"))
            } else if outdated {
                latest.clone().map(|l| div().text_color(c.accent).child(format!("→ {l}")))
            } else if v.is_some() && latest.is_some() {
                Some(div().flex().items_center().gap_0p5().text_color(c.subtle)
                    .child(Icon::new(IconName::Check).size(px(10.5)).text_color(c.success))
                    .child("Up to date"))
            } else if v.is_none() && checking {
                Some(div().text_color(c.subtle).child("Checking…"))
            } else {
                None
            };

            let head = div()
                .flex()
                .items_center()
                .gap_2p5()
                .child(
                    div()
                        .size(px(30.0))
                        .flex_shrink_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_lg()
                        .bg(Hsla { a: if neutral { 0.08 } else { 0.14 }, ..tint })
                        .border_1()
                        .border_color(Hsla { a: if neutral { 0.14 } else { 0.25 }, ..tint })
                        .child(crate::ui::code_pickers::mark(c, &p.id, 16.0, 1.0)),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(1.0))
                        .child(div().truncate().text_size(px(13.0)).font_weight(FontWeight::SEMIBOLD).text_color(c.fg).child(p.name.clone()))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_1p5()
                                .text_size(px(11.0))
                                .text_color(c.subtle)
                                .when(!current_ver.is_empty(), |d| d.child(div().font_family(mono.clone()).child(current_ver.clone())))
                                .children(status_el),
                        ),
                )
                .child(div().flex().items_center().gap_1().flex_shrink_0().children(update_btn).child(default_badge));

            let has_controls = p.model_arg.is_some() || !p.auto_approve_args.is_empty();
            let mut controls = div().flex().items_center().gap_1p5();
            if p.model_arg.is_some() {
                let list = self.models.get(&p.id).cloned().unwrap_or_default();
                let mut options = vec![(String::new(), "CLI default".to_string())];
                options.extend(list.iter().map(|m| (m.id.clone(), m.label.clone())));
                let chosen = s.agent_models.get(&p.id).cloned().unwrap_or_default();
                let label = options.iter().find(|(id, _)| *id == chosen).map_or_else(|| if chosen.is_empty() { "CLI default".to_string() } else { chosen.clone() }, |(_, l)| l.clone());
                let model_changed = s.agent_models.contains_key(&p.id);
                let id = p.id.clone();
                let select = self.select_button_flex(&format!("model-{}", p.id), label, options, 232.0, model_changed, c, cx, move |this, value, cx| {
                    let id = id.clone();
                    this.update_state(cx, |s| {
                        if value.is_empty() {
                            s.agent_models.remove(&id);
                        } else {
                            s.agent_models.insert(id, value);
                        }
                    });
                });
                controls = controls.child(div().flex_1().min_w_0().child(select));
                if model_changed {
                    let reset_id = p.id.clone();
                    let reset_name = p.name.clone();
                    controls = controls.child(
                        div()
                            .id(SharedString::from(format!("reset-agent-model-{}", p.id)))
                            .size(px(26.0))
                            .flex_shrink_0()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_md()
                            .text_color(c.subtle)
                            .cursor_pointer()
                            .hover(move |d| d.bg(c.hover).text_color(c.fg))
                            .tooltip(move |window, cx| Tooltip::new(format!("Reset {reset_name} model to CLI default")).build(window, cx))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let id = reset_id.clone();
                                this.update_state(cx, |s| {
                                    s.agent_models.remove(&id);
                                });
                            }))
                            .child(Icon::new(IconName::Undo2).size(px(11.5))),
                    );
                }
            }
            if !p.auto_approve_args.is_empty() {
                let (id, on) = (p.id.clone(), s.auto_approve.contains(&p.id));
                let tip = if on {
                    "Auto-approve is on — runs tools and edits without asking. Click to turn off."
                } else {
                    "Auto-approve — skip permission prompts on start. Only for sandboxes or work you can undo."
                };
                let pill = div()
                    .id(SharedString::from(format!("auto-{id}")))
                    .h(px(28.0))
                    .px_2()
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .rounded_md()
                    .border_1()
                    .text_size(px(11.5))
                    .cursor_pointer()
                    .when(on, |d| {
                        d.bg(Hsla { a: 0.14, ..c.warning })
                            .border_color(Hsla { a: 0.42, ..c.warning })
                            .text_color(c.fg)
                            .font_weight(FontWeight::MEDIUM)
                    })
                    .when(!on, |d| {
                        d.bg(c.raised)
                            .border_color(card_border)
                            .text_color(c.muted)
                            .hover(move |d| d.bg(c.hover).text_color(c.fg))
                    })
                    .tooltip(move |window, cx| Tooltip::new(tip).build(window, cx))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let id = id.clone();
                        this.update_state(cx, |s| {
                            if !s.auto_approve.iter().any(|x| *x == id) {
                                s.auto_approve.push(id);
                            } else {
                                s.auto_approve.retain(|x| *x != id);
                            }
                        });
                    }))
                    .child(Icon::new(if on { IconName::Zap } else { IconName::Shield }).size(px(11.5)).text_color(if on { c.warning } else { c.subtle }))
                    .child("Auto-approve")
                    .when(on, |d| d.child(div().size(px(5.0)).rounded_full().bg(c.warning)));
                controls = controls.child(pill);
            }

            let card_box = div()
                .size_full()
                .flex()
                .flex_col()
                .gap_2p5()
                .rounded(px(10.0))
                .bg(c.panel)
                .border_1()
                .border_color(if is_default { Hsla { a: 0.42, ..c.accent } } else { card_border })
                .p_3()
                .child(head)
                .when(has_controls, |d| d.child(div().h(px(1.0)).w_full().bg(divider_color)).child(controls));

            agent_grid = agent_grid.child(div().w(relative(0.5)).p(px(5.0)).child(card_box));
        }

        let agents_head = div()
            .flex()
            .items_center()
            .justify_between()
            .gap_3()
            .px_0p5()
            .child(
                div().flex().flex_col().gap_0p5()
                    .child(
                        div().flex().items_center().gap_2()
                            .child(div().text_size(px(12.5)).font_weight(FontWeight::SEMIBOLD).text_color(c.fg).child("Your agents"))
                            .child(div().rounded_full().bg(c.panel).border_1().border_color(card_border).px_1p5().py(px(1.0)).font_family(mono.clone()).text_size(px(10.0)).text_color(c.subtle).child(format!("{} installed", installed.len()))),
                    )
                    .child(div().text_size(px(11.5)).text_color(c.subtle).child("Pick each agent's starting model, default for new panes (★), or sandbox auto-approve.")),
            )
            .child(
                div()
                    .id("check-versions")
                    .h(px(28.0))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .rounded_md()
                    .bg(c.panel)
                    .border_1()
                    .border_color(card_border)
                    .px_2p5()
                    .text_xs()
                    .text_color(c.fg)
                    .cursor_pointer()
                    .hover(move |d| d.bg(c.hover))
                    .on_click(cx.listener(|this, _, _, cx| this.check_versions(cx)))
                    .child(Icon::new(IconName::RefreshCw).size(px(11.5)).text_color(c.subtle))
                    .child(if checking { "Checking…" } else { "Check for updates" }),
            );

        // 2. Available to install — compact 2-column tile grid (keeps install_bounds for Welcome link).
        let install_bounds = self.settings.as_ref().unwrap().install_bounds.clone();
        let missing_list: Vec<_> = s.providers.iter().filter(|p| !p.installed && p.id != "shell" && p.command.is_some()).collect();
        let any_missing = !missing_list.is_empty();
        let mut missing_grid = div().flex().flex_wrap().px_2p5().pb_2p5();
        for p in &missing_list {
            let command = p.command.clone().unwrap_or_default();
            let install: Option<String> = p.update.as_ref().and_then(|u| u.npm.as_ref().map(|n| format!("npm install -g {n}")).or_else(|| u.url.as_ref().map(|url| format!("curl -fsSL {url} | bash"))));
            let id = p.id.clone();
            let name = p.name.clone();
            let copy = install.clone();
            let tint = provider_tint(c, &p.id);
            let neutral = tint == c.fg;
            let tile = div()
                .size_full()
                .flex()
                .flex_col()
                .gap_1p5()
                .rounded_lg()
                .bg(c.raised)
                .border_1()
                .border_color(Hsla { a: 0.55, ..c.border })
                .px_2p5()
                .py_2()
                .child(
                    div().flex().items_center().gap_2()
                        .child(
                            div()
                                .size(px(22.0))
                                .flex_shrink_0()
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded_md()
                                .bg(Hsla { a: if neutral { 0.08 } else { 0.14 }, ..tint })
                                .child(crate::ui::code_pickers::mark(c, &p.id, 13.0, 0.9)),
                        )
                        .child(div().flex_1().min_w_0().truncate().text_size(px(12.5)).font_weight(FontWeight::MEDIUM).text_color(c.fg).child(p.name.clone()))
                        .when_some(copy, |d, text| {
                            d.child(
                                div()
                                    .id(SharedString::from(format!("copy-install-{id}")))
                                    .h(px(22.0))
                                    .px_1p5()
                                    .flex_shrink_0()
                                    .flex()
                                    .items_center()
                                    .gap_1()
                                    .rounded_sm()
                                    .text_size(px(11.0))
                                    .text_color(c.muted)
                                    .cursor_pointer()
                                    .hover(move |d| d.bg(c.hover).text_color(c.fg))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
                                        this.toast(format!("Copied install command for {name}"), cx);
                                    }))
                                    .child(Icon::new(IconName::Copy).size(px(10.5)))
                                    .child("Copy"),
                            )
                        }),
                )
                .child(div().truncate().font_family(mono.clone()).text_size(px(10.5)).text_color(c.subtle).child(install.unwrap_or_else(|| format!("Needs `{command}` on PATH"))));
            missing_grid = missing_grid.child(div().w(relative(0.5)).p(px(4.0)).child(tile));
        }
        let missing_card = surface_card()
            .relative()
            .child(canvas(move |bounds, _, _| install_bounds.set(Some(bounds)), |_, _, _, _| {}).absolute().size_full())
            .child(
                div().px_3p5().pt_3().pb_1p5().flex().flex_col().gap_0p5()
                    .child(
                        div().flex().items_center().gap_2()
                            .child(div().text_size(px(12.5)).font_weight(FontWeight::SEMIBOLD).text_color(c.fg).child("Available to install"))
                            .child(div().rounded_full().bg(c.raised).px_1p5().py(px(1.0)).font_family(mono.clone()).text_size(px(10.0)).text_color(c.subtle).child(missing_list.len().to_string())),
                    )
                    .child(div().text_size(px(11.5)).text_color(c.subtle).child("Install one, then Check for updates (or restart winfer) and it shows up above.")),
            )
            .child(missing_grid);

        let agents_section = div()
            .flex()
            .flex_col()
            .gap_2()
            .child(agents_head)
            .when(!installed.is_empty(), |d| d.child(agent_grid))
            .when(any_missing, |d| d.child(missing_card));

        // 3. New panes — compact workspace & default-pane card.
        let provider_options: Vec<(String, String)> = s.providers.iter().map(|p| (p.id.clone(), if p.installed { p.name.clone() } else { format!("{} (not installed)", p.name) })).collect();
        let current_default = s.provider_name(&default_id);
        let default_select = self.select_button_inner("default-agent", current_default, provider_options, Some(184.0), 200.0, s.default_provider.is_some(), c, cx, |this, id, cx| this.update_state(cx, |s| s.default_provider = Some(id)));
        let mut panes = surface_card()
            .child(div().px_3p5().pt_3().pb_1p5().text_size(px(12.5)).font_weight(FontWeight::SEMIBOLD).text_color(c.fg).child("New panes"))
            .child(compact_row(
                "Default agent".into(),
                self.reset_mark("agents.default", c, cx),
                Some("⌘T, split and “open a new window” start this agent.".to_string()),
                default_select,
                true,
            ));
        if let Some(active) = s.active() {
            let toggle = self.toggle("use-worktrees", active.use_worktrees, c, cx, |this, cx| this.update_state(cx, |s| {
                if let Some(w) = s.active_mut() {
                    w.use_worktrees = !w.use_worktrees;
                }
            }));
            panes = panes.child(compact_row(
                "Always open new agents in their own worktree".into(),
                None,
                Some(format!("Applies to the workspace “{}”.", active.name)),
                toggle,
                false,
            ));
        }

        // 4. System prompt — collapsible card (compact summary row by default).
        let prompt = self.settings.as_ref().unwrap().system_prompt.clone();
        let custom = s.system_prompt.is_some();
        let line_count = prompt.read(cx).value().lines().count();
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
        let prompt_toggle = div()
            .id("prompt-toggle")
            .h(px(28.0))
            .flex()
            .items_center()
            .gap_1p5()
            .rounded_md()
            .bg(c.raised)
            .border_1()
            .border_color(card_border)
            .px_2p5()
            .text_xs()
            .text_color(c.fg)
            .cursor_pointer()
            .hover(move |d| d.bg(c.hover))
            .on_click(cx.listener(|this, _, _, cx| {
                if let Some(ui) = this.settings.as_mut() {
                    ui.prompt_open = !ui.prompt_open;
                }
                cx.notify();
            }))
            .child(if prompt_open { "Hide" } else { "Edit" })
            .child(Icon::new(if prompt_open { IconName::ChevronUp } else { IconName::ChevronDown }).size(px(12.0)).text_color(c.subtle));

        let prompt_card = surface_card()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_4()
                    .px_3p5()
                    .py_3()
                    .child(
                        div().flex_1().min_w_0().flex().flex_col().gap_0p5()
                            .child(
                                div().flex().items_center().gap_2()
                                    .child(div().text_size(px(12.5)).font_weight(FontWeight::SEMIBOLD).text_color(c.fg).child("System prompt"))
                                    .child(
                                        div()
                                            .rounded_full()
                                            .px_1p5()
                                            .py(px(1.0))
                                            .text_size(px(10.0))
                                            .font_weight(FontWeight::MEDIUM)
                                            .when(custom, |d| d.bg(Hsla { a: 0.14, ..c.accent }).text_color(c.accent))
                                            .when(!custom, |d| d.bg(c.raised).text_color(c.subtle))
                                            .child(if custom { "Custom" } else { "Default" }),
                                    )
                                    .children(self.reset_mark("agents.prompt", c, cx)),
                            )
                            .child(div().text_size(px(11.5)).text_color(c.subtle).child("Given to every agent when it starts (or restarts). Leave empty for none.")),
                    )
                    .child(prompt_toggle),
            )
            .when(prompt_open, |d| {
                d.child(
                    div()
                        .border_t_1()
                        .border_color(divider_color)
                        .px_3p5()
                        .py_3()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(Editor::new(&prompt).h(px(190.0)))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .gap_2()
                                .child(div().font_family(mono.clone()).text_size(px(11.0)).text_color(c.subtle).child(format!("{line_count} lines")))
                                .child(
                                    div().flex().items_center().gap_2()
                                        .when(custom, |d| d.child(plain_button(c, "prompt-reset", "Reset to default").on_click(reset_prompt)))
                                        .child(super::orchestration::accent_button(c, "prompt-save", "Save", true).on_click(save_prompt)),
                                ),
                        ),
                )
            });

        // 5. Optional agent tools — compact 2-column interactive tile grid.
        let active_plugins = self.enabled_plugins();
        let tools: Vec<&'static str> = OPTIONAL_TOOLS.into_iter().filter(|n| optional_tool_plugin(n).is_none_or(|id| active_plugins.iter().any(|p| p["id"] == id))).collect();
        let enabled_tools_count = tools.iter().filter(|n| s.enabled_tools.iter().any(|t| t == *n)).count();
        let tools_len = tools.len();
        let mut tools_grid = div().flex().flex_wrap().px_2p5().pb_2p5();
        for name in tools {
            let on = s.enabled_tools.iter().any(|t| t == name);
            let (group, label, hint, icon) = optional_tool_meta(name);
            let tile = div()
                .id(SharedString::from(format!("tool-{name}")))
                .size_full()
                .flex()
                .items_center()
                .gap_2p5()
                .rounded_lg()
                .border_1()
                .px_2p5()
                .py_2()
                .cursor_pointer()
                .when(on, |d| d.bg(Hsla { a: 0.1, ..c.accent }).border_color(Hsla { a: 0.36, ..c.accent }))
                .when(!on, |d| d.bg(c.raised).border_color(Hsla { a: 0.55, ..c.border }).hover(move |d| d.bg(c.hover)))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.update_state(cx, |s| {
                        if s.enabled_tools.iter().any(|t| t == name) {
                            s.enabled_tools.retain(|t| t != name);
                        } else {
                            s.enabled_tools.push(name.to_string());
                        }
                    });
                }))
                .child(
                    div()
                        .size(px(24.0))
                        .flex_shrink_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_md()
                        .bg(if on { Hsla { a: 0.16, ..c.accent } } else { c.panel })
                        .child(Icon::new(icon).size(px(12.5)).text_color(if on { c.accent } else { c.subtle })),
                )
                .child(
                    div().flex_1().min_w_0().flex().flex_col()
                        .child(
                            div().flex().items_center().gap_1p5()
                                .child(div().truncate().text_size(px(12.0)).font_weight(FontWeight::MEDIUM).text_color(c.fg).child(label))
                                .child(div().flex_shrink_0().text_size(px(10.0)).text_color(c.subtle).child(group)),
                        )
                        .when(!hint.is_empty(), |d| d.child(div().truncate().text_size(px(11.0)).text_color(c.subtle).child(hint))),
                )
                .child(
                    div()
                        .w(px(26.0))
                        .h(px(15.0))
                        .flex_shrink_0()
                        .rounded_full()
                        .bg(if on { c.accent } else { c.active })
                        .p(px(2.0))
                        .flex()
                        .when(on, |d| d.justify_end())
                        .child(div().size(px(11.0)).rounded_full().bg(hsla(0.0, 0.0, 1.0, 1.0))),
                );
            tools_grid = tools_grid.child(div().w(relative(0.5)).p(px(4.0)).child(tile));
        }
        let tools_card = surface_card()
            .child(
                div().px_3p5().pt_3().pb_1p5().flex().flex_col().gap_0p5()
                    .child(
                        div().flex().items_center().gap_2()
                            .child(div().text_size(px(12.5)).font_weight(FontWeight::SEMIBOLD).text_color(c.fg).child("Agent tools"))
                            .child(
                                div()
                                    .rounded_full()
                                    .px_1p5()
                                    .py(px(1.0))
                                    .font_family(mono.clone())
                                    .text_size(px(10.0))
                                    .when(enabled_tools_count > 0, |d| d.bg(Hsla { a: 0.14, ..c.accent }).text_color(c.accent))
                                    .when(enabled_tools_count == 0, |d| d.bg(c.raised).text_color(c.subtle))
                                    .child(format!("{enabled_tools_count}/{tools_len} on")),
                            )
                            .children(self.reset_mark("agents.tools", c, cx)),
                    )
                    .child(div().text_size(px(11.5)).text_color(c.subtle).child("Rarely used tools stay off by default so they don't spend context on every request.")),
            )
            .child(tools_grid);

        // 6. Review & progress — unified automation card with hairline dividers.
        let auto_tests = self.rest_bool("review", "autoTests", false, cx);
        let shots = self.rest_bool("review", "screenshots", true, cx);
        let tests_toggle = self.toggle("review-tests", auto_tests, c, cx, move |this, cx| this.set_rest("review", "autoTests", Value::Bool(!auto_tests), cx));
        let shots_toggle = self.toggle("review-shots", shots, c, cx, move |this, cx| this.set_rest("review", "screenshots", Value::Bool(!shots), cx));

        let progress_on = self.rest_bool("progress", "enabled", true, cx);
        let mark_interval = self.reset_mark("progress.interval", c, cx);
        let interval = s.rest.get("progress").and_then(|p| p["intervalSec"].as_u64()).unwrap_or(60);
        let options: Vec<(String, String)> = [(30, "30 seconds"), (60, "minute"), (120, "2 minutes"), (300, "5 minutes")].iter().map(|(v, l)| (v.to_string(), l.to_string())).collect();
        let label = options.iter().find(|(v, _)| *v == interval.to_string()).map_or_else(String::new, |(_, l)| l.clone());
        let on_toggle = self.toggle("progress-on", progress_on, c, cx, move |this, cx| this.set_rest("progress", "enabled", Value::Bool(!progress_on), cx));
        let interval_select = self.select_button_inner("progress-interval", label, options, Some(140.0), 152.0, interval != 60, c, cx, |this, v, cx| this.set_rest("progress", "intervalSec", json!(v.parse::<u64>().unwrap_or(60)), cx));

        let automation = surface_card()
            .child(
                div().px_3p5().pt_3().pb_1p5().flex().flex_col().gap_0p5()
                    .child(div().text_size(px(12.5)).font_weight(FontWeight::SEMIBOLD).text_color(c.fg).child("Review & progress"))
                    .child(div().text_size(px(11.5)).text_color(c.subtle).child("Automatic checks when agents finish, and progress estimates using the LLM under AI.")),
            )
            .child(compact_row("Run tests when an agent finishes".into(), self.reset_mark("review.tests", c, cx), None, tests_toggle, true))
            .child(compact_row("Screenshot the dev server".into(), self.reset_mark("review.screenshots", c, cx), Some("Shows the agent's running app on its review card.".to_string()), shots_toggle, false))
            .child(compact_row("Estimate how far working agents are".into(), self.reset_mark("progress.enabled", c, cx), Some("Agents whose output hasn't changed aren't re-checked.".to_string()), on_toggle, false))
            .when(progress_on, |d| d.child(compact_row("Check every".into(), mark_interval, None, interval_select, false)));

        div()
            .flex()
            .flex_col()
            .gap_4()
            .child(agents_section)
            .child(panes)
            .child(prompt_card)
            .child(tools_card)
            .child(automation)
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
        let (_, (id, label, icon, description)) = page_info(&page);
        let search = self.settings.as_ref().map(|s| s.search.clone()).expect("settings ui");
        let scroll = self.settings.as_ref().map(|s| s.page_scroll.clone()).expect("settings ui");
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
                let changed_here = super::settings_resets::changed_on(self.state.read(cx), pid).len();
                let dot = (pid == "updates" && self.updates.available.is_some()) || changed_here > 0;
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
        let search_box = div().flex().items_center().gap_2().h(px(32.0)).pl_2p5().pr_1().rounded_md().bg(c.app).border_1().border_color(c.edge).text_color(c.subtle)
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
        let foot = div().flex().items_center().justify_between().gap_2().px_3p5().py_2p5().text_size(px(11.0)).text_color(c.subtle)
            .child(div().font_family(mono.clone()).child(format!("winfer {version}")))
            .when_some(update, |d, v| d.child(div().id("settings-update-pill").flex().items_center().gap_1().rounded_full().px_2().py_0p5().bg(Hsla { a: 0.14, ..c.accent }).text_color(c.accent).cursor_pointer()
                .tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(format!("winfer {v} is available")).build(window, cx))
                .on_click(cx.listener(|this, _, window, cx| this.open_settings_page("updates", window, cx)))
                .child(Icon::new(IconName::Download).size(px(11.0))).child("Update")));
        // GPUI clips children to the window's rectangle, not its rounded shape, so the nav rounds its
        // own left corners (the window's radius less its border).
        let nav = div().w(px(232.0)).flex_shrink_0().flex().flex_col().rounded_tl(px(13.0)).rounded_bl(px(13.0)).bg(c.panel)
            .child(div().flex().flex_col().gap_3().px_3().pt_3p5().pb_1()
                .child(div().flex().items_center().gap_2().px_1().text_size(px(14.0)).font_weight(FontWeight::SEMIBOLD).text_color(c.fg)
                    .child(Icon::new(IconName::Settings).size(px(15.0)).text_color(c.accent)).child("Settings"))
                .child(search_box))
            .child(list)
            .child(foot);

        let (head, content): (Div, AnyElement) = if searching {
            let head = div().flex().items_center().min_w_0()
                .child(div().truncate().text_size(px(15.0)).font_weight(FontWeight::SEMIBOLD).text_color(c.fg).child(format!("Results for “{query}”")));
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
                        div().id(SharedString::from(format!("hit-{n}"))).flex().items_center().gap_3().rounded_lg().px_3().py_2p5().cursor_pointer()
                            .hover(move |d| d.bg(c.panel))
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
            let changed_here = super::settings_resets::changed_on(self.state.read(cx), id).len();
            let page_id = id;
            let head = div().flex().items_center().gap_2p5()
                .child(div().size(px(28.0)).flex_shrink_0().flex().items_center().justify_center().rounded_md().bg(Hsla { a: 0.14, ..c.accent }).text_color(c.accent).child(Icon::new(icon).size(px(15.0))))
                .child(div().flex().items_center().gap_2().min_w_0()
                    .child(div().flex_shrink_0().text_size(px(15.0)).font_weight(FontWeight::SEMIBOLD).text_color(c.fg).child(label))
                    .child(div().truncate().text_size(px(12.0)).text_color(c.muted).child(description)))
                .when(changed_here > 0, |d| d.child(div().flex_1()).child(
                    div().id("reset-page").flex().flex_shrink_0().items_center().gap_1p5().h(px(28.0)).px_2p5().rounded_md().border_1().border_color(c.edge).text_size(px(12.0)).text_color(c.muted).cursor_pointer()
                        .hover(move |d| d.bg(c.hover).text_color(c.fg))
                        .tooltip(|window, cx| gpui_kit::component::tooltip::Tooltip::new("Put every changed setting on this page back to its default").build(window, cx))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let mut n = 0;
                            this.update_state(cx, |s| n = super::settings_resets::reset_page(s, page_id));
                            this.toast(format!("Reset {n} setting{}", if n == 1 { "" } else { "s" }), cx);
                        }))
                        .child(Icon::new(IconName::Undo2).size(px(12.0)))
                        .child("Reset page")
                        .child(div().rounded_full().px_1p5().bg(Hsla { a: 0.14, ..c.accent }).font_family(mono.clone()).text_size(px(10.0)).text_color(c.accent).child(changed_here.to_string())),
                ));
            let content: AnyElement = match id {
                "general" => self.page_general(c, cx).into_any_element(),
                "theme" => self.page_theme(c, window, cx).into_any_element(),
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
                    div().id("settings").w(px(1000.0)).max_w(relative(0.94)).h(px(700.0)).max_h(relative(0.9)).flex().overflow_hidden().rounded(px(14.0)).border_1().border_color(c.edge).bg(c.raised).shadow_2xl()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(nav)
                        .child(
                            div().relative().flex_1().min_w_0().flex().flex_col()
                                .child(div().flex_shrink_0().h(px(56.0)).flex().items_center().pl_8().pr(px(96.0)).child(head.flex_1().min_w_0()))
                                .child(
                                    div().id("settings-close").absolute().top(px(16.0)).right(px(16.0)).flex().items_center().gap_1p5().rounded_md().px_1p5().py_1().cursor_pointer().text_color(c.subtle)
                                        .hover(move |d| d.bg(c.hover).text_color(c.fg))
                                        .on_click(cx.listener(|this, _, _, cx| this.update_state(cx, |s| s.settings_open = false)))
                                        .child(Icon::new(IconName::X).size(px(15.0)))
                                        .child(div().rounded_sm().border_1().border_color(c.edge).px_1().font_family(mono.clone()).text_size(px(10.0)).child("esc")),
                                )
                                .child(div().id(SharedString::from(format!("settings-page-{id}-{searching}"))).flex_1().min_h_0().overflow_y_scroll().track_scroll(&scroll).px_8().pt_1().pb_10().child(content)),
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

    /// A dot and a Reset button for a setting that differs from its default (None when it
    /// doesn't). `id` is an entry of [`settings_resets::RESETS`].
    pub(super) fn reset_mark(&self, id: &'static str, c: Colors, cx: &mut Context<Self>) -> Option<AnyElement> {
        let entry = super::settings_resets::find(id)?;
        let changed = (entry.changed)(self.state.read(cx));
        self.reset_mark_with(id, changed, entry.label, c, cx, entry.reset)
    }

    /// The same mark for a setting with its own state, e.g. one agent's model.
    pub(super) fn reset_mark_with(&self, id: impl std::fmt::Display, changed: bool, label: &'static str, c: Colors, cx: &mut Context<Self>, reset: impl Fn(&mut AppState) + 'static) -> Option<AnyElement> {
        use gpui_kit::component::tooltip::Tooltip;
        changed.then(|| {
            div().flex().items_center().gap_1p5().font_weight(FontWeight::NORMAL)
                .child(div().size(px(6.0)).rounded_full().bg(c.accent))
                .child(div().id(SharedString::from(format!("reset-{id}"))).flex().items_center().gap_1().rounded_sm().px_1().text_size(px(11.0)).text_color(c.subtle).cursor_pointer()
                    .hover(move |d| d.bg(c.hover).text_color(c.fg))
                    .tooltip(move |window, cx| Tooltip::new(format!("Put “{label}” back to its default")).build(window, cx))
                    .on_click(cx.listener(move |this, _, _, cx| this.update_state(cx, |s| reset(s))))
                    .child(Icon::new(IconName::Undo2).size(px(10.0)))
                    .child("Reset"))
                .into_any_element()
        })
    }

    /// Scrolls the Agents settings page, once it has laid out, so "Available to install" is at the top.
    pub(super) fn scroll_settings_to_install(&mut self, cx: &mut Context<Self>) {
        if let Some(s) = self.settings.as_ref() {
            s.install_bounds.set(None);
        }
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(std::time::Duration::from_millis(120)).await;
            let _ = this.update(cx, |this, cx| {
                let Some(s) = this.settings.as_ref() else { return };
                let Some(card) = s.install_bounds.get() else { return };
                let (view, offset, max) = (s.page_scroll.bounds(), s.page_scroll.offset(), s.page_scroll.max_offset());
                // The card's top within the content, minus a little room above it.
                let y = offset.y - (card.top() - view.top()) + px(8.0);
                s.page_scroll.set_offset(point(offset.x, y.clamp(-max.y, px(0.0))));
                cx.notify();
            });
        })
        .detach();
    }

    /// Opens a settings page (from the navigation or a search result) and clears the search.
    fn open_settings_page(&mut self, page: &'static str, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(s) = self.settings.as_ref() {
            s.page_scroll.set_offset(point(px(0.0), px(0.0)));
        }
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
    use super::{GROUPS, SEARCH_INDEX, matches_words, search_settings};

    #[test]
    fn every_index_entry_is_on_a_page() {
        let pages: Vec<&str> = GROUPS.iter().flat_map(|(_, p)| p.iter().map(|p| p.0)).collect();
        for (page, label, _) in SEARCH_INDEX.iter() {
            assert!(pages.contains(page), "{label} points to unknown page {page}");
        }
    }

    #[test]
    fn search_finds_settings_by_label_and_words() {
        assert!(search_settings("approve").iter().any(|h| h.0 == "agents"));
        assert!(search_settings("nebula").iter().any(|h| h.0 == "updates"));
        assert_eq!(search_settings("text size")[0].1, "Text size");
        assert!(search_settings("   ").is_empty());
        assert!(search_settings("local").iter().any(|h| h.0 == "winfer-code" && h.1 == "Use local models"));
        assert!(search_settings("host").iter().any(|h| h.0 == "winfer-code" && h.1 == "Host / Port"));
        assert!(matches_words("Host / Port", &["port".to_string()]));
        assert!(!matches_words("Export", &["port".to_string()]));
        assert_eq!(search_settings("auto-approve").iter().filter(|h| h.0 == "agents").count(), 1);
        assert!(search_settings("zzz-nothing").is_empty());
    }
}
