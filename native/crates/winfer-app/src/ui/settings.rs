//! The settings window (port of `src/features/settings/SettingsModal.tsx`).

use super::commands::{BINDINGS, Binding, chord_for, event_chord, format_chord};
use super::*;
use crate::model::AppState;
use crate::theme::THEMES;
use gpui_kit::component::input::{Input, InputEvent, InputState, TextareaState};
use serde_json::json;
use winfer_backend::{editors, provider_updates, providers};

/// A settings page: id, label, icon and a one-line description for its header.
type Page = (&'static str, &'static str, IconName, &'static str);

/// The pages in five groups (the Graphite settings).
const GROUPS: [(&str, &[Page]); 5] = [
    ("Workspace", &[
        ("general", "General", IconName::SlidersHorizontal, "Terminals, your code editor and keyboard shortcuts."),
        ("theme", "Appearance", IconName::Palette, "Light or dark, and the theme for each."),
    ]),
    ("Agents", &[
        ("agents", "Agents", IconName::Bot, "Your agents, new panes, the system prompt, tools and review."),
        ("winfer-code", "WinferCode", IconName::SquareTerminal, "winfer's own agent: interface, model and context."),
    ]),
    ("AI & voice", &[
        ("accounts", "Accounts", IconName::Users, "Sign-ins and the accounts winfer switches between."),
        ("ai", "Smart features", IconName::Sparkles, "The model winfer uses for summaries, titles and progress."),
        ("voice", "Voice", IconName::Mic, "Talk to your agents, and have them talk back."),
    ]),
    ("Extensions", &[
        ("plugins", "Plugins", IconName::Puzzle, "Panels, tools and agents others built for winfer."),
        ("skills", "Skills", IconName::WandSparkles, "Reusable instructions every agent can load."),
    ]),
    ("System", &[
        ("usage", "Usage & limits", IconName::ChartColumn, "Plan limits, spend and warnings."),
        ("mobile", "Mobile", IconName::Smartphone, "Watch and steer agents from your phone."),
        ("connect", "Connect", IconName::Network, "Pair your other machines and servers."),
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
            let page = e.page;
            match merged.iter_mut().find(|m| m.0 == page && m.1 == e.label) {
                Some(m) => {
                    m.2.push(' ');
                    m.2.push_str(&e.words);
                }
                None => merged.push((page, e.label, e.words)),
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
    pub(super) install_bounds: std::rc::Rc<std::cell::Cell<Option<Bounds<Pixels>>>>,
    open_select: Option<String>,
    pub(super) system_prompt: Entity<TextareaState>,
    editors: Vec<(String, String)>,
    pub(super) versions: HashMap<String, Value>,
    pub(super) checking: bool,
    pub(super) updating: Vec<String>,
    pub(super) with_secrets: bool,
    pub(super) passphrase: Entity<InputState>,
    pub(super) confirm: Entity<InputState>,
    pub(super) import_passphrase: Entity<InputState>,
    pub(super) importing: Option<(String, Value)>,
    pub(super) export_result: Option<Result<String, String>>,
    pub(super) import_result: Option<Result<String, String>>,
    pub(super) busy: bool,
    pub(super) diagnostics: Option<Value>,
    pub(super) verbose: bool,
    pub(super) diagnostics_result: Option<Result<String, String>>,
    pub(super) diagnostics_polled: Option<std::time::Instant>,
    search: Entity<InputState>,
    /// The open tab of each tabbed page (see `settings_kit`).
    pub(super) page_tabs: HashMap<&'static str, &'static str>,
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

pub(super) fn plain_button(c: Colors, id: impl Into<ElementId>, label: impl Into<SharedString>) -> Stateful<Div> {
    div().id(id).h(px(28.0)).flex().items_center().gap_1p5().rounded_md().border_1().border_color(c.edge).px_2p5().text_xs().text_color(c.fg).cursor_pointer().hover(move |d| d.bg(c.hover)).child(label.into())
}

impl Root {
    fn settings_ui(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.settings.is_some() {
            return;
        }
        let saved = self.state.read(cx).system_prompt_of();
        let system_prompt = cx.new(|cx| TextareaState::new(window, cx).soft_wrap(true).placeholder("Optional instructions given to every agent…").default_value(saved));
        cx.subscribe(&system_prompt, |_, _, _: &InputEvent, cx| cx.notify()).detach();
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
            page_tabs: HashMap::new(),
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

    pub(super) fn check_versions(&mut self, cx: &mut Context<Self>) {
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

    pub(super) fn update_provider(&mut self, id: String, cx: &mut Context<Self>) {
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

    pub(super) fn select_button(&self, key: &str, current: String, options: Vec<(String, String)>, c: Colors, cx: &mut Context<Self>, on_pick: impl Fn(&mut Self, String, &mut Context<Self>) + Clone + 'static) -> AnyElement {
        self.select_button_inner(key, current, options, Some(200.0), 200.0, false, None, c, cx, on_pick)
    }

    pub(super) fn select_button_flex(&self, key: &str, current: String, options: Vec<(String, String)>, menu_w: f32, changed: bool, c: Colors, cx: &mut Context<Self>, on_pick: impl Fn(&mut Self, String, &mut Context<Self>) + Clone + 'static) -> AnyElement {
        self.select_button_inner(key, current, options, None, menu_w, changed, None, c, cx, on_pick)
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
        recommended: Option<&str>,
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
                let is_rec = recommended.is_some_and(|r| r == value);
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
                        .when(!selected, |d| d.text_color(if is_rec { c.fg } else { c.muted }).hover(move |d| d.bg(c.hover).text_color(c.fg)))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(s) = this.settings.as_mut() {
                                s.open_select = None;
                            }
                            on_pick(this, picked.clone(), cx);
                            cx.notify();
                        }))
                        .child(div().flex_1().min_w_0().truncate().child(label))
                        .when(is_rec, |d| d.child(crate::ui::code_pickers::recommended_badge(c, true)))
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
                grid = grid.child(
                    div().id(SharedString::from(format!("theme-{id}"))).w(px(160.0)).overflow_hidden().rounded_lg().border_1().cursor_pointer()
                        .border_color(if selected { c.accent } else { c.edge })
                        .when(!selected, |d| d.hover(move |d| d.border_color(Hsla { a: 0.5, ..c.accent })))
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

    pub(super) fn render_settings(&mut self, c: &Colors, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.state.read(cx).settings_open {
            self.settings = None;
            return None;
        }
        self.settings_ui(window, cx);
        let c = *c;
        let mut page = self.state.read(cx).settings_page.clone();
        // Local models used to be a page of their own; they are a tab of WinferCode now.
        if page == "local-models" {
            page = "winfer-code".into();
            self.harness_page.ui.tab = super::settings_code::CodeTab::Local;
            self.state.update(cx, |s, _| s.settings_page = page.clone());
        }
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

        let tabs = if searching { None } else { self.settings_page_tabs(id, c, cx) };
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
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.open_settings_page(pid, window, cx);
                                this.open_tab_for_hit(pid, plabel);
                            }))
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
                "connect" => self.page_connect(c, window, cx).into_any_element(),
                "usage" => {
                    // A paired server's numbers, drawn as this machine's are.
                    self.swap_machine_usage();
                    let page = self.page_usage(c, cx).into_any_element();
                    self.swap_machine_usage();
                    page
                }
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
                                .children(tabs)
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

    /// Development aid: scrolls the settings page down by `y` once it has laid out.
    #[cfg(debug_assertions)]
    pub(super) fn scroll_settings_for_dev(&mut self, y: f32, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(std::time::Duration::from_millis(1500)).await;
            let _ = this.update(cx, |this, cx| {
                if let Some(s) = this.settings.as_ref() {
                    let max = s.page_scroll.max_offset();
                    s.page_scroll.set_offset(point(px(0.0), (-px(y)).max(-max.y)));
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Scrolls the Agents settings page, once it has laid out, so "Available to install" is at the top.
    pub(super) fn scroll_settings_to_install(&mut self, cx: &mut Context<Self>) {
        if let Some(s) = self.settings.as_mut() {
            s.install_bounds.set(None);
            s.page_tabs.insert("agents", "agents");
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

    /// Scrolls the settings page back to its top (e.g. on another tab).
    pub(super) fn scroll_settings_top(&mut self) {
        if let Some(s) = self.settings.as_mut() {
            s.page_scroll.set_offset(point(px(0.0), px(0.0)));
            s.open_select = None;
        }
    }

    /// Opens a settings page (from the navigation or a search result) and clears the search.
    pub(super) fn open_settings_page(&mut self, page: &'static str, window: &mut Window, cx: &mut Context<Self>) {
        if page == "local-models" {
            self.harness_page.ui.tab = super::settings_code::CodeTab::Local;
            return self.open_settings_page("winfer-code", window, cx);
        }
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

    #[test]
    fn winfer_provider_is_always_ordered_first() {
        use crate::model::{AgentProvider, AppState};
        use winfer_backend::providers::Provider;
        let make = |id: &str| AgentProvider {
            info: Provider {
                id: id.into(),
                name: id.into(),
                command: Some(id.into()),
                args: vec![],
                resume_args: vec![],
                resume_session_args: vec![],
                aliases: vec![],
                status_hooks: false,
                prompt_arg: false,
                prompt_flag: None,
                approve_input: None,
                model_arg: None,
                models: vec![],
                models_command: vec![],
                model_command: None,
                auto_approve_args: vec![],
                system_prompt_args: vec![],
                update: None,
            },
            installed: true,
        };
        let mut state = AppState::default();
        state.set_providers(vec![make("claude"), make("codex"), make("agy"), make("winfer"), make("shell")]);
        let ids: Vec<&str> = state.providers.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids, vec!["winfer", "claude", "codex", "agy", "shell"]);
        assert_eq!(state.providers[0].name, "WinferCode");
        assert_eq!(state.default_provider_id(), "winfer");
    }
}
