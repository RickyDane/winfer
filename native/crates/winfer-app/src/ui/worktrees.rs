//! Worktrees (port of `features/worktree/BranchDialog.tsx`, `WorktreeList.tsx` and the
//! worktree parts of `sidebarMenus.ts`): a new worktree from a branch, and the sidebar's list of
//! the project folder and the worktrees no pane uses.

use super::commands::Command;
use super::dialogs::relative_time;
use super::menu::MenuItem;
use super::orchestration::accent_button;
use super::review::small_button;
use super::*;
use crate::model::{Overlay, Workspace, WorktreeInfo};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::tooltip::Tooltip;
use serde_json::{Value, json};
use std::collections::HashSet;
use winfer_backend::worktree;

/// Worktrees with no activity for this long count as stale.
pub const STALE_DAYS: u64 = 10;
const STALE_MS: f64 = STALE_DAYS as f64 * 86_400_000.0;
/// How often the active workspace's worktrees are re-listed; other expanded workspaces' less often.
const REFRESH_SECS: u64 = 20;
const IDLE_REFRESH_SECS: u64 = 120;

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// Comparable form of a path: forward slashes, no trailing slash, Windows paths case-folded.
fn norm_path(p: &str) -> String {
    let s = p.replace('\\', "/").trim_end_matches('/').to_string();
    let b = s.as_bytes();
    if b.len() > 2 && b[1] == b':' && b[2] == b'/' { s.to_lowercase() } else { s }
}

/// Last activity: the branch's last commit, else when the folder was made.
pub fn last_activity(wt: &Value) -> Option<f64> {
    wt["lastCommitAt"].as_f64().or(wt["createdAt"].as_f64())
}

/// No commit (or, without one, folder creation) for `STALE_DAYS` days. Never the project folder.
pub fn is_stale(wt: &Value, now: f64) -> bool {
    wt["isMain"] != true && last_activity(wt).is_some_and(|at| now - at > STALE_MS)
}

/// Whether a pane already works in `wt` (by path, or by branch in the same repo).
pub fn in_use(wt: &Value, panes: &[&Pane], workspace: &str) -> bool {
    let path = norm_path(wt["info"]["path"].as_str().unwrap_or_default());
    let repo = norm_path(wt["info"]["repo"].as_str().unwrap_or_default());
    let branch = wt["info"]["branch"].as_str().unwrap_or_default();
    let inside = |p: &str| {
        let n = norm_path(p);
        n == path || n.starts_with(&format!("{path}/"))
    };
    panes.iter().any(|p| match &p.worktree {
        Some(t) => inside(&t.path) || (t.branch == branch && (p.workspace_id == workspace || norm_path(&t.repo) == repo)),
        None => inside(&p.cwd),
    })
}

/// The confirmation for removing `list`: what goes, and what would be lost.
pub fn bulk_warning(list: &[Value]) -> String {
    let own = list.iter().filter(|w| w["byWinfer"] == true).count();
    let dirty = list.iter().filter(|w| w["dirty"] == true).count();
    let unmerged = list.iter().filter(|w| w["byWinfer"] == true && w["ahead"].as_u64().unwrap_or(0) > 0).count();
    let mut lines = vec![
        format!("Delete {}{}?", plural(list.len(), "worktree", "worktrees"), if own > 0 { format!(", and the branches of the {own} winfer made") } else { String::new() }),
        String::new(),
    ];
    lines.extend(list.iter().take(12).map(|w| format!("• {}", w["info"]["branch"].as_str().unwrap_or_default())));
    if list.len() > 12 {
        lines.push(format!("• …and {} more", list.len() - 12));
    }
    let mut lost = Vec::new();
    if dirty > 0 {
        lost.push(format!("{} with uncommitted changes", plural(dirty, "worktree", "worktrees")));
    }
    if unmerged > 0 {
        lost.push(format!("{} with unmerged commits", plural(unmerged, "branch", "branches")));
    }
    if !lost.is_empty() {
        lines.push(String::new());
        lines.push(format!("Lost for good: {}.", lost.join(" and ")));
    }
    if own < list.len() {
        lines.push(String::new());
        lines.push("Branches not made by winfer are kept.".into());
    }
    lines.join("\n")
}

fn describe_state(wt: &Value) -> String {
    let mut parts = Vec::new();
    let ahead = wt["ahead"].as_u64().unwrap_or(0);
    if wt["merged"] == true {
        parts.push("Nothing unmerged — safe to remove".to_string());
    }
    if ahead > 0 {
        parts.push(format!("{ahead} commit{} not on {}", if ahead == 1 { "" } else { "s" }, wt["info"]["base"].as_str().unwrap_or_default()));
    }
    if wt["dirty"] == true {
        parts.push("uncommitted changes".into());
    }
    if let Some(at) = wt["lastCommitAt"].as_f64() {
        parts.push(format!("last commit {}", relative_time(at)));
    }
    if let Some(at) = wt["createdAt"].as_f64() {
        parts.push(format!("created {}", relative_time(at)));
    }
    parts.join(" · ")
}

fn info_of(wt: &Value) -> Option<WorktreeInfo> {
    serde_json::from_value(wt["info"].clone()).ok()
}

fn now_ms() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_millis() as f64)
}

pub(super) struct BranchDialog {
    search: Entity<InputState>,
    new_branch: Entity<InputState>,
    branches: Option<Vec<Value>>,
    selected: Option<Value>,
    /// Starting a new branch from the selected one.
    making_branch: bool,
    busy: Option<&'static str>,
    error: Option<String>,
}

#[derive(Default)]
pub(super) struct WorktreesUi {
    pub dialog: Option<BranchDialog>,
    /// Kept between openings: whoever pulls first usually always does.
    latest: bool,
    /// Listing per workspace; `None` until the first one comes back.
    found: HashMap<String, Option<Vec<Value>>>,
    busy: HashSet<String>,
    removing: HashSet<String>,
    /// Workspace picking worktrees to remove together, and the paths picked.
    selecting: Option<(String, HashSet<String>)>,
    /// Each workspace's panes in worktrees, to re-list when one closes.
    owned: HashMap<String, usize>,
    seconds: u64,
}

impl Root {
    fn list_worktrees(&mut self, ws: &Workspace, cx: &mut Context<Self>) {
        if !self.worktrees.busy.insert(ws.id.clone()) {
            return;
        }
        let (id, dir) = (ws.id.clone(), ws.root_dir.clone());
        cx.spawn(async move |this, cx| {
            let found = tauri::async_runtime::spawn(worktree::worktree_list(dir)).await;
            let list = found.ok().and_then(Result::ok).and_then(|l| serde_json::to_value(l).ok()).and_then(|v| v.as_array().cloned()).unwrap_or_default();
            let _ = this.update(cx, |this, cx| {
                this.worktrees.busy.remove(&id);
                this.worktrees.found.insert(id, Some(list));
                cx.notify();
            });
        })
        .detach();
    }

    /// Every second: re-lists expanded workspaces' worktrees now and then, and after agents come and go.
    pub(super) fn tick_worktrees(&mut self, cx: &mut Context<Self>) {
        self.worktrees.seconds += 1;
        let t = self.worktrees.seconds;
        let s = self.state.read(cx);
        let due: Vec<Workspace> = s.workspaces.iter().filter(|w| s.expanded_workspaces.contains(&w.id)).filter(|w| {
            let owned = w.panes.iter().filter(|p| p.worktree.is_some()).count();
            let changed = self.worktrees.owned.get(&w.id).is_some_and(|n| *n != owned);
            let every = if w.id == s.active_workspace_id { REFRESH_SECS } else { IDLE_REFRESH_SECS };
            !self.worktrees.found.contains_key(&w.id) || changed || t % every == 0
        }).cloned().collect();
        for w in s.workspaces.iter() {
            self.worktrees.owned.insert(w.id.clone(), w.panes.iter().filter(|p| p.worktree.is_some()).count());
        }
        for w in due {
            self.list_worktrees(&w, cx);
        }
    }

    /// Opens `provider` (or a shell) in an existing worktree, resuming the agent's last session there unless `fresh`.
    fn open_in_worktree(&mut self, ws: &Workspace, wt: &Value, provider: &str, fresh: bool, cx: &mut Context<Self>) {
        let Some(info) = info_of(wt) else { return };
        let main = wt["isMain"] == true;
        let resume = if fresh { None } else { self.worktree_session(ws, wt, provider, cx) };
        let id = ws.id.clone();
        let provider = provider.to_string();
        let branch = info.branch.clone();
        self.update_state(cx, |s| {
            if s.active_workspace_id != id {
                s.set_active_workspace(&id);
            }
            // In the project folder itself the agent works directly on its branch, without a worktree.
            let opts = if main {
                AddPane { workspace_id: Some(id.clone()), worktree: Some(false), cwd: Some(info.path.clone()), resume_session: resume, ..Default::default() }
            } else {
                AddPane { workspace_id: Some(id.clone()), existing_worktree: Some(info), resume_session: resume, ..Default::default() }
            };
            s.add_pane(&provider, opts);
            s.log_activity(&id, format!("Opened worktree {branch}"), None);
        });
    }

    /// The agent's last session in a worktree no open pane runs, if the agent can resume it.
    fn worktree_session(&self, ws: &Workspace, wt: &Value, provider: &str, cx: &App) -> Option<String> {
        let s = self.state.read(cx);
        if !s.providers.iter().any(|p| p.id == provider && !p.resume_session_args.is_empty()) {
            return None;
        }
        let path = wt["info"]["path"].as_str().unwrap_or_default();
        let mut paths = vec![path.to_string()];
        if wt["isMain"] == true {
            paths.push(ws.root_dir.clone());
        }
        paths.iter().find_map(|p| s.recent_session(provider, p))
    }

    fn worktree_menu(&self, ws: &Workspace, wt: &Value, cx: &App) -> Vec<MenuItem> {
        let s = self.state.read(cx);
        let missing = wt["prunable"] == true;
        let agents: Vec<(String, String)> = s.providers.iter().filter(|p| p.installed && p.id != "shell").map(|p| (p.id.clone(), p.name.clone())).collect();
        let mut items = Vec::new();
        for (id, name) in agents {
            let open = |fresh: bool| {
                let (ws, wt, id) = (ws.clone(), wt.clone(), id.clone());
                move |this: &mut Root, _: &mut Window, cx: &mut Context<Root>| this.open_in_worktree(&ws, &wt, &id, fresh, cx)
            };
            if self.worktree_session(ws, wt, &id, cx).is_some() {
                items.push(MenuItem::run(format!("Resume {name} session here"), open(false)).disabled(missing));
                items.push(MenuItem::run(format!("New {name} session here"), open(true)).disabled(missing));
            } else {
                items.push(MenuItem::run(format!("Open {name} here"), open(false)).disabled(missing));
            }
        }
        let (w1, t1) = (ws.clone(), wt.clone());
        items.push(MenuItem::run("Open a shell here", move |this, _, cx| this.open_in_worktree(&w1, &t1, "shell", false, cx)).disabled(missing));
        items.push(MenuItem::Separator);
        let path = wt["info"]["path"].as_str().unwrap_or_default().to_string();
        let branch = wt["info"]["branch"].as_str().unwrap_or_default().to_string();
        let (reveal, copy_path, edit) = (path.clone(), path.clone(), path);
        items.push(MenuItem::run("Open in code editor", move |this, _, cx| {
            let editor = this.state.read(cx).editor.clone();
            this.open_in_editor(edit.clone(), editor, cx);
        }).disabled(missing));
        items.push(MenuItem::run(if cfg!(target_os = "macos") { "Reveal in Finder" } else { "Show in Explorer" }, move |_, _, _| sidebar::reveal_path(&reveal)).disabled(missing));
        items.push(MenuItem::run("Copy worktree path", move |this, _, cx| this.copy_with_toast(copy_path.clone(), cx)));
        items.push(MenuItem::run("Copy branch name", move |this, _, cx| this.copy_with_toast(branch.clone(), cx)));
        items.push(MenuItem::Separator);
        let switch = ws.id.clone();
        items.push(MenuItem::commands("New worktree from a branch…", vec![Command::WorkspaceSwitch(switch), Command::Overlay { overlay: Overlay::Branch, provider: None }]));
        if wt["isMain"] != true {
            let (w2, t2) = (ws.clone(), wt.clone());
            items.push(MenuItem::Separator);
            items.push(MenuItem::run(if missing { "Forget missing worktree…" } else { "Remove worktree…" }, move |this, window, cx| this.remove_worktree(&w2, &t2, window, cx)).danger());
        }
        items
    }

    fn remove_worktree(&mut self, ws: &Workspace, wt: &Value, window: &mut Window, cx: &mut Context<Self>) {
        let Some(info) = info_of(wt) else { return };
        let mine = wt["byWinfer"] == true;
        let detail = if wt["prunable"] == true {
            format!("The folder of {} is gone. Remove it from git's worktree list?", info.branch)
        } else {
            format!(
                "Delete the worktree at {}{}? Uncommitted changes in it are lost{}.",
                info.path,
                if mine { format!(" and its branch {}", info.branch) } else { String::new() },
                if mine { ", and so are commits that weren't merged" } else { "" }
            )
        };
        let workspace = ws.clone();
        self.confirm("Remove worktree", detail, "Remove", window, cx, move |this, cx| {
            let path = info.path.clone();
            let branch = info.branch.clone();
            this.worktrees.removing.insert(path.clone());
            this.toast(format!("Removing {branch}…"), cx);
            cx.spawn(async move |this, cx| {
                let result = tauri::async_runtime::spawn(async move {
                    let info = commands::worktree_info(&info)?;
                    worktree::worktree_remove(info, mine).await
                })
                .await
                .map_err(|e| e.to_string())
                .and_then(|r| r);
                let _ = this.update(cx, |this, cx| {
                    this.worktrees.removing.remove(&path);
                    match result {
                        Ok(()) => {
                            this.toast(format!("Removed {branch}"), cx);
                            this.update_state(cx, |s| s.log_activity(&workspace.id, format!("Removed worktree {branch}"), None));
                            if let Some(Some(list)) = this.worktrees.found.get_mut(&workspace.id) {
                                list.retain(|w| w["info"]["path"].as_str() != Some(path.as_str()));
                            }
                        }
                        Err(e) => this.toast(format!("Couldn't remove worktree: {e}"), cx),
                    }
                    this.list_worktrees(&workspace, cx);
                });
            })
            .detach();
        });
    }

    /// Removes `list` together (winfer's own branches too), after one confirmation.
    fn remove_many(&mut self, ws: &Workspace, list: Vec<Value>, window: &mut Window, cx: &mut Context<Self>) {
        if list.is_empty() {
            return;
        }
        let title = format!("Remove {}", plural(list.len(), "worktree", "worktrees"));
        let workspace = ws.clone();
        self.confirm(&title, bulk_warning(&list), "Remove", window, cx, move |this, cx| {
            let paths: Vec<String> = list.iter().filter_map(|w| w["info"]["path"].as_str().map(str::to_string)).collect();
            this.worktrees.removing.extend(paths.iter().cloned());
            this.toast(format!("Removing {}…", plural(list.len(), "worktree", "worktrees")), cx);
            let items: Vec<Value> = list.iter().map(|w| json!({ "info": w["info"], "deleteBranch": w["byWinfer"] == true })).collect();
            let total = list.len();
            cx.spawn(async move |this, cx| {
                let result = tauri::async_runtime::spawn(async move {
                    let items = serde_json::from_value(Value::Array(items)).map_err(|e| e.to_string())?;
                    worktree::worktree_remove_many(items).await.and_then(|f| serde_json::to_value(f).map_err(|e| e.to_string()))
                })
                .await
                .map_err(|e| e.to_string())
                .and_then(|r| r);
                let failed: Vec<Value> = match result {
                    Ok(v) => v.as_array().cloned().unwrap_or_default(),
                    Err(e) => list.iter().map(|w| json!({ "path": w["info"]["path"], "branch": w["info"]["branch"], "error": e })).collect(),
                };
                let _ = this.update(cx, |this, cx| {
                    let done = total - failed.len();
                    if failed.is_empty() {
                        this.toast(format!("Removed {}", plural(done, "worktree", "worktrees")), cx);
                    } else {
                        let detail = failed.iter().map(|f| format!("{}: {}", f["branch"].as_str().unwrap_or_default(), f["error"].as_str().unwrap_or_default())).collect::<Vec<_>>().join("\n");
                        let head = if done > 0 { format!("Removed {done} of {total} worktrees") } else { "Couldn't remove worktrees".into() };
                        this.toast(format!("{head}\n{detail}"), cx);
                    }
                    if done > 0 {
                        this.update_state(cx, |s| s.log_activity(&workspace.id, format!("Removed {}", plural(done, "worktree", "worktrees")), None));
                    }
                    for p in &paths {
                        this.worktrees.removing.remove(p);
                    }
                    this.worktrees.selecting = None;
                    this.list_worktrees(&workspace, cx);
                });
            })
            .detach();
        });
    }

    /// The sidebar's "Branches" list under an expanded workspace.
    pub(super) fn render_worktree_list(&self, ws: &Workspace, c: Colors, cx: &mut Context<Self>) -> Option<AnyElement> {
        let found = self.worktrees.found.get(&ws.id).cloned();
        let loading = !matches!(found, Some(Some(_)));
        let list = found.flatten().unwrap_or_default();
        let s = self.state.read(cx);
        let panes: Vec<&Pane> = s.all_panes().collect();
        let now = now_ms();
        let main = list.iter().find(|w| w["isMain"] == true).cloned();
        let mut free: Vec<Value> = list.iter().filter(|w| w["isMain"] != true && !in_use(w, &panes, &ws.id)).cloned().collect();
        // Newest activity first; merged ones sink to the bottom, ready to clean up.
        free.sort_by(|a, b| (a["merged"] == true).cmp(&(b["merged"] == true)).then(last_activity(b).unwrap_or(0.0).total_cmp(&last_activity(a).unwrap_or(0.0))));
        let rows: Vec<Value> = main.into_iter().chain(free.iter().cloned()).collect();
        if !loading && rows.is_empty() {
            return None;
        }
        let selecting = self.worktrees.selecting.as_ref().filter(|(w, _)| *w == ws.id).map(|(_, set)| set.clone());
        let stale: Vec<Value> = free.iter().filter(|w| is_stale(w, now)).cloned().collect();
        let merged: Vec<Value> = free.iter().filter(|w| w["merged"] == true && w["prunable"] != true).cloned().collect();
        let missing: Vec<Value> = free.iter().filter(|w| w["prunable"] == true).cloned().collect();
        let small = move |id: SharedString, label: String| div().id(id).rounded_sm().px_1().py_0p5().text_size(px(10.0)).text_color(c.subtle).cursor_pointer().hover(move |d| d.bg(c.hover).text_color(c.fg)).child(label);
        let wsid = ws.id.clone();
        let header = match &selecting {
            Some(picked) => {
                let all = picked.len() == free.len();
                let (f1, st, w1, w2, w3) = (free.clone(), stale.clone(), ws.clone(), wsid.clone(), wsid.clone());
                let chosen: Vec<Value> = free.iter().filter(|w| w["info"]["path"].as_str().is_some_and(|p| picked.contains(p))).cloned().collect();
                div().flex().items_center().gap_0p5().pt_1().pb_0p5().pr_1()
                    .child(div().flex_1().truncate().text_size(px(10.0)).text_color(c.subtle).child(format!("{} selected", picked.len())))
                    .child(small(format!("wt-all-{wsid}").into(), if all { "None".into() } else { "All".into() }).on_click(cx.listener(move |this, _, _, cx| {
                        if let Some((_, set)) = this.worktrees.selecting.as_mut() {
                            *set = if all { HashSet::new() } else { f1.iter().filter_map(|w| w["info"]["path"].as_str().map(str::to_string)).collect() };
                        }
                        cx.notify();
                    })))
                    .child(small(format!("wt-stale-{wsid}").into(), format!(">{STALE_DAYS}d")).when(stale.is_empty(), |d| d.opacity(0.4)).on_click(cx.listener(move |this, _, _, cx| {
                        if let Some((_, set)) = this.worktrees.selecting.as_mut() {
                            *set = st.iter().filter_map(|w| w["info"]["path"].as_str().map(str::to_string)).collect();
                        }
                        cx.notify();
                    })))
                    .child(small(format!("wt-remove-{w2}").into(), "Remove".into()).text_color(c.danger).when(chosen.is_empty(), |d| d.opacity(0.4)).on_click(cx.listener(move |this, _, window, cx| this.remove_many(&w1, chosen.clone(), window, cx))))
                    .child(div().id(SharedString::from(format!("wt-stop-{w3}"))).rounded_sm().p_0p5().text_color(c.subtle).cursor_pointer().hover(move |d| d.bg(c.hover).text_color(c.fg))
                        .tooltip(|window, cx| Tooltip::new("Stop selecting (Esc)").build(window, cx))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.worktrees.selecting = None;
                            cx.notify();
                        }))
                        .child(Icon::new(IconName::X).size(px(12.0))))
            }
            None => {
                let (w1, w2) = (ws.clone(), ws.id.clone());
                let (st, mg, ms, fr) = (stale.clone(), merged.clone(), missing.clone(), free.clone());
                div().flex().items_center().pt_1().pb_0p5().pr_1()
                    .child(div().flex_1().flex().items_center().gap_1p5().text_size(px(10.0)).text_color(c.subtle).child("BRANCHES").when(loading, |d| d.child("…")))
                    .when(!free.is_empty(), |d| d.child(div().id(SharedString::from(format!("wt-clean-{wsid}"))).rounded_sm().p_0p5().text_color(c.subtle).cursor_pointer().hover(move |d| d.bg(c.hover).text_color(c.fg))
                        .tooltip(|window, cx| Tooltip::new("Clean up worktrees…").build(window, cx))
                        .on_click(cx.listener(move |this, e: &ClickEvent, _, cx| {
                            let pick = |list: Vec<Value>, ws: String| move |this: &mut Root, _: &mut Window, cx: &mut Context<Root>| {
                                this.worktrees.selecting = Some((ws.clone(), list.iter().filter_map(|w| w["info"]["path"].as_str().map(str::to_string)).collect()));
                                cx.notify();
                            };
                            let remove = |list: Vec<Value>, ws: Workspace| move |this: &mut Root, window: &mut Window, cx: &mut Context<Root>| this.remove_many(&ws, list.clone(), window, cx);
                            let mut items = vec![
                                MenuItem::run("Select worktrees to remove…", pick(Vec::new(), w2.clone())).disabled(fr.is_empty()),
                                MenuItem::run(format!("Select older than {STALE_DAYS} days…"), pick(st.clone(), w2.clone())).disabled(st.is_empty()),
                                MenuItem::Separator,
                                MenuItem::run(format!("Remove older than {STALE_DAYS} days ({})…", st.len()), remove(st.clone(), w1.clone())).disabled(st.is_empty()).danger(),
                                MenuItem::run(format!("Remove merged ({})…", mg.len()), remove(mg.clone(), w1.clone())).disabled(mg.is_empty()).danger(),
                            ];
                            if !ms.is_empty() {
                                items.push(MenuItem::run(format!("Forget missing folders ({})…", ms.len()), remove(ms.clone(), w1.clone())).danger());
                            }
                            this.open_menu(e.position(), Some("Clean up worktrees".into()), items, cx);
                        }))
                        .child(Icon::new(IconName::ListChecks).size(px(12.0)))))
                    .child(div().id(SharedString::from(format!("wt-new-{wsid}"))).rounded_sm().p_0p5().text_color(c.subtle).cursor_pointer().hover(move |d| d.bg(c.hover).text_color(c.fg))
                        .tooltip(|window, cx| Tooltip::new("New worktree from a branch…").build(window, cx))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.execute(Command::WorkspaceSwitch(wsid.clone()), window, cx);
                            this.execute(Command::Overlay { overlay: Overlay::Branch, provider: None }, window, cx);
                        }))
                        .child(Icon::new(IconName::Plus).size(px(12.0))))
            }
        };
        let mono = crate::fonts::mono(cx);
        let default_provider = s.default_provider_id();
        let mut out = div().mt_1().pt_1p5().pb_0p5().px_1().child(header);
        for (i, wt) in rows.iter().enumerate() {
            let path = wt["info"]["path"].as_str().unwrap_or_default().to_string();
            let branch = wt["info"]["branch"].as_str().unwrap_or_default().to_string();
            let is_main = wt["isMain"] == true;
            let prunable = wt["prunable"] == true;
            let removing = self.worktrees.removing.contains(&path);
            let picked = selecting.as_ref().map(|set| set.contains(&path));
            let tip = if is_main {
                format!("{path}\nThe project folder itself, on {branch}\nClick to open an agent here (no worktree)")
            } else if prunable {
                format!("Branch: {branch}\nWorktree: {path} (folder missing)")
            } else {
                format!("Branch: {branch}\nWorktree: {path}\nBase branch: {}\n{}\n{}", wt["info"]["base"].as_str().unwrap_or_default(), describe_state(wt), if picked.is_some() { "Click to select" } else { "Click to open an agent here" })
            };
            let mut state = div().flex().flex_wrap().items_center().gap_x_1p5().text_size(px(10.0)).text_color(c.subtle);
            if prunable {
                state = state.child("folder missing");
            } else {
                if wt["merged"] == true {
                    state = state.child(div().text_color(c.success).child("✓ merged"));
                }
                if let Some(at) = last_activity(wt) {
                    let age = relative_time(at);
                    state = state.child(if age == "now" { "just now".to_string() } else { format!("{age} ago") });
                }
                let ahead = wt["ahead"].as_u64().unwrap_or(0);
                if ahead > 0 {
                    state = state.child(format!("{ahead} ahead"));
                }
                if wt["dirty"] == true {
                    state = state.child(div().text_color(c.warning).child("uncommitted"));
                }
            }
            let (w_open, t_open, w_menu, t_menu, w_rm, t_rm, provider, pick_path) = (ws.clone(), wt.clone(), ws.clone(), wt.clone(), ws.clone(), wt.clone(), default_provider.clone(), path.clone());
            let icon = match picked {
                Some(on) if !is_main => div().mt_1().size_3().flex_shrink_0().rounded_sm().border_1().border_color(c.edge).when(on, |d| d.bg(c.accent)),
                _ => div().mt_1().flex_shrink_0().child(Icon::new(if is_main { IconName::FolderGit2 } else { IconName::GitBranch }).size(px(12.0)).text_color(if is_main { c.accent } else { c.subtle })),
            };
            out = out.child(
                div().id(SharedString::from(format!("wt-{}-{i}", ws.id))).flex().items_center().gap_1().rounded_md().px_1().hover(move |d| d.bg(c.hover))
                    .when(removing || (picked.is_some() && is_main), |d| d.opacity(0.5))
                    .on_mouse_down(MouseButton::Right, cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                        if this.worktrees.selecting.is_some() {
                            return;
                        }
                        let items = this.worktree_menu(&w_menu, &t_menu, cx);
                        let title = t_menu["info"]["path"].as_str().map(str::to_string);
                        this.open_menu(e.position, title, items, cx);
                    }))
                    .child(div().id(SharedString::from(format!("wt-row-{}-{i}", ws.id))).flex().min_w_0().flex_1().items_start().gap_2().py_1().pr_1().text_size(px(13.0)).text_color(c.muted).cursor_pointer()
                        .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some((_, set)) = this.worktrees.selecting.as_mut() {
                                if !set.remove(&pick_path) {
                                    set.insert(pick_path.clone());
                                }
                                cx.notify();
                            } else if !prunable {
                                this.open_in_worktree(&w_open, &t_open, &provider, false, cx);
                            }
                        }))
                        .child(icon)
                        .child(div().min_w_0().flex_1()
                            .child(div().flex().items_center().gap_1p5()
                                .child(div().truncate().font_family(mono.clone()).text_size(px(12.0)).when(prunable, |d| d.line_through()).child(branch.trim_start_matches("winfer/").to_string()))
                                .when(is_main, |d| d.child(div().flex_shrink_0().rounded_sm().bg(Hsla { a: 0.15, ..c.accent }).px_1().text_size(px(9.0)).text_color(c.accent).child("PROJECT"))))
                            .child(state)))
                    .when(!is_main && picked.is_none() && !removing, |d| d.child(div().id(SharedString::from(format!("wt-del-{}-{i}", ws.id))).rounded_sm().p_0p5().text_color(c.subtle).cursor_pointer().hover(move |d| d.text_color(c.danger))
                        .tooltip(|window, cx| Tooltip::new("Remove this worktree").build(window, cx))
                        .on_click(cx.listener(move |this, _, window, cx| this.remove_worktree(&w_rm, &t_rm, window, cx)))
                        .child(Icon::new(IconName::Trash).size(px(11.0))))),
            );
        }
        Some(out.into_any_element())
    }

    // Branch dialog.

    pub(super) fn sync_branch_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let open = self.state.read(cx).overlay == Overlay::Branch;
        if !open {
            if self.worktrees.dialog.take().is_some() {
                self.focused_view = None;
            }
            return;
        }
        if self.worktrees.dialog.is_some() {
            return;
        }
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Find a branch…"));
        let new_branch = cx.new(|cx| InputState::new(window, cx).placeholder("feature/my-idea"));
        cx.subscribe(&search, |_, _, _: &InputEvent, cx| cx.notify()).detach();
        cx.subscribe(&new_branch, |this, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Focus)
                && let Some(d) = this.worktrees.dialog.as_mut()
            {
                d.making_branch = true;
            }
            cx.notify();
        })
        .detach();
        search.update(cx, |i, cx| i.focus(window, cx));
        self.worktrees.dialog = Some(BranchDialog { search, new_branch, branches: None, selected: None, making_branch: false, busy: None, error: None });
        self.load_branches(cx);
    }

    fn load_branches(&mut self, cx: &mut Context<Self>) {
        let Some(dir) = self.state.read(cx).active().map(|w| w.root_dir.clone()) else { return };
        cx.spawn(async move |this, cx| {
            let found = tauri::async_runtime::spawn(worktree::worktree_branches(dir)).await.map_err(|e| e.to_string()).and_then(|r| r);
            let _ = this.update(cx, |this, cx| {
                if let Some(d) = this.worktrees.dialog.as_mut() {
                    match found {
                        Ok(list) => d.branches = serde_json::to_value(list).ok().and_then(|v| v.as_array().cloned()),
                        Err(e) => {
                            d.branches = Some(Vec::new());
                            d.error = Some(e);
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn fetch_branches(&mut self, cx: &mut Context<Self>) {
        let Some(dir) = self.state.read(cx).active().map(|w| w.root_dir.clone()) else { return };
        let Some(d) = self.worktrees.dialog.as_mut() else { return };
        d.busy = Some("fetch");
        d.error = None;
        cx.spawn(async move |this, cx| {
            let result = tauri::async_runtime::spawn(worktree::worktree_fetch(dir)).await.map_err(|e| e.to_string()).and_then(|r| r);
            let _ = this.update(cx, |this, cx| {
                if let Some(d) = this.worktrees.dialog.as_mut() {
                    d.busy = None;
                    if let Err(e) = result {
                        d.error = Some(format!("Fetch failed: {e}"));
                    }
                }
                this.load_branches(cx);
            });
        })
        .detach();
    }

    fn create_from_branch(&mut self, as_workspace: bool, cx: &mut Context<Self>) {
        let Some(ws) = self.state.read(cx).active().cloned() else { return };
        let latest = self.worktrees.latest;
        let Some(d) = self.worktrees.dialog.as_mut() else { return };
        let Some(selected) = d.selected.clone() else { return };
        let new_branch = d.making_branch.then(|| d.new_branch.read(cx).value().trim().to_string()).filter(|b| !b.is_empty());
        let pull = latest && selected["upstream"].is_string();
        d.busy = Some(if as_workspace { "workspace" } else { "agent" });
        d.error = None;
        let name = selected["name"].as_str().unwrap_or_default().to_string();
        cx.spawn(async move |this, cx| {
            let dir = ws.root_dir.clone();
            let result = tauri::async_runtime::spawn(async move {
                let info = worktree::worktree_from_branch(backend::app().clone(), dir, name, new_branch, Some(pull)).await?;
                serde_json::to_value(info).map_err(|e| e.to_string())
            })
            .await
            .map_err(|e| e.to_string())
            .and_then(|r| r);
            let _ = this.update(cx, |this, cx| {
                let info: Option<WorktreeInfo> = result.as_ref().ok().and_then(|v| serde_json::from_value(v.clone()).ok());
                match (info, result) {
                    (Some(info), _) => {
                        this.update_state(cx, |s| {
                            if as_workspace {
                                // Its own workspace, rooted in the worktree: agents there work on this branch directly.
                                let id = s.create_workspace(&info.branch, &info.path);
                                if let Some(w) = s.workspaces.iter_mut().find(|w| w.id == id) {
                                    w.use_worktrees = false;
                                }
                                s.log_activity(&id, format!("Workspace for branch {}", info.branch), None);
                            } else {
                                let provider = s.default_provider_id();
                                s.add_pane(&provider, AddPane { workspace_id: Some(ws.id.clone()), existing_worktree: Some(info.clone()), ..Default::default() });
                                s.log_activity(&ws.id, format!("Worktree for {}", info.branch), None);
                            }
                            s.overlay = Overlay::None;
                        });
                    }
                    (None, result) => {
                        if let Some(d) = this.worktrees.dialog.as_mut() {
                            d.busy = None;
                            d.error = Some(result.err().unwrap_or_else(|| "The worktree couldn't be read".into()));
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// New worktree from a branch: pick a branch (or start a new one from it), then open an agent
    /// in it or make it a workspace of its own.
    pub(super) fn render_branch_dialog(&mut self, c: &Colors, cx: &mut Context<Self>) -> Option<AnyElement> {
        let d = self.worktrees.dialog.as_ref()?;
        let c = *c;
        let s = self.state.read(cx);
        let ws_name = s.active().map(|w| w.name.clone()).unwrap_or_default();
        let agent = s.provider_name(&s.default_provider_id());
        let query = d.search.read(cx).value().to_lowercase();
        let words: Vec<&str> = query.split_whitespace().collect();
        let mono = crate::fonts::mono(cx);
        let selected_name = d.selected.as_ref().and_then(|b| b["name"].as_str()).map(str::to_string);
        let mut list = div().id("branch-list").flex_1().min_h(px(128.0)).overflow_y_scroll().p_2().flex().flex_col();
        match &d.branches {
            None => list = list.child(div().py_8().text_center().text_xs().text_color(c.subtle).child("Reading branches…")),
            Some(all) => {
                let shown: Vec<&Value> = all.iter().filter(|b| words.iter().all(|w| b["name"].as_str().unwrap_or_default().to_lowercase().contains(w))).collect();
                if shown.is_empty() {
                    list = list.child(div().py_8().text_center().text_xs().text_color(c.subtle).child(if all.is_empty() { "No branches (is this a git repository?)" } else { "No branch matches." }));
                }
                for (i, b) in shown.into_iter().enumerate() {
                    let name = b["name"].as_str().unwrap_or_default().to_string();
                    let on = selected_name.as_deref() == Some(name.as_str());
                    let pick = b.clone();
                    let checked = b["checkedOutAt"].as_str().map(|p| p.to_string());
                    list = list.child(
                        div().id(("branch", i)).flex().items_center().gap_2().rounded_md().px_2p5().py_1p5().text_size(px(13.0)).cursor_pointer()
                            .when(on, |d| d.bg(Hsla { a: 0.15, ..c.accent }).text_color(c.fg))
                            .when(!on, |d| d.text_color(c.muted).hover(move |d| d.bg(c.hover).text_color(c.fg)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(d) = this.worktrees.dialog.as_mut() {
                                    d.selected = Some(pick.clone());
                                    d.error = None;
                                }
                                cx.notify();
                            }))
                            .child(Icon::new(IconName::GitBranch).size(px(12.0)).text_color(c.subtle))
                            .child(div().min_w_0().flex_1().truncate().font_family(mono.clone()).text_size(px(12.0)).child(name))
                            .when(b["remote"] == true, |d| d.child(div().flex_shrink_0().rounded_sm().bg(c.hover).px_1().text_size(px(10.0)).text_color(c.subtle).child("remote")))
                            .when_some(checked, |d, p| {
                                let folder = p.split(['/', '\\']).filter(|s| !s.is_empty()).last().unwrap_or(&p).to_string();
                                d.child(div().max_w(px(160.0)).flex_shrink_0().truncate().text_size(px(10.0)).text_color(c.warning).child(format!("in {folder}")))
                            })
                            .when_some(b["lastCommitAt"].as_f64(), |d, at| d.child(div().flex_shrink_0().text_size(px(11.0)).text_color(c.subtle).child(relative_time(at)))),
                    );
                }
            }
        }
        let selected = d.selected.clone();
        let upstream = selected.as_ref().and_then(|b| b["upstream"].as_str()).map(str::to_string);
        let can_pull = upstream.is_some();
        let latest = self.worktrees.latest;
        let pull = latest && can_pull;
        let making = d.making_branch;
        let new_name_empty = making && d.new_branch.read(cx).value().trim().is_empty();
        let blocked = selected.as_ref().filter(|_| !making).and_then(|b| b["checkedOutAt"].as_str()).map(|p| p.split(['/', '\\']).filter(|s| !s.is_empty()).last().unwrap_or(p).to_string());
        let disabled = selected.is_none() || blocked.is_some() || d.busy.is_some() || new_name_empty;
        let work_on = selected.as_ref().and_then(|b| b["name"].as_str()).map(|n| if selected.as_ref().is_some_and(|b| b["remote"] == true) { n.split('/').skip(1).collect::<Vec<_>>().join("/") } else { n.to_string() });
        let radio = |on: bool| div().size_3().rounded_full().border_1().border_color(if on { c.accent } else { c.border_strong }).when(on, |d| d.bg(c.accent));
        let mode = |id: &'static str, label: &'static str, on: bool, enabled: bool, tip: String| {
            div().id(id).flex().items_center().gap_1().rounded_sm().px_2().py_0p5().cursor_pointer()
                .when(on, |d| d.bg(Hsla { a: 0.15, ..c.accent }).text_color(c.fg))
                .when(!enabled, |d| d.opacity(0.4))
                .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
                .child(label)
        };
        let pull_tip = match (&selected, &upstream) {
            (Some(b), None) => format!("{} has no remote branch to pull from", b["name"].as_str().unwrap_or_default()),
            (_, Some(u)) => format!("Fetch {u} first and start from its newest commits"),
            (None, None) => "Fetch the remote first and start from its newest commits".into(),
        };
        let footer = div().flex().flex_col().gap_2p5().px_5().py_3()
            .child(div().flex().items_center().gap_3().text_xs().text_color(c.muted)
                .child(div().id("work-on").flex().items_center().gap_1p5().cursor_pointer()
                    .on_click(cx.listener(|this, _, _, cx| {
                        if let Some(d) = this.worktrees.dialog.as_mut() {
                            d.making_branch = false;
                        }
                        cx.notify();
                    }))
                    .child(radio(!making))
                    .child("Work on")
                    .child(match &work_on {
                        Some(n) => div().font_family(mono.clone()).text_color(c.fg).child(n.clone()),
                        None => div().child("the branch"),
                    }))
                .child(div().id("make-branch").flex_1().flex().items_center().gap_1p5().cursor_pointer()
                    .on_click(cx.listener(|this, _, _, cx| {
                        if let Some(d) = this.worktrees.dialog.as_mut() {
                            d.making_branch = true;
                        }
                        cx.notify();
                    }))
                    .child(radio(making))
                    .child("New branch from it:")
                    .child(div().flex_1().min_w_0().child(Input::new(&d.new_branch)))))
            .child(div().flex().items_center().gap_2().text_xs().text_color(c.muted)
                .child("Start from")
                .child(div().flex().rounded_md().border_1().border_color(c.edge).p_0p5()
                    .child(mode("from-local", "Local branch", !pull, true, "Use the branch as it is in this repo, without contacting the remote".into()).on_click(cx.listener(|this, _, _, cx| {
                        this.worktrees.latest = false;
                        cx.notify();
                    })))
                    .child(mode("from-remote", "Latest from remote", pull || (latest && selected.is_none()), selected.is_none() || can_pull, pull_tip).on_click(cx.listener(move |this, _, _, cx| {
                        if selected.is_none() || can_pull {
                            this.worktrees.latest = true;
                        }
                        cx.notify();
                    }))))
                .when_some(upstream.clone().filter(|_| pull), |d, u| d.child(div().truncate().font_family(mono.clone()).text_size(px(11.0)).text_color(c.subtle).child(u)))
                .when(latest && self.worktrees.dialog.as_ref().is_some_and(|d| d.selected.is_some()) && !can_pull, |d| d.child(div().text_size(px(11.0)).text_color(c.subtle).child("No remote branch: using the local one"))))
            .when_some(blocked, |d, folder| d.child(div().text_xs().text_color(c.warning).child(format!("Already checked out in {folder}: open that one from the sidebar, or start a new branch from it."))))
            .when_some(self.worktrees.dialog.as_ref().and_then(|d| d.error.clone()), |d, e| d.child(div().text_xs().text_color(c.danger).child(e)))
            .child(div().flex().items_center().justify_end().gap_2()
                .child(small_button(c, "branch-workspace", if self.worktrees.dialog.as_ref().and_then(|d| d.busy) == Some("workspace") { "Creating…" } else { "New workspace from it" })
                    .when(disabled, |d| d.opacity(0.4))
                    .tooltip(|window, cx| Tooltip::new("Create the worktree and make it a workspace of its own").build(window, cx))
                    .on_click(cx.listener(move |this, _, _, cx| if !disabled { this.create_from_branch(true, cx) })))
                .child(accent_button(c, "branch-agent", if self.worktrees.dialog.as_ref().and_then(|d| d.busy) == Some("agent") { "Creating…".to_string() } else { format!("Open {agent} in a worktree") }, !disabled)
                    .on_click(cx.listener(move |this, _, _, cx| if !disabled { this.create_from_branch(false, cx) }))));
        let fetching = self.worktrees.dialog.as_ref().and_then(|d| d.busy) == Some("fetch");
        let search = self.worktrees.dialog.as_ref().unwrap().search.clone();
        Some(
            div().id("branch-backdrop").absolute().size_full().flex().items_start().justify_center().pt(relative(0.1)).bg(hsla(0.0, 0.0, 0.0, 0.4)).occlude()
                .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| this.update_state(cx, |s| s.overlay = Overlay::None)))
                .child(
                    div().w(px(620.0)).max_h(relative(0.78)).flex().flex_col().rounded_xl().border_1().border_color(c.edge).bg(c.panel)
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(div().flex().items_center().gap_2().px_5().py_3()
                            .child(Icon::new(IconName::GitBranch).size(px(14.0)).text_color(c.accent))
                            .child(div().flex_1().text_sm().text_color(c.fg).child("New worktree from a branch"))
                            .child(div().truncate().text_xs().text_color(c.subtle).child(ws_name)))
                        .child(div().flex().items_center().gap_2().px_5().py_2p5()
                            .child(div().flex_1().child(Input::new(&search)))
                            .child(small_button(c, "branch-fetch", if fetching { "Fetching…" } else { "Fetch" })
                                .tooltip(|window, cx| Tooltip::new("Get the latest branches from the remotes (git fetch --all --prune)").build(window, cx))
                                .on_click(cx.listener(move |this, _, _, cx| if !fetching { this.fetch_branches(cx) }))))
                        .child(list)
                        .child(footer),
                )
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{bulk_warning, is_stale, last_activity};
    use serde_json::json;

    #[test]
    fn stale_worktrees_use_last_activity() {
        let day = 86_400_000.0;
        let old = json!({ "isMain": false, "lastCommitAt": 0.0, "createdAt": 5.0 * day });
        assert_eq!(last_activity(&old), Some(0.0));
        assert!(is_stale(&old, 11.0 * day));
        assert!(!is_stale(&old, 9.0 * day));
        assert!(!is_stale(&json!({ "isMain": true, "lastCommitAt": 0.0 }), 99.0 * day));
    }

    #[test]
    fn bulk_warnings_say_what_is_lost() {
        let list = vec![
            json!({ "info": { "branch": "winfer/a" }, "byWinfer": true, "dirty": true, "ahead": 2 }),
            json!({ "info": { "branch": "mine" }, "byWinfer": false, "dirty": false, "ahead": 0 }),
        ];
        assert_eq!(
            bulk_warning(&list),
            "Delete 2 worktrees, and the branches of the 1 winfer made?\n\n• winfer/a\n• mine\n\nLost for good: 1 worktree with uncommitted changes and 1 branch with unmerged commits.\n\nBranches not made by winfer are kept."
        );
    }
}
