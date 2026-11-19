//! Race mode, A/B tests, broadcast, recipes and the agent bar (port of `features/orchestration`,
//! `features/recipes` and `features/overview/AgentBar.tsx`).

use super::commands::{Command, PaneRef, worktree_info};
use super::review::{self, ReviewStatus, small_button};
use super::*;
use crate::model::layout::{PRESETS, Preset, build_preset};
use crate::model::{AgentProvider, Overlay};
use crate::model::progress::RunTime;
use gpui_kit::component::Sizable;
use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashSet;
use winfer_backend::{usage_history, worktree};

#[derive(Clone, Copy, Debug, Default)]
pub struct Contender {
    pub time_ms: Option<f64>,
    pub cost: Option<f64>,
    pub tokens: Option<f64>,
    /// Lines and files changed; `None` while unknown.
    pub changed: Option<f64>,
    pub errored: bool,
    pub finished: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Verdict {
    /// Index of the winner, or `None` when there is none (tie, too few numbers, everyone out).
    pub winner: Option<usize>,
    /// Overall score per contender, 0–1; `None` when it's out or unscored.
    pub scores: Vec<Option<f64>>,
    /// Why each contender that's out is out.
    pub out: Vec<Option<&'static str>>,
    /// Whether every contender has finished, so the verdict is final.
    pub finished: bool,
    pub reason: String,
}

/// Scores this close count as a tie.
const TIE: f64 = 0.02;

type Key = (&'static str, f64, fn(&Contender) -> Option<f64>);
const WEIGHTS: [Key; 3] = [("cost", 0.4, |c| c.cost), ("time", 0.4, |c| c.time_ms), ("tokens", 0.2, |c| c.tokens)];

/// Picks the overall winner: each agent scores 0–1 per metric against the best one, weighted and
/// averaged over the metrics known for every contender. Agents that errored, or did nothing while
/// others changed code, are out.
pub fn judge(cs: &[Contender]) -> Verdict {
    let any_changed = cs.iter().any(|c| c.changed.unwrap_or(0.0) > 0.0);
    let out: Vec<Option<&'static str>> = cs.iter().map(|c| if c.errored { Some("errored") } else if any_changed && c.changed == Some(0.0) { Some("changed nothing") } else { None }).collect();
    let finished = cs.iter().all(|c| c.finished);
    let mut scores: Vec<Option<f64>> = vec![None; cs.len()];
    let live: Vec<usize> = (0..cs.len()).filter(|i| out[*i].is_none()).collect();
    let none = |scores: Vec<Option<f64>>, reason: &str| Verdict { winner: None, scores, out: out.clone(), finished, reason: reason.into() };
    if live.is_empty() {
        return none(scores, "No agent finished the task cleanly.");
    }
    if live.len() == 1 {
        scores[live[0]] = Some(1.0);
        let reason = format!("The only agent that {}.", if any_changed { "changed code" } else { "didn't error" });
        return Verdict { winner: Some(live[0]), scores, out, finished, reason };
    }
    let keys: Vec<&Key> = WEIGHTS.iter().filter(|(_, _, get)| live.iter().all(|i| get(&cs[*i]).is_some())).collect();
    if keys.is_empty() {
        return none(scores, "Not enough numbers yet to pick a winner.");
    }
    let total: f64 = keys.iter().map(|k| k.1).sum();
    let best: Vec<f64> = keys.iter().map(|(_, _, get)| live.iter().map(|i| get(&cs[*i]).unwrap()).fold(f64::INFINITY, f64::min)).collect();
    for i in &live {
        let sum: f64 = keys.iter().zip(&best).map(|((_, weight, get), b)| {
            let v = get(&cs[*i]).unwrap();
            weight * if v <= 0.0 { 1.0 } else { b / v }
        }).sum();
        scores[*i] = Some(sum / total);
    }
    let mut ranked = live.clone();
    ranked.sort_by(|x, y| scores[*y].unwrap().total_cmp(&scores[*x].unwrap()));
    let (first, second) = (ranked[0], ranked[1]);
    if scores[first].unwrap() - scores[second].unwrap() < TIE {
        return none(scores, "Too close to call: the top agents score within 2%.");
    }
    let wins: Vec<&str> = keys.iter().zip(&best).filter(|((_, _, get), b)| {
        get(&cs[first]) == Some(**b) && live.iter().any(|i| *i != first && get(&cs[*i]).unwrap() > **b)
    }).map(|((name, _, _), _)| *name).collect();
    let reason = if wins.is_empty() {
        "Best balance of cost, time and tokens.".to_string()
    } else {
        format!("Best overall; {} on {}.", if wins.len() == keys.len() { "best" } else { "lowest" }, wins.join(", "))
    };
    Verdict { winner: Some(first), scores, out, finished, reason }
}

pub fn format_tokens(n: f64) -> String {
    if n >= 1e9 {
        format!("{:.*}B", if n >= 1e10 { 1 } else { 2 }, n / 1e9)
    } else if n >= 1e6 {
        format!("{:.*}M", if n >= 1e7 { 1 } else { 2 }, n / 1e6)
    } else if n >= 1e3 {
        format!("{:.*}K", if n >= 1e4 { 0 } else { 1 }, n / 1e3)
    } else {
        format!("{}", n.round())
    }
}

fn grouped(n: f64) -> String {
    let digits = format!("{}", n.round() as i64);
    let (sign, digits) = digits.strip_prefix('-').map_or(("", digits.as_str()), |d| ("-", d));
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    format!("{sign}{out}")
}

pub fn format_cost(n: f64) -> String {
    if n == 0.0 {
        "$0".into()
    } else if n < 0.01 {
        "<$0.01".into()
    } else if n >= 1000.0 {
        format!("${}", grouped(n))
    } else {
        format!("${n:.2}")
    }
}

/// "45s", "3m 20s", "1h 05m": seconds matter when comparing short runs.
pub fn format_duration(ms: f64) -> String {
    let sec = (ms / 1000.0).round() as u64;
    if sec < 60 {
        return format!("{sec}s");
    }
    let min = sec / 60;
    if min < 60 {
        return format!("{min}m {:02}s", sec % 60);
    }
    format!("{}h {:02}m", min / 60, min % 60)
}

/// Folders compare with forward slashes and without a trailing slash, like the backend's.
fn norm_dir(dir: &str) -> String {
    let d = dir.replace('\\', "/");
    if d.len() > 1 { d.trim_end_matches('/').to_string() } else { d }
}

#[derive(Clone, Debug, Default, PartialEq)]
struct Changes {
    files: u64,
    adds: u64,
    dels: u64,
    untracked: u64,
    untracked_lines: u64,
    ahead: u64,
    dirty: bool,
}

fn changes_of(d: &Value) -> Changes {
    let mut c = Changes {
        untracked: d["untracked"].as_array().map_or(0, |a| a.len() as u64),
        untracked_lines: d["untrackedLines"].as_u64().unwrap_or(0),
        ahead: d["ahead"].as_u64().unwrap_or(0),
        dirty: d["dirty"] == true,
        ..Default::default()
    };
    for line in d["diff"].as_str().unwrap_or_default().split('\n') {
        if line.starts_with("diff --git ") {
            c.files += 1;
        } else if line.starts_with('+') && !line.starts_with("+++") {
            c.adds += 1;
        } else if line.starts_with('-') && !line.starts_with("---") {
            c.dels += 1;
        }
    }
    c
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RecipePane {
    provider: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    prompt: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Recipe {
    id: String,
    name: String,
    panes: Vec<RecipePane>,
    preset: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    url: Option<String>,
}

fn preset_from(name: &str) -> Preset {
    match name {
        "main-stack" => Preset::MainStack,
        "grid" => Preset::Grid,
        "columns" => Preset::Columns,
        "rows" => Preset::Rows,
        _ => Preset::Auto,
    }
}

fn preset_name(preset: Preset) -> &'static str {
    match preset {
        Preset::Auto => "auto",
        Preset::MainStack => "main-stack",
        Preset::Grid => "grid",
        Preset::Columns => "columns",
        Preset::Rows => "rows",
    }
}

/// Shown until the user saves their own.
fn built_in() -> Vec<Recipe> {
    let pane = |p: &str| RecipePane { provider: p.into(), prompt: None };
    vec![
        Recipe { id: "builtin-pair".into(), name: "Pair programming".into(), panes: vec![pane("claude"), pane("codex")], preset: "columns".into(), url: None },
        Recipe { id: "builtin-fullstack".into(), name: "Full stack".into(), panes: vec![pane("claude"), pane("codex"), pane("shell")], preset: "main-stack".into(), url: Some("localhost:3000".into()) },
    ]
}

#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum Target {
    #[default]
    All,
    Working,
    Waiting,
}

impl Target {
    fn includes(self, status: AgentStatus) -> bool {
        match self {
            Self::All => true,
            Self::Working => status == AgentStatus::Working,
            Self::Waiting => status == AgentStatus::NeedsInput,
        }
    }

    fn next(self) -> Self {
        match self {
            Self::All => Self::Working,
            Self::Working => Self::Waiting,
            Self::Waiting => Self::All,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::All => "All agents",
            Self::Working => "Working",
            Self::Waiting => "Waiting for me",
        }
    }
}

struct RaceForm {
    input: Entity<TextareaState>,
    ab_provider: String,
    counts: HashMap<String, usize>,
    show_form: bool,
}

struct BroadcastUi {
    input: Entity<TextareaState>,
    excluded: HashSet<String>,
}

#[derive(Default)]
pub(super) struct Orchestration {
    race: Option<RaceForm>,
    broadcast: Option<BroadcastUi>,
    recipe_name: Option<Entity<InputState>>,
    bar_input: Option<Entity<InputState>>,
    bar_target: Target,
    /// The race shown in the results dialog.
    results: Option<String>,
    changes: HashMap<String, Result<Changes, String>>,
    changes_key: String,
    usage: HashMap<String, Value>,
    notified: HashSet<String>,
}

impl Orchestration {
    pub(super) fn has_layer(&self) -> bool {
        self.race.is_some() || self.broadcast.is_some() || self.recipe_name.is_some() || self.results.is_some()
    }
}

pub(super) fn accent_button(c: Colors, id: impl Into<ElementId>, label: impl Into<SharedString>, enabled: bool) -> Stateful<Div> {
    div().id(id).rounded_md().bg(c.accent).px_2p5().py_1().text_xs().text_color(c.on_accent)
        .when(enabled, |d| d.cursor_pointer().hover(|d| d.opacity(0.9)))
        .when(!enabled, |d| d.opacity(0.4))
        .child(label.into())
}

fn race_panes(state: &AppState, only_active: bool) -> Vec<(String, Vec<Pane>)> {
    let mut races: Vec<(String, Vec<Pane>)> = Vec::new();
    let active = state.active_workspace_id.as_str();
    for pane in state.workspaces.iter().filter(|w| !only_active || w.id == active).flat_map(|w| &w.panes) {
        let Some(id) = &pane.race_id else { continue };
        match races.iter_mut().find(|(r, _)| r == id) {
            Some((_, list)) => list.push(pane.clone()),
            None => races.push((id.clone(), vec![pane.clone()])),
        }
    }
    races
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

impl Root {
    /// (id, name) of the recipes, for voice.
    pub(super) fn recipe_names(&self, cx: &App) -> Vec<(String, String)> {
        self.recipes(cx).into_iter().map(|r| (r.id, r.name)).collect()
    }

    fn recipes(&self, cx: &App) -> Vec<Recipe> {
        let saved = self.saved_recipes(cx);
        let mut all = if saved.is_empty() { built_in() } else { saved };
        all.extend(self.plugin_recipes().into_iter().filter_map(|r| serde_json::from_value(r).ok()));
        all
    }

    fn saved_recipes(&self, cx: &App) -> Vec<Recipe> {
        self.state.read(cx).rest.get("recipes").and_then(|v| serde_json::from_value(v.clone()).ok()).unwrap_or_default()
    }

    fn set_recipes(&self, recipes: Vec<Recipe>, cx: &mut Context<Self>) {
        self.update_state(cx, |s| {
            s.rest.insert("recipes".into(), serde_json::to_value(recipes).unwrap_or(json!([])));
        });
    }

    pub(super) fn recipe_entries(&self, cx: &App) -> Vec<(String, String)> {
        self.recipes(cx).into_iter().map(|r| (r.id, r.name)).collect()
    }

    pub(super) fn load_recipe(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(recipe) = self.recipes(cx).into_iter().find(|r| r.id == id) else { return };
        self.update_state(cx, |s| {
            let installed: Vec<String> = s.providers.iter().filter(|p| p.installed).map(|p| p.id.clone()).collect();
            for pane in &recipe.panes {
                if installed.contains(&pane.provider) {
                    s.add_pane(&pane.provider, AddPane { prompt: pane.prompt.clone(), ..Default::default() });
                }
            }
            let preset = preset_from(&recipe.preset);
            if let Some(w) = s.active_mut() {
                w.layout = w.layout.as_ref().and_then(|l| build_preset(preset, &leaves(l)));
                w.preset_index = PRESETS.iter().position(|p| *p == preset).unwrap_or(0);
                w.zoomed_pane_id = None;
            }
            let ws = s.active_workspace_id.clone();
            s.log_activity(&ws, format!("Loaded recipe “{}”", recipe.name), None);
        });
        if let Some(url) = recipe.url {
            self.execute(Command::BrowserOpen { url, new_tab: false }, window, cx);
        }
    }

    fn save_recipe(&mut self, name: String, cx: &mut Context<Self>) {
        let s = self.state.read(cx);
        let Some(ws) = s.active().filter(|w| w.layout.is_some()) else { return };
        let panes = leaves(ws.layout.as_ref().unwrap()).iter().filter_map(|id| ws.panes.iter().find(|p| p.id == *id)).map(|p| RecipePane { provider: p.provider.clone(), prompt: None }).collect();
        let recipe = Recipe {
            id: crate::model::new_id("recipe"),
            name,
            panes,
            preset: preset_name(PRESETS.get(ws.preset_index).copied().unwrap_or(Preset::Auto)).to_string(),
            url: self.browser.read(cx).active_url(&ws.id).filter(|u| u != "about:blank"),
        };
        let mut recipes = self.saved_recipes(cx);
        recipes.push(recipe);
        self.set_recipes(recipes, cx);
    }

    pub(super) fn start_race(&mut self, counts: Vec<(String, usize)>, prompt: String, cx: &mut Context<Self>) {
        let race_id = crate::model::new_id("race");
        let racers: usize = counts.iter().map(|c| c.1).sum();
        self.update_state(cx, |s| {
            for (provider, count) in &counts {
                for _ in 0..*count {
                    s.add_pane(provider, AddPane { prompt: Some(prompt.clone()), worktree: Some(true), race_id: Some(race_id.clone()), ..Default::default() });
                }
            }
            if let Some(w) = s.active_mut() {
                w.layout = w.layout.as_ref().and_then(|l| build_preset(Preset::Grid, &leaves(l)));
            }
            s.overlay = Overlay::Race;
            let ws = s.active_workspace_id.clone();
            s.log_activity(&ws, format!("Race started with {racers} agents: {prompt}"), None);
        });
    }

    pub(super) fn start_ab_test(&mut self, provider: String, prompt: String, model: Option<String>, cx: &mut Context<Self>) {
        let race_id = crate::model::new_id("ab");
        self.update_state(cx, |s| {
            let name = s.provider_name(&provider);
            for (plain, title) in [(false, format!("A · {name} with tools")), (true, format!("B · {name} plain"))] {
                s.add_pane(&provider, AddPane { prompt: Some(prompt.clone()), worktree: Some(true), race_id: Some(race_id.clone()), model: model.clone(), plain, custom_title: Some(title), ..Default::default() });
            }
            if let Some(w) = s.active_mut() {
                w.layout = w.layout.as_ref().and_then(|l| build_preset(Preset::Grid, &leaves(l)));
            }
            s.overlay = Overlay::AbTest;
            let ws = s.active_workspace_id.clone();
            s.log_activity(&ws, format!("A/B test started with {name}: {prompt}"), Some(provider.clone()));
        });
    }

    pub(super) fn open_race_results(&mut self, race: String, cx: &mut Context<Self>) {
        self.orchestration.results = Some(race);
        self.orchestration.changes_key.clear();
        cx.notify();
    }

    fn close_race_results(&mut self, cx: &mut Context<Self>) {
        self.orchestration.results = None;
        self.orchestration.changes_key.clear();
        self.focused_view = None;
        cx.notify();
    }

    pub(super) fn dismiss_overlay(&mut self, cx: &mut Context<Self>) {
        self.update_state(cx, |s| s.overlay = Overlay::None);
        self.focused_view = None;
    }

    /// The topmost orchestration layer, for Escape.
    pub(super) fn close_orchestration_layer(&mut self, cx: &mut Context<Self>) -> bool {
        if self.orchestration.results.is_some() {
            self.close_race_results(cx);
            return true;
        }
        if matches!(self.state.read(cx).overlay, Overlay::Race | Overlay::AbTest | Overlay::Broadcast | Overlay::Recipes | Overlay::Progress) {
            self.dismiss_overlay(cx);
            return true;
        }
        false
    }

    /// Creates and drops each layer's inputs as overlays open and close, and reloads numbers.
    pub(super) fn sync_orchestration(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let overlay = self.state.read(cx).overlay;
        let wants_race = matches!(overlay, Overlay::Race | Overlay::AbTest);
        if wants_race && self.orchestration.race.is_none() {
            let input = cx.new(|cx| TextareaState::new(window, cx).auto_grow(3, 8).soft_wrap(true).placeholder("Task for every agent, e.g. “Make the cancelled-tier guard return 403 and add a regression test”"));
            cx.subscribe(&input, |_, _, _: &InputEvent, cx| cx.notify()).detach();
            input.read(cx).focus_handle(cx).focus(window, cx);
            self.orchestration.race = Some(RaceForm { input, ab_provider: String::new(), counts: HashMap::new(), show_form: false });
        } else if !wants_race {
            self.orchestration.race = None;
        }
        // Providers load after the form can open.
        if let Some(form) = self.orchestration.race.as_mut().filter(|f| f.counts.is_empty() && f.ab_provider.is_empty()) {
            let installed: Vec<String> = self.state.read(cx).providers.iter().filter(|p| p.installed && p.id != "shell").map(|p| p.id.clone()).collect();
            form.counts = installed.iter().take(2).map(|p| (p.clone(), 1)).collect();
            form.ab_provider = installed.first().cloned().unwrap_or_default();
        }
        let wants_broadcast = overlay == Overlay::Broadcast;
        if wants_broadcast && self.orchestration.broadcast.is_none() {
            let input = cx.new(|cx| TextareaState::new(window, cx).auto_grow(3, 8).soft_wrap(true).submit_on_enter(true).placeholder("e.g. run the tests and fix any failures (Shift+Enter for a new line)"));
            cx.subscribe_in(&input, window, |this, _, event: &InputEvent, window, cx| match event {
                InputEvent::PressEnter { shift: false, .. } => this.send_broadcast(window, cx),
                _ => cx.notify(),
            })
            .detach();
            input.read(cx).focus_handle(cx).focus(window, cx);
            self.orchestration.broadcast = Some(BroadcastUi { input, excluded: HashSet::new() });
        } else if !wants_broadcast {
            self.orchestration.broadcast = None;
        }
        let wants_recipes = overlay == Overlay::Recipes;
        if wants_recipes && self.orchestration.recipe_name.is_none() {
            let name = self.state.read(cx).active().map_or("this workspace".to_string(), |w| w.name.clone());
            let input = cx.new(|cx| InputState::new(window, cx).placeholder(format!("Save “{name}” layout as…")));
            cx.subscribe_in(&input, window, |this, input, event: &InputEvent, window, cx| match event {
                InputEvent::PressEnter { .. } => {
                    let text = input.read(cx).value().trim().to_string();
                    if !text.is_empty() {
                        this.save_recipe(text, cx);
                        input.update(cx, |i, cx| i.set_value("", window, cx));
                    }
                }
                _ => cx.notify(),
            })
            .detach();
            self.orchestration.recipe_name = Some(input);
        } else if !wants_recipes {
            self.orchestration.recipe_name = None;
        }
        if self.orchestration.bar_input.is_none() {
            let input = cx.new(|cx| InputState::new(window, cx));
            cx.subscribe_in(&input, window, |this, _, event: &InputEvent, window, cx| match event {
                InputEvent::PressEnter { .. } => this.bar_send(true, window, cx),
                _ => cx.notify(),
            })
            .detach();
            self.orchestration.bar_input = Some(input);
        }
        self.reload_race_numbers(wants_race, cx);
        self.notify_ab_finished(cx);
    }

    fn reload_race_numbers(&mut self, board: bool, cx: &mut Context<Self>) {
        let results = self.orchestration.results.clone();
        if !board && results.is_none() {
            return;
        }
        let s = self.state.read(cx);
        let panes: Vec<Pane> = race_panes(s, results.is_none()).into_iter().filter(|(r, _)| results.as_ref().is_none_or(|x| x == r)).flat_map(|(_, p)| p).collect();
        let key = panes.iter().map(|p| format!("{}:{:?}", p.worktree.as_ref().map_or("", |t| &t.path), p.status)).collect::<Vec<_>>().join("|");
        if key != self.orchestration.changes_key {
            self.orchestration.changes_key = key;
            self.load_race_numbers(panes, results.is_some(), cx);
        }
    }

    fn load_race_numbers(&mut self, panes: Vec<Pane>, with_usage: bool, cx: &mut Context<Self>) {
        for pane in panes.iter().filter(|p| p.worktree.is_some()) {
            let (id, info) = (pane.id.clone(), pane.worktree.clone().unwrap());
            cx.spawn(async move |this, cx| {
                let found = tauri::async_runtime::spawn(async move {
                    let d = worktree::worktree_diff(worktree_info(&info)?).await?;
                    serde_json::to_value(d).map_err(|e| e.to_string())
                })
                .await
                .map_err(|e| e.to_string())
                .and_then(|r| r)
                .map(|v| changes_of(&v));
                let _ = this.update(cx, |this, cx| {
                    this.orchestration.changes.insert(id, found);
                    cx.notify();
                });
            })
            .detach();
        }
        if !with_usage {
            return;
        }
        cx.spawn(async move |this, cx| {
            let dirs: Vec<String> = panes.iter().filter_map(|p| p.worktree.as_ref().map(|t| norm_dir(&t.path))).collect();
            let found = tauri::async_runtime::spawn(usage_history::usage_dirs(backend::app().clone(), dirs)).await;
            let found: Vec<Value> = found.ok().and_then(Result::ok).and_then(|d| serde_json::to_value(d).ok()).and_then(|v| v.as_array().cloned()).unwrap_or_default();
            let mut by_pane: HashMap<String, Value> = HashMap::new();
            for pane in &panes {
                let Some(t) = &pane.worktree else { continue };
                if let Some(u) = found.iter().find(|d| d["dir"].as_str().map(norm_dir) == Some(norm_dir(&t.path))) {
                    by_pane.insert(pane.id.clone(), u.clone());
                }
            }
            // An agent can move its session out of the worktree: its session's spend stands in.
            let moved: Vec<(String, String)> = panes.iter().filter(|p| by_pane.get(&p.id).is_none_or(|u| u["calls"].as_u64().unwrap_or(0) == 0)).filter_map(|p| Some((p.id.clone(), p.session_id.clone()?))).collect();
            if !moved.is_empty() {
                let sessions: Vec<String> = moved.iter().map(|m| m.1.clone()).collect();
                let found = tauri::async_runtime::spawn(usage_history::usage_sessions(backend::app().clone(), sessions)).await;
                let found: Vec<Value> = found.ok().and_then(Result::ok).and_then(|d| serde_json::to_value(d).ok()).and_then(|v| v.as_array().cloned()).unwrap_or_default();
                for (pane, session) in moved {
                    if let Some(u) = found.iter().find(|d| d["dir"].as_str() == Some(&session)).filter(|u| u["calls"].as_u64().unwrap_or(0) > 0) {
                        by_pane.insert(pane, u.clone());
                    }
                }
            }
            let _ = this.update(cx, |this, cx| {
                this.orchestration.usage.extend(by_pane);
                cx.notify();
            });
        })
        .detach();
    }

    /// Tells the user once every agent of an A/B test has stopped.
    fn notify_ab_finished(&mut self, cx: &mut Context<Self>) {
        let s = self.state.read(cx);
        let done: Vec<(String, Pane)> = race_panes(s, false).into_iter()
            .filter(|(r, panes)| r.starts_with("ab-") && !self.orchestration.notified.contains(r) && panes.iter().all(|p| s.is_finished(p)) && panes.iter().any(|p| s.run_times.contains_key(&p.id)))
            .map(|(r, panes)| (r, panes[0].clone()))
            .collect();
        for (race, pane) in done {
            self.orchestration.notified.insert(race.clone());
            let name = self.state.read(cx).provider_name(&pane.provider);
            self.update_state(cx, |s| s.log_activity(&pane.workspace_id, format!("A/B test with {name} finished"), Some(pane.provider.clone())));
            self.toast_action(format!("A/B test with {name} finished"), "Results", Command::RaceResults(race), cx);
        }
    }

    fn send_broadcast(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ui) = &self.orchestration.broadcast else { return };
        let text = ui.input.read(cx).value().trim().to_string();
        let excluded = ui.excluded.clone();
        let targets: Vec<String> = self.broadcast_agents(cx).into_iter().map(|p| p.id).filter(|id| !excluded.contains(id)).collect();
        if text.is_empty() || targets.is_empty() {
            return;
        }
        self.dismiss_overlay(cx);
        self.execute(Command::Broadcast { text, submit: true, pane_ids: Some(targets) }, window, cx);
    }

    fn broadcast_agents(&self, cx: &App) -> Vec<Pane> {
        let s = self.state.read(cx);
        let Some(w) = s.active() else { return Vec::new() };
        w.layout.as_ref().map(leaves).unwrap_or_default().iter().filter_map(|id| w.panes.iter().find(|p| p.id == *id)).filter(|p| p.provider != "shell").cloned().collect()
    }

    fn bar_panes(&self, cx: &App) -> Vec<Pane> {
        let s = self.state.read(cx);
        if s.view() == View::Overview {
            s.all_panes().filter(|p| !p.archived).cloned().collect()
        } else {
            s.active().map(|w| w.panes.iter().filter(|p| !p.archived).cloned().collect()).unwrap_or_default()
        }
    }

    fn bar_agents(&self, cx: &App) -> Vec<Pane> {
        let target = self.orchestration.bar_target;
        self.bar_panes(cx).into_iter().filter(|p| p.provider != "shell" && target.includes(p.status)).collect()
    }

    fn bar_send(&mut self, submit: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(input) = self.orchestration.bar_input.clone() else { return };
        let text = input.read(cx).value().trim().to_string();
        let ids: Vec<String> = self.bar_agents(cx).into_iter().map(|p| p.id).collect();
        if text.is_empty() || ids.is_empty() {
            return;
        }
        self.execute(Command::Broadcast { text, submit, pane_ids: Some(ids) }, window, cx);
        input.update(cx, |i, cx| i.set_value("", window, cx));
    }

    /// Messages, approves or interrupts a group of agents at once.
    pub(super) fn render_agent_bar(&mut self, c: &Colors, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let view = self.state.read(cx).view();
        let overview = view == View::Overview;
        let panes = self.bar_panes(cx);
        if !matches!(view, View::Workspace | View::Overview) || (!overview && panes.iter().all(|p| p.provider == "shell")) || (overview && panes.is_empty()) {
            return None;
        }
        let c = *c;
        let input = self.orchestration.bar_input.clone()?;
        let target = self.orchestration.bar_target;
        let agents = self.bar_agents(cx);
        let waiting = panes.iter().filter(|p| p.status == AgentStatus::NeedsInput).count();
        let s = self.state.read(cx);
        let workspaces: HashSet<&String> = panes.iter().map(|p| &p.workspace_id).collect();
        let scope = if overview { format!(" in {} workspace{}", workspaces.len(), if workspaces.len() == 1 { "" } else { "s" }) } else { String::new() };
        let count = panes.iter().filter(|p| p.provider != "shell" && target.includes(p.status)).count();
        let summary = format!("{} session{}{scope}", panes.len(), if panes.len() == 1 { "" } else { "s" });
        let placeholder: SharedString = format!("Message {} agent{} {} (Enter sends)", agents.len(), if agents.len() == 1 { "" } else { "s" }, if overview { "across workspaces".to_string() } else { format!("in {}", s.active().map_or("", |w| w.name.as_str())) }).into();
        input.update(cx, |i, cx| i.set_placeholder(placeholder.clone(), window, cx));
        let has_text = !input.read(cx).value().trim().is_empty();
        let enabled = !agents.is_empty();
        let targets: Vec<String> = agents.iter().map(|p| p.id.clone()).collect();
        let approve = targets.clone();
        Some(
            div().flex().flex_shrink_0().flex_wrap().items_center().gap_2().pb_2().text_xs()
                .child(div().flex().flex_shrink_0().items_center().gap_1p5().text_color(c.muted)
                    .child(Icon::new(IconName::LayoutDashboard).size(px(13.0)).text_color(c.accent))
                    .child(summary)
                    .when(waiting > 0, |d| d.child(div().text_color(c.warning).child(format!("· {waiting} waiting")))))
                .child(small_button(c, "bar-target", format!("{} ({count})", target.label())).on_click(cx.listener(|this, _, _, cx| {
                    this.orchestration.bar_target = this.orchestration.bar_target.next();
                    cx.notify();
                })))
                .child(div().flex_1().min_w(px(224.0)).flex().items_center().gap_1()
                    .child(div().flex_1().child(Input::new(&input).small()))
                    .child(small_button(c, "bar-type", "Type only").when(!has_text, |d| d.opacity(0.4)).on_click(cx.listener(|this, _, window, cx| this.bar_send(false, window, cx)))))
                .child(small_button(c, "bar-approve", "Approve").when(!enabled, |d| d.opacity(0.4)).on_click(cx.listener(move |this, _, window, cx| {
                    for id in &approve {
                        this.execute(Command::Approve(PaneRef::Id(id.clone())), window, cx);
                    }
                })))
                .child(small_button(c, "bar-interrupt", "Interrupt").when(!enabled, |d| d.opacity(0.4)).on_click(cx.listener(move |this, _, window, cx| {
                    for id in &targets {
                        this.execute(Command::Interrupt(PaneRef::Id(id.clone())), window, cx);
                    }
                })))
                .into_any_element(),
        )
    }

    fn dialog(&self, id: &'static str, width: f32, c: Colors, on_close: fn(&mut Self, &mut Context<Self>), cx: &mut Context<Self>) -> (Stateful<Div>, Stateful<Div>) {
        let backdrop = div().id(SharedString::from(format!("{id}-backdrop"))).absolute().size_full().flex().items_start().justify_center().pt(relative(0.1)).bg(hsla(0.0, 0.0, 0.0, 0.4)).occlude()
            .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| on_close(this, cx)));
        let panel = div().id(id).w(px(width)).max_h(relative(0.8)).flex().flex_col().gap_4().overflow_y_scroll().rounded_xl().border_1().border_color(c.border_strong).bg(c.raised).p_5()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation());
        (backdrop, panel)
    }

    pub(super) fn render_broadcast(&mut self, c: &Colors, cx: &mut Context<Self>) -> Option<AnyElement> {
        let ui = self.orchestration.broadcast.as_ref()?;
        let c = *c;
        let (input, excluded) = (ui.input.clone(), ui.excluded.clone());
        let agents = self.broadcast_agents(cx);
        let count = agents.iter().filter(|p| !excluded.contains(&p.id)).count();
        let can_send = !input.read(cx).value().trim().is_empty() && count > 0;
        let (backdrop, panel) = self.dialog("broadcast", 600.0, c, Self::dismiss_overlay, cx);
        let mut chips = div().flex().flex_wrap().items_center().gap_1p5();
        if agents.is_empty() {
            chips = chips.child(div().text_xs().text_color(c.subtle).child("No agents in this workspace."));
        }
        let s = self.state.read(cx);
        for (i, pane) in agents.iter().enumerate() {
            let on = !excluded.contains(&pane.id);
            let toggle = pane.id.clone();
            chips = chips.child(
                div().id(SharedString::from(format!("broadcast-{}", pane.id))).rounded_md().border_1().px_2().py_1().text_xs().cursor_pointer()
                    .when(on, |d| d.border_color(c.accent).text_color(c.fg))
                    .when(!on, |d| d.border_color(c.border).text_color(c.subtle).line_through())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(ui) = this.orchestration.broadcast.as_mut() {
                            if !ui.excluded.remove(&toggle) {
                                ui.excluded.insert(toggle.clone());
                            }
                        }
                        cx.notify();
                    }))
                    .child(format!("{} · {}", i + 1, s.provider_name(&pane.provider))),
            );
        }
        chips = chips.child(div().ml_auto().child(accent_button(c, "broadcast-send", format!("Send to {count}"), can_send).on_click(cx.listener(|this, _, window, cx| this.send_broadcast(window, cx)))));
        Some(
            backdrop.child(panel.gap_3()
                .child(div().text_xs().text_color(c.subtle).child("SEND TO AGENTS"))
                .child(Textarea::new(&input))
                .child(chips))
                .into_any_element(),
        )
    }

    pub(super) fn render_recipes(&mut self, c: &Colors, cx: &mut Context<Self>) -> Option<AnyElement> {
        let input = self.orchestration.recipe_name.clone()?;
        let c = *c;
        let saved = self.saved_recipes(cx);
        let recipes = self.recipes(cx);
        let (backdrop, panel) = self.dialog("recipes", 560.0, c, Self::dismiss_overlay, cx);
        let s = self.state.read(cx);
        let mut list = div().flex().flex_col().rounded_lg().border_1().border_color(c.border);
        for (i, r) in recipes.iter().enumerate() {
            let description = r.panes.iter().map(|p| s.provider_name(&p.provider)).collect::<Vec<_>>().join(" + ") + &r.url.as_ref().map_or(String::new(), |u| format!(" · {u}")) + &format!(" · {}", r.preset);
            let (load, delete) = (r.id.clone(), r.id.clone());
            list = list.child(
                div().flex().items_center().gap_3().px_3().py_2().text_xs().when(i > 0, |d| d.border_t_1().border_color(c.border))
                    .child(div().min_w_0().flex_1().child(div().text_size(px(13.0)).text_color(c.fg).child(r.name.clone())).child(div().truncate().text_color(c.subtle).child(description)))
                    .child(accent_button(c, SharedString::from(format!("recipe-load-{i}")), "Load", true).on_click(cx.listener(move |this, _, window, cx| {
                        this.dismiss_overlay(cx);
                        this.load_recipe(&load, window, cx);
                    })))
                    .when(saved.iter().any(|own| own.id == r.id), |d| {
                        d.child(small_button(c, SharedString::from(format!("recipe-del-{i}")), "Delete").on_click(cx.listener(move |this, _, _, cx| {
                            let mut all = this.saved_recipes(cx);
                            all.retain(|x| x.id != delete);
                            this.set_recipes(all, cx);
                        })))
                    }),
            );
        }
        let can_save = !input.read(cx).value().trim().is_empty() && s.active().is_some_and(|w| w.layout.is_some());
        let save_from = input.clone();
        Some(
            backdrop.child(panel
                .child(div().text_sm().child("Recipes"))
                .child(list)
                .when(saved.is_empty(), |d| d.child(div().text_size(px(11.0)).text_color(c.subtle).child("These are examples (2). Save your own and they replace them.")))
                .child(div().flex().items_center().gap_2()
                    .child(div().flex_1().child(Input::new(&input)))
                    .child(small_button(c, "recipe-save", "Save current").when(!can_save, |d| d.opacity(0.4)).on_click(cx.listener(move |this, _, window, cx| {
                        let name = save_from.read(cx).value().trim().to_string();
                        if !name.is_empty() {
                            this.save_recipe(name, cx);
                            save_from.update(cx, |i, cx| i.set_value("", window, cx));
                        }
                    })))))
                .into_any_element(),
        )
    }

    fn render_start_form(&self, ab: bool, c: Colors, cx: &mut Context<Self>) -> Option<Div> {
        let form = self.orchestration.race.as_ref()?;
        let s = self.state.read(cx);
        let providers: Vec<AgentProvider> = s.providers.iter().filter(|p| p.installed && p.id != "shell").cloned().collect();
        let prompt = form.input.read(cx).value().trim().to_string();
        let total: usize = form.counts.values().sum();
        let mut row = div().flex().flex_wrap().items_center().gap_2();
        for p in &providers {
            if ab {
                let id = p.id.clone();
                let on = form.ab_provider == p.id;
                row = row.child(div().id(SharedString::from(format!("ab-{}", p.id))).rounded_md().border_1().px_2().py_1().text_xs().cursor_pointer()
                    .when(on, |d| d.border_color(c.accent).text_color(c.accent))
                    .when(!on, |d| d.border_color(c.border).text_color(c.fg).hover(move |d| d.bg(c.hover)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(f) = this.orchestration.race.as_mut() {
                            f.ab_provider = id.clone();
                        }
                        cx.notify();
                    }))
                    .child(p.name.clone()));
            } else {
                let bump = |d: isize| {
                    let id = p.id.clone();
                    cx.listener(move |this: &mut Self, _: &ClickEvent, _: &mut Window, cx: &mut Context<Self>| {
                        if let Some(f) = this.orchestration.race.as_mut() {
                            let n = f.counts.entry(id.clone()).or_default();
                            *n = n.saturating_add_signed(d).min(3);
                        }
                        cx.notify();
                    })
                };
                row = row.child(div().flex().items_center().gap_1p5().rounded_md().border_1().border_color(c.border).px_2().py_1().text_xs().text_color(c.fg)
                    .child(p.name.clone())
                    .child(small_button(c, SharedString::from(format!("minus-{}", p.id)), "−").on_click(bump(-1)))
                    .child(div().w_3().text_center().child(form.counts.get(&p.id).copied().unwrap_or(0).to_string()))
                    .child(small_button(c, SharedString::from(format!("plus-{}", p.id)), "+").on_click(bump(1))));
            }
        }
        let ready = !prompt.is_empty() && if ab { !form.ab_provider.is_empty() } else { total >= 2 };
        let label = if ab { "Start A/B test".to_string() } else { format!("Start race ({total})") };
        row = row.child(div().ml_auto().child(accent_button(c, "race-start", label, ready).on_click(cx.listener(move |this, _, window, cx| {
            let Some(f) = this.orchestration.race.as_mut() else { return };
            let prompt = f.input.read(cx).value().trim().to_string();
            let (provider, counts): (String, Vec<(String, usize)>) = (f.ab_provider.clone(), f.counts.iter().filter(|(_, n)| **n > 0).map(|(p, n)| (p.clone(), *n)).collect());
            let total: usize = counts.iter().map(|c| c.1).sum();
            if prompt.is_empty() || (ab && provider.is_empty()) || (!ab && total < 2) {
                return;
            }
            f.show_form = false;
            let input = f.input.clone();
            input.update(cx, |i, cx| i.set_value("", window, cx));
            if ab {
                this.start_ab_test(provider, prompt, None, cx);
            } else {
                this.start_race(counts, prompt, cx);
            }
        }))));
        let hint = if ab {
            "Starts the agent twice on the same task, each in its own git worktree: A as usual, with winfer's tools, the browser and your MCP servers and skills; B plain, without them. Compare time, tokens, cost and code written once both finish."
        } else {
            "Each agent gets its own git worktree and branch, so they can't overwrite each other."
        };
        Some(div().flex().flex_col().gap_3().child(Textarea::new(&form.input)).child(row).child(div().text_size(px(11.0)).text_color(c.subtle).child(hint)))
    }

    fn render_race_board(&self, race: String, panes: Vec<Pane>, c: Colors, cx: &mut Context<Self>) -> Div {
        let s = self.state.read(cx);
        let ab = race.starts_with("ab-");
        let finished = panes.iter().all(|p| s.is_finished(p));
        let isolated = panes.iter().all(|p| p.worktree.is_some());
        let mono = crate::fonts::mono(cx);
        let title = panes[0].prompt.clone().or_else(|| s.pane_tasks.get(&panes[0].id).and_then(|t| t.task.clone())).unwrap_or_else(|| if ab { "A/B test".into() } else { "Race".into() });
        let open = race.clone();
        let results = if finished { accent_button(c, SharedString::from(format!("results-{race}")), "Results", true) } else { small_button(c, SharedString::from(format!("results-{race}")), "Results") };
        let reload = panes.clone();
        let header = div().flex().items_center().gap_2().text_xs().text_color(c.muted)
            .child(div().flex_1().truncate().child(title))
            .child(results.on_click(cx.listener(move |this, _, _, cx| this.open_race_results(open.clone(), cx))))
            .child(small_button(c, SharedString::from(format!("refresh-{race}")), "Refresh").on_click(cx.listener(move |this, _, _, cx| this.load_race_numbers(reload.clone(), false, cx))));
        let mut table = div().flex().flex_col().rounded_lg().border_1().border_color(c.border);
        for (i, pane) in panes.iter().enumerate() {
            let stats = match self.orchestration.changes.get(&pane.id) {
                Some(Err(e)) => div().text_color(c.danger).child(e.clone()),
                Some(Ok(st)) => {
                    let commits = format!(" · {} commit{}{}", st.ahead, if st.ahead == 1 { "" } else { "s" }, if st.dirty { " · uncommitted" } else { "" });
                    div().flex().gap_1()
                        .child(div().text_color(c.muted).child(format!("{} files", st.files + st.untracked)))
                        .child(div().text_color(c.success).child(format!("+{}", st.adds)))
                        .child(div().text_color(c.danger).child(format!("−{}", st.dels)))
                        .child(div().text_color(c.subtle).child(commits))
                }
                None => div().text_color(c.subtle).child("…"),
            };
            let branch = pane.worktree.as_ref().map_or("—".to_string(), |t| t.branch.trim_start_matches("winfer/").to_string());
            let has_tree = pane.worktree.is_some();
            let (focus, diff) = (pane.id.clone(), pane.id.clone());
            let (winner, field) = (pane.clone(), panes.clone());
            table = table.child(
                div().flex().items_center().gap_3().px_3().py_2().text_xs().when(i > 0, |d| d.border_t_1().border_color(c.border))
                    .child(div().size_1p5().rounded_full().bg(status_color(pane.status, &c)))
                    .child(div().w(px(112.0)).truncate().text_color(c.fg).child(s.provider_name(&pane.provider)))
                    .when(ab, |d| d.child(div().w(px(64.0)).flex_shrink_0().rounded_sm().text_center().text_size(px(10.0)).when(pane.plain, |d| d.bg(c.hover).text_color(c.muted)).when(!pane.plain, |d| d.text_color(c.accent)).child(if pane.plain { "plain" } else { "with tools" })))
                    .child(div().w(px(128.0)).truncate().font_family(mono.clone()).text_size(px(11.0)).text_color(c.subtle).child(branch))
                    .child(div().flex_1().min_w_0().truncate().font_family(mono.clone()).text_size(px(11.0)).child(stats))
                    .child(small_button(c, SharedString::from(format!("focus-{}", pane.id)), "Focus").on_click(cx.listener(move |this, _, window, cx| {
                        this.dismiss_overlay(cx);
                        this.execute(Command::Focus(PaneRef::Id(focus.clone())), window, cx);
                    })))
                    .child(small_button(c, SharedString::from(format!("diff-{}", pane.id)), "Diff").when(!has_tree, |d| d.opacity(0.4)).on_click(cx.listener(move |this, _, window, cx| {
                        this.execute(Command::ShowChanges(PaneRef::Id(diff.clone())), window, cx);
                    })))
                    .child(accent_button(c, SharedString::from(format!("pick-{}", pane.id)), "Pick", has_tree).on_click(cx.listener(move |this, _, window, cx| {
                        this.pick_race_winner(winner.clone(), field.clone(), window, cx);
                    }))),
            );
        }
        div().flex().flex_col().gap_2().child(header)
            .when(!isolated, |d| d.child(div().text_size(px(11.0)).text_color(c.warning).child("Some agents have no worktree, so they weren't started: races and A/B tests need the folder to be a git repository.")))
            .child(table)
    }

    /// Merges the winner of a race and closes the other racers, deleting their worktrees.
    fn pick_race_winner(&mut self, winner: Pane, panes: Vec<Pane>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(info) = winner.worktree.clone() else { return };
        let losers: Vec<Pane> = panes.into_iter().filter(|p| p.id != winner.id).collect();
        let detail = format!("Merge {} into {}, then close the other {} agent{} and delete their worktrees?", info.branch, info.base, losers.len(), if losers.len() == 1 { "" } else { "s" });
        self.confirm("Pick winner", detail, "Merge and clean up", window, cx, move |this, cx| {
            let name = this.state.read(cx).provider_name(&winner.provider);
            let branch = info.branch.clone();
            this.toast(format!("Merging {branch}…"), cx);
            let message = format!("Changes by {name}");
            cx.spawn(async move |this, cx| {
                let merged = tauri::async_runtime::spawn(async move {
                    let diff = worktree::worktree_diff(worktree_info(&info)?).await.ok().and_then(|d| serde_json::to_value(d).ok());
                    // Agents often stop without committing; their work would otherwise be left behind.
                    worktree::worktree_commit(worktree_info(&info)?, message).await?;
                    worktree::worktree_merge(worktree_info(&info)?).await?;
                    Ok::<_, String>(diff)
                })
                .await
                .map_err(|e| e.to_string())
                .and_then(|r| r);
                let _ = this.update(cx, |this, cx| {
                    let diff = match merged {
                        Ok(diff) => diff,
                        Err(err) => return this.toast(format!("Merge not possible: {err}"), cx),
                    };
                    this.record_merge_of(&winner, diff.as_ref().map(review::diff_stats), cx);
                    this.set_review_status_for(&winner.id, ReviewStatus::Merged, cx);
                    for loser in &losers {
                        this.close_pane(&loser.id, cx);
                        this.set_review_status_for(&loser.id, ReviewStatus::Discarded, cx);
                        if let Some(tree) = loser.worktree.clone() {
                            tauri::async_runtime::spawn(async move {
                                if let Ok(info) = worktree_info(&tree) {
                                    let _ = worktree::worktree_remove(info, true).await;
                                }
                            });
                        }
                    }
                    this.update_state(cx, |s| {
                        if let Some(p) = s.pane_mut(&winner.id) {
                            p.race_id = None;
                        }
                        s.log_activity(&winner.workspace_id, format!("Race won by {name} ({branch})"), Some(winner.provider.clone()));
                    });
                    this.toast(format!("Merged {branch}"), cx);
                    this.close_race_results(cx);
                });
            })
            .detach();
        });
    }

    /// ⌘⇧R: start a race (or an A/B test) and compare the competing agents' changes.
    pub(super) fn render_race(&mut self, c: &Colors, cx: &mut Context<Self>) -> Option<AnyElement> {
        let overlay = self.state.read(cx).overlay;
        if !matches!(overlay, Overlay::Race | Overlay::AbTest) || self.orchestration.race.is_none() {
            return None;
        }
        let c = *c;
        let ab = overlay == Overlay::AbTest;
        let races = race_panes(self.state.read(cx), true);
        let show_form = races.is_empty() || self.orchestration.race.as_ref().unwrap().show_form;
        let form = show_form.then(|| self.render_start_form(ab, c, cx)).flatten();
        let boards: Vec<Div> = races.into_iter().map(|(race, panes)| self.render_race_board(race, panes, c, cx)).collect();
        let (backdrop, panel) = self.dialog("race", 760.0, c, Self::dismiss_overlay, cx);
        let tab = |label: &'static str, target: Overlay| {
            div().id(label).rounded_sm().px_1p5().py_0p5().text_size(px(11.0)).cursor_pointer()
                .when(overlay == target, |d| d.bg(c.hover).text_color(c.fg))
                .when(overlay != target, |d| d.text_color(c.subtle))
                .on_click(cx.listener(move |this, _, _, cx| this.update_state(cx, |s| s.overlay = target)))
                .child(label)
        };
        let header = div().flex().items_center().gap_2()
            .child(div().text_sm().child(if ab { "A/B test" } else { "Race mode" }))
            .child(div().ml_2().flex_1().flex().gap_0p5().child(tab("Race", Overlay::Race)).child(tab("A/B test", Overlay::AbTest)))
            .when(!show_form, |d| d.child(small_button(c, "race-new", if ab { "New A/B test" } else { "New race" }).on_click(cx.listener(|this, _, _, cx| {
                if let Some(f) = this.orchestration.race.as_mut() {
                    f.show_form = true;
                }
                cx.notify();
            }))));
        Some(backdrop.child(panel.child(header).children(form).children(boards)).into_any_element())
    }

    fn render_results_body(&self, race: &str, panes: Vec<Pane>, c: Colors, cx: &mut Context<Self>) -> Div {
        let s = self.state.read(cx);
        let ab = race.starts_with("ab-");
        let now = now_ms();
        let mono = crate::fonts::mono(cx);
        let usage = |p: &Pane| self.orchestration.usage.get(&p.id);
        let changes = |p: &Pane| self.orchestration.changes.get(&p.id).and_then(|c| c.as_ref().ok());
        let times = |p: &Pane| s.run_times.get(&p.id).copied();
        let active = |p: &Pane| times(p).map(|t: RunTime| t.active_ms(now) as f64);
        // After a restart there are no status timings: the first and last model call stand in.
        let wall = |p: &Pane| -> Option<f64> {
            if let Some(t) = times(p) {
                let end = if t.since.is_some() { now } else { t.finished_at.unwrap_or(now) };
                return Some(end.saturating_sub(t.started_at) as f64);
            }
            let u = usage(p)?;
            Some(u["lastAt"].as_f64()? - u["firstAt"].as_f64()?)
        };
        let field = |p: &Pane, key: &str| usage(p).and_then(|u| u[key].as_f64());
        let sum = |p: &Pane, key: &str| usage(p).map(|u| u["models"].as_array().map_or(0.0, |m| m.iter().filter_map(|x| x[key].as_f64()).sum()));
        type Row = (&'static str, bool, Vec<Option<f64>>, fn(f64) -> String);
        let per = |f: &dyn Fn(&Pane) -> Option<f64>| panes.iter().map(f).collect::<Vec<_>>();
        let cost_per_100 = per(&|p| {
            let (cost, lines) = (field(p, "cost")?, changes(p)?.adds + changes(p)?.untracked_lines);
            (lines > 0).then(|| cost / lines as f64 * 100.0)
        });
        let metrics: Vec<Row> = vec![
            ("Working time", true, per(&active), format_duration),
            ("Run time", true, per(&wall), format_duration),
            ("Cost", true, per(&|p| field(p, "cost")), format_cost),
            ("Tokens", true, per(&|p| field(p, "tokens")), format_tokens),
            ("· Input", true, per(&|p| sum(p, "input")), format_tokens),
            ("· Output", true, per(&|p| sum(p, "output")), format_tokens),
            ("· Cache read", false, per(&|p| sum(p, "cacheRead")), format_tokens),
            ("· Cache write", false, per(&|p| sum(p, "cacheWrite")), format_tokens),
            ("Model calls", true, per(&|p| field(p, "calls")), grouped),
            ("Lines written", false, per(&|p| changes(p).map(|c| (c.adds + c.untracked_lines) as f64)), grouped),
            ("Lines removed", false, per(&|p| changes(p).map(|c| c.dels as f64)), grouped),
            ("Files changed", false, per(&|p| changes(p).map(|c| (c.files + c.untracked) as f64)), grouped),
            ("Commits", false, per(&|p| changes(p).map(|c| c.ahead as f64)), grouped),
            ("Cost per 100 lines", true, cost_per_100, format_cost),
        ];
        let verdict = judge(&panes.iter().map(|p| {
            let ch = changes(p);
            Contender {
                time_ms: active(p).or_else(|| wall(p)),
                cost: field(p, "cost"),
                tokens: field(p, "tokens"),
                changed: ch.map(|c| (c.files + c.untracked + c.adds + c.untracked_lines + c.dels) as f64),
                errored: p.status == AgentStatus::Error,
                finished: s.is_finished(p),
            }
        }).collect::<Vec<_>>());
        let label = |p: &Pane| p.custom_title.clone().unwrap_or_else(|| s.provider_name(&p.provider));
        let models = |p: &Pane| {
            let names: Vec<String> = usage(p).and_then(|u| u["models"].as_array()).map_or(Vec::new(), |m| m.iter().filter_map(|x| x["model"].as_str().map(str::to_string)).collect());
            let mut unique: Vec<String> = Vec::new();
            names.into_iter().for_each(|n| if !unique.contains(&n) { unique.push(n) });
            if unique.is_empty() { p.model.clone().unwrap_or_else(|| "default".into()) } else { unique.join(", ") }
        };
        let mut head = div().flex().border_b_1().border_color(c.border).bg(c.panel).text_xs().child(div().w(px(160.0)).flex_shrink_0().px_3().py_2());
        for (i, p) in panes.iter().enumerate() {
            head = head.child(div().flex_1().min_w_0().px_3().py_2()
                .child(div().flex().items_center().gap_1p5().text_color(c.fg)
                    .child(div().size_1p5().rounded_full().bg(status_color(p.status, &c)))
                    .child(div().truncate().child(label(p)))
                    .when(verdict.winner == Some(i), |d| d.child(div().text_color(c.success).child("★"))))
                .child(div().flex().gap_1().text_size(px(10.0))
                    .when(ab, |d| d.child(div().flex_shrink_0().rounded_sm().px_1().when(p.plain, |d| d.bg(c.hover).text_color(c.muted)).when(!p.plain, |d| d.text_color(c.accent)).child(if p.plain { "plain" } else { "with tools" })))
                    .child(div().truncate().font_family(mono.clone()).text_color(c.subtle).child(models(p)))));
        }
        let mut table = div().flex().flex_col().rounded_lg().border_1().border_color(c.border).overflow_hidden().child(head);
        for (name, lower_better, values, format) in &metrics {
            let known: Vec<f64> = values.iter().flatten().copied().collect();
            let best = (*lower_better && known.len() >= 2 && known.iter().any(|v| *v != known[0])).then(|| known.iter().copied().fold(f64::INFINITY, f64::min));
            let sub = name.starts_with('·');
            let mut row = div().flex().border_t_1().border_color(c.border).text_xs().child(
                div().w(px(160.0)).flex_shrink_0().py_1p5().px_3().when(sub, |d| d.pl_5().text_color(c.subtle)).when(!sub, |d| d.text_color(c.muted)).child(name.trim_start_matches("· ").to_string()),
            );
            let base = values.first().copied().flatten();
            for (i, value) in values.iter().enumerate() {
                let delta = value.zip(base).filter(|(v, b)| i > 0 && *b != 0.0 && v != b).map(|(v, b)| {
                    let pct = (v - b) / b * 100.0;
                    let text = format!("{}{}%", if pct > 0.0 { "+" } else { "−" }, if pct.abs() >= 10.0 { format!("{}", pct.abs().round()) } else { format!("{:.1}", pct.abs()) });
                    (text, if !*lower_better { c.subtle } else if pct < 0.0 { c.success } else { c.danger })
                });
                let winner = value.is_some() && *value == best;
                row = row.child(div().flex_1().min_w_0().flex().items_baseline().gap_1p5().px_3().py_1p5().font_family(mono.clone())
                    .text_color(if winner { c.success } else if value.is_some() { c.fg } else { c.subtle })
                    .child(value.map_or("—".to_string(), |v| format(v)))
                    .when_some(delta, |d, (text, color)| d.child(div().text_size(px(10.0)).text_color(color).child(text))));
            }
            table = table.child(row);
        }
        let mut score = div().flex().border_t_1().border_color(c.border).bg(c.panel).text_xs().child(div().w(px(160.0)).flex_shrink_0().px_3().py_1p5().text_color(c.fg).child("Overall score"));
        for i in 0..panes.len() {
            let (text, color) = match (verdict.out[i], verdict.scores[i]) {
                (Some(out), _) => (out.to_string(), c.danger),
                (_, Some(v)) => (format!("{}", (v * 100.0).round()), if verdict.winner == Some(i) { c.success } else { c.fg }),
                _ => ("—".to_string(), c.subtle),
            };
            score = score.child(div().flex_1().min_w_0().px_3().py_1p5().font_family(mono.clone()).text_color(color).child(text));
        }
        table = table.child(score);
        let winner = verdict.winner.map(|i| panes[i].clone());
        let who = winner.as_ref().map(|p| if ab { if p.plain { "Plain".to_string() } else { "With tools".to_string() } } else { label(p) });
        let can_merge = verdict.finished && winner.as_ref().is_some_and(|p| p.worktree.is_some());
        let field_all = panes.clone();
        let merge_winner = winner.clone();
        let banner = div().flex().items_center().gap_2().rounded_lg().border_1().border_color(if winner.is_some() { Hsla { a: 0.4, ..c.success } } else { c.border }).bg(c.panel).px_3().py_2().text_xs()
            .child(div().flex_1().text_color(c.muted)
                .when_some(who, |d, who| d.child(div().text_color(c.fg).child(format!("{}: {who}", if verdict.finished { "Winner" } else { "Leading" }))))
                .child(format!("{}{}", verdict.reason, if winner.is_some() && !verdict.finished { " Not every agent has finished yet." } else { "" })))
            .when(can_merge, |d| d.child(accent_button(c, "merge-winner", "Merge winner", true).on_click(cx.listener(move |this, _, window, cx| {
                if let Some(w) = merge_winner.clone() {
                    this.pick_race_winner(w, field_all.clone(), window, cx);
                }
            }))));
        let mut summary = Vec::new();
        if ab && let (Some(a), Some(b)) = (panes.iter().position(|p| !p.plain), panes.iter().position(|p| p.plain)) {
            for (name, verb, unit) in [("Cost", "cost", ""), ("Tokens", "used", " tokens"), ("Working time", "took", ""), ("Lines written", "wrote", " lines")] {
                let Some((_, _, values, format)) = metrics.iter().find(|m| m.0 == name) else { continue };
                let (Some(va), Some(vb)) = (values[a], values[b]) else { continue };
                if va == vb {
                    continue;
                }
                let more = va > vb;
                let pct = if vb > 0.0 { format!(" ({}{}% vs. plain)", if more { "+" } else { "−" }, ((va - vb).abs() / vb * 100.0).round()) } else { String::new() };
                summary.push(format!("With tools {verb} {} {}{unit}{pct}", format((va - vb).abs()), if more { "more" } else { "less" }));
            }
        }
        let errors: Vec<String> = panes.iter().filter_map(|p| self.orchestration.changes.get(&p.id).and_then(|c| c.as_ref().err().cloned())).collect();
        let note = format!("Cost is at API prices, read from the agents' usage logs per worktree; it can lag a few seconds behind. Working time is measured while winfer runs.{}", if ab { " The plain run started without winfer's tools, the browser, and your own MCP servers and skills." } else { "" });
        div().flex().flex_col().gap_4()
            .child(div().truncate().text_xs().text_color(c.muted).child(panes[0].prompt.clone().unwrap_or_else(|| if ab { "A/B test".into() } else { "Race".into() })))
            .child(table)
            .child(banner)
            .when(!summary.is_empty(), |d| d.child(div().flex().flex_col().gap_0p5().rounded_lg().border_1().border_color(c.border).bg(c.panel).px_3().py_2().text_xs().text_color(c.muted).children(summary.into_iter().map(|l| format!("• {l}")))))
            .when(!errors.is_empty(), |d| d.child(div().text_size(px(11.0)).text_color(c.danger).child(errors.join(" · "))))
            .child(div().text_size(px(11.0)).text_color(c.subtle).child(note))
    }

    /// Side-by-side numbers for a race or A/B test.
    pub(super) fn render_race_results(&mut self, c: &Colors, cx: &mut Context<Self>) -> Option<AnyElement> {
        let race = self.orchestration.results.clone()?;
        let c = *c;
        let panes: Vec<Pane> = self.state.read(cx).all_panes().filter(|p| p.race_id.as_deref() == Some(race.as_str())).cloned().collect();
        let ab = race.starts_with("ab-");
        let body = if panes.is_empty() { div().text_xs().text_color(c.muted).child("These agents have been closed.") } else { self.render_results_body(&race, panes.clone(), c, cx) };
        let (backdrop, panel) = self.dialog("race-results", 860.0, c, Self::close_race_results, cx);
        let refresh = small_button(c, "results-refresh", "Refresh").on_click(cx.listener(move |this, _, _, cx| this.load_race_numbers(panes.clone(), true, cx)));
        Some(backdrop.child(panel.child(div().flex().items_center().gap_2().child(div().flex_1().text_sm().child(if ab { "A/B test results" } else { "Race results" })).child(refresh)).child(body)).into_any_element())
    }
}

#[cfg(test)]
mod tests {
    use super::{Contender, format_cost, format_duration, format_tokens, judge};

    fn c(time: f64, cost: f64, tokens: f64) -> Contender {
        Contender { time_ms: Some(time), cost: Some(cost), tokens: Some(tokens), changed: Some(10.0), errored: false, finished: true }
    }

    #[test]
    fn the_cheapest_fastest_agent_wins() {
        let v = judge(&[c(100.0, 1.0, 100.0), c(200.0, 2.0, 200.0)]);
        assert_eq!(v.winner, Some(0));
        assert_eq!(v.scores[0], Some(1.0));
        assert!(v.reason.starts_with("Best overall; best on cost, time, tokens"));
        assert!(v.finished);
    }

    #[test]
    fn near_ties_have_no_winner() {
        let v = judge(&[c(100.0, 1.0, 100.0), c(101.0, 1.0, 100.0)]);
        assert_eq!(v.winner, None);
        assert!(v.reason.starts_with("Too close"));
    }

    #[test]
    fn errors_and_idle_agents_are_out() {
        let errored = Contender { errored: true, ..c(1.0, 0.1, 1.0) };
        let idle = Contender { changed: Some(0.0), ..c(1.0, 0.1, 1.0) };
        let v = judge(&[errored, idle, c(500.0, 5.0, 500.0)]);
        assert_eq!(v.out, vec![Some("errored"), Some("changed nothing"), None]);
        assert_eq!(v.winner, Some(2));
        assert!(v.reason.contains("changed code"));
    }

    #[test]
    fn unknown_numbers_leave_no_verdict() {
        let unknown = Contender { finished: false, ..Default::default() };
        let v = judge(&[unknown, unknown]);
        assert_eq!(v.winner, None);
        assert!(!v.finished);
    }

    #[test]
    fn formats_match_the_tauri_app() {
        assert_eq!(format_cost(0.0), "$0");
        assert_eq!(format_cost(0.001), "<$0.01");
        assert_eq!(format_cost(12.5), "$12.50");
        assert_eq!(format_cost(1234.4), "$1,234");
        assert_eq!(format_tokens(950.0), "950");
        assert_eq!(format_tokens(1500.0), "1.5K");
        assert_eq!(format_tokens(25_000.0), "25K");
        assert_eq!(format_tokens(2_500_000.0), "2.50M");
        assert_eq!(format_duration(45_000.0), "45s");
        assert_eq!(format_duration(200_000.0), "3m 20s");
        assert_eq!(format_duration(3_900_000.0), "1h 05m");
    }
}
