//! Settings → Skills (port of `SkillSettings` and `features/skills/skills.ts`): installs agent
//! skills (SKILL.md folders) from GitHub repos into each agent's skills folder.

use super::orchestration::accent_button;
use super::plugins::matches_search;
use super::review::small_button;
use super::settings::plain_button;
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
    let project = copy["project"].as_str().and_then(|p| p.split(['/', '\\']).filter(|x| !x.is_empty()).last()).map_or(String::new(), |p| format!(" · {p}"));
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

#[derive(Default)]
pub(super) struct SkillsPage {
    search: Option<Entity<InputState>>,
    new_repo: Option<Entity<InputState>>,
    tab: Option<&'static str>,
    targets: Vec<Value>,
    installed: Option<Vec<Value>>,
    source: Option<String>,
    listings: HashMap<String, Value>,
    pending: std::collections::HashSet<String>,
    error: Option<String>,
    busy: Option<String>,
    expanded: Option<String>,
    projects: Vec<String>,
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
                    Err(e) if this.skill_page.source.as_ref() == Some(&repo) => this.skill_page.error = Some(e),
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
            return self.toast("Enter a GitHub repo as owner/repo or its URL", cx);
        };
        let mut sources = self.skill_list("sources", cx).unwrap_or_default();
        if !FEATURED.iter().any(|f| f.0.eq_ignore_ascii_case(&repo)) && !sources.contains(&repo) {
            sources.push(repo.clone());
            self.set_rest("skills", "sources", json!(sources), cx);
        }
        input.update(cx, |i, cx| i.set_value("", window, cx));
        self.skill_page.source = Some(repo);
        self.skill_page.error = None;
        cx.notify();
    }

    pub(super) fn page_skills(&mut self, c: Colors, window: &mut Window, cx: &mut Context<Self>) -> Div {
        if self.skill_page.search.is_none() {
            let search = cx.new(|cx| InputState::new(window, cx));
            let new_repo = cx.new(|cx| InputState::new(window, cx).placeholder("Add owner/repo"));
            cx.subscribe(&search, |_, _, _: &InputEvent, cx| cx.notify()).detach();
            cx.subscribe_in(&new_repo, window, |this, _, e: &InputEvent, window, cx| match e {
                InputEvent::PressEnter { .. } => this.add_source(window, cx),
                _ => cx.notify(),
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
        let source = self.skill_page.source.clone().unwrap_or_else(|| FEATURED[0].0.to_string());
        self.load_listing(source.clone(), false, cx);
        let tab = self.skill_page.tab.unwrap_or("installed");
        let search = self.skill_page.search.clone().unwrap();
        let featured = FEATURED.iter().find(|f| f.0 == source);
        let placeholder = if tab == "discover" { format!("Search {}", featured.map_or(source.as_str(), |f| f.1)) } else { "Search installed skills".to_string() };
        search.update(cx, |i, cx| i.set_placeholder(placeholder, window, cx));
        let query = search.read(cx).value().to_string();
        let installed = self.skill_page.installed.clone();
        let listing = self.skill_page.listings.get(&source).cloned();
        let outdated: Vec<Value> = installed.iter().flatten().filter(|sk| origin_of(sk).is_some_and(|o| is_outdated(sk, self.skill_page.listings.get(s(&o["repo"]))))).cloned().collect();
        let tab_button = |id: &'static str, label: String, on: bool, dot: bool, cx: &mut Context<Self>| {
            div().id(id).flex().items_center().gap_1p5().rounded_md().px_2p5().py_1().text_xs().cursor_pointer()
                .when(on, |d| d.bg(c.active).text_color(c.fg)).when(!on, |d| d.text_color(c.muted).hover(move |d| d.text_color(c.fg)))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.skill_page.tab = Some(id);
                    cx.notify();
                }))
                .child(label)
                .when(dot, |d| d.child(div().size_1p5().rounded_full().bg(c.accent)))
        };
        let discover = listing.as_ref().map_or("Discover".to_string(), |l| format!("Discover {}", l["skills"].as_array().map_or(0, Vec::len)));
        let have = installed.as_ref().map_or("Installed".to_string(), |l| format!("Installed {}", l.len()));
        let toolbar = div().flex().items_center().gap_3()
            .child(div().flex().rounded_lg().border_1().border_color(c.border).bg(c.panel).p_0p5()
                .child(tab_button("skills-discover", discover, tab == "discover", false, cx))
                .child(tab_button("skills-installed", have, tab == "installed", !outdated.is_empty(), cx)))
            .child(div().flex_1().child(Input::new(&search)));
        let selected = self.skill_targets(cx);
        let mut chips = div().flex().flex_wrap().gap_1p5();
        for t in self.skill_page.targets.clone() {
            let id = s(&t["id"]).to_string();
            let on = selected.contains(&id);
            let current = selected.clone();
            chips = chips.child(div().id(SharedString::from(format!("target-{id}"))).flex().h(px(24.0)).items_center().rounded_md().border_1().px_2().text_size(px(11.0)).cursor_pointer()
                .when(on, |d| d.border_color(c.accent).text_color(c.fg)).when(!on, |d| d.border_color(c.border_strong).text_color(c.muted))
                .on_click(cx.listener(move |this, _, _, cx| {
                    let mut next = current.clone();
                    if let Some(i) = next.iter().position(|x| *x == id) { next.remove(i); } else { next.push(id.clone()); }
                    this.set_rest("skills", "targets", json!(next), cx);
                }))
                .child(format!("{}{}", s(&t["label"]), if t["detected"] == true { "" } else { " · not found" })));
        }
        let install_for = div().flex().items_start().gap_3().rounded_lg().border_1().border_color(c.border).bg(c.panel).px_3p5().py_2p5()
            .child(div().mt_1().flex_shrink_0().text_xs().text_color(c.muted).child("Install for"))
            .child(div().min_w_0().flex_1().child(chips).child(div().mt_1p5().text_size(px(11.0)).text_color(if selected.is_empty() { c.danger } else { c.subtle }).child(if selected.is_empty() { "Pick at least one agent." } else { "Codex, OpenCode, Cursor and other agents also read the shared .agents folder." })));
        let page = div().flex().flex_col().gap_5().child(toolbar);
        if tab == "discover" {
            page.child(self.skills_discover(source, listing, &query, selected, install_for, c, cx))
        } else {
            page.child(self.skills_installed(installed, outdated, &query, selected, install_for, c, cx))
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn skills_discover(&mut self, source: String, listing: Option<Value>, query: &str, selected: Vec<String>, install_for: Div, c: Colors, cx: &mut Context<Self>) -> Div {
        let sources = self.skill_list("sources", cx).unwrap_or_default();
        let new_repo = self.skill_page.new_repo.clone().unwrap();
        let chip = |id: String, label: String, repo: String, on: bool, cx: &mut Context<Self>| {
            div().id(SharedString::from(id)).flex().h(px(24.0)).items_center().rounded_md().border_1().px_2().text_size(px(11.0)).cursor_pointer()
                .when(on, |d| d.border_color(c.accent).text_color(c.fg)).when(!on, |d| d.border_color(c.border_strong).text_color(c.muted))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.skill_page.source = Some(repo.clone());
                    this.skill_page.error = None;
                    cx.notify();
                }))
                .child(label)
        };
        let mut row = div().flex().flex_wrap().items_center().gap_1p5();
        for f in FEATURED {
            row = row.child(chip(format!("source-{}", f.0), f.1.to_string(), f.0.to_string(), source == f.0, cx));
        }
        for repo in sources.clone() {
            let (remove, rest) = (repo.clone(), sources.clone());
            row = row.child(div().flex().items_center().gap_0p5().child(chip(format!("source-{repo}"), repo.clone(), repo.clone(), source == repo, cx))
                .child(div().id(SharedString::from(format!("source-x-{repo}"))).text_size(px(10.0)).text_color(c.subtle).cursor_pointer().on_click(cx.listener(move |this, _, _, cx| {
                    let next: Vec<&String> = rest.iter().filter(|r| **r != remove).collect();
                    this.set_rest("skills", "sources", json!(next), cx);
                    if this.skill_page.source.as_ref() == Some(&remove) {
                        this.skill_page.source = None;
                    }
                })).child("×")));
        }
        row = row.child(div().w(px(144.0)).child(Input::new(&new_repo)));
        let description = FEATURED.iter().find(|f| f.0 == source).map_or(String::new(), |f| format!(" · {}", f.2));
        let refresh = source.clone();
        let mut out = div().flex().flex_col().gap_5()
            .child(div().flex().flex_col().gap_2p5().child(row).child(div().flex().items_center().gap_2().px_1()
                .child(div().min_w_0().flex_1().truncate().text_xs().text_color(c.muted).child(format!("{source}{description}")))
                .child(small_button(c, "skills-refresh", "Refresh").on_click(cx.listener(move |this, _, _, cx| this.load_listing(refresh.clone(), true, cx))))))
            .child(install_for);
        let note = |t: String| div().py_8().text_center().text_xs().text_color(c.muted).child(t);
        if let Some(e) = self.skill_page.error.clone() {
            out = out.child(note(format!("Couldn't load {source}: {e}")));
        } else if let Some(listing) = listing {
            let all = listing["skills"].as_array().cloned().unwrap_or_default();
            let shown: Vec<&Value> = all.iter().filter(|x| matches_search(query, &[s(&x["name"]), s(&x["description"]), s(&x["path"])])).collect();
            if shown.is_empty() {
                out = out.child(note(if all.is_empty() { format!("{source} has no skills. No folder in it has a SKILL.md.") } else { format!("No skill in {source} matches “{query}”") }));
            }
            let busy = self.skill_page.busy.clone();
            let mut grid = div().flex().flex_wrap().gap_3();
            for skill in shown {
                let name = s(&skill["name"]).to_string();
                let path = s(&skill["path"]).to_string();
                let have = self.skill_page.installed.iter().flatten().find(|i| i["name"] == name.as_str()).cloned();
                let origin = have.as_ref().and_then(origin_of);
                let same = origin.is_some_and(|o| s(&o["repo"]).eq_ignore_ascii_case(s(&listing["repo"])) && o["path"] == skill["path"]);
                let outdated = same && have.as_ref().is_some_and(|h| is_outdated(h, Some(&listing)));
                let copies: Vec<String> = have.as_ref().map_or(Vec::new(), |h| home_copies(h).iter().map(|c| s(&c["target"]).to_string()).collect());
                let everywhere = have.is_some() && selected.iter().all(|t| copies.contains(t));
                let open = self.skill_page.expanded.as_ref() == Some(&path);
                let toggle = path.clone();
                let files = skill["files"].as_u64().unwrap_or(0);
                let can = busy.is_none() && !selected.is_empty();
                let label = if outdated { "Update" } else if have.is_some() && same { "Install for more" } else { "Install" };
                let mut into = selected.clone();
                if outdated {
                    into.extend(copies.iter().filter(|c| !selected.contains(c)).cloned());
                }
                let (repo, entry) = (s(&listing["repo"]).to_string(), skill.clone());
                let for_text = format!("For {}", self.target_names(&copies));
                grid = grid.child(div().w(px(250.0)).flex().flex_col().rounded_lg().border_1().border_color(c.border).bg(c.panel).p_3p5()
                    .child(div().truncate().text_size(px(12.0)).text_color(c.fg).child(name.clone()))
                    .child(div().truncate().text_size(px(11.0)).text_color(c.subtle).child(format!("{path} · {files} file{}", if files == 1 { "" } else { "s" })))
                    .child(div().id(SharedString::from(format!("skill-desc-{path}"))).mt_2p5().text_xs().text_color(c.muted).cursor_pointer().when(!open, |d| d.line_clamp(3))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.skill_page.expanded = if open { None } else { Some(toggle.clone()) };
                            cx.notify();
                        }))
                        .child(skill["description"].as_str().filter(|d| !d.is_empty()).unwrap_or("No description.").to_string()))
                    .when(have.is_some() && !same, |d| d.child(div().mt_2().rounded_md().bg(Hsla { a: 0.1, ..c.warning }).px_2().py_1p5().text_size(px(11.0)).text_color(c.warning).child(format!("A different “{name}” is installed; installing replaces it."))))
                    .child(div().mt_3().flex().items_center().justify_end().gap_2()
                        .when(have.is_some() && same && !outdated, |d| d.child(div().mr_auto().truncate().text_size(px(11.0)).text_color(c.success).child(if everywhere { "Installed".to_string() } else { for_text })))
                        .when(!(have.is_some() && everywhere && !outdated), |d| d.child(accent_button(c, SharedString::from(format!("skill-install-{path}")), label, can).on_click(cx.listener(move |this, _, _, cx| {
                            if can {
                                this.install_skill(repo.clone(), entry.clone(), outdated, into.clone(), cx);
                            }
                        }))))));
            }
            out = out.child(grid);
        } else {
            out = out.child(note("Reading the repo…".into()));
        }
        out.child(div().px_1().text_size(px(11.0)).text_color(c.subtle).child("Every folder with a SKILL.md in a GitHub repo is a skill. Skills are instructions and scripts your agents follow, so install them from repos you trust."))
    }

    #[allow(clippy::too_many_arguments)]
    fn skills_installed(&mut self, installed: Option<Vec<Value>>, outdated: Vec<Value>, query: &str, selected: Vec<String>, install_for: Div, c: Colors, cx: &mut Context<Self>) -> Div {
        let busy = self.skill_page.busy.clone();
        let free = busy.is_none();
        let mut out = div().flex().flex_col().gap_5();
        if !outdated.is_empty() && query.is_empty() {
            let text = if outdated.len() == 1 { format!("An update for {} is available.", s(&outdated[0]["name"])) } else { format!("{} skill updates are available.", outdated.len()) };
            let list = outdated.clone();
            out = out.child(div().flex().items_center().gap_2().rounded_lg().border_1().border_color(c.accent).px_3().py_2().text_xs().child(div().flex_1().text_color(c.fg).child(text))
                .child(accent_button(c, "skills-update-all", if outdated.len() == 1 { "Update" } else { "Update all" }, free).on_click(cx.listener(move |this, _, _, cx| {
                    if free {
                        for skill in &list {
                            this.update_skill(skill, cx);
                        }
                    }
                }))));
        }
        let Some(all) = installed else {
            return out.child(div().py_8().text_center().text_xs().text_color(c.muted).child("Reading skills…")).child(install_for);
        };
        let shown: Vec<&Value> = all.iter().filter(|x| matches_search(query, &[s(&x["name"]), s(&x["description"]), origin_of(x).map_or("", |o| s(&o["repo"]))])).collect();
        if shown.is_empty() {
            out = out.child(div().py_8().text_center().text_xs().text_color(c.muted).child(if all.is_empty() { "No skills installed. Skills are folders with a SKILL.md that agents load from their skills folder.".to_string() } else { format!("No installed skill matches “{query}”") }));
        } else {
            let projects = self.skill_page.projects.clone();
            let mut list = div().flex().flex_col().overflow_hidden().rounded_lg().border_1().border_color(c.border).bg(c.panel);
            for (i, skill) in shown.into_iter().enumerate() {
                let name = s(&skill["name"]).to_string();
                let origin = origin_of(skill);
                let is_old = origin.is_some_and(|o| is_outdated(skill, self.skill_page.listings.get(s(&o["repo"]))));
                let copies = skill["copies"].as_array().cloned().unwrap_or_default();
                let can_copy = copies.iter().any(|c| c["kind"] == "skill");
                let home: Vec<String> = home_copies(skill).iter().map(|c| s(&c["target"]).to_string()).collect();
                let missing: Vec<String> = if can_copy { selected.iter().filter(|t| !home.contains(t)).cloned().collect() } else { Vec::new() };
                let source = origin.map_or_else(|| if copies.iter().all(|c| c["project"].is_string()) { "in the project".to_string() } else { "added by hand".to_string() }, |o| format!("from {}", s(&o["repo"])));
                let mut pills = div().mt_1p5().flex().flex_wrap().items_center().gap_1();
                for copy in &copies {
                    pills = pills.child(div().rounded_sm().bg(c.active).px_1().text_size(px(10.0)).text_color(c.subtle).child(format!("{}{}", copy_label(copy), if copy["link"] == true { " ↗" } else { "" })));
                }
                pills = pills.child(div().ml_1().truncate().text_size(px(11.0)).text_color(c.subtle).child(source));
                let (up, add, rm) = (skill.clone(), skill.clone(), skill.clone());
                let missing_names = self.target_names(&missing);
                let missing_copy = missing.clone();
                let project_list = projects.clone();
                list = list.child(div().flex().gap_3().px_3p5().py_3().when(i > 0, |d| d.border_t_1().border_color(c.border))
                    .child(div().min_w_0().flex_1()
                        .child(div().flex().flex_wrap().items_center().gap_1().child(div().truncate().text_size(px(12.0)).text_color(c.fg).child(name.clone())).when(is_old, |d| d.child(div().px_1().text_size(px(10.0)).text_color(c.accent).child("update available"))))
                        .when(skill["description"].as_str().is_some_and(|d| !d.is_empty()), |d| d.child(div().mt_0p5().line_clamp(2).text_xs().text_color(c.muted).child(s(&skill["description"]).to_string())))
                        .child(pills)
                        .when(is_old || !missing.is_empty(), |d| d.child(div().mt_2p5().flex().flex_wrap().gap_1p5()
                            .when(is_old, |d| d.child(accent_button(c, SharedString::from(format!("skill-up-{name}")), "Update", free).on_click(cx.listener(move |this, _, _, cx| if free { this.update_skill(&up, cx) }))))
                            .when(!missing.is_empty(), |d| d.child(plain_button(c, SharedString::from(format!("skill-add-{name}")), format!("Add to {missing_names}")).on_click(cx.listener(move |this, _, _, cx| {
                                if !free {
                                    return;
                                }
                                let from = home_copies(&add).first().copied().or_else(|| add["copies"].as_array()?.iter().find(|c| c["kind"] == "skill")).map(|c| s(&c["dir"]).to_string());
                                let Some(from) = from else { return };
                                let (sname, targets, projects) = (s(&add["name"]).to_string(), missing_copy.clone(), project_list.clone());
                                let done = format!("Added {sname} to {}", this.target_names(&targets));
                                let key = sname.clone();
                                this.skill_work(key, Some(done), move || tauri::async_runtime::block_on(skills::skills_copy(sname, from, targets, Some(projects))), cx);
                            })))))))
                    .child(small_button(c, SharedString::from(format!("skill-rm-{name}")), "Remove").on_click(cx.listener(move |this, _, window, cx| {
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
                    }))));
            }
            out = out.child(list);
        }
        out.child(install_for)
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
