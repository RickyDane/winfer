//! Settings → Skills in two tabs (mockups/settings-overhaul/skills.html): installs agent skills
//! (SKILL.md folders) from GitHub repos into each agent's skills folder. "Install for" sits above
//! both tabs, as it drives installs and "Add to…".

use super::plugins::matches_search;
use super::settings_kit::{PageTab, alpha, badge, checkbox, empty_state, icon_tile, panel, tab_count, tab_dot};
use super::settings_plugins::{Btn, busy_label, button, note, search_field, tint_of, update_banner};
use super::*;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use serde_json::{Value, json};
use winfer_backend::skills;

const FEATURED: [(&str, &str, &str); 2] = [
    ("JuliusBrussee/caveman", "Caveman", "Terse output modes (caveman, ultracave, megacave) and lean coding workflows"),
    ("anthropics/skills", "Anthropic", "Anthropic's skills: documents (pdf, docx, xlsx, pptx), frontend design, MCP builder, skill creator"),
];

fn s(v: &Value) -> &str {
    v.as_str().unwrap_or_default()
}

/// `owner/repo` from what the user typed (a URL works too), or `None`.
pub fn normalize_repo(input: &str) -> Option<String> {
    let t = input.trim();
    let lower = t.to_lowercase();
    let mut rest = t;
    for prefix in ["https://", "http://"] {
        if lower.starts_with(prefix) {
            rest = &t[prefix.len()..];
        }
    }
    if rest.to_lowercase().starts_with("www.") {
        rest = &rest[4..];
    }
    if rest.to_lowercase().starts_with("github.com/") {
        rest = &rest["github.com/".len()..];
    }
    let mut parts = rest.split('/');
    let (owner, repo) = (parts.next()?, parts.next()?);
    let repo = repo.strip_suffix(".git").unwrap_or(repo);
    let ok = |p: &str| !p.is_empty() && p.chars().all(|c| c.is_alphanumeric() || "_.-".contains(c));
    (ok(owner) && ok(repo)).then(|| format!("{owner}/{repo}"))
}

/// A copy's short label, e.g. "Cursor command · winfer".
pub fn copy_label(copy: &Value) -> String {
    let project = copy["project"].as_str().and_then(|p| p.split(['/', '\\']).rfind(|x| !x.is_empty())).map_or(String::new(), |p| format!(" · {p}"));
    format!("{}{}{project}", s(&copy["label"]), if copy["kind"] == "command" { " command" } else { "" })
}

fn origin_of(skill: &Value) -> Option<&Value> {
    skill["copies"].as_array()?.iter().find(|c| c["origin"].is_object()).map(|c| &c["origin"])
}

fn home_copies(skill: &Value) -> Vec<&Value> {
    skill["copies"].as_array().into_iter().flatten().filter(|c| c["kind"] == "skill" && c["project"].is_null()).collect()
}

/// True when a repo's listing has a changed version of an installed skill.
pub fn is_outdated(skill: &Value, listing: Option<&Value>) -> bool {
    let (Some(origin), Some(listing)) = (origin_of(skill), listing) else { return false };
    if s(&listing["repo"]).to_lowercase() != s(&origin["repo"]).to_lowercase() {
        return false;
    }
    let Some(latest) = listing["skills"].as_array().and_then(|l| l.iter().find(|x| x["path"] == origin["path"])) else { return false };
    let (a, b) = (s(&origin["hash"]), s(&latest["hash"]));
    if !a.is_empty() && !b.is_empty() { a != b } else { listing["rev"] != origin["rev"] }
}

/// A small mark for an install target (an agent's skills folder).
fn target_mark(c: Colors, id: &str, size: f32) -> Div {
    let (color, letter) = match id {
        "claude" => (c.claude, "C"),
        "codex" => (crate::theme::hex("#10a37f"), "X"),
        "gemini" => (crate::theme::hex("#4f8cff"), "A"),
        "pi" => (crate::theme::hex("#7ee787"), "π"),
        _ => (c.muted, ""),
    };
    let d = div().size(px(size)).flex_shrink_0().flex().items_center().justify_center().rounded(px(size * 0.28)).bg(alpha(color, 0.16)).text_color(color);
    if letter.is_empty() { d.child(Icon::new(IconName::FolderTree).size(px(size * 0.6))) } else { d.text_size(px(size * 0.5)).font_weight(FontWeight::BOLD).child(letter) }
}

#[derive(Default)]
pub(super) struct SkillsPage {
    search: Option<Entity<InputState>>,
    new_repo: Option<Entity<InputState>>,
    targets: Vec<Value>,
    installed: Option<Vec<Value>>,
    source: Option<String>,
    listings: HashMap<String, Value>,
    pending: std::collections::HashSet<String>,
    error: Option<String>,
    busy: Option<String>,
    expanded: Option<String>,
    projects: Vec<String>,
    /// What was typed under Sources isn't a GitHub repo.
    repo_error: bool,
}

impl Root {
    fn skill_list(&self, key: &str, cx: &App) -> Option<Vec<String>> {
        self.state.read(cx).rest.get("skills").and_then(|v| serde_json::from_value(v[key].clone()).ok())
    }

    fn skill_targets(&self, cx: &App) -> Vec<String> {
        self.skill_list("targets", cx).unwrap_or_else(|| vec!["claude".into(), "agents".into()])
    }

    fn reload_skills(&mut self, cx: &mut Context<Self>) {
        let mut projects: Vec<String> = self.state.read(cx).workspaces.iter().map(|w| w.root_dir.clone()).filter(|d| !d.is_empty()).collect();
        projects.dedup();
        self.skill_page.projects = projects.clone();
        cx.spawn(async move |this, cx| {
            let found = tauri::async_runtime::spawn(skills::skills_list(Some(projects))).await.map_err(|e| e.to_string()).and_then(|r| r);
            let _ = this.update(cx, |this, cx| {
                match found {
                    Ok(list) => {
                        let list: Vec<Value> = serde_json::to_value(list).ok().and_then(|v| v.as_array().cloned()).unwrap_or_default();
                        // The repos installed skills came from, for their updates.
                        for repo in list.iter().filter_map(origin_of).map(|o| s(&o["repo"]).to_string()).collect::<Vec<_>>() {
                            this.load_listing(repo, false, cx);
                        }
                        this.skill_page.installed = Some(list);
                    }
                    Err(e) => this.toast(e, cx),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn load_listing(&mut self, repo: String, fresh: bool, cx: &mut Context<Self>) {
        if (!fresh && self.skill_page.listings.contains_key(&repo)) || !self.skill_page.pending.insert(repo.clone()) {
            return;
        }
        if fresh {
            self.skill_page.listings.remove(&repo);
            self.skill_page.error = None;
        }
        cx.spawn(async move |this, cx| {
            let found = tauri::async_runtime::spawn(skills::skills_repo(repo.clone(), Some(fresh))).await.map_err(|e| e.to_string()).and_then(|r| r);
            let _ = this.update(cx, |this, cx| {
                this.skill_page.pending.remove(&repo);
                match found {
                    Ok(l) => {
                        this.skill_page.listings.insert(repo, serde_json::to_value(l).unwrap_or_default());
                    }
                    Err(e) if this.skill_page.source.as_ref() == Some(&repo) || (this.skill_page.source.is_none() && repo == FEATURED[0].0) => this.skill_page.error = Some(e),
                    Err(_) => {}
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn skill_work(&mut self, key: String, done: Option<String>, work: impl FnOnce() -> Result<(), String> + Send + 'static, cx: &mut Context<Self>) {
        self.skill_page.busy = Some(key);
        cx.spawn(async move |this, cx| {
            let result = backend::background(work).await.unwrap_or_else(|e| Err(e.to_string()));
            let _ = this.update(cx, |this, cx| {
                this.skill_page.busy = None;
                match result {
                    Ok(()) => {
                        if let Some(t) = done {
                            this.toast(t, cx);
                        }
                    }
                    Err(e) => this.toast(e, cx),
                }
                this.reload_skills(cx);
            });
        })
        .detach();
    }

    fn target_names(&self, ids: &[String]) -> String {
        ids.iter().map(|id| self.skill_page.targets.iter().find(|t| t["id"] == id.as_str()).map_or(id.clone(), |t| s(&t["label"]).to_string())).collect::<Vec<_>>().join(", ")
    }

    fn install_skill(&mut self, repo: String, skill: Value, update: bool, into: Vec<String>, cx: &mut Context<Self>) {
        let name = s(&skill["name"]).to_string();
        if into.is_empty() {
            return self.toast("Pick at least one agent under “Install for”.", cx);
        }
        let done = if update { format!("Updated {name}") } else { format!("Installed {name} for {}", self.target_names(&into)) };
        let path = s(&skill["path"]).to_string();
        self.skill_work(name, Some(done), move || tauri::async_runtime::block_on(skills::skills_install(repo, path, into)).map(|_| ()), cx);
    }

    fn update_skill(&mut self, skill: &Value, cx: &mut Context<Self>) {
        let Some(origin) = origin_of(skill) else { return };
        let repo = s(&origin["repo"]).to_string();
        let latest = self.skill_page.listings.get(&repo).and_then(|l| l["skills"].as_array()?.iter().find(|x| x["path"] == origin["path"]).cloned());
        let into: Vec<String> = home_copies(skill).iter().filter(|c| c["origin"].is_object()).map(|c| s(&c["target"]).to_string()).collect();
        if let Some(latest) = latest {
            self.install_skill(repo, latest, true, into, cx);
        }
    }

    fn add_source(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(input) = self.skill_page.new_repo.clone() else { return };
        let Some(repo) = normalize_repo(&input.read(cx).value()) else {
            self.skill_page.repo_error = true;
            return cx.notify();
        };
        let mut sources = self.skill_list("sources", cx).unwrap_or_default();
        if !FEATURED.iter().any(|f| f.0.eq_ignore_ascii_case(&repo)) && !sources.contains(&repo) {
            sources.push(repo.clone());
            self.set_rest("skills", "sources", json!(sources), cx);
        }
        input.update(cx, |i, cx| i.set_value("", window, cx));
        self.skill_page.source = Some(repo);
        self.skill_page.error = None;
        self.skill_page.repo_error = false;
        cx.notify();
    }

    /// Opens the page's inputs and reads the targets and installed skills, once.
    fn init_skills_page(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.skill_page.search.is_some() {
            return;
        }
        let search = cx.new(|cx| InputState::new(window, cx));
        let new_repo = cx.new(|cx| InputState::new(window, cx).placeholder("owner/repo or URL"));
        cx.subscribe(&search, |_, _, _: &InputEvent, cx| cx.notify()).detach();
        cx.subscribe_in(&new_repo, window, |this, _, e: &InputEvent, window, cx| match e {
            InputEvent::PressEnter { .. } => this.add_source(window, cx),
            _ => {
                this.skill_page.repo_error = false;
                cx.notify();
            }
        })
        .detach();
        self.skill_page.search = Some(search);
        self.skill_page.new_repo = Some(new_repo);
        match skills::skills_targets().map(|t| serde_json::to_value(t).unwrap_or_default()) {
            Ok(v) => self.skill_page.targets = v.as_array().cloned().unwrap_or_default(),
            Err(e) => self.toast(e, cx),
        }
        self.reload_skills(cx);
    }

    fn current_source(&self) -> String {
        self.skill_page.source.clone().unwrap_or_else(|| FEATURED[0].0.to_string())
    }

    /// Installed skills with a newer version in their repo.
    fn outdated_skills(&self) -> Vec<Value> {
        self.skill_page.installed.iter().flatten().filter(|sk| origin_of(sk).is_some_and(|o| is_outdated(sk, self.skill_page.listings.get(s(&o["repo"]))))).cloned().collect()
    }

    /// The tab bar: Installed (a dot for updates) and Discover, with their counts.
    pub(super) fn skills_tabs(&mut self, c: Colors, cx: &mut Context<Self>) -> Div {
        let mono = crate::fonts::mono(cx);
        let has_updates = !self.outdated_skills().is_empty();
        let installed = div().flex().items_center().gap_1p5()
            .children(self.skill_page.installed.as_ref().map(|l| tab_count(c, &mono, l.len())))
            .when(has_updates, |d| d.child(tab_dot(c.accent)));
        let discover = self.skill_page.listings.get(&self.current_source()).map(|l| tab_count(c, &mono, l["skills"].as_array().map_or(0, Vec::len)));
        let tabs = vec![
            PageTab::new("installed", "Installed", IconName::PackageCheck).extra(Some(installed.into_any_element())),
            PageTab::new("discover", "Discover", IconName::Compass).extra(discover),
        ];
        self.page_tab_bar("skills", "installed", tabs, c, cx)
    }

    pub(super) fn page_skills(&mut self, c: Colors, window: &mut Window, cx: &mut Context<Self>) -> Div {
        self.init_skills_page(window, cx);
        let source = self.current_source();
        self.load_listing(source.clone(), false, cx);
        let tab = self.page_tab("skills", "installed");
        let search = self.skill_page.search.clone().unwrap();
        let featured = FEATURED.iter().find(|f| f.0 == source);
        let placeholder = if tab == "discover" { format!("Search {}", featured.map_or(source.as_str(), |f| f.1)) } else { "Search installed skills".to_string() };
        search.update(cx, |i, cx| i.set_placeholder(placeholder, window, cx));
        let query = search.read(cx).value().to_string();
        let selected = self.skill_targets(cx);
        let page = div().flex().flex_col().gap_4().pt_4().child(self.install_for_bar(&selected, c, cx));
        if tab == "discover" {
            page.child(self.skills_discover(source, &search, &query, selected, c, cx))
        } else {
            page.child(self.skills_installed(&search, &query, selected, c, cx))
        }
    }

    /// "Install for": the agents new installs and "Add to…" copy skills into.
    fn install_for_bar(&mut self, selected: &[String], c: Colors, cx: &mut Context<Self>) -> Div {
        let mut chips = div().flex_1().min_w_0().flex().flex_wrap().gap(px(5.0));
        for t in self.skill_page.targets.clone() {
            let id = s(&t["id"]).to_string();
            let on = selected.contains(&id);
            let found = t["detected"] == true;
            let current = selected.to_vec();
            let mark = target_mark(c, &id, 18.0);
            chips = chips.child(div().id(SharedString::from(format!("target-{id}"))).flex().items_center().gap_1p5().h(px(28.0)).pl_1().pr_2p5().rounded(px(7.0)).text_size(px(12.0)).cursor_pointer()
                .when(on, |d| d.bg(alpha(c.accent, 0.14)).border_1().border_color(alpha(c.accent, 0.5)).text_color(c.fg))
                .when(!on, |d| d.bg(c.app).border_1().border_color(c.edge).text_color(c.muted).hover(move |d| d.text_color(c.fg)))
                .when(!found, |d| d.opacity(0.6))
                .on_click(cx.listener(move |this, _, _, cx| {
                    let mut next = current.clone();
                    if let Some(i) = next.iter().position(|x| *x == id) {
                        next.remove(i);
                    } else {
                        next.push(id.clone());
                    }
                    this.set_rest("skills", "targets", json!(next), cx);
                }))
                .child(div().ml_1().child(checkbox(c, on)))
                .child(mark)
                .child(s(&t["label"]).to_string())
                .when(!found, |d| d.child(div().text_size(px(10.5)).text_color(c.warning).child("· not found"))));
        }
        let none = selected.is_empty();
        let hint = if none {
            div().flex().items_center().gap_1p5().text_color(c.danger).child(Icon::new(IconName::CircleAlert).size(px(12.0))).child("Pick at least one agent.")
        } else {
            div().flex().items_center().gap_1p5().text_color(c.subtle).child(Icon::new(IconName::Info).size(px(12.0))).child("New installs and “Add to…” go here. Codex, OpenCode, Cursor and other agents also read the shared .agents folder.")
        };
        div().flex().flex_col().gap_2()
            .child(panel(c).flex_row().items_center().gap_2p5().pl_3p5().pr_2p5().py_2()
                .child(div().flex_shrink_0().flex().items_center().gap_1p5().text_size(px(12.0)).font_weight(FontWeight::MEDIUM).text_color(c.fg).child(Icon::new(IconName::Download).size(px(13.0)).text_color(c.accent)).child("Install for"))
                .child(chips))
            .child(hint.mx_1().text_size(px(11.5)))
    }

    fn skills_discover(&mut self, source: String, search: &Entity<InputState>, query: &str, selected: Vec<String>, c: Colors, cx: &mut Context<Self>) -> Div {
        let listing = self.skill_page.listings.get(&source).cloned();
        let out = div().flex().flex_col().gap_4()
            .child(note(c, IconName::ShieldAlert, "Every folder with a SKILL.md in a GitHub repo is a skill. Skills are instructions and scripts your agents follow, so install them from repos you trust."));
        let sources = self.skill_sources(&source, c, cx);
        let featured = FEATURED.iter().find(|f| f.0 == source);
        let refresh = source.clone();
        let head = div().flex().items_center().gap_2p5().px_0p5()
            .child(div().flex_1().min_w_0().flex().flex_col().gap_0p5()
                .child(div().flex().items_center().gap_1p5().text_size(px(13.0)).font_weight(FontWeight::MEDIUM).text_color(c.fg).child(Icon::new(IconName::Github).size(px(14.0))).child(div().truncate().child(source.clone())))
                .child(div().text_size(px(11.5)).text_color(c.subtle).child(featured.map_or("Added by you".to_string(), |f| f.2.to_string()))))
            .child(button(c, "skills-refresh", Some(IconName::RefreshCw), "Refresh", Btn::Plain, true, true).on_click(cx.listener(move |this, _, _, cx| this.load_listing(refresh.clone(), true, cx))));
        let all = listing.as_ref().and_then(|l| l["skills"].as_array().cloned());
        let shown: Vec<Value> = all.iter().flatten().filter(|x| matches_search(query, &[s(&x["name"]), s(&x["description"]), s(&x["path"])])).cloned().collect();
        let count = all.as_ref().filter(|_| !query.is_empty()).map(|a| format!("{} of {}", shown.len(), a.len()));
        let state_box = |icon: IconName, text: String| panel(c).child(empty_state(c, icon, text));
        let body: Div = if let Some(e) = self.skill_page.error.clone() {
            panel(c).items_center().gap_1p5().py_6().px_4()
                .child(Icon::new(IconName::CloudOff).size(px(22.0)).text_color(c.subtle))
                .child(div().text_size(px(12.5)).text_color(c.muted).child(format!("Couldn't load {source}")))
                .child(div().text_center().font_family(crate::fonts::mono(cx)).text_size(px(11.0)).text_color(c.subtle).child(e))
        } else if let (Some(listing), Some(all)) = (listing.as_ref(), all.as_ref()) {
            if all.is_empty() {
                state_box(IconName::FolderX, format!("{source} has no skills. No folder in it has a SKILL.md."))
            } else if shown.is_empty() {
                state_box(IconName::SearchX, format!("No skill in {source} matches “{query}”"))
            } else {
                self.skill_grid(listing, shown, &selected, c, cx)
            }
        } else {
            state_box(IconName::LoaderCircle, "Reading the repo…".into())
        };
        out.child(div().flex().items_start().gap_3p5()
            .child(sources)
            .child(div().flex_1().min_w_0().flex().flex_col().gap_2p5().child(head).child(search_field(c, search, count)).child(body)))
    }

    /// The repos to browse: the built-in ones, yours (removable) and a field to add one.
    fn skill_sources(&mut self, source: &str, c: Colors, cx: &mut Context<Self>) -> Div {
        let sources = self.skill_list("sources", cx).unwrap_or_default();
        let new_repo = self.skill_page.new_repo.clone().unwrap();
        let mono = crate::fonts::mono(cx);
        let mut list = panel(c).w(px(184.0)).flex_shrink_0().gap_0p5().p_1p5()
            .child(div().px_2().pt_1p5().pb_1().text_size(px(10.5)).font_weight(FontWeight::SEMIBOLD).text_color(c.subtle).child("SOURCES"));
        let entries: Vec<(String, String, bool)> = FEATURED.iter().map(|f| (f.0.to_string(), f.1.to_string(), true)).chain(sources.iter().map(|r| (r.clone(), r.clone(), false))).collect();
        for (repo, name, builtin) in entries {
            let on = repo == source;
            let count = self.skill_page.listings.get(&repo).and_then(|l| l["skills"].as_array().map(Vec::len));
            let tint = if builtin { tint_of(&repo) } else { c.muted };
            let letter = name.chars().next().map(|ch| ch.to_uppercase().to_string()).unwrap_or_default();
            let pick = repo.clone();
            let (remove, rest) = (repo.clone(), sources.clone());
            list = list.child(div().id(SharedString::from(format!("source-{repo}"))).flex().items_center().gap_2().h(px(32.0)).px_2().rounded(px(7.0)).text_size(px(12.5)).cursor_pointer()
                .when(on, |d| d.bg(alpha(c.accent, 0.14)).text_color(c.fg))
                .when(!on, |d| d.text_color(c.muted).hover(move |d| d.bg(c.hover).text_color(c.fg)))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.skill_page.source = Some(pick.clone());
                    this.skill_page.error = None;
                    this.skill_page.expanded = None;
                    this.set_page_tab("skills", "discover", cx);
                }))
                .child(div().size(px(20.0)).flex_shrink_0().flex().items_center().justify_center().rounded(px(5.0)).bg(alpha(tint, 0.2)).text_size(px(10.0)).font_weight(FontWeight::BOLD).text_color(tint).child(letter))
                .child(div().flex_1().min_w_0().truncate().child(name))
                .when_some(count, |d, n| d.child(div().font_family(mono.clone()).text_size(px(10.5)).text_color(c.subtle).child(n.to_string())))
                .when(!builtin, |d| d.child(div().id(SharedString::from(format!("source-x-{repo}"))).size(px(18.0)).flex().items_center().justify_center().rounded(px(4.0)).text_color(c.subtle).cursor_pointer()
                    .hover(move |d| d.bg(c.active).text_color(c.fg))
                    .tooltip(|window, cx| gpui_kit::component::tooltip::Tooltip::new("Remove source").build(window, cx))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        let next: Vec<&String> = rest.iter().filter(|r| **r != remove).collect();
                        this.set_rest("skills", "sources", json!(next), cx);
                        if this.skill_page.source.as_ref() == Some(&remove) {
                            this.skill_page.source = None;
                        }
                    }))
                    .child(Icon::new(IconName::X).size(px(11.0))))));
        }
        let error = self.skill_page.repo_error;
        list.child(div().mt_1().pt_1p5().px_0p5().border_t_1().border_color(c.border).flex().items_center().gap_1()
                .child(div().flex_1().min_w_0().h(px(26.0)).flex().items_center().px_2().rounded(px(6.0)).bg(c.app).border_1().border_color(if error { alpha(c.danger, 0.6) } else { c.edge })
                    .child(Input::new(&new_repo).appearance(false).text_size(px(11.5))))
                .child(div().id("skills-add-source").size(px(26.0)).flex_shrink_0().flex().items_center().justify_center().rounded(px(6.0)).bg(c.raised).border_1().border_color(c.edge).text_color(c.fg).cursor_pointer()
                    .hover(move |d| d.bg(c.hover))
                    .tooltip(|window, cx| gpui_kit::component::tooltip::Tooltip::new("Add source").build(window, cx))
                    .on_click(cx.listener(|this, _, window, cx| this.add_source(window, cx)))
                    .child(Icon::new(IconName::Plus).size(px(13.0)))))
            .when(error, |d| d.child(div().px_1().pb_1().text_size(px(10.5)).text_color(c.danger).child("Enter a GitHub repo as owner/repo or its URL")))
    }

    fn skill_grid(&mut self, listing: &Value, shown: Vec<Value>, selected: &[String], c: Colors, cx: &mut Context<Self>) -> Div {
        let busy = self.skill_page.busy.clone();
        let mono = crate::fonts::mono(cx);
        // Two even columns (see the plugins page).
        let mut grid = div().flex().flex_wrap().mx(px(-4.0)).my(px(-4.0));
        for skill in shown {
            let name = s(&skill["name"]).to_string();
            let path = s(&skill["path"]).to_string();
            let have = self.skill_page.installed.iter().flatten().find(|i| i["name"] == name.as_str()).cloned();
            let origin = have.as_ref().and_then(origin_of);
            let same = origin.is_some_and(|o| s(&o["repo"]).eq_ignore_ascii_case(s(&listing["repo"])) && o["path"] == skill["path"]);
            let outdated = same && have.as_ref().is_some_and(|h| is_outdated(h, Some(listing)));
            let copies: Vec<String> = have.as_ref().map_or(Vec::new(), |h| home_copies(h).iter().map(|c| s(&c["target"]).to_string()).collect());
            let everywhere = have.is_some() && selected.iter().all(|t| copies.contains(t));
            let open = self.skill_page.expanded.as_ref() == Some(&path);
            let toggle = path.clone();
            let files = skill["files"].as_u64().unwrap_or(0);
            let can = busy.is_none() && !selected.is_empty();
            let mut into = selected.to_vec();
            if outdated {
                into.extend(copies.iter().filter(|c| !selected.contains(c)).cloned());
            }
            let (repo, entry) = (s(&listing["repo"]).to_string(), skill.clone());
            let status = (have.is_some() && same && !outdated).then(|| if everywhere { "Installed".to_string() } else { format!("For {}", self.target_names(&copies)) });
            let action: Option<AnyElement> = if busy.as_ref() == Some(&name) {
                Some(busy_label(c, if outdated { "Updating…" } else { "Installing…" }).into_any_element())
            } else if have.is_some() && same && everywhere && !outdated {
                None
            } else {
                let (label, icon, style) = if outdated { ("Update", IconName::CircleArrowUp, Btn::Primary) } else if have.is_some() && same { ("Install for more", IconName::Download, Btn::Plain) } else { ("Install", IconName::Download, Btn::Primary) };
                Some(button(c, SharedString::from(format!("skill-install-{path}")), Some(icon), label, style, can, true).on_click(cx.listener(move |this, _, _, cx| {
                    if can {
                        this.install_skill(repo.clone(), entry.clone(), outdated, into.clone(), cx);
                    }
                })).into_any_element())
            };
            let description = skill["description"].as_str().filter(|d| !d.is_empty()).map(str::to_string);
            grid = grid.child(div().w(relative(0.5)).p(px(4.0)).child(panel(c).size_full().gap_1p5().px_3().py(px(11.0))
                .child(div().flex().items_center().gap_1p5().text_size(px(12.5)).font_weight(FontWeight::MEDIUM).text_color(c.fg).child(Icon::new(IconName::WandSparkles).size(px(13.0)).text_color(c.accent)).child(div().truncate().child(name.clone())))
                .child(div().truncate().font_family(mono.clone()).text_size(px(10.5)).text_color(c.subtle).child(format!("{path} · {files} file{}", if files == 1 { "" } else { "s" })))
                .child(div().id(SharedString::from(format!("skill-desc-{path}"))).text_size(px(11.5)).text_color(if description.is_some() { c.muted } else { c.subtle }).cursor_pointer().when(!open, |d| d.line_clamp(3))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.skill_page.expanded = if open { None } else { Some(toggle.clone()) };
                        cx.notify();
                    }))
                    .child(description.unwrap_or_else(|| "No description.".into())))
                .when(have.is_some() && !same, |d| d.child(div().flex().items_start().gap_1p5().rounded_md().bg(alpha(c.warning, 0.12)).px_2().py_1p5().text_size(px(11.0)).text_color(c.warning)
                    .child(Icon::new(IconName::TriangleAlert).size(px(12.0)).flex_shrink_0().mt(px(1.0)))
                    .child(div().flex_1().min_w_0().child(format!("A different “{name}” is installed; installing replaces it.")))))
                .child(div().flex_1())
                .child(div().min_h(px(24.0)).flex().items_center().gap_1p5()
                    .when_some(status, |d, t| d.child(div().min_w_0().flex().items_center().gap_1().text_size(px(11.0)).text_color(c.success).child(Icon::new(IconName::Check).size(px(12.0))).child(div().truncate().child(t))))
                    .child(div().flex_1())
                    .children(action))));
        }
        grid
    }

    fn skills_installed(&mut self, search: &Entity<InputState>, query: &str, selected: Vec<String>, c: Colors, cx: &mut Context<Self>) -> Div {
        let busy = self.skill_page.busy.clone();
        let free = busy.is_none();
        let outdated = self.outdated_skills();
        let mut out = div().flex().flex_col().gap_4();
        if !outdated.is_empty() && query.is_empty() {
            let text = if outdated.len() == 1 { format!("An update for {} is available.", s(&outdated[0]["name"])) } else { format!("{} skill updates are available.", outdated.len()) };
            let list = outdated.clone();
            let action = button(c, "skills-update-all", None, if outdated.len() == 1 { "Update" } else { "Update all" }, Btn::Primary, free, true).on_click(cx.listener(move |this, _, _, cx| {
                if free {
                    for skill in &list {
                        this.update_skill(skill, cx);
                    }
                }
            }));
            out = out.child(update_banner(c, text, action));
        }
        let Some(all) = self.skill_page.installed.clone() else {
            return out.child(search_field(c, search, None)).child(panel(c).child(empty_state(c, IconName::LoaderCircle, "Reading skills…")));
        };
        let shown: Vec<&Value> = all.iter().filter(|x| matches_search(query, &[s(&x["name"]), s(&x["description"]), origin_of(x).map_or("", |o| s(&o["repo"]))])).collect();
        let count = (!query.is_empty()).then(|| format!("{} of {}", shown.len(), all.len()));
        out = out.child(search_field(c, search, count));
        if shown.is_empty() {
            let (icon, text) = if all.is_empty() { (IconName::WandSparkles, "No skills installed. Skills are folders with a SKILL.md that agents load from their skills folder.".to_string()) } else { (IconName::SearchX, format!("No installed skill matches “{query}”")) };
            return out.child(panel(c).child(empty_state(c, icon, text)));
        }
        let projects = self.skill_page.projects.clone();
        let mut list = panel(c);
        for (i, skill) in shown.into_iter().enumerate() {
            let name = s(&skill["name"]).to_string();
            let origin = origin_of(skill);
            let is_old = origin.is_some_and(|o| is_outdated(skill, self.skill_page.listings.get(s(&o["repo"]))));
            let copies = skill["copies"].as_array().cloned().unwrap_or_default();
            let can_copy = copies.iter().any(|c| c["kind"] == "skill");
            let home: Vec<String> = home_copies(skill).iter().map(|c| s(&c["target"]).to_string()).collect();
            let missing: Vec<String> = if can_copy { selected.iter().filter(|t| !home.contains(t)).cloned().collect() } else { Vec::new() };
            let (source_icon, source) = match origin {
                Some(o) => (IconName::Github, format!("from {}", s(&o["repo"]))),
                None if copies.iter().all(|c| c["project"].is_string()) => (IconName::FolderGit2, "in the project".to_string()),
                None => (IconName::Hand, "added by hand".to_string()),
            };
            let mut pills = div().flex().flex_wrap().items_center().gap(px(5.0)).mt_0p5();
            for copy in &copies {
                pills = pills.child(div().flex().items_center().gap_1().h(px(21.0)).pl(px(3.0)).pr(px(7.0)).rounded(px(5.0)).bg(c.app).text_size(px(11.0)).text_color(c.muted)
                    .child(target_mark(c, s(&copy["target"]), 15.0))
                    .child(copy_label(copy))
                    .when(copy["link"] == true, |d| d.child(Icon::new(IconName::Link).size(px(10.0)).text_color(c.subtle))));
            }
            for t in &missing {
                pills = pills.child(div().flex().items_center().h(px(21.0)).px(px(7.0)).rounded(px(5.0)).border_1().border_color(c.border_strong).text_size(px(11.0)).text_color(c.subtle).child(format!("+ {}", self.target_names(std::slice::from_ref(t)))));
            }
            pills = pills.child(div().ml_1().flex().items_center().gap_1().text_size(px(11.0)).text_color(c.subtle).child(Icon::new(source_icon).size(px(11.0))).child(source));
            let (up, add, rm) = (skill.clone(), skill.clone(), skill.clone());
            let missing_names = self.target_names(&missing);
            let missing_copy = missing.clone();
            let project_list = projects.clone();
            let actions: AnyElement = if busy.as_ref() == Some(&name) {
                busy_label(c, "Working…").into_any_element()
            } else {
                div().flex().items_center().gap_1p5()
                    .when(is_old, |d| d.child(button(c, SharedString::from(format!("skill-up-{name}")), None, "Update", Btn::Primary, free, true).on_click(cx.listener(move |this, _, _, cx| if free { this.update_skill(&up, cx) }))))
                    .when(!missing.is_empty(), |d| d.child(button(c, SharedString::from(format!("skill-add-{name}")), None, format!("Add to {missing_names}"), Btn::Plain, free, true)
                        .tooltip(|window, cx| gpui_kit::component::tooltip::Tooltip::new("Copy into the agents picked under Install for").build(window, cx))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if !free {
                                return;
                            }
                            let from = home_copies(&add).first().copied().or_else(|| add["copies"].as_array()?.iter().find(|c| c["kind"] == "skill")).map(|c| s(&c["dir"]).to_string());
                            let Some(from) = from else { return };
                            let (sname, targets, projects) = (s(&add["name"]).to_string(), missing_copy.clone(), project_list.clone());
                            let done = format!("Added {sname} to {}", this.target_names(&targets));
                            let key = sname.clone();
                            this.skill_work(key, Some(done), move || tauri::async_runtime::block_on(skills::skills_copy(sname, from, targets, Some(projects))), cx);
                        }))))
                    .child(div().id(SharedString::from(format!("skill-rm-{name}"))).size(px(28.0)).flex().items_center().justify_center().rounded_md().text_color(c.subtle).cursor_pointer()
                        .hover(move |d| d.bg(c.hover).text_color(c.danger))
                        .tooltip(|window, cx| gpui_kit::component::tooltip::Tooltip::new("Remove").build(window, cx))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            if !free {
                                return;
                            }
                            let copies = rm["copies"].as_array().cloned().unwrap_or_default();
                            let labels = copies.iter().map(copy_label).collect::<Vec<_>>().join(", ");
                            let dirs: Vec<String> = copies.iter().map(|c| s(&c["dir"]).to_string()).collect();
                            let sname = s(&rm["name"]).to_string();
                            let projects = this.skill_page.projects.clone();
                            this.confirm("Remove skill", format!("Remove “{sname}” from {labels}?"), "Remove", window, cx, move |this, cx| {
                                let key = sname.clone();
                                let (n, d, p) = (sname.clone(), dirs.clone(), projects.clone());
                                this.skill_work(key, None, move || tauri::async_runtime::block_on(skills::skills_uninstall(n, d, Some(p))), cx);
                            });
                        }))
                        .child(Icon::new(IconName::Trash).size(px(14.0))))
                    .into_any_element()
            };
            let description = skill["description"].as_str().filter(|d| !d.is_empty()).map(str::to_string);
            list = list.child(div().flex().items_start().gap_3().px_3p5().py(px(11.0)).when(i > 0, |d| d.border_t_1().border_color(c.border))
                .child(div().mt(px(1.0)).child(icon_tile(c.accent, IconName::WandSparkles, 26.0)))
                .child(div().min_w_0().flex_1().flex().flex_col().gap_1()
                    .child(div().flex().items_center().gap_1p5().child(div().truncate().text_size(px(13.0)).font_weight(FontWeight::MEDIUM).text_color(c.fg).child(name.clone())).when(is_old, |d| d.child(badge(c.accent, "update available"))))
                    .when_some(description, |d, t| d.child(div().line_clamp(2).text_size(px(12.0)).text_color(c.muted).child(t)))
                    .child(pills))
                .child(div().flex_shrink_0().pt(px(2.0)).child(actions)));
        }
        out.child(list)
    }
}

#[cfg(test)]
mod tests {
    use super::{copy_label, is_outdated, normalize_repo};
    use serde_json::json;

    #[test]
    fn repos_normalize() {
        assert_eq!(normalize_repo("anthropics/skills").as_deref(), Some("anthropics/skills"));
        assert_eq!(normalize_repo(" https://github.com/a/b.git/tree/main ").as_deref(), Some("a/b"));
        assert_eq!(normalize_repo("nope"), None);
    }

    #[test]
    fn labels_and_updates() {
        assert_eq!(copy_label(&json!({ "label": "Cursor", "kind": "command", "project": "/x/winfer" })), "Cursor command · winfer");
        let skill = json!({ "copies": [{ "origin": { "repo": "a/b", "path": "p", "rev": "1", "hash": "x" } }] });
        let listing = |hash: &str| json!({ "repo": "A/B", "rev": "2", "skills": [{ "path": "p", "hash": hash }] });
        assert!(is_outdated(&skill, Some(&listing("y"))));
        assert!(!is_outdated(&skill, Some(&listing("x"))));
        assert!(!is_outdated(&skill, None));
    }
}
