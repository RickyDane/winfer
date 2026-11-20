//! Settings → Plugins (port of `PluginSettings` in `features/settings/SettingsModal.tsx`):
//! the store, installed plugins, updates and options.

use super::orchestration::accent_button;
use super::plugins::matches_search;
use super::review::small_button;
use super::settings::{card, plain_button, row, row_marked};
use super::*;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use serde_json::Value;
use std::collections::HashMap as Map;
use winfer_backend::plugins as backend_plugins;

fn trust_warning(name: &str) -> String {
    format!("“{name}” runs its own code with full access to your files, terminals and network. Only turn on plugins you trust.")
}

/// "2 tools · 1 agent · code": what a plugin adds.
pub fn contributions(plugin: &Value) -> String {
    let m = &plugin["manifest"];
    if !m.is_object() {
        return String::new();
    }
    let mut parts = Vec::new();
    for (key, one) in [("tools", "tool"), ("providers", "agent"), ("recipes", "recipe")] {
        let n = m[key].as_array().map_or(0, Vec::len);
        if n > 0 {
            parts.push(format!("{n} {one}{}", if n == 1 { "" } else { "s" }));
        }
    }
    if m["main"].is_string() {
        parts.push("code".to_string());
    }
    parts.join(" · ")
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

#[derive(Default)]
pub(super) struct PluginsPage {
    search: Option<Entity<InputState>>,
    catalog: Option<Vec<Value>>,
    catalog_loading: bool,
    catalog_error: Option<String>,
    tab: Option<&'static str>,
    tag: Option<String>,
    expanded: Option<String>,
    busy: Option<String>,
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
                    Ok(list) => this.plugin_page.catalog = serde_json::to_value(list).ok().and_then(|v| v.as_array().cloned()),
                    Err(e) => this.plugin_page.catalog_error = Some(e),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn plugin_work(&mut self, id: String, done: Option<String>, work: impl FnOnce() -> Result<(), String> + Send + 'static, cx: &mut Context<Self>) {
        self.plugin_page.busy = Some(id);
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

    fn uninstall_plugin(&mut self, plugin: Value, window: &mut Window, cx: &mut Context<Self>) {
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

    pub(super) fn page_plugins(&mut self, c: Colors, window: &mut Window, cx: &mut Context<Self>) -> Div {
        if self.plugin_page.search.is_none() {
            let input = cx.new(|cx| InputState::new(window, cx));
            cx.subscribe(&input, |_, _, _: &InputEvent, cx| cx.notify()).detach();
            self.plugin_page.search = Some(input);
        }
        if self.plugin_page.catalog.is_none() && !self.plugin_page.catalog_loading && self.plugin_page.catalog_error.is_none() {
            self.load_catalog(cx);
        }
        let plugins = self.plugins.infos.clone();
        let tab = self.plugin_page.tab.unwrap_or(if plugins.iter().any(|p| p["manifest"].is_object()) { "installed" } else { "discover" });
        let search = self.plugin_page.search.clone().unwrap();
        let placeholder = if tab == "discover" { "Search the store" } else { "Search installed plugins" };
        search.update(cx, |i, cx| i.set_placeholder(placeholder, window, cx));
        let query = search.read(cx).value().to_string();
        let catalog = self.plugin_page.catalog.clone();
        let latest_for = |p: &Value| -> Option<Value> {
            if p["fromStore"] != true {
                return None;
            }
            let current = p["storeVersion"].as_str().or(p["manifest"]["version"].as_str());
            catalog.as_ref()?.iter().find(|e| e["id"] == p["id"] && e["version"].as_str() != current).cloned()
        };
        let updates: Vec<Value> = plugins.iter().filter_map(latest_for).collect();
        let not_installed: Vec<Value> = catalog.iter().flatten().filter(|e| !plugins.iter().any(|p| p["id"] == e["id"])).cloned().collect();
        let tab_button = |id: &'static str, label: String, on: bool, dot: bool, cx: &mut Context<Self>| {
            div().id(id).flex().items_center().gap_1p5().rounded_md().px_2p5().py_1().text_xs().cursor_pointer()
                .when(on, |d| d.bg(c.active).text_color(c.fg)).when(!on, |d| d.text_color(c.muted).hover(move |d| d.text_color(c.fg)))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.plugin_page.tab = Some(id);
                    cx.notify();
                }))
                .child(label)
                .when(dot, |d| d.child(div().size_1p5().rounded_full().bg(c.accent)))
        };
        let discover_label = catalog.as_ref().map_or("Discover".to_string(), |_| format!("Discover {}", not_installed.len()));
        let toolbar = div().flex().items_center().gap_3()
            .child(div().flex().rounded_lg().border_1().border_color(c.border).bg(c.panel).p_0p5()
                .child(tab_button("plugins-discover", discover_label, tab == "discover", false, cx))
                .child(tab_button("plugins-installed", format!("Installed {}", plugins.len()), tab == "installed", !updates.is_empty(), cx)))
            .child(div().flex_1().child(Input::new(&search)));
        let page = div().flex().flex_col().gap_5().child(toolbar);
        if tab == "discover" {
            page.child(self.plugins_discover(not_installed, &query, c, cx))
        } else {
            page.child(self.plugins_installed(plugins, updates, &query, c, window, cx))
        }
    }

    fn plugins_discover(&mut self, not_installed: Vec<Value>, query: &str, c: Colors, cx: &mut Context<Self>) -> Div {
        let empty = |title: String| div().py_8().text_center().text_xs().text_color(c.muted).child(title);
        if let Some(e) = self.plugin_page.catalog_error.clone() {
            return div().flex().flex_col().items_center().gap_2().py_8().child(div().text_xs().text_color(c.fg).child("Couldn't load the plugin store")).child(div().text_xs().text_color(c.subtle).child(e))
                .child(plain_button(c, "catalog-retry", "Retry").on_click(cx.listener(|this, _, _, cx| this.load_catalog(cx))));
        }
        if self.plugin_page.catalog.is_none() {
            return empty("Loading the store…".into());
        }
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
        let chip = |id: SharedString, label: String, on: bool| div().id(id).flex().h(px(24.0)).items_center().rounded_md().border_1().px_2().text_size(px(11.0)).cursor_pointer()
            .when(on, |d| d.border_color(c.accent).text_color(c.fg)).when(!on, |d| d.border_color(c.border_strong).text_color(c.muted).hover(move |d| d.bg(c.hover).text_color(c.fg))).child(label);
        let mut out = div().flex().flex_col().gap_3();
        if !tags.is_empty() {
            let mut row = div().flex().flex_wrap().items_center().gap_1p5().child(chip("tag-all".into(), "All".into(), tag.is_none()).on_click(cx.listener(|this, _, _, cx| {
                this.plugin_page.tag = None;
                cx.notify();
            })));
            for t in tags {
                let (on, pick) = (tag.as_ref() == Some(&t), t.clone());
                row = row.child(chip(format!("tag-{t}").into(), t, on).on_click(cx.listener(move |this, _, _, cx| {
                    this.plugin_page.tag = if on { None } else { Some(pick.clone()) };
                    cx.notify();
                })));
            }
            out = out.child(row);
        }
        if shown.is_empty() {
            return out.child(empty(if not_installed.is_empty() { "You have every plugin from the store".into() } else { "Nothing in the store matches".to_string() }));
        }
        let busy = self.plugin_page.busy.clone();
        let mut grid = div().flex().flex_wrap().gap_3();
        for e in shown {
            let id = str_of(&e["id"]).to_string();
            let open = self.plugin_page.expanded.as_ref() == Some(&id);
            let toggle = id.clone();
            let author = e["author"].as_str().map_or("Unknown author".to_string(), |a| format!("by {a}"));
            let version = e["version"].as_str().filter(|v| !v.is_empty()).map_or(String::new(), |v| format!(" · {v}"));
            let description = e["description"].as_str().filter(|d| !d.is_empty()).unwrap_or("No description.").to_string();
            let tags: Vec<String> = e["tags"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect();
            let label = if busy.as_ref() == Some(&id) { "Installing…" } else { "Install" };
            let entry = e.clone();
            let enabled = busy.is_none();
            grid = grid.child(div().w(px(250.0)).flex().flex_col().rounded_lg().border_1().border_color(c.border).bg(c.panel).p_3p5()
                .child(div().truncate().text_size(px(13.0)).text_color(c.fg).child(str_of(&e["name"]).to_string()))
                .child(div().truncate().text_size(px(11.0)).text_color(c.subtle).child(format!("{author}{version}")))
                .child(div().id(SharedString::from(format!("desc-{id}"))).mt_2p5().text_xs().text_color(c.muted).cursor_pointer().when(!open, |d| d.line_clamp(3))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.plugin_page.expanded = if open { None } else { Some(toggle.clone()) };
                        cx.notify();
                    }))
                    .child(description))
                .child(div().mt_3().flex().items_end().gap_2()
                    .child(div().flex().min_w_0().flex_1().flex_wrap().gap_1().children(tags.into_iter().take(if open { usize::MAX } else { 2 }).map(|t| div().rounded_sm().bg(c.active).px_1p5().py_0p5().text_size(px(10.0)).text_color(c.subtle).child(t))))
                    .child(accent_button(c, SharedString::from(format!("install-{id}")), label, enabled).on_click(cx.listener(move |this, _, window, cx| {
                        if enabled {
                            this.install_plugin(entry.clone(), false, window, cx);
                        }
                    })))));
        }
        out.child(grid)
    }

    fn plugins_installed(&mut self, plugins: Vec<Value>, updates: Vec<Value>, query: &str, c: Colors, _window: &mut Window, cx: &mut Context<Self>) -> Div {
        let busy = self.plugin_page.busy.clone();
        let enabled_count = plugins.iter().filter(|p| p["enabled"] == true && p["error"].is_null()).count();
        let mut page = div().flex().flex_col().gap_5();
        if !updates.is_empty() && query.is_empty() {
            let text = if updates.len() == 1 { format!("An update for {} is available.", str_of(&updates[0]["name"])) } else { format!("{} plugin updates are available.", updates.len()) };
            let list = updates.clone();
            let free = busy.is_none();
            page = page.child(div().flex().items_center().gap_2().rounded_lg().border_1().border_color(c.accent).px_3().py_2().text_xs()
                .child(div().flex_1().text_color(c.fg).child(text))
                .child(accent_button(c, "plugins-update-all", if updates.len() == 1 { "Update" } else { "Update all" }, free).on_click(cx.listener(move |this, _, window, cx| {
                    if !free {
                        return;
                    }
                    if list.len() == 1 {
                        this.install_plugin(list[0].clone(), true, window, cx);
                    } else {
                        let ids: Vec<String> = list.iter().map(|e| str_of(&e["id"]).to_string()).collect();
                        let done = format!("Updated {} plugins", ids.len());
                        this.plugin_work("*".into(), Some(done), move || {
                            for id in ids {
                                tauri::async_runtime::block_on(backend_plugins::plugins_install(id))?;
                            }
                            Ok(())
                        }, cx);
                    }
                }))));
        }
        let shown: Vec<&Value> = plugins.iter().filter(|p| matches_search(query, &[str_of(&p["id"]), str_of(&p["manifest"]["name"]), str_of(&p["manifest"]["description"])])).collect();
        if shown.is_empty() {
            page = page.child(div().py_8().text_center().text_xs().text_color(c.muted).child(if plugins.is_empty() { "No plugins installed. Plugins add pages, agents, recipes, commands, voice actions and tools for your agents.".to_string() } else { format!("No installed plugin matches “{query}”") }));
        } else {
            let mut list = div().flex().flex_col().overflow_hidden().rounded_lg().border_1().border_color(c.border).bg(c.panel);
            for (i, p) in shown.into_iter().enumerate() {
                let id = str_of(&p["id"]).to_string();
                let name = p["manifest"]["name"].as_str().unwrap_or(&id).to_string();
                let enabled = p["enabled"] == true;
                let error = p["error"].as_str().map(str::to_string).or_else(|| enabled.then(|| self.plugins.runtime_errors.get(&id).cloned()).flatten());
                let current = p["storeVersion"].as_str().or(p["manifest"]["version"].as_str()).map(str::to_string);
                let latest = updates.iter().find(|e| e["id"] == p["id"]).cloned();
                let adds = contributions(p);
                let free = busy.is_none();
                let (remove, flip) = (p.clone(), p.clone());
                let has_error = !p["error"].is_null();
                let toggle = (!has_error).then(|| self.toggle(SharedString::from(format!("plugin-on-{id}")), enabled, c, cx, |_, _| {}));
                let toggle_area = toggle.map(|t| div().id(SharedString::from(format!("plugin-area-{id}"))).on_click(cx.listener(move |this, _, window, cx| if this.plugin_page.busy.is_none() { this.toggle_plugin(flip.clone(), !enabled, window, cx) })).child(t));
                let description = p["manifest"]["description"].as_str().map(str::to_string);
                list = list.child(div().flex().gap_3().px_3p5().py_3().when(i > 0, |d| d.border_t_1().border_color(c.border))
                    .child(div().min_w_0().flex_1().when(!enabled && error.is_none(), |d| d.opacity(0.6))
                        .child(div().flex().min_w_0().items_center().gap_1p5()
                            .child(div().truncate().text_size(px(13.0)).text_color(c.fg).child(name))
                            .when_some(current, |d, v| d.child(div().flex_shrink_0().text_size(px(11.0)).text_color(c.subtle).child(v)))
                            .when(p["fromStore"] != true, |d| d.child(div().rounded_sm().bg(c.active).px_1().text_size(px(10.0)).text_color(c.subtle).child("local")))
                            .when_some(latest.clone(), |d, l| d.child(div().rounded_sm().px_1().text_size(px(10.0)).text_color(c.accent).child(format!("{} available", str_of(&l["version"]))))))
                        .when_some(description, |d, t| d.child(div().mt_0p5().line_clamp(2).text_xs().text_color(c.muted).child(t)))
                        .when(!adds.is_empty(), |d| d.child(div().mt_1().text_size(px(11.0)).text_color(c.subtle).child(format!("Adds {adds}"))))
                        .when_some(error, |d, e| d.child(div().mt_2().rounded_md().bg(Hsla { a: 0.1, ..c.danger }).px_2().py_1p5().text_size(px(11.0)).text_color(c.danger).child(e))))
                    .child(div().flex().flex_shrink_0().items_start().gap_1p5()
                        .when(busy.as_ref() == Some(&id), |d| d.child(div().text_xs().text_color(c.subtle).child("…")))
                        .when_some(latest, |d, l| d.child(small_button(c, SharedString::from(format!("plugin-update-{id}")), "Update").on_click(cx.listener(move |this, _, window, cx| if free { this.install_plugin(l.clone(), true, window, cx) }))))
                        .child(small_button(c, SharedString::from(format!("plugin-remove-{id}")), "Uninstall").on_click(cx.listener(move |this, _, window, cx| if free { this.uninstall_plugin(remove.clone(), window, cx) })))
                        .children(toggle_area)));
            }
            page = page.child(div().flex().flex_col().gap_2().child(div().px_1().text_size(px(11.0)).text_color(c.subtle).child(format!("{enabled_count} of {} on", plugins.len()))).child(list));
        }
        let icons_only = self.state.read(cx).page_tab_icons_only;
        let names = self.toggle("plugin-icons", icons_only, c, cx, |this, cx| this.update_state(cx, |s| s.page_tab_icons_only = !s.page_tab_icons_only));
        let buttons = div().flex().gap_2()
            .child(plain_button(c, "plugins-folder", "Open folder").on_click(cx.listener(|_, _, _, _| {
                if let Ok(dir) = backend_plugins::plugins_dir() {
                    sidebar::reveal_path(&dir);
                }
            })))
            .child(plain_button(c, "plugins-reload", "Reload").on_click(cx.listener(|this, _, _, cx| this.reload_plugins(cx))));
        page.child(card(c, "Options", None)
            .child(row_marked(c, "Hide names on plugin tabs", self.reset_mark("plugins.icons", c, cx), Some("Plugin page tabs in the title bar show just their icon. Hover a tab to see its name.".to_string()), names))
            .child(row(c, "Your own plugins", Some("A folder with a plugin.json in the plugins folder shows up here, off until you turn it on. Reload after changing one.".to_string()), buttons)))
    }
}

#[cfg(test)]
mod tests {
    use super::{contributions, top_tags};
    use serde_json::json;

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
