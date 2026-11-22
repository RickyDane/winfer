//! Dialogs for starting and inspecting agents: new pane, model picker, worktree changes
//! (port of `NewPaneDialog.tsx`, `ModelPicker.tsx` and `DiffView.tsx`).

use super::commands::{Command, PaneRef};
use super::*;
use crate::model::{Overlay, WorktreeInfo};
use gpui_kit::component::input::{Input, InputEvent, InputState, MoveDown, MoveUp};
use serde::Deserialize;
use std::sync::Arc;
use winfer_backend::{providers, worktree};

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Branch {
    name: String,
    remote: bool,
    last_commit_at: Option<f64>,
    checked_out_at: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExistingWorktree {
    info: WorktreeInfo,
    is_main: bool,
}

#[derive(Clone, Debug, PartialEq)]
enum Row {
    Auto,
    New(String),
    Branch(usize),
}

pub struct NewPaneDialog {
    workspace: String,
    dir: String,
    input: Entity<InputState>,
    branches: Option<Vec<Branch>>,
    worktrees: Vec<ExistingWorktree>,
    index: usize,
    busy: bool,
    error: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ModelInfo {
    pub id: String,
    pub label: String,
}

pub struct ModelDialog {
    input: Entity<InputState>,
    pane: Option<String>,
}

#[derive(Clone, Copy, PartialEq)]
enum LineKind {
    File,
    Added,
    Removed,
    Hunk,
    Note,
    Context,
}

pub struct DiffDialog {
    pane: String,
    info: WorktreeInfo,
    lines: Option<Arc<Vec<(LineKind, SharedString)>>>,
    ahead: u64,
    dirty: bool,
    error: Option<String>,
    loading: bool,
}

/// How long ago a millisecond timestamp was.
pub fn relative_time(at_ms: f64) -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_millis() as f64);
    let s = ((now - at_ms) / 1000.0).max(0.0).round() as u64;
    match s {
        0..45 => "now".to_string(),
        45..3600 => format!("{}m", ((s as f64) / 60.0).round() as u64),
        3600..86400 => format!("{}h", ((s as f64) / 3600.0).round() as u64),
        _ => format!("{}d", ((s as f64) / 86400.0).round() as u64),
    }
}

fn folder_name(path: &str) -> String {
    path.split(['/', '\\']).filter(|p| !p.is_empty()).next_back().unwrap_or(path).to_string()
}

fn same_path(a: &str, b: &str) -> bool {
    a.trim_end_matches(['/', '\\']) == b.trim_end_matches(['/', '\\'])
}

/// Flattens a unified diff into display lines, dropping git's header lines.
fn diff_lines(diff: &str, untracked: &[String], truncated_mb: Option<u64>) -> Vec<(LineKind, SharedString)> {
    let mut out: Vec<(LineKind, SharedString)> = Vec::new();
    if let Some(mb) = truncated_mb {
        out.push((LineKind::Note, format!("This diff is {mb} MB — showing only its first part. Very large diffs are not sent in full.").into()));
    }
    if !untracked.is_empty() {
        out.push((LineKind::File, "New files (untracked)".into()));
        out.extend(untracked.iter().map(|f| (LineKind::Added, format!("+ {f}").into())));
    }
    const HEADERS: [&str; 8] = ["index ", "--- ", "+++ ", "new file mode", "deleted file mode", "similarity index", "rename from", "rename to"];
    for line in diff.split('\n') {
        if let Some(rest) = line.strip_prefix("diff --git a/") {
            let name = rest.split(" b/").next().unwrap_or(rest);
            out.push((LineKind::File, name.to_string().into()));
        } else if !out.is_empty() && !HEADERS.iter().any(|h| line.starts_with(h)) {
            let kind = match line.chars().next() {
                Some('+') => LineKind::Added,
                Some('-') => LineKind::Removed,
                Some('@') if line.starts_with("@@") => LineKind::Hunk,
                _ => LineKind::Context,
            };
            out.push((kind, if line.is_empty() { " ".into() } else { line.to_string().into() }));
        }
    }
    out
}

impl Root {
    /// Opens and closes dialogs to match the state.
    pub(super) fn sync_dialogs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (overlay, diff) = {
            let s = self.state.read(cx);
            (s.overlay, s.diff_pane_id.clone())
        };
        if overlay == Overlay::NewPane && self.new_pane.is_none() {
            self.open_new_pane(window, cx);
        } else if overlay != Overlay::NewPane && self.new_pane.is_some() {
            self.new_pane = None;
            self.focused_view = None;
        }
        if overlay == Overlay::Model && self.model_dialog.is_none() {
            self.open_model_dialog(window, cx);
        } else if overlay != Overlay::Model && self.model_dialog.is_some() {
            self.model_dialog = None;
            self.focused_view = None;
        }
        match diff {
            Some(id) if self.diff_dialog.as_ref().is_none_or(|d| d.pane != id) => self.open_diff(id, cx),
            None if self.diff_dialog.is_some() => {
                self.diff_dialog = None;
                self.focused_view = None;
            }
            _ => {}
        }
    }

    fn open_new_pane(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((workspace, dir)) = self.state.read(cx).active().map(|w| (w.id.clone(), w.root_dir.clone())) else { return };
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("New branch name, or find a branch…"));
        cx.subscribe_in(&input, window, |this, _, event: &InputEvent, window, cx| match event {
            InputEvent::Change => {
                if let Some(d) = this.new_pane.as_mut() {
                    d.index = 0;
                    d.error = None;
                }
                cx.notify();
            }
            InputEvent::PressEnter { secondary, .. } => this.confirm_new_pane(*secondary, window, cx),
            _ => {}
        })
        .detach();
        input.read(cx).focus_handle(cx).focus(window, cx);
        self.new_pane = Some(NewPaneDialog { workspace, dir: dir.clone(), input, branches: None, worktrees: Vec::new(), index: 0, busy: false, error: None });
        cx.spawn(async move |this, cx| {
            let d = dir.clone();
            let branches = tauri::async_runtime::spawn(async move { worktree::worktree_branches(d).await.ok().and_then(|b| serde_json::to_value(b).ok()) }).await.ok().flatten();
            let d = dir.clone();
            let trees = tauri::async_runtime::spawn(async move { worktree::worktree_list(d).await.ok().and_then(|b| serde_json::to_value(b).ok()) }).await.ok().flatten();
            let _ = this.update(cx, |this, cx| {
                if let Some(dialog) = this.new_pane.as_mut() {
                    dialog.branches = Some(branches.and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default());
                    dialog.worktrees = trees.and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default();
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn new_pane_rows(dialog: &NewPaneDialog, cx: &App) -> Vec<Row> {
        let query = dialog.input.read(cx).value().to_string();
        let name = query.trim();
        let lower = query.to_lowercase();
        let words: Vec<&str> = lower.split_whitespace().collect();
        let branches = dialog.branches.as_deref().unwrap_or(&[]);
        let exact = branches.iter().any(|b| b.name == name || (b.remote && b.name.split('/').skip(1).collect::<Vec<_>>().join("/") == name));
        let mut rows = Vec::new();
        if name.is_empty() {
            rows.push(Row::Auto);
        } else if !exact {
            rows.push(Row::New(name.to_string()));
        }
        rows.extend(branches.iter().enumerate().filter(|(_, b)| words.iter().all(|w| b.name.to_lowercase().contains(w))).map(|(i, _)| Row::Branch(i)));
        rows
    }

    fn close_overlay(&mut self, cx: &mut Context<Self>) {
        self.update_state(cx, |s| {
            s.overlay = Overlay::None;
            s.new_pane_provider = None;
        });
    }

    fn confirm_new_pane(&mut self, secondary: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dialog) = self.new_pane.as_ref() else { return };
        if dialog.busy {
            return;
        }
        if secondary {
            let (workspace, provider) = (dialog.workspace.clone(), self.new_pane_provider(cx));
            self.update_state(cx, |s| {
                s.add_pane(&provider, AddPane { workspace_id: Some(workspace), worktree: Some(false), ..Default::default() });
            });
            return self.close_overlay(cx);
        }
        let rows = Self::new_pane_rows(dialog, cx);
        if let Some(row) = rows.get(dialog.index.min(rows.len().saturating_sub(1))).cloned() {
            self.open_row(row, window, cx);
        }
    }

    fn new_pane_provider(&self, cx: &App) -> String {
        let s = self.state.read(cx);
        s.new_pane_provider.clone().unwrap_or_else(|| s.default_provider_id())
    }

    fn open_row(&mut self, row: Row, _: &mut Window, cx: &mut Context<Self>) {
        let Some(dialog) = self.new_pane.as_ref() else { return };
        let is_repo = dialog.branches.as_ref().is_none_or(|b| !b.is_empty());
        let (workspace, dir) = (dialog.workspace.clone(), dialog.dir.clone());
        let provider = self.new_pane_provider(cx);
        let agent = self.state.read(cx).provider_name(&provider);
        let base = AddPane { workspace_id: Some(workspace.clone()), ..Default::default() };
        if row == Row::Auto || !is_repo {
            self.update_state(cx, |s| {
                s.add_pane(&provider, base);
                s.log_activity(&workspace, format!("Opened {agent}"), Some(provider.clone()));
            });
            return self.close_overlay(cx);
        }
        let branch = if let Row::Branch(i) = row { dialog.branches.as_ref().and_then(|b| b.get(i)).cloned() } else { None };
        if let Some(at) = branch.as_ref().and_then(|b| b.checked_out_at.clone()) {
            let found = dialog.worktrees.iter().find(|w| same_path(&w.info.path, &at)).filter(|w| !w.is_main).cloned();
            let opts = match found {
                Some(w) => AddPane { existing_worktree: Some(w.info), ..base },
                None => AddPane { worktree: Some(false), cwd: Some(at), ..base },
            };
            let label = branch.as_ref().map_or(String::new(), |b| b.name.clone());
            self.update_state(cx, |s| {
                s.add_pane(&provider, opts);
                s.log_activity(&workspace, format!("Opened {agent} on {label}"), Some(provider.clone()));
            });
            return self.close_overlay(cx);
        }
        let (source, new_branch) = match row {
            Row::New(name) => ("HEAD".to_string(), Some(name)),
            _ => (branch.map_or_else(|| "HEAD".to_string(), |b| b.name), None),
        };
        if let Some(d) = self.new_pane.as_mut() {
            d.busy = true;
            d.error = None;
        }
        cx.notify();
        cx.spawn(async move |this, cx| {
            let created = tauri::async_runtime::spawn(async move {
                let info = worktree::worktree_from_branch(backend::app().clone(), dir, source, new_branch, Some(false)).await?;
                serde_json::to_value(info).and_then(serde_json::from_value::<WorktreeInfo>).map_err(|e| e.to_string())
            })
            .await
            .map_err(|e| e.to_string())
            .and_then(|r| r);
            let _ = this.update(cx, |this, cx| match created {
                Ok(info) => {
                    let branch = info.branch.clone();
                    this.update_state(cx, |s| {
                        s.add_pane(&provider, AddPane { workspace_id: Some(workspace.clone()), existing_worktree: Some(info), ..Default::default() });
                        s.log_activity(&workspace, format!("Worktree for {branch}"), Some(provider.clone()));
                    });
                    this.close_overlay(cx);
                }
                Err(err) => {
                    if let Some(d) = this.new_pane.as_mut() {
                        d.busy = false;
                        d.error = Some(err);
                    }
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub(super) fn render_new_pane(&mut self, c: &Colors, cx: &mut Context<Self>) -> Option<AnyElement> {
        let dialog = self.new_pane.as_ref()?;
        let c = *c;
        let s = self.state.read(cx);
        let provider = self.new_pane_provider(cx);
        let agent = s.provider_name(&provider);
        let mut installed: Vec<(String, String)> = s.providers.iter().filter(|p| p.installed).map(|p| (p.id.clone(), p.name.clone())).collect();
        if let Some(pos) = installed.iter().position(|(id, _)| id == "winfer")
            && pos > 0
        {
            let winfer = installed.remove(pos);
            installed.insert(0, winfer);
        }
        let ws_name = s.workspaces.iter().find(|w| w.id == dialog.workspace).map_or(String::new(), |w| w.name.clone());
        let is_repo = dialog.branches.as_ref().is_none_or(|b| !b.is_empty());
        let rows = Self::new_pane_rows(dialog, cx);
        let current = dialog.index.min(rows.len().saturating_sub(1));
        let mono = crate::fonts::mono(cx);
        let mut list = div().id("new-pane-list").max_h(px(460.0)).min_h(px(96.0)).overflow_y_scroll().p_2().flex().flex_col();
        for (i, row) in rows.iter().enumerate() {
            let target = row.clone();
            let mut item = div().id(("new-pane-row", i)).flex().items_center().gap_2p5().rounded_lg().px_2p5().h(px(34.0)).flex_shrink_0().text_size(px(13.0)).cursor_pointer()
                .text_color(if i == current { c.fg } else { c.muted })
                .when(i == current, |d| d.bg(c.active))
                .hover(move |d| d.bg(c.hover).text_color(c.fg))
                .on_mouse_move(cx.listener(move |this, _, _, cx| {
                    if let Some(d) = this.new_pane.as_mut().filter(|d| d.index != i) {
                        d.index = i;
                        cx.notify();
                    }
                }))
                .on_click(cx.listener(move |this, _, window, cx| {
                    if !this.new_pane.as_ref().is_some_and(|d| d.busy) {
                        this.open_row(target.clone(), window, cx);
                    }
                }));
            item = match row {
                Row::Auto => item
                    .child(Icon::new(IconName::Sparkles).size(px(12.0)).text_color(c.accent))
                    .child(div().flex_1().child(if is_repo { "New worktree, auto-named branch" } else { "New pane" })),
                Row::New(name) => item
                    .child(Icon::new(IconName::Plus).size(px(12.0)).text_color(c.accent))
                    .child(div().flex_1().truncate().child("New branch ").child(div().font_family(mono.clone()).text_size(px(12.0)).text_color(c.fg).child(name.clone()))),
                Row::Branch(index) => {
                    let b = &dialog.branches.as_ref().unwrap()[*index];
                    item.child(Icon::new(IconName::GitBranch).size(px(12.0)).text_color(c.subtle))
                        .child(div().flex_1().min_w_0().truncate().font_family(mono.clone()).text_size(px(12.0)).child(b.name.clone()))
                        .when(b.remote, |d| d.child(div().rounded_sm().bg(c.hover).px_1().text_size(px(10.0)).text_color(c.subtle).child("remote")))
                        .when_some(b.checked_out_at.clone(), |d, at| d.child(div().max_w(px(160.0)).truncate().text_size(px(10.0)).text_color(c.subtle).child(folder_name(&at))))
                        .when_some(b.last_commit_at, |d, at| d.child(div().text_size(px(11.0)).text_color(c.subtle).child(relative_time(at))))
                }
            };
            list = list.child(item);
        }
        if dialog.branches.is_none() {
            list = list.child(div().py_4().text_center().text_xs().text_color(c.subtle).child("Reading branches…"));
        }
        let count = rows.len();
        let mut picker = div().flex().flex_wrap().gap_1p5();
        for (id, name) in installed {
            let on = id == provider;
            let is_winfer = id == "winfer";
            let pick = id.clone();
            picker = picker.child(
                div().id(SharedString::from(format!("new-pane-agent-{id}"))).flex().items_center().gap_1p5().h(px(30.0)).px_2p5().rounded_lg().border_1().text_size(px(12.5)).cursor_pointer()
                    .bg(if on { Hsla { a: 0.12, ..c.accent } } else { c.panel })
                    .border_color(if on { Hsla { a: 0.42, ..c.accent } } else { Hsla { a: 0.65, ..c.border } })
                    .text_color(if on { c.fg } else { c.muted })
                    .when(!on, |d| d.hover(move |d| d.text_color(c.fg).bg(c.hover)))
                    .on_click(cx.listener(move |this, _, _, cx| this.update_state(cx, |s| s.new_pane_provider = Some(pick.clone()))))
                    .child(crate::ui::code_pickers::mark(c, &id, 14.0, 1.0))
                    .child(name)
                    .when(is_winfer, |d| d.child(crate::ui::code_pickers::recommended_badge(c, true)))
                    .when(on, |d| d.child(Icon::new(IconName::Check).size(px(12.0)).text_color(c.accent))),
            );
        }
        let key = |k: &'static str| div().rounded_sm().border_1().border_color(c.edge).bg(c.panel).px_1().text_size(px(10.5)).text_color(c.muted).child(k);
        let hint = |k: &'static str, label: &'static str| div().flex().items_center().gap_1().child(key(k)).child(label);
        Some(
            div().id("new-pane-backdrop").absolute().size_full().flex().items_start().justify_center().pt(px(90.0)).bg(hsla(0.0, 0.0, 0.0, 0.4)).occlude()
                .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| this.close_overlay(cx)))
                .child(
                    div().id("new-pane").w(px(600.0)).max_h(px(660.0)).flex().flex_col().overflow_hidden().rounded(px(14.0)).border_1().border_color(c.edge).bg(c.raised).shadow_2xl()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .capture_action(cx.listener(move |this, _: &MoveDown, _, cx| {
                            if let Some(d) = this.new_pane.as_mut() {
                                d.index = (d.index + 1).min(count.saturating_sub(1));
                            }
                            cx.stop_propagation();
                            cx.notify();
                        }))
                        .capture_action(cx.listener(|this, _: &MoveUp, _, cx| {
                            if let Some(d) = this.new_pane.as_mut() {
                                d.index = d.index.saturating_sub(1);
                            }
                            cx.stop_propagation();
                            cx.notify();
                        }))
                        .child(
                            div().flex().flex_col().gap_3().px_5().pt_4().pb_3p5()
                                .child(div().flex().items_center().gap_2()
                                    .child(div().size(px(26.0)).flex().items_center().justify_center().rounded_lg().bg(Hsla { a: 0.14, ..c.accent }).text_color(c.accent).child(Icon::new(IconName::Plus).size(px(14.0))))
                                    .child(div().flex_1().text_size(px(15.0)).font_weight(FontWeight::SEMIBOLD).text_color(c.fg).child(format!("New {agent}")))
                                    .child(div().rounded_md().bg(c.hover).px_2().py_0p5().text_size(px(11.5)).text_color(c.muted).truncate().child(ws_name)))
                                .child(picker),
                        )
                        .child(div().flex().items_center().gap_2p5().px_5().h(px(46.0))
                            .child(Icon::new(IconName::GitBranch).size(px(15.0)).text_color(c.subtle))
                            .child(div().flex_1().child(Input::new(&dialog.input).appearance(false))))
                        .child(list)
                        .child(
                            div().flex().flex_col().gap_1p5().px_5().py_2p5()
                                .when_some(dialog.error.clone(), |d, e| d.child(div().text_xs().text_color(c.danger).child(e)))
                                .child(div().flex().items_center().gap_3().text_size(px(11.0)).text_color(c.subtle)
                                    .child(hint("↑↓", "choose")).child(hint("↩", "open")).child(hint(if cfg!(target_os = "macos") { "⌘↩" } else { "Ctrl+↩" }, "without worktree")).child(hint("esc", "cancel"))),
                        ),
                )
                .into_any_element(),
        )
    }

    fn open_model_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let pane = self.state.read(cx).focused_pane_id.clone();
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Search models, or type a model name"));
        cx.subscribe_in(&input, window, |this, input, event: &InputEvent, window, cx| match event {
            InputEvent::Change => cx.notify(),
            InputEvent::PressEnter { .. } => {
                let query = input.read(cx).value().trim().to_string();
                if !query.is_empty() {
                    let first = this.filtered_models(&query, cx).into_iter().next().map(|m| m.id);
                    this.choose_model(Some(first.unwrap_or(query)), window, cx);
                }
            }
            _ => {}
        })
        .detach();
        input.read(cx).focus_handle(cx).focus(window, cx);
        self.model_dialog = Some(ModelDialog { input, pane: pane.clone() });
        let Some(provider) = pane.and_then(|id| self.state.read(cx).pane(&id).map(|p| p.provider.clone())) else { return };
        if self.models.contains_key(&provider) {
            return;
        }
        cx.spawn(async move |this, cx| {
            let id = provider.clone();
            let found = tauri::async_runtime::spawn(async move { providers::providers_models(id).await.ok().and_then(|m| serde_json::to_value(m).ok()) }).await.ok().flatten();
            let list: Vec<ModelInfo> = found.and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default();
            let _ = this.update(cx, |this, cx| {
                this.models.insert(provider, list);
                cx.notify();
            });
        })
        .detach();
    }

    fn filtered_models(&self, query: &str, cx: &App) -> Vec<ModelInfo> {
        let provider = self.model_dialog.as_ref().and_then(|d| d.pane.as_ref()).and_then(|id| self.state.read(cx).pane(id)).map(|p| p.provider.clone());
        let q = query.to_lowercase();
        provider.and_then(|p| self.models.get(&p)).map(|l| l.iter().filter(|m| m.id.to_lowercase().contains(&q) || m.label.to_lowercase().contains(&q)).take(200).cloned().collect()).unwrap_or_default()
    }

    fn choose_model(&mut self, model: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        let pane = self.model_dialog.as_ref().and_then(|d| d.pane.clone());
        self.close_overlay(cx);
        if let Some(id) = pane {
            self.execute(Command::SetModel { target: PaneRef::Id(id), model }, window, cx);
        }
    }

    pub(super) fn render_model_dialog(&mut self, c: &Colors, cx: &mut Context<Self>) -> Option<AnyElement> {
        let dialog = self.model_dialog.as_ref()?;
        let c = *c;
        let s = self.state.read(cx);
        let pane = dialog.pane.as_ref().and_then(|id| s.pane(id)).cloned();
        let provider = pane.as_ref().and_then(|p| s.providers.iter().find(|x| x.id == p.provider)).cloned();
        let query = dialog.input.read(cx).value().trim().to_string();
        let input = dialog.input.clone();
        let mut content = div().flex().flex_col();
        match (pane, provider.filter(|p| p.model_arg.is_some())) {
            (Some(pane), Some(provider)) => {
                let loading = !self.models.contains_key(&provider.id);
                let filtered = self.filtered_models(&query, cx);
                let mut rows: Vec<(String, Option<String>, bool)> = vec![("CLI default".to_string(), None, pane.model.is_none())];
                rows.extend(filtered.iter().map(|m| (m.label.clone(), Some(m.id.clone()), pane.model.as_ref() == Some(&m.id))));
                if !query.is_empty() && !filtered.iter().any(|m| m.id == query) {
                    rows.push((format!("Use “{query}”"), Some(query.clone()), false));
                }
                let mut list = div().id("model-list").overflow_y_scroll().max_h(px(360.0)).p_1p5().flex().flex_col();
                if loading {
                    list = list.child(div().px_3().py_2().text_xs().text_color(c.subtle).child(format!("Asking {} for its models…", provider.name)));
                }
                for (i, (label, id, active)) in rows.into_iter().enumerate() {
                    let pick = id.clone();
                    list = list.child(
                        div().id(("model-row", i)).flex().items_center().gap_2().rounded_md().px_3().py_1p5().text_size(px(13.0)).cursor_pointer()
                            .text_color(if active { c.fg } else { c.muted })
                            .hover(move |d| d.bg(c.hover))
                            .on_click(cx.listener(move |this, _, window, cx| this.choose_model(pick.clone(), window, cx)))
                            .child(div().flex_1().truncate().child(label.clone()))
                            .when_some(id.filter(|x| *x != label), |d, x| d.child(div().truncate().text_size(px(11.0)).text_color(c.subtle).child(x)))
                            .when(active, |d| d.child(Icon::new(IconName::Check).size(px(13.0)).text_color(c.accent))),
                    );
                }
                let how = if provider.model_command.is_some() { "switches in place" } else { "restarts and resumes the session" };
                content = content
                    .child(div().px_4().py_2p5().text_xs().text_color(c.subtle).child(format!("Model for {} · {how}", provider.name)))
                    .child(div().p_2().child(Input::new(&input).appearance(false)))
                    .child(list);
            }
            (Some(pane), None) => content = content.child(div().p_5().text_xs().text_color(c.muted).child(format!("{} can't switch models.", s.provider_name(&pane.provider)))),
            _ => content = content.child(div().p_5().text_xs().text_color(c.muted).child("Focus an agent pane first.")),
        }
        Some(
            div().id("model-backdrop").absolute().size_full().flex().justify_center().pt(px(120.0)).bg(hsla(0.0, 0.0, 0.0, 0.4)).occlude()
                .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| this.close_overlay(cx)))
                .child(div().id("model").w(px(480.0)).h_auto().overflow_hidden().rounded_xl().border_1().border_color(c.edge).bg(c.raised).on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation()).child(content))
                .into_any_element(),
        )
    }

    fn open_diff(&mut self, pane: String, cx: &mut Context<Self>) {
        let Some(info) = self.state.read(cx).pane(&pane).and_then(|p| p.worktree.clone()) else {
            return self.update_state(cx, |s| s.diff_pane_id = None);
        };
        self.diff_dialog = Some(DiffDialog { pane, info, lines: None, ahead: 0, dirty: false, error: None, loading: false });
        self.reload_diff(cx);
    }

    fn reload_diff(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.diff_dialog.as_mut() else { return };
        dialog.loading = true;
        dialog.error = None;
        let info = dialog.info.clone();
        cx.spawn(async move |this, cx| {
            let loaded = tauri::async_runtime::spawn(async move {
                let diff = worktree::worktree_diff(commands::worktree_info(&info)?).await?;
                serde_json::to_value(diff).map_err(|e| e.to_string())
            })
            .await
            .map_err(|e| e.to_string())
            .and_then(|r| r);
            let parsed = loaded.map(|v| {
                let untracked: Vec<String> = serde_json::from_value(v["untracked"].clone()).unwrap_or_default();
                let truncated = (v["diffTruncated"] == true).then(|| (v["diffBytes"].as_f64().unwrap_or(0.0) / 1048576.0).round() as u64);
                (diff_lines(v["diff"].as_str().unwrap_or_default(), &untracked, truncated), v["ahead"].as_u64().unwrap_or(0), v["dirty"] == true)
            });
            let _ = this.update(cx, |this, cx| {
                if let Some(d) = this.diff_dialog.as_mut() {
                    d.loading = false;
                    match parsed {
                        Ok((lines, ahead, dirty)) => {
                            d.lines = Some(Arc::new(lines));
                            d.ahead = ahead;
                            d.dirty = dirty;
                        }
                        Err(err) => d.error = Some(err),
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn render_diff(&mut self, c: &Colors, cx: &mut Context<Self>) -> Option<AnyElement> {
        let dialog = self.diff_dialog.as_ref()?;
        let c = *c;
        let mono = crate::fonts::mono(cx);
        let pane = dialog.pane.clone();
        let info = dialog.info.clone();
        let button = move |id: &'static str, icon: IconName| div().id(id).rounded_sm().p_1p5().text_color(c.subtle).cursor_pointer().hover(move |d| d.bg(c.hover).text_color(c.fg)).child(Icon::new(icon).size(px(14.0)));
        let summary = dialog.lines.as_ref().map(|_| {
            let commits = if dialog.ahead == 1 { "commit" } else { "commits" };
            format!("{} {commits} ahead{}", dialog.ahead, if dialog.dirty { " · uncommitted changes" } else { "" })
        });
        let body: AnyElement = match (&dialog.lines, &dialog.error) {
            (_, Some(err)) => div().p_4().text_color(c.danger).child(err.clone()).into_any_element(),
            (None, _) => div().p_4().text_color(c.subtle).child("Loading changes…").into_any_element(),
            (Some(lines), _) if lines.is_empty() => div().p_4().text_color(c.subtle).child("No changes yet.").into_any_element(),
            (Some(lines), _) => {
                let lines = lines.clone();
                uniform_list("diff-lines", lines.len(), move |range, _, _| {
                    range
                        .map(|i| {
                            let (kind, text) = lines[i].clone();
                            let row = div().w_full().px_4().h(px(19.0)).whitespace_nowrap();
                            match kind {
                                LineKind::File => row.bg(c.raised).text_color(c.fg).child(text),
                                LineKind::Added => row.bg(Hsla { a: 0.1, ..c.success }).text_color(c.success).child(text),
                                LineKind::Removed => row.bg(Hsla { a: 0.1, ..c.danger }).text_color(c.danger).child(text),
                                LineKind::Hunk => row.text_color(c.accent).child(text),
                                LineKind::Note => row.text_color(c.warning).child(text),
                                LineKind::Context => row.text_color(c.muted).child(text),
                            }
                        })
                        .collect()
                })
                .size_full()
                .into_any_element()
            }
        };
        let (merge_target, discard_target) = (pane.clone(), pane);
        Some(
            div().id("diff-backdrop").absolute().size_full().flex().items_center().justify_center().p_10().bg(hsla(0.0, 0.0, 0.0, 0.4)).occlude()
                .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| this.update_state(cx, |s| s.diff_pane_id = None)))
                .child(
                    div().id("diff").w_full().max_w(px(1024.0)).h_full().flex().flex_col().overflow_hidden().rounded_xl().border_1().border_color(c.edge).bg(c.panel)
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(
                            div().flex().items_center().gap_3().px_4().py_3()
                                .child(Icon::new(IconName::GitBranch).size(px(15.0)).text_color(c.accent))
                                .child(
                                    div().flex_1().min_w_0().flex().flex_col().font_family(mono.clone())
                                        .child(div().text_size(px(13.0)).child(format!("{} → {}", info.branch, info.base)))
                                        .child(div().text_size(px(11.0)).text_color(c.subtle).truncate().child(info.path.clone())),
                                )
                                .when_some(summary, |d, t| d.child(div().text_xs().text_color(c.muted).child(t)))
                                .child(button("diff-refresh", IconName::RefreshCw).on_click(cx.listener(|this, _, _, cx| this.reload_diff(cx))))
                                .child(
                                    div().id("diff-merge").flex().items_center().gap_1p5().rounded_md().bg(c.accent).text_color(c.on_accent).px_2p5().py_1().text_xs().cursor_pointer()
                                        .on_click(cx.listener(move |this, _, window, cx| this.execute(Command::MergeWorktree(PaneRef::Id(merge_target.clone())), window, cx)))
                                        .child(Icon::new(IconName::GitMerge).size(px(13.0)))
                                        .child(format!("Merge into {}", info.base)),
                                )
                                .child(button("diff-discard", IconName::Trash).on_click(cx.listener(move |this, _, window, cx| {
                                    this.update_state(cx, |s| s.diff_pane_id = None);
                                    this.execute(Command::DiscardWorktree(PaneRef::Id(discard_target.clone())), window, cx);
                                })))
                                .child(button("diff-close", IconName::X).on_click(cx.listener(|this, _, _, cx| this.update_state(cx, |s| s.diff_pane_id = None)))),
                        )
                        .child(div().flex_1().min_h_0().font_family(mono.clone()).text_size(px(12.0)).child(body)),
                )
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{LineKind, diff_lines, folder_name, same_path};

    #[test]
    fn diffs_are_split_into_files() {
        let diff = "diff --git a/src/a.rs b/src/a.rs\nindex 1..2 100644\n--- a/src/a.rs\n+++ b/src/a.rs\n@@ -1 +1 @@\n-old\n+new\n ctx";
        let lines = diff_lines(diff, &[], None);
        assert_eq!(lines.len(), 5);
        assert!(lines[0].0 == LineKind::File && lines[0].1 == "src/a.rs");
        assert!(lines[1].0 == LineKind::Hunk);
        assert!(lines[2].0 == LineKind::Removed && lines[3].0 == LineKind::Added && lines[4].0 == LineKind::Context);
    }

    #[test]
    fn paths() {
        assert_eq!(folder_name("/a/b/"), "b");
        assert!(same_path("/a/b/", "/a/b"));
    }
}
