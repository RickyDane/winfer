//! Settings → Plugins in three tabs (mockups/settings-overhaul/plugins.html): Installed (with
//! updates), Discover (the store) and Options (title-bar names, your own plugins).

use super::plugins::matches_search;
use super::settings_kit::{PageTab, alpha, badge, empty_state, icon_tile, panel, tab_count, tab_dot};
use super::*;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use serde_json::Value;
use std::collections::HashMap as Map;
use std::time::Instant;
use winfer_backend::plugins as backend_plugins;

fn trust_warning(name: &str) -> String {
    format!("“{name}” runs its own code with full access to your files, terminals and network. Only turn on plugins you trust.")
}

/// What a plugin adds, each with its icon: tools, agents, recipes and code of its own.
fn adds_of(plugin: &Value) -> Vec<(IconName, String)> {
    let m = &plugin["manifest"];
    if !m.is_object() {
        return Vec::new();
    }
    let mut parts = Vec::new();
    for (key, one, icon) in [("tools", "tool", IconName::Wrench), ("providers", "agent", IconName::Bot), ("recipes", "recipe", IconName::BookOpen)] {
        let n = m[key].as_array().map_or(0, Vec::len);
        if n > 0 {
            parts.push((icon, format!("{n} {one}{}", if n == 1 { "" } else { "s" })));
        }
    }
    if m["main"].is_string() {
        parts.push((IconName::Code, "code".to_string()));
    }
    parts
}

/// The most used tags among plugins left to install, as filters.
pub fn top_tags(entries: &[Value]) -> Vec<String> {
    let mut counts: Map<String, usize> = Map::new();
    for t in entries.iter().flat_map(|e| e["tags"].as_array().into_iter().flatten()).filter_map(Value::as_str) {
        *counts.entry(t.to_string()).or_default() += 1;
    }
    let mut tags: Vec<(String, usize)> = counts.into_iter().filter(|(_, n)| *n > 1).collect();
    tags.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    tags.into_iter().take(8).map(|t| t.0).collect()
}

fn str_of(v: &Value) -> &str {
    v.as_str().unwrap_or_default()
}

/// A steady color for a plugin or skill, from its id (they have no icons of their own).
pub(super) fn tint_of(id: &str) -> Hsla {
    const TINTS: [&str; 8] = ["#7c8cff", "#4f8cff", "#25a28f", "#f5b544", "#fb7185", "#a78bfa", "#38bdf8", "#7ee787"];
    let h = id.bytes().fold(0u32, |h, b| h.wrapping_mul(31).wrapping_add(b as u32));
    crate::theme::hex(TINTS[h as usize % TINTS.len()])
}

/// A plugin's icon, from the main thing it adds.
fn plugin_icon(plugin: &Value) -> IconName {
    let m = &plugin["manifest"];
    let has = |k: &str| m[k].as_array().is_some_and(|a| !a.is_empty());
    if has("providers") {
        IconName::Bot
    } else if has("tools") {
        IconName::Wrench
    } else if has("recipes") {
        IconName::BookOpen
    } else if m["main"].is_string() {
        IconName::Code
    } else {
        IconName::Puzzle
    }
}

/// "just now", "4 min ago", "2 h ago".
fn ago(at: Instant) -> String {
    match at.elapsed().as_secs() {
        s if s < 60 => "just now".into(),
        s if s < 3600 => format!("{} min ago", s / 60),
        s => format!("{} h ago", s / 3600),
    }
}

#[derive(Clone, Copy, PartialEq)]
pub(super) enum Btn {
    Primary,
    Plain,
}

/// A 28px button with an optional icon (24px with `small`).
pub(super) fn button(c: Colors, id: impl Into<ElementId>, icon: Option<IconName>, label: impl Into<SharedString>, style: Btn, enabled: bool, small: bool) -> Stateful<Div> {
    let d = div().id(id).flex_shrink_0().flex().items_center().gap_1p5().h(px(if small { 24.0 } else { 28.0 })).px(px(if small { 8.0 } else { 10.0 })).rounded_md().text_size(px(if small { 11.5 } else { 12.0 }));
    let d = match style {
        Btn::Primary => d.bg(c.accent).text_color(c.on_accent).font_weight(FontWeight::MEDIUM),
        Btn::Plain => d.bg(c.raised).border_1().border_color(c.edge).text_color(c.fg),
    };
    d.when(enabled, |d| d.cursor_pointer().hover(move |d| if style == Btn::Primary { d.opacity(0.9) } else { d.bg(c.hover) }))
        .when(!enabled, |d| d.opacity(0.45))
        .when_some(icon, |d, i| d.child(Icon::new(i).size(px(13.0))))
        .child(label.into())
}

/// A search field: icon, input and an optional count on the right.
pub(super) fn search_field(c: Colors, input: &Entity<InputState>, right: Option<String>) -> Div {
    div().flex().items_center().gap_2().h(px(30.0)).pl_2p5().pr_2p5().rounded(px(7.0)).bg(c.raised).border_1().border_color(c.edge).text_color(c.subtle)
        .child(Icon::new(IconName::Search).size(px(13.0)))
        .child(div().flex_1().min_w_0().child(Input::new(input).appearance(false).text_size(px(12.5))))
        .when_some(right, |d, t| d.child(div().flex_shrink_0().text_size(px(11.5)).child(t)))
}

/// A note with an icon (the trust warnings).
pub(super) fn note(c: Colors, icon: IconName, text: impl Into<SharedString>) -> Div {
    panel(c).flex_row().items_start().gap_2p5().px_3().py_2p5().text_size(px(12.0)).text_color(c.muted)
        .child(Icon::new(icon).size(px(15.0)).flex_shrink_0().mt(px(1.0)).text_color(c.accent))
        .child(div().flex_1().min_w_0().child(text.into()))
}

/// An update banner: text and one button.
pub(super) fn update_banner(c: Colors, text: String, action: Stateful<Div>) -> Div {
    div().flex().items_center().gap_2p5().pl_3().pr_2().py_2().rounded(px(9.0)).bg(alpha(c.accent, 0.12)).border_1().border_color(alpha(c.accent, 0.35)).text_size(px(12.5)).text_color(c.fg)
        .child(Icon::new(IconName::CircleArrowUp).size(px(15.0)).flex_shrink_0().text_color(c.accent))
        .child(div().flex_1().min_w_0().child(text))
        .child(action)
}

/// A chip that filters a list (tags).
fn filter_chip(c: Colors, id: SharedString, label: String, on: bool) -> Stateful<Div> {
    div().id(id).flex().h(px(24.0)).items_center().rounded(px(6.0)).px_2().text_size(px(11.5)).cursor_pointer()
        .when(on, |d| d.bg(c.accent).text_color(c.on_accent))
        .when(!on, |d| d.border_1().border_color(c.edge).text_color(c.muted).hover(move |d| d.bg(c.hover).text_color(c.fg)))
        .child(label)
}

#[derive(Default)]
pub(super) struct PluginsPage {
    search: Option<Entity<InputState>>,
    catalog: Option<Vec<Value>>,
    catalog_loading: bool,
    catalog_error: Option<String>,
    /// When the store was last read.
    catalog_at: Option<Instant>,
    tag: Option<String>,
    expanded: Option<String>,
    busy: Option<String>,
    /// The plugin whose ⋯ menu is open.
    menu: Option<String>,
}

impl Root {
    fn load_catalog(&mut self, cx: &mut Context<Self>) {
        self.plugin_page.catalog_loading = true;
        self.plugin_page.catalog_error = None;
        cx.spawn(async move |this, cx| {
            let found = tauri::async_runtime::spawn(backend_plugins::plugins_catalog()).await.map_err(|e| e.to_string()).and_then(|r| r);
            let _ = this.update(cx, |this, cx| {
                this.plugin_page.catalog_loading = false;
                match found {
                    Ok(list) => {
                        this.plugin_page.catalog = serde_json::to_value(list).ok().and_then(|v| v.as_array().cloned());
                        this.plugin_page.catalog_at = Some(Instant::now());
                    }
                    Err(e) => this.plugin_page.catalog_error = Some(e),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn plugin_work(&mut self, id: String, done: Option<String>, work: impl FnOnce() -> Result<(), String> + Send + 'static, cx: &mut Context<Self>) {
        self.plugin_page.busy = Some(id);
        self.plugin_page.menu = None;
        cx.spawn(async move |this, cx| {
            let result = backend::background(work).await.unwrap_or_else(|e| Err(e.to_string()));
            let _ = this.update(cx, |this, cx| {
                this.plugin_page.busy = None;
                match result {
                    Ok(()) => {
                        if let Some(text) = done {
                            this.toast(text, cx);
                        }
                    }
                    Err(e) => this.toast(e, cx),
                }
                this.reload_plugins(cx);
            });
        })
        .detach();
    }

    fn install_plugin(&mut self, entry: Value, update: bool, window: &mut Window, cx: &mut Context<Self>) {
        let (id, name, version) = (str_of(&entry["id"]).to_string(), str_of(&entry["name"]).to_string(), str_of(&entry["version"]).to_string());
        let done = if update { format!("Updated {name} to {version}") } else { format!("Installed {name}") };
        let run = move |this: &mut Root, cx: &mut Context<Root>| {
            let target = id.clone();
            this.plugin_work(id.clone(), Some(done.clone()), move || tauri::async_runtime::block_on(backend_plugins::plugins_install(target)), cx);
        };
        if update {
            run(self, cx);
        } else {
            self.confirm(&format!("Install {name}"), trust_warning(&name), "Install", window, cx, run);
        }
    }

    fn update_plugins(&mut self, list: Vec<Value>, window: &mut Window, cx: &mut Context<Self>) {
        if list.len() == 1 {
            return self.install_plugin(list[0].clone(), true, window, cx);
        }
        let ids: Vec<String> = list.iter().map(|e| str_of(&e["id"]).to_string()).collect();
        let done = format!("Updated {} plugins", ids.len());
        self.plugin_work("*".into(), Some(done), move || {
            for id in ids {
                tauri::async_runtime::block_on(backend_plugins::plugins_install(id))?;
            }
            Ok(())
        }, cx);
    }

    fn uninstall_plugin(&mut self, plugin: Value, window: &mut Window, cx: &mut Context<Self>) {
        self.plugin_page.menu = None;
        let id = str_of(&plugin["id"]).to_string();
        let name = plugin["manifest"]["name"].as_str().unwrap_or(&id).to_string();
        self.confirm("Uninstall plugin", format!("Remove “{name}” and its files? Data it stored (like saved secrets) is kept for a reinstall."), "Uninstall", window, cx, move |this, cx| {
            let target = id.clone();
            this.plugin_work(id.clone(), None, move || backend_plugins::plugins_uninstall(target), cx);
        });
    }

    fn toggle_plugin(&mut self, plugin: Value, enabled: bool, window: &mut Window, cx: &mut Context<Self>) {
        let id = str_of(&plugin["id"]).to_string();
        let m = &plugin["manifest"];
        // Plugin code and agent commands run with winfer's access to files, terminals and the network.
        let risky = enabled && plugin["fromStore"] != true && (m["main"].is_string() || m["providers"].as_array().is_some_and(|p| !p.is_empty()));
        let run = move |this: &mut Root, cx: &mut Context<Root>| {
            let target = id.clone();
            this.plugin_work(id.clone(), None, move || backend_plugins::plugins_set_enabled(target, enabled), cx);
        };
        if risky {
            self.confirm("Turn on plugin", trust_warning(str_of(&m["name"])), "Turn on", window, cx, run);
        } else {
            run(self, cx);
        }
    }

    /// Store entries newer than the installed store plugins.
    fn plugin_updates(&self) -> Vec<Value> {
        let Some(catalog) = self.plugin_page.catalog.as_ref() else { return Vec::new() };
        self.plugins.infos.iter().filter(|p| p["fromStore"] == true).filter_map(|p| {
            let current = p["storeVersion"].as_str().or(p["manifest"]["version"].as_str());
            catalog.iter().find(|e| e["id"] == p["id"] && e["version"].as_str() != current).cloned()
        }).collect()
    }

    /// Store entries that aren't installed.
    fn store_left(&self) -> Vec<Value> {
        let plugins = &self.plugins.infos;
        self.plugin_page.catalog.iter().flatten().filter(|e| !plugins.iter().any(|p| p["id"] == e["id"])).cloned().collect()
    }

    fn plugins_default_tab(&self) -> &'static str {
        if self.plugins.infos.iter().any(|p| p["manifest"].is_object()) { "installed" } else { "discover" }
    }

    /// The tab bar: Installed (with a dot for updates), Discover and Options, and when the store
    /// was read on the right.
    pub(super) fn plugins_tabs(&mut self, c: Colors, cx: &mut Context<Self>) -> Div {
        let mono = crate::fonts::mono(cx);
        let has_updates = !self.plugin_updates().is_empty();
        let installed = div().flex().items_center().gap_1p5().child(tab_count(c, &mono, self.plugins.infos.len())).when(has_updates, |d| d.child(tab_dot(c.accent)));
        let discover = self.plugin_page.catalog.is_some().then(|| tab_count(c, &mono, self.store_left().len()));
        let default = self.plugins_default_tab();
        let tab = self.page_tab("plugins", default);
        let tabs = vec![
            PageTab::new("installed", "Installed", IconName::PackageCheck).extra(Some(installed.into_any_element())),
            PageTab::new("discover", "Discover", IconName::Compass).extra(discover),
            PageTab::new("options", "Options", IconName::Settings2),
        ];
        let p = &self.plugin_page;
        let status = if p.catalog_loading {
            "Reading the store…".to_string()
        } else if p.catalog_error.is_some() {
            "Couldn't reach the store".to_string()
        } else {
            p.catalog_at.map_or(String::new(), |at| format!("Store checked {}", ago(at)))
        };
        let right = (tab != "options").then(|| {
            div().h(px(38.0)).flex().items_center().gap_1p5().text_size(px(11.5)).text_color(c.subtle)
                .child(Icon::new(IconName::Store).size(px(13.0)))
                .child(status)
                .child(div().id("plugins-store-refresh").size(px(24.0)).flex().items_center().justify_center().rounded_md().cursor_pointer()
                    .hover(move |d| d.bg(c.hover).text_color(c.fg))
                    .tooltip(|window, cx| gpui_kit::component::tooltip::Tooltip::new("Check the store again").build(window, cx))
                    .on_click(cx.listener(|this, _, _, cx| {
                        if !this.plugin_page.catalog_loading {
                            this.load_catalog(cx);
                        }
                    }))
                    .child(Icon::new(IconName::RefreshCw).size(px(12.0))))
        });
        self.page_tab_bar("plugins", default, tabs, c, cx).child(div().flex_1()).children(right)
    }

    pub(super) fn page_plugins(&mut self, c: Colors, window: &mut Window, cx: &mut Context<Self>) -> Div {
        if self.plugin_page.search.is_none() {
            let input = cx.new(|cx| InputState::new(window, cx));
            cx.subscribe(&input, |_, _, _: &InputEvent, cx| cx.notify()).detach();
            self.plugin_page.search = Some(input);
        }
        if self.plugin_page.catalog.is_none() && !self.plugin_page.catalog_loading && self.plugin_page.catalog_error.is_none() {
            self.load_catalog(cx);
        }
        let tab = self.page_tab("plugins", self.plugins_default_tab());
        let search = self.plugin_page.search.clone().unwrap();
        let placeholder = if tab == "discover" { "Search the store" } else { "Search installed plugins" };
        search.update(cx, |i, cx| i.set_placeholder(placeholder, window, cx));
        let query = search.read(cx).value().to_string();
        let body = match tab {
            "discover" => self.plugins_discover(&search, &query, c, cx),
            "options" => self.plugins_options(c, cx),
            _ => self.plugins_installed(&search, &query, c, cx),
        };
        body.pt_4()
    }

    fn plugins_discover(&mut self, search: &Entity<InputState>, query: &str, c: Colors, cx: &mut Context<Self>) -> Div {
        let mut out = div().flex().flex_col().gap_4()
            .child(note(c, IconName::ShieldAlert, "Plugins run their own code with full access to your files, terminals and network. Install only plugins you trust; winfer asks once more before installing."));
        if let Some(e) = self.plugin_page.catalog_error.clone() {
            return out.child(panel(c).items_center().gap_2().py_6().px_4()
                .child(Icon::new(IconName::CloudOff).size(px(22.0)).text_color(c.subtle))
                .child(div().text_size(px(12.5)).text_color(c.muted).child("Couldn't load the plugin store"))
                .child(div().text_size(px(11.0)).text_color(c.subtle).font_family(crate::fonts::mono(cx)).child(e))
                .child(button(c, "catalog-retry", Some(IconName::RefreshCw), "Retry", Btn::Plain, true, false).on_click(cx.listener(|this, _, _, cx| this.load_catalog(cx)))));
        }
        if self.plugin_page.catalog.is_none() {
            return out.child(panel(c).child(empty_state(c, IconName::LoaderCircle, "Loading the store…")));
        }
        let not_installed = self.store_left();
        let tag = self.plugin_page.tag.clone();
        let mut tags = top_tags(&not_installed);
        if let Some(t) = tag.clone().filter(|t| !tags.contains(t)) {
            tags.push(t);
        }
        let shown: Vec<Value> = not_installed.iter().filter(|e| {
            let tag_list: Vec<&str> = e["tags"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
            let mut fields = vec![str_of(&e["id"]), str_of(&e["name"]), str_of(&e["description"]), str_of(&e["author"])];
            fields.extend(tag_list.iter());
            tag.as_ref().is_none_or(|t| tag_list.contains(&t.as_str())) && matches_search(query, &fields)
        }).cloned().collect();
        let count = (!query.is_empty() || tag.is_some()).then(|| format!("{} of {}", shown.len(), not_installed.len()));
        out = out.child(search_field(c, search, count));
        if !tags.is_empty() {
            let mut row = div().flex().flex_wrap().items_center().gap_1p5().mt(px(-6.0)).child(filter_chip(c, "tag-all".into(), "All".into(), tag.is_none()).on_click(cx.listener(|this, _, _, cx| {
                this.plugin_page.tag = None;
                cx.notify();
            })));
            for t in tags {
                let (on, pick) = (tag.as_ref() == Some(&t), t.clone());
                row = row.child(filter_chip(c, format!("tag-{t}").into(), t, on).on_click(cx.listener(move |this, _, _, cx| {
                    this.plugin_page.tag = if on { None } else { Some(pick.clone()) };
                    cx.notify();
                })));
            }
            out = out.child(row);
        }
        if shown.is_empty() {
            let (icon, text) = if not_installed.is_empty() { (IconName::PackageCheck, "You have every plugin from the store".to_string()) } else { (IconName::SearchX, "Nothing in the store matches".to_string()) };
            return out.child(panel(c).child(empty_state(c, icon, text)));
        }
        let busy = self.plugin_page.busy.clone();
        let mono = crate::fonts::mono(cx);
        // Two even columns: each cell is half the width with the gap as padding (the grid's
        // negative margin takes the outer padding back).
        let mut grid = div().flex().flex_wrap().mx(px(-5.0)).my(px(-5.0));
        for e in shown {
            let id = str_of(&e["id"]).to_string();
            let open = self.plugin_page.expanded.as_ref() == Some(&id);
            let toggle = id.clone();
            let author = e["author"].as_str().filter(|a| !a.is_empty()).map_or("Unknown author".to_string(), |a| format!("by {a}"));
            let version = e["version"].as_str().filter(|v| !v.is_empty()).map_or(String::new(), |v| format!(" · {v}"));
            let description = e["description"].as_str().filter(|d| !d.is_empty()).map(str::to_string);
            let tags: Vec<String> = e["tags"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect();
            let installing = busy.as_ref() == Some(&id);
            let entry = e.clone();
            let enabled = busy.is_none();
            let action: AnyElement = if installing {
                busy_label(c, "Installing…").into_any_element()
            } else {
                button(c, SharedString::from(format!("install-{id}")), Some(IconName::Download), "Install", Btn::Primary, enabled, true).on_click(cx.listener(move |this, _, window, cx| {
                    if enabled {
                        this.install_plugin(entry.clone(), false, window, cx);
                    }
                })).into_any_element()
            };
            grid = grid.child(div().w(relative(0.5)).p(px(5.0)).child(panel(c).size_full().gap_2().p_3()
                .child(div().flex().items_center().gap_2p5()
                    .child(icon_tile(tint_of(&id), IconName::Puzzle, 30.0))
                    .child(div().flex_1().min_w_0().flex().flex_col()
                        .child(div().truncate().text_size(px(13.0)).font_weight(FontWeight::MEDIUM).text_color(c.fg).child(str_of(&e["name"]).to_string()))
                        .child(div().truncate().text_size(px(11.0)).text_color(c.subtle).child(format!("{author}{version}")))))
                .child(div().id(SharedString::from(format!("desc-{id}"))).text_size(px(12.0)).text_color(if description.is_some() { c.muted } else { c.subtle }).cursor_pointer().when(!open, |d| d.line_clamp(3))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.plugin_page.expanded = if open { None } else { Some(toggle.clone()) };
                        cx.notify();
                    }))
                    .child(description.unwrap_or_else(|| "No description.".into())))
                .child(div().flex_1())
                .child(div().flex().items_end().gap_2()
                    .child(div().flex().min_w_0().flex_1().flex_wrap().gap_1().children(tags.into_iter().take(if open { usize::MAX } else { 2 }).map(|t| div().rounded(px(4.0)).bg(c.active).px_1p5().py_0p5().font_family(mono.clone()).text_size(px(10.0)).text_color(c.subtle).child(t))))
                    .child(action))));
        }
        out.child(grid)
    }

    fn plugins_installed(&mut self, search: &Entity<InputState>, query: &str, c: Colors, cx: &mut Context<Self>) -> Div {
        let plugins = self.plugins.infos.clone();
        let updates = self.plugin_updates();
        let busy = self.plugin_page.busy.clone();
        let free = busy.is_none();
        let enabled_count = plugins.iter().filter(|p| p["enabled"] == true && p["error"].is_null()).count();
        let mut page = div().flex().flex_col().gap_4();
        if !updates.is_empty() && query.is_empty() {
            let names = updates.iter().map(|u| str_of(&u["name"])).collect::<Vec<_>>().join(", ");
            let text = if updates.len() == 1 { format!("An update for {names} is available.") } else { format!("{} plugin updates are available: {names}.", updates.len()) };
            let list = updates.clone();
            let action: AnyElement = if busy.as_deref() == Some("*") {
                busy_label(c, "Updating…").into_any_element()
            } else {
                button(c, "plugins-update-all", None, if updates.len() == 1 { "Update" } else { "Update all" }, Btn::Primary, free, true).on_click(cx.listener(move |this, _, window, cx| {
                    if free {
                        this.update_plugins(list.clone(), window, cx);
                    }
                })).into_any_element()
            };
            page = page.child(update_banner(c, text, div().id("plugins-update-wrap").child(action)));
        }
        let count = (!plugins.is_empty()).then(|| format!("{enabled_count} of {} on", plugins.len()));
        page = page.child(search_field(c, search, count));
        let shown: Vec<&Value> = plugins.iter().filter(|p| matches_search(query, &[str_of(&p["id"]), str_of(&p["manifest"]["name"]), str_of(&p["manifest"]["description"])])).collect();
        if shown.is_empty() {
            let (icon, text) = if plugins.is_empty() { (IconName::Puzzle, "No plugins installed. Plugins add pages, agents, recipes, commands, voice actions and tools for your agents.".to_string()) } else { (IconName::SearchX, format!("No installed plugin matches “{query}”")) };
            return page.child(panel(c).child(empty_state(c, icon, text)));
        }
        let mono = crate::fonts::mono(cx);
        let mut list = panel(c);
        for (i, p) in shown.into_iter().enumerate() {
            let id = str_of(&p["id"]).to_string();
            let name = p["manifest"]["name"].as_str().unwrap_or(&id).to_string();
            let enabled = p["enabled"] == true;
            let error = p["error"].as_str().map(str::to_string).or_else(|| enabled.then(|| self.plugins.runtime_errors.get(&id).cloned()).flatten());
            let current = p["storeVersion"].as_str().or(p["manifest"]["version"].as_str()).map(str::to_string);
            let latest = updates.iter().find(|e| e["id"] == p["id"]).cloned();
            let local = p["fromStore"] != true;
            let has_error = !p["error"].is_null();
            let description = p["manifest"]["description"].as_str().filter(|d| !d.is_empty()).map(str::to_string);
            let dim = !enabled && error.is_none();
            let mut adds = div().flex().flex_wrap().gap_1p5();
            for (icon, label) in adds_of(p) {
                let label = if label == "code" { "runs code".to_string() } else { label };
                adds = adds.child(div().flex().items_center().gap_1().h(px(20.0)).px_1p5().rounded(px(5.0)).bg(c.app).text_size(px(11.0)).text_color(c.muted).child(Icon::new(icon).size(px(11.0)).text_color(c.subtle)).child(label));
            }
            let side: AnyElement = if busy.as_ref() == Some(&id) {
                busy_label(c, "Working…").into_any_element()
            } else {
                let flip = p.clone();
                let toggle = (!has_error).then(|| {
                    let t = self.toggle(SharedString::from(format!("plugin-on-{id}")), enabled, c, cx, |_, _| {});
                    div().id(SharedString::from(format!("plugin-area-{id}"))).on_click(cx.listener(move |this, _, window, cx| if this.plugin_page.busy.is_none() { this.toggle_plugin(flip.clone(), !enabled, window, cx) })).child(t)
                });
                let update = latest.clone().map(|l| button(c, SharedString::from(format!("plugin-update-{id}")), None, "Update", Btn::Plain, free, true).on_click(cx.listener(move |this, _, window, cx| if free { this.install_plugin(l.clone(), true, window, cx) })));
                div().flex().items_center().gap_1p5()
                    .children(update)
                    .child(self.plugin_menu(p, latest.clone(), local, free, c, cx))
                    .when(has_error, |d| d.child(badge(c.danger, "Error")))
                    .children(toggle)
                    .into_any_element()
            };
            list = list.child(div().flex().items_start().gap_3().px_3p5().py_3().when(i > 0, |d| d.border_t_1().border_color(c.border))
                .child(div().when(dim, |d| d.opacity(0.55)).child(icon_tile(tint_of(&id), plugin_icon(p), 30.0)))
                .child(div().min_w_0().flex_1().flex().flex_col().gap_1().when(dim, |d| d.opacity(0.6))
                    .child(div().flex().min_w_0().items_center().gap_1p5()
                        .child(div().truncate().text_size(px(13.0)).font_weight(FontWeight::MEDIUM).text_color(c.fg).child(name))
                        .when_some(current, |d, v| d.child(div().flex_shrink_0().font_family(mono.clone()).text_size(px(11.0)).text_color(c.subtle).child(v)))
                        .when(local, |d| d.child(badge(c.subtle, "local")))
                        .when_some(latest, |d, l| d.child(badge(c.accent, format!("↑ {}", str_of(&l["version"]))))))
                    .when_some(description, |d, t| d.child(div().line_clamp(2).text_size(px(12.0)).text_color(c.muted).child(t)))
                    .child(adds)
                    .when_some(error, |d, e| d.child(div().mt_1().flex().items_start().gap_1p5().rounded_md().bg(alpha(c.danger, 0.1)).px_2().py_1p5().text_size(px(11.5)).text_color(c.danger)
                        .child(Icon::new(IconName::CircleAlert).size(px(12.0)).flex_shrink_0().mt(px(1.0)))
                        .child(div().flex_1().min_w_0().child(e)))))
                .child(div().flex_shrink_0().pt(px(2.0)).child(side)));
        }
        page.child(list)
    }

    /// The ⋯ button of an installed plugin and its menu: update, show in folder, uninstall.
    fn plugin_menu(&mut self, plugin: &Value, latest: Option<Value>, local: bool, free: bool, c: Colors, cx: &mut Context<Self>) -> AnyElement {
        let id = str_of(&plugin["id"]).to_string();
        let open = self.plugin_page.menu.as_ref() == Some(&id);
        let toggle_id = id.clone();
        let trigger = div().id(SharedString::from(format!("plugin-menu-{id}"))).size(px(28.0)).flex().items_center().justify_center().rounded_md().text_color(c.subtle).cursor_pointer()
            .when(open, |d| d.bg(c.hover).text_color(c.fg))
            .hover(move |d| d.bg(c.hover).text_color(c.fg))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.plugin_page.menu = if this.plugin_page.menu.as_ref() == Some(&toggle_id) { None } else { Some(toggle_id.clone()) };
                cx.notify();
            }))
            .child(Icon::new(IconName::Ellipsis).size(px(15.0)));
        if !open {
            return trigger.into_any_element();
        }
        let item = |key: &str, icon: IconName, label: String, color: Hsla| {
            div().id(SharedString::from(format!("plugin-menu-{key}-{id}"))).flex().items_center().gap_2().h(px(28.0)).px_2().rounded_md().text_size(px(12.5)).text_color(color).cursor_pointer()
                .hover(move |d| d.bg(c.hover))
                .child(Icon::new(icon).size(px(13.0)))
                .child(label)
        };
        let dir = str_of(&plugin["dir"]).to_string();
        let remove = plugin.clone();
        let menu = div().id(SharedString::from(format!("plugin-menu-list-{id}"))).w(px(196.0)).mt_1().p_1().flex().flex_col().rounded_lg().bg(c.raised).border_1().border_color(c.edge).shadow_lg()
            .occlude()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.plugin_page.menu = None;
                cx.notify();
            }))
            .when_some(latest.filter(|_| free), |d, l| d.child(item("update", IconName::CircleArrowUp, format!("Update to {}", str_of(&l["version"])), c.fg).on_click(cx.listener(move |this, _, window, cx| this.install_plugin(l.clone(), true, window, cx)))))
            .when(!dir.is_empty(), |d| d.child(item("folder", IconName::FolderOpen, if local { "Show in folder".into() } else { "Show files".into() }, c.fg).on_click(cx.listener(move |this, _, _, cx| {
                sidebar::reveal_path(&dir);
                this.plugin_page.menu = None;
                cx.notify();
            }))))
            .child(div().my_1().h(px(1.0)).bg(c.border))
            .child(item("remove", IconName::Trash, "Uninstall".into(), c.danger).on_click(cx.listener(move |this, _, window, cx| if free { this.uninstall_plugin(remove.clone(), window, cx) })));
        div().relative().child(trigger).child(deferred(anchored().snap_to_window_with_margin(px(8.0)).child(menu)).with_priority(5)).into_any_element()
    }

    fn plugins_options(&mut self, c: Colors, cx: &mut Context<Self>) -> Div {
        let icons_only = self.state.read(cx).page_tab_icons_only;
        let names = self.toggle("plugin-icons", icons_only, c, cx, |this, cx| this.update_state(cx, |s| s.page_tab_icons_only = !s.page_tab_icons_only));
        let mark = self.reset_mark("plugins.icons", c, cx);
        let mono = crate::fonts::mono(cx);
        let head = |icon: Div, title: &str, mark: Option<AnyElement>, sub: &str, right: AnyElement| {
            div().flex().items_center().gap_3()
                .child(icon)
                .child(div().flex_1().min_w_0().flex().flex_col().gap_0p5()
                    .child(div().flex().items_center().gap_2().text_size(px(13.0)).font_weight(FontWeight::MEDIUM).text_color(c.fg).child(title.to_string()).children(mark))
                    .child(div().text_size(px(12.0)).text_color(c.subtle).child(sub.to_string())))
                .child(right)
        };
        // How the title bar's plugin tabs look: the installed plugins that are on, by name.
        let mut pages: Vec<String> = self.plugins.infos.iter().filter(|p| p["enabled"] == true && p["error"].is_null()).filter_map(|p| p["manifest"]["name"].as_str().map(str::to_string)).take(3).collect();
        if pages.is_empty() {
            pages.push("Plugin".into());
        }
        let mut bar = div().flex().items_center().gap_1().h(px(34.0)).px_2p5().rounded(px(8.0)).bg(c.app)
            .child(div().flex().gap_1().mr_2().children((0..3).map(|_| div().size(px(8.0)).rounded_full().bg(c.active))))
            .child(div().flex().items_center().gap_1p5().h(px(24.0)).px_2().rounded_md().text_size(px(11.5)).text_color(c.muted).child(Icon::new(IconName::LayoutDashboard).size(px(12.0))).child("Panes"));
        for (i, name) in pages.into_iter().enumerate() {
            let on = i == 0;
            bar = bar.child(div().flex().items_center().gap_1p5().h(px(24.0)).px_2().rounded_md().text_size(px(11.5))
                .when(on, |d| d.bg(c.active).text_color(c.fg)).when(!on, |d| d.text_color(c.muted))
                .child(Icon::new(IconName::Puzzle).size(px(12.0)))
                .when(!icons_only, |d| d.child(name)));
        }
        bar = bar.child(div().flex_1()).child(div().text_size(px(10.5)).text_color(c.subtle).child("title bar preview"));
        let dir = backend_plugins::plugins_dir().unwrap_or_default();
        let shown_dir = std::env::var("HOME").ok().filter(|h| !h.is_empty() && dir.starts_with(h.as_str())).map_or(dir.clone(), |h| format!("~{}", &dir[h.len()..]));
        let local = self.plugins.infos.iter().filter(|p| p["fromStore"] != true).count();
        let step = |n: &str, text: &str| {
            div().flex_1().min_w_0().flex().items_start().gap_2().px_2p5().py_2().rounded(px(8.0)).bg(c.app).text_size(px(12.0)).text_color(c.muted)
                .child(div().size(px(18.0)).flex_shrink_0().flex().items_center().justify_center().rounded_full().bg(alpha(c.accent, 0.16)).text_size(px(10.5)).font_weight(FontWeight::SEMIBOLD).text_color(c.accent).child(n.to_string()))
                .child(div().flex_1().min_w_0().child(text.to_string()))
        };
        let buttons = div().flex().gap_1p5()
            .child(button(c, "plugins-folder", Some(IconName::FolderOpen), "Open folder", Btn::Plain, true, false).on_click(cx.listener(|_, _, _, _| {
                if let Ok(dir) = backend_plugins::plugins_dir() {
                    sidebar::reveal_path(&dir);
                }
            })))
            .child(button(c, "plugins-reload", Some(IconName::RotateCw), "Reload", Btn::Plain, true, false).on_click(cx.listener(|this, _, _, cx| {
                this.reload_plugins(cx);
                this.toast("Plugins reloaded", cx);
            })));
        div().flex().flex_col().gap_4()
            .child(panel(c).gap_3().p_3p5()
                .child(head(icon_tile(c.accent, IconName::PanelTop, 30.0), "Hide names on plugin tabs", mark, "Plugin pages in the title bar show just their icon. Hover a tab to see its name.", names))
                .child(bar))
            .child(panel(c).gap_3().p_3p5()
                .child(head(icon_tile(c.muted, IconName::FolderCode, 30.0), "Your own plugins", None, "Build one, or try a plugin that isn't in the store.", buttons.into_any_element()))
                .child(div().flex().items_center().gap_2().h(px(32.0)).px_2p5().rounded(px(8.0)).bg(c.app).text_color(c.subtle)
                    .child(Icon::new(IconName::Folder).size(px(13.0)))
                    .child(div().flex_1().min_w_0().truncate().font_family(mono).text_size(px(11.5)).text_color(c.muted).child(shown_dir))
                    .when(local > 0, |d| d.child(badge(c.subtle, format!("{local} local")))))
                .child(div().flex().gap_2()
                    .child(step("1", "Add a folder with a plugin.json to the plugins folder."))
                    .child(step("2", "It shows up under Installed with a “local” badge, off."))
                    .child(step("3", "Turn it on. After editing it, press Reload."))))
    }
}

/// A spinner-free busy label ("Installing…") in place of a button.
pub(super) fn busy_label(c: Colors, text: &'static str) -> Div {
    div().flex().items_center().gap_1p5().h(px(24.0)).text_size(px(11.5)).text_color(c.accent).child(Icon::new(IconName::LoaderCircle).size(px(13.0))).child(text)
}

#[cfg(test)]
mod tests {
    use super::{adds_of, top_tags};
    use serde_json::{Value, json};

    fn contributions(p: &Value) -> String {
        adds_of(p).into_iter().map(|(_, label)| label).collect::<Vec<_>>().join(" · ")
    }

    #[test]
    fn contributions_list_what_a_plugin_adds() {
        let p = json!({ "manifest": { "tools": [1, 2], "providers": [1], "recipes": [], "main": "index.js" } });
        assert_eq!(contributions(&p), "2 tools · 1 agent · code");
        assert_eq!(contributions(&json!({ "manifest": null })), "");
    }

    #[test]
    fn top_tags_need_two_uses() {
        let e = vec![json!({ "tags": ["a", "b"] }), json!({ "tags": ["a"] })];
        assert_eq!(top_tags(&e), vec!["a"]);
    }
}
