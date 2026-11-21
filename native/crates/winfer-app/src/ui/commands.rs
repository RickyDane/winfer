//! The command bus and keymap (port of `src/commands/bus.ts` and `keymap.ts`): every keyboard
//! shortcut, palette entry and button becomes a `Command` and runs through `Root::execute`.

use super::*;
use crate::model::Overlay;
use crate::model::layout::{Preset, PRESETS, build_preset, equalize, resize_pane, swap_leaves};
use std::sync::LazyLock;

#[derive(Clone, Debug, PartialEq)]
pub enum PaneRef {
    Focused,
    Id(String),
    Index(usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ViewTarget {
    Set(View),
    Toggle,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    Spawn { provider: Option<String>, count: usize, prompt: Option<String>, worktree: Option<bool>, model: Option<String> },
    SetModel { target: PaneRef, model: Option<String> },
    Split { dir: SplitDir },
    Close(PaneRef),
    Focus(PaneRef),
    FocusDir(Direction),
    SwapDir(Direction),
    Resize { target: PaneRef, axis: SplitDir, delta: f64 },
    Zoom(PaneRef),
    Archive { target: PaneRef, restore: bool },
    ToggleArchiveFocused,
    InterruptAll,
    CloseAll,
    Interrupt(PaneRef),
    Approve(PaneRef),
    Broadcast { text: String, submit: bool, pane_ids: Option<Vec<String>> },
    Search,
    SendText { target: PaneRef, text: String, submit: bool },
    ShowChanges(PaneRef),
    MergeWorktree(PaneRef),
    DiscardWorktree(PaneRef),
    EditorOpen { target: Option<PaneRef>, editor: Option<String> },
    NextAttention,
    LayoutEqualize,
    LayoutPreset(Option<Preset>),
    WorkspaceSwitchIndex(usize),
    WorkspaceSwitch(String),
    WorkspaceCreate,
    WorkspaceRemove(String),
    ToggleWorktrees,
    BrowserOpen { url: String, new_tab: bool },
    BrowserNewTab,
    BrowserCloseTab,
    BrowserHistory(i64),
    BrowserFocusUrl,
    BrowserPick,
    LiveVoice,
    CheckUpdates,
    DownloadUpdate,
    InstallUpdate,
    ShowReleaseNotes(bool),
    BrowserUndock,
    ToggleSidebar,
    ToggleBrowser,
    SidePanel { panel: SidePanel, show: bool },
    Palette(Option<String>),
    CheatSheet,
    Settings(Option<String>),
    Theme(String),
    Overlay { overlay: Overlay, provider: Option<String> },
    RaceResults(String),
    /// Carries a pane's task over to another agent.
    HandOff { pane: String, to: String },
    RedeemCodexReset,
    /// A plugin tool's palette entry with its fixed arguments.
    PluginTool { id: String, args: serde_json::Value },
    RecipeLoad(String),
    TerminalZoom(i32),
    View(ViewTarget),
    /// A command whose feature this build doesn't have yet.
    Unavailable(&'static str),
}

const IS_MAC: bool = cfg!(target_os = "macos");
const RESIZE_STEP: f64 = 0.05;

pub struct Binding {
    pub id: String,
    pub digit: Option<usize>,
    keys: String,
    win: Option<String>,
    pub title: String,
    pub group: &'static str,
    repeat: bool,
    command: Box<dyn Fn(&AppState) -> Option<Command> + Send + Sync>,
}

fn slug(text: &str) -> String {
    let mapped: String = text.to_lowercase().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    mapped.split('-').filter(|p| !p.is_empty()).collect::<Vec<_>>().join("-")
}

fn focused() -> PaneRef {
    PaneRef::Focused
}

pub static BINDINGS: LazyLock<Vec<Binding>> = LazyLock::new(|| {
    use Command as C;
    let mut out: Vec<Binding> = Vec::new();
    let mut add = |keys: &str, win: Option<&str>, title: String, group: &'static str, repeat: bool, digit: Option<usize>, command: Box<dyn Fn(&AppState) -> Option<Command> + Send + Sync>| {
        let id = match digit {
            Some(_) => slug(title.rsplit_once(' ').map_or(title.as_str(), |(head, _)| head)),
            None => slug(&title),
        };
        out.push(Binding { id, digit, keys: keys.into(), win: win.map(str::to_string), title, group, repeat, command });
    };
    let fixed = |c: Command| -> Box<dyn Fn(&AppState) -> Option<Command> + Send + Sync> { Box::new(move |_| Some(c.clone())) };
    let spawn_default = |worktree| C::Spawn { provider: None, count: 1, prompt: None, worktree, model: None };

    add("Mod+T", None, "New pane (default agent)".into(), "Panes", false, None, fixed(C::Overlay { overlay: Overlay::NewPane, provider: None }));
    add("Mod+Shift+N", Some("Ctrl+Shift+Alt+N"), "New pane without worktree".into(), "Panes", false, None, fixed(spawn_default(Some(false))));
    add("Mod+Shift+T", Some("Ctrl+Shift+Alt+T"), "New pane: pick provider".into(), "Panes", false, None, fixed(C::Palette(Some("New pane: ".into()))));
    for n in 1..=9 {
        add(&format!("Mod+Alt+{n}"), None, format!("New pane with provider {n}"), "Panes", false, Some(n), Box::new(move |s| {
            let provider = s.providers.iter().filter(|p| p.installed).nth(n - 1)?;
            Some(C::Overlay { overlay: Overlay::NewPane, provider: Some(provider.id.clone()) })
        }));
    }
    add("Mod+D", None, "Split right".into(), "Panes", false, None, fixed(C::Split { dir: SplitDir::Row }));
    add("Mod+Shift+D", Some("Ctrl+Shift+Alt+D"), "Split down".into(), "Panes", false, None, fixed(C::Split { dir: SplitDir::Col }));
    add("Mod+W", None, "Close pane".into(), "Panes", false, None, fixed(C::Close(focused())));
    for n in 1..=9 {
        add(&format!("Mod+{n}"), None, format!("Focus pane {n}"), "Panes", false, Some(n), fixed(C::Focus(PaneRef::Index(n))));
    }
    let arrows = [("Left", "left", Direction::Left), ("Right", "right", Direction::Right), ("Up", "up", Direction::Up), ("Down", "down", Direction::Down)];
    for (key, name, dir) in arrows {
        add(&format!("Mod+Alt+{key}"), None, format!("Focus pane {name}"), "Panes", true, None, fixed(C::FocusDir(dir)));
    }
    for (key, name, dir) in arrows {
        let win = format!("Ctrl+Alt+{key}");
        add(&format!("Mod+Shift+Alt+{key}"), Some(&win), format!("Swap pane {name}"), "Panes", true, None, fixed(C::SwapDir(dir)));
    }
    let resize = |axis, delta| C::Resize { target: focused(), axis, delta };
    add("Mod+Ctrl+Right", None, "Make pane wider".into(), "Layout", true, None, fixed(resize(SplitDir::Row, RESIZE_STEP)));
    add("Mod+Ctrl+Left", None, "Make pane narrower".into(), "Layout", true, None, fixed(resize(SplitDir::Row, -RESIZE_STEP)));
    add("Mod+Ctrl+Down", None, "Make pane taller".into(), "Layout", true, None, fixed(resize(SplitDir::Col, RESIZE_STEP)));
    add("Mod+Ctrl+Up", None, "Make pane shorter".into(), "Layout", true, None, fixed(resize(SplitDir::Col, -RESIZE_STEP)));
    add("Mod+Shift+O", Some("Ctrl+Shift+Alt+O"), "Overview of all workspaces".into(), "Workspaces", false, None, fixed(C::View(ViewTarget::Toggle)));
    add("Mod+Shift+Y", Some("Ctrl+Shift+Alt+Y"), "Archived sessions".into(), "Workspaces", false, None, fixed(C::View(ViewTarget::Set(View::Archive))));
    add("Mod+Shift+A", Some("Ctrl+Shift+Alt+A"), "Archive or restore pane".into(), "Panes", false, None, fixed(C::ToggleArchiveFocused));
    let overlay = |overlay| C::Overlay { overlay, provider: None };
    add("Mod+O", None, "Session progress".into(), "Panes", false, None, fixed(overlay(Overlay::Progress)));
    add("Mod+Shift+H", Some("Ctrl+Shift+Alt+H"), "History of merged features".into(), "Panes", false, None, fixed(overlay(Overlay::History)));
    add("Mod+Shift+E", Some("Ctrl+Shift+Alt+E"), "Open in code editor".into(), "Panes", false, None, fixed(C::EditorOpen { target: Some(focused()), editor: None }));
    add("Mod+E", None, "Review finished work".into(), "Panes", false, None, fixed(overlay(Overlay::Review)));
    add("Mod+J", None, "Next agent needing attention".into(), "Panes", false, None, fixed(C::NextAttention));
    add("Mod+Shift+I", Some("Ctrl+Shift+Alt+I"), "Broadcast to all agents".into(), "Panes", false, None, fixed(overlay(Overlay::Broadcast)));
    add("Mod+Shift+R", Some("Ctrl+Shift+Alt+R"), "Race mode".into(), "Panes", false, None, fixed(overlay(Overlay::Race)));
    add("Mod+Shift+.", Some("Ctrl+Shift+Alt+."), "Stop all agents".into(), "Panes", false, None, fixed(C::InterruptAll));
    add("Mod+.", None, "Interrupt agent".into(), "Panes", false, None, fixed(C::Interrupt(focused())));
    add("Mod+F", None, "Search scrollback".into(), "Panes", false, None, fixed(C::Search));
    add("Mod+Enter", None, "Zoom pane".into(), "Layout", false, None, fixed(C::Zoom(focused())));
    add("Mod+=", None, "Bigger text".into(), "Layout", true, None, fixed(C::TerminalZoom(1)));
    add("Mod+-", None, "Smaller text".into(), "Layout", true, None, fixed(C::TerminalZoom(-1)));
    add("Mod+0", None, "Default text size".into(), "Layout", false, None, fixed(C::TerminalZoom(0)));
    add("Mod+Ctrl+=", Some("Ctrl+Shift+Alt+="), "Equalize panes".into(), "Layout", false, None, fixed(C::LayoutEqualize));
    add("Mod+Ctrl+L", Some("Ctrl+Shift+Alt+L"), "Next layout preset".into(), "Layout", false, None, fixed(C::LayoutPreset(None)));
    for n in 1..=9 {
        let win = format!("Ctrl+Alt+{n}");
        add(&format!("Mod+Ctrl+{n}"), Some(&win), format!("Switch to workspace {n}"), "Workspaces", false, Some(n), fixed(C::WorkspaceSwitchIndex(n)));
    }
    add("Mod+K", None, "Command palette".into(), "App", false, None, fixed(C::Palette(None)));
    add("Mod+Alt+V", Some("Ctrl+Shift+Alt+V"), "Live voice conversation".into(), "App", false, None, fixed(C::LiveVoice));
    add("Mod+B", None, "Toggle sidebar".into(), "App", false, None, fixed(C::ToggleSidebar));
    add("Mod+Shift+B", Some("Ctrl+Shift+Alt+B"), "Toggle browser".into(), "App", false, None, fixed(C::ToggleBrowser));
    add("Ctrl+`", None, "Toggle side terminal".into(), "App", false, None, fixed(C::SidePanel { panel: SidePanel::Terminal, show: false }));
    add("Mod+Shift+C", Some("Ctrl+Shift+Alt+C"), "Pick element for agent".into(), "App", false, None, fixed(C::BrowserPick));
    add("Mod+L", None, "Browser address bar".into(), "App", false, None, fixed(C::BrowserFocusUrl));
    add("Mod+Shift+U", Some("Ctrl+Shift+Alt+U"), "Undock / dock browser".into(), "App", false, None, fixed(C::BrowserUndock));
    add("Mod+,", None, "Settings".into(), "App", false, None, fixed(C::Settings(None)));
    add("Mod+/", None, "Keyboard shortcuts".into(), "App", false, None, fixed(C::CheatSheet));
    out
});

const MOD_ORDER: [&str; 4] = ["Ctrl", "Alt", "Shift", "Meta"];

fn canonical(parts: &[String]) -> String {
    let mut out: Vec<String> = MOD_ORDER.iter().filter(|m| parts.iter().any(|p| p == *m)).map(|m| m.to_string()).collect();
    out.extend(parts.iter().filter(|p| !MOD_ORDER.contains(&p.as_str())).cloned());
    out.join("+")
}

fn default_chord(b: &Binding) -> String {
    let spec = if !IS_MAC { b.win.as_deref().unwrap_or(&b.keys) } else { &b.keys };
    let mut parts: Vec<String> = Vec::new();
    for p in spec.split('+') {
        let expanded: &[&str] = if p == "Mod" { if IS_MAC { &["Meta"] } else { &["Ctrl", "Shift"] } } else { std::slice::from_ref(&p) };
        for e in expanded {
            if !parts.iter().any(|x| x == e) {
                parts.push(e.to_string());
            }
        }
    }
    canonical(&parts)
}

/// The chord a binding currently uses: user override or default; `None` when switched off.
pub fn chord_for(b: &Binding, overrides: &HashMap<String, String>) -> Option<String> {
    match overrides.get(&b.id) {
        None => Some(default_chord(b)),
        Some(o) if o.is_empty() => None,
        Some(o) => Some(match b.digit {
            Some(d) => o.replace('#', &d.to_string()),
            None => o.clone(),
        }),
    }
}

/// A keystroke as a canonical chord ("Ctrl+Shift+T"), by its physical key.
pub fn event_chord(k: &Keystroke) -> Option<String> {
    let key = match k.key.as_str() {
        "left" => "Left".to_string(),
        "right" => "Right".to_string(),
        "up" => "Up".to_string(),
        "down" => "Down".to_string(),
        "enter" => "Enter".to_string(),
        "space" => "Space".to_string(),
        "escape" => "Escape".to_string(),
        "tab" => "Tab".to_string(),
        "backspace" => "Backspace".to_string(),
        "delete" => "Delete".to_string(),
        "home" => "Home".to_string(),
        "end" => "End".to_string(),
        "pageup" => "PageUp".to_string(),
        "pagedown" => "PageDown".to_string(),
        k if k.starts_with('f') && k.len() <= 3 && k[1..].chars().all(|c| c.is_ascii_digit()) && k.len() > 1 => k.to_uppercase(),
        k if k.chars().count() == 1 => k.to_uppercase(),
        _ => return None,
    };
    let m = k.modifiers;
    let mut parts: Vec<String> = Vec::new();
    for (on, name) in [(m.control, "Ctrl"), (m.alt, "Alt"), (m.shift, "Shift"), (m.platform, "Meta")] {
        if on {
            parts.push(name.to_string());
        }
    }
    parts.push(key);
    Some(canonical(&parts))
}

/// A canonical chord in this platform's notation.
pub fn format_chord(chord: &str) -> String {
    let chord = chord.replace('#', "1–9");
    if !IS_MAC {
        return chord.replace("Meta", "Win");
    }
    chord
        .split('+')
        .map(|p| match p {
            "Ctrl" => "⌃",
            "Alt" => "⌥",
            "Shift" => "⇧",
            "Meta" => "⌘",
            "Left" => "←",
            "Right" => "→",
            "Up" => "↑",
            "Down" => "↓",
            "Enter" => "↩",
            other => other,
        })
        .collect()
}

/// The shortcut shown next to a command, if a binding runs exactly it.
pub fn keys_for(cmd: &Command, state: &AppState) -> Option<String> {
    let binding = BINDINGS.iter().find(|b| (b.command)(state).as_ref() == Some(cmd))?;
    Some(format_chord(&chord_for(binding, &state.shortcuts)?))
}

/// Forces the split directly containing `id` into `dir`.
fn force_split_dir(node: &LayoutNode, id: &str, dir: SplitDir) -> LayoutNode {
    match node {
        LayoutNode::Leaf { .. } => node.clone(),
        LayoutNode::Split { dir: d, sizes, children } => {
            if children.iter().any(|c| matches!(c, LayoutNode::Leaf { pane_id } if pane_id == id)) {
                return LayoutNode::Split { dir, sizes: sizes.clone(), children: children.clone() };
            }
            LayoutNode::Split { dir: *d, sizes: sizes.clone(), children: children.iter().map(|c| force_split_dir(c, id, dir)).collect() }
        }
    }
}

#[derive(Clone)]
pub(super) struct Toast {
    pub text: SharedString,
    pub action: Option<(SharedString, Command)>,
}

impl Root {
    pub(super) fn update_state(&self, cx: &mut Context<Self>, f: impl FnOnce(&mut AppState)) {
        self.state.update(cx, |s, cx| {
            f(s);
            cx.notify();
        });
    }

    /// Applies a layout change to what's on screen: the active workspace or the overview.
    fn update_layout(&self, cx: &mut Context<Self>, f: impl FnOnce(&mut Option<LayoutNode>, &mut Option<String>, &mut usize)) {
        self.update_state(cx, |s| {
            if s.view() == View::Overview {
                let o = &mut s.overview;
                f(&mut o.layout, &mut o.zoomed_pane_id, &mut o.preset_index);
            } else if let Some(w) = s.active_mut() {
                f(&mut w.layout, &mut w.zoomed_pane_id, &mut w.preset_index);
            }
        });
    }

    /// "Focused" and positions refer to what's on screen; ids work in any workspace.
    pub(super) fn resolve_pane(&self, target: &PaneRef, cx: &App) -> Option<String> {
        let s = self.state.read(cx);
        let view = s.view();
        let ids: Vec<String> = if view == View::Archive {
            s.all_panes().filter(|p| p.archived).map(|p| p.id.clone()).collect()
        } else {
            let tree = if view == View::Overview { s.overview.layout.as_ref() } else { s.active().and_then(|w| w.layout.as_ref()) };
            tree.map(leaves).unwrap_or_default()
        };
        match target {
            PaneRef::Focused => s.focused_pane_id.clone().filter(|id| ids.contains(id)),
            PaneRef::Id(id) => s.pane(id).map(|_| id.clone()),
            PaneRef::Index(n) => ids.get(n.wrapping_sub(1)).cloned(),
        }
    }

    pub(super) fn type_into(&self, id: &str, text: &str, submit: bool, cx: &mut Context<Self>) {
        let Some(session) = SESSIONS.get(id) else { return };
        if text.contains('\n') {
            session.send(&format!("\x1b[200~{}\x1b[201~", text.replace("\r\n", "\r").replace('\n', "\r")));
        } else {
            session.send(text);
        }
        if submit {
            session.send("\r");
            status::set_task(&self.state, id, text, cx);
        }
    }

    fn pane_in_direction(&self, from: &str, dir: Direction, cx: &App) -> Option<String> {
        let s = self.state.read(cx);
        let tree = if s.view() == View::Overview { s.overview.layout.clone() } else { s.active().and_then(|w| w.layout.clone()) }?;
        let mut rects = Vec::new();
        place(&tree, self.tile_bounds, &mut Vec::new(), &mut rects, &mut Vec::new());
        let origin = rects.iter().find(|r| r.id == from)?.bounds;
        let (cx0, cy0) = (origin.center().x, origin.center().y);
        let score = |r: &Rect| {
            let b = r.bounds;
            let ok = match dir {
                Direction::Left => b.right() <= origin.left() + px(1.0),
                Direction::Right => b.left() >= origin.right() - px(1.0),
                Direction::Up => b.bottom() <= origin.top() + px(1.0),
                Direction::Down => b.top() >= origin.bottom() - px(1.0),
            };
            if !ok || r.id == from {
                return None;
            }
            let dx = (b.center().x - cx0).abs() / px(1.0);
            let dy = (b.center().y - cy0).abs() / px(1.0);
            Some(if matches!(dir, Direction::Left | Direction::Right) { dx + 2.0 * dy } else { dy + 2.0 * dx })
        };
        rects
            .iter()
            .filter_map(|r| score(r).map(|v| (v, r.id.clone())))
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, id)| id)
    }

    pub(super) fn toast(&mut self, text: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.push_toast(text.into(), None, cx);
    }

    pub(super) fn toast_action(&mut self, text: impl Into<SharedString>, label: &'static str, action: Command, cx: &mut Context<Self>) {
        self.push_toast(text.into(), Some((label.into(), action)), cx);
    }

    pub(super) fn toast_with_label(&mut self, text: impl Into<SharedString>, label: String, action: Command, cx: &mut Context<Self>) {
        self.push_toast(text.into(), Some((label.into(), action)), cx);
    }

    fn push_toast(&mut self, text: SharedString, action: Option<(SharedString, Command)>, cx: &mut Context<Self>) {
        let lifetime = if action.is_some() { 12 } else { 4 };
        self.toasts.push(Toast { text: text.clone(), action });
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_secs(lifetime)).await;
            let _ = this.update(cx, |this, cx| {
                if let Some(i) = this.toasts.iter().position(|t| t.text == text) {
                    this.toasts.remove(i);
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn worktree_of(&mut self, target: &PaneRef, cx: &mut Context<Self>) -> Option<Pane> {
        let id = self.resolve_pane(target, cx)?;
        let pane = self.state.read(cx).pane(&id).cloned()?;
        if pane.worktree.is_none() {
            self.toast("This pane isn't running in its own worktree.", cx);
            return None;
        }
        Some(pane)
    }

    /// Asks, in a native dialog; runs `then` if confirmed.
    pub(super) fn confirm(&self, title: &str, detail: String, ok: &'static str, window: &mut Window, cx: &mut Context<Self>, then: impl FnOnce(&mut Self, &mut Context<Self>) + 'static) {
        let answer = window.prompt(PromptLevel::Warning, title, Some(&detail), &[PromptButton::Ok(ok.into()), PromptButton::Cancel("Cancel".into())], cx);
        cx.spawn(async move |this, cx| {
            if answer.await == Ok(0) {
                let _ = this.update(cx, |this, cx| then(this, cx));
            }
        })
        .detach();
    }

    fn close_all_of(&mut self, ws: &str, cx: &mut Context<Self>) {
        let ids: Vec<String> = self.state.read(cx).workspaces.iter().find(|w| w.id == ws).map(|w| w.panes.iter().map(|p| p.id.clone()).collect()).unwrap_or_default();
        for id in ids {
            terminal::close(&id);
            self.terminals.remove(&id);
            self.forget_code(&id);
            self.code_choice.remove(&id);
        }
    }

    fn merge_pane(&mut self, pane: Pane, window: &mut Window, cx: &mut Context<Self>) {
        let info = pane.worktree.clone().unwrap();
        let (branch, base) = (info.branch.clone(), info.base.clone());
        let name = self.state.read(cx).provider_name(&pane.provider);
        let workspace = pane.workspace_id.clone();
        let commit_message = self.commit_message_for(&pane.id).unwrap_or_else(|| format!("Changes by {name}"));
        cx.spawn_in(window, async move |this, cx| {
            let probe = info.clone();
            let diff = tauri::async_runtime::spawn(async move { winfer_backend::worktree::worktree_diff(worktree_info(&probe)?).await }).await;
            let diff = diff.ok().and_then(Result::ok).and_then(|d| serde_json::to_value(d).ok());
            let dirty = diff.as_ref().is_some_and(|v| v["dirty"] == true);
            let stats = diff.as_ref().map(review::diff_stats);
            let (question, button) = if dirty {
                (format!("The agent left uncommitted changes in {branch}. Commit them and merge into {base}?"), "Commit and merge")
            } else {
                (format!("Merge {branch} into {base}?"), "Merge")
            };
            let Ok(answer) = cx.update(|window, cx| window.prompt(PromptLevel::Info, "Merge worktree", Some(&question), &[PromptButton::Ok(button.into()), PromptButton::Cancel("Cancel".into())], cx)) else { return };
            if answer.await != Ok(0) {
                return;
            }
            let _ = this.update(cx, |this, cx| this.toast(format!("Merging {branch} into {base}…"), cx));
            let result = tauri::async_runtime::spawn(async move {
                let info = worktree_info(&info)?;
                if dirty {
                    winfer_backend::worktree::worktree_commit(info.clone(), commit_message).await?;
                }
                winfer_backend::worktree::worktree_merge(info).await.map(|_| ())
            })
            .await;
            let _ = this.update(cx, |this, cx| match result {
                Ok(Ok(())) => {
                    this.toast(format!("Merged {branch} into {base}"), cx);
                    this.record_merge_of(&pane, stats, cx);
                    this.set_review_status_for(&pane.id, review::ReviewStatus::Merged, cx);
                    this.update_state(cx, |s| s.log_activity(&workspace, format!("Merged {branch} into {base}"), None));
                }
                Ok(Err(err)) => this.toast(format!("Merge not possible: {err}"), cx),
                Err(err) => this.toast(format!("Merge not possible: {err}"), cx),
            });
        })
        .detach();
    }

    fn discard_pane(&mut self, pane: Pane, window: &mut Window, cx: &mut Context<Self>) {
        let info = pane.worktree.clone().unwrap();
        let detail = format!("Close this pane, delete {} and the branch {}? Unmerged work is lost.", info.path, info.branch);
        self.confirm("Discard worktree", detail, "Discard", window, cx, move |this, cx| {
            let branch = info.branch.clone();
            this.close_pane(&pane.id, cx);
            this.set_review_status_for(&pane.id, review::ReviewStatus::Discarded, cx);
            this.update_state(cx, |s| s.log_activity(&pane.workspace_id, format!("Discarded {branch}"), None));
            this.toast(format!("Discarding {branch}…"), cx);
            cx.spawn(async move |this, cx| {
                let result = tauri::async_runtime::spawn(async move {
                    let info = worktree_info(&info)?;
                    winfer_backend::worktree::worktree_remove(info, true).await.map_err(|e| e.to_string())
                })
                .await;
                let _ = this.update(cx, |this, cx| match result {
                    Ok(Ok(_)) => this.toast(format!("Discarded {branch}"), cx),
                    Ok(Err(err)) => this.toast(format!("Couldn't remove worktree: {err}"), cx),
                    Err(err) => this.toast(format!("Couldn't remove worktree: {err}"), cx),
                });
            })
            .detach();
        });
    }

    pub(super) fn execute(&mut self, cmd: Command, window: &mut Window, cx: &mut Context<Self>) {
        use Command as C;
        match cmd {
            C::Spawn { provider, count, prompt, worktree, model } => {
                let provider = provider.unwrap_or_else(|| self.state.read(cx).default_provider_id());
                let count = count.max(1);
                self.update_state(cx, |s| {
                    for _ in 0..count {
                        s.add_pane(&provider, AddPane { prompt: prompt.clone(), worktree, model: model.clone(), ..Default::default() });
                    }
                    let name = s.provider_name(&provider);
                    let times = if count > 1 { format!("{count}× ") } else { String::new() };
                    let ws = s.active_workspace_id.clone();
                    s.log_activity(&ws, format!("Opened {times}{name}"), Some(provider.clone()));
                });
            }
            C::SetModel { target, model } => {
                let Some(pane) = self.resolve_pane(&target, cx).and_then(|id| self.state.read(cx).pane(&id).cloned()) else { return };
                let provider = self.state.read(cx).providers.iter().find(|p| p.id == pane.provider).cloned();
                let Some(provider) = provider.filter(|p| p.model_arg.is_some() && pane.model != model) else { return };
                match (&provider.model_command, &model) {
                    (Some(command), Some(m)) => {
                        if let Some(session) = SESSIONS.get(&pane.id) {
                            session.send(&format!("{}\r", command.replace("{model}", m)));
                        }
                        self.update_state(cx, |s| {
                            if let Some(p) = s.pane_mut(&pane.id) {
                                p.model = model.clone();
                            }
                        });
                    }
                    _ => {
                        self.update_state(cx, |s| {
                            if let Some(p) = s.pane_mut(&pane.id) {
                                p.model = model.clone();
                                p.resume = true;
                                p.prompt = None;
                            }
                        });
                        self.restart_pane(&pane.id, cx);
                    }
                }
            }
            C::Split { dir } => {
                let from = self.resolve_pane(&PaneRef::Focused, cx);
                let new_id = self.state.update(cx, |s, cx| {
                    let provider = s.default_provider_id();
                    let id = s.add_pane(&provider, AddPane { split_from: from.clone(), ..Default::default() });
                    cx.notify();
                    id
                });
                if from.is_some() {
                    self.update_state(cx, |s| {
                        if let Some(w) = s.active_mut() {
                            w.layout = w.layout.as_ref().map(|l| force_split_dir(l, &new_id, dir));
                        }
                    });
                }
            }
            C::Close(target) => {
                if let Some(id) = self.resolve_pane(&target, cx) {
                    self.close_pane(&id, cx);
                }
            }
            C::Focus(target) => {
                let Some(pane) = self.resolve_pane(&target, cx).and_then(|id| self.state.read(cx).pane(&id).cloned()) else { return };
                self.update_state(cx, |s| {
                    if pane.workspace_id != s.active_workspace_id {
                        s.set_active_workspace(&pane.workspace_id);
                    }
                    if pane.archived {
                        s.set_view(View::Archive);
                    } else if s.view() == View::Archive {
                        s.set_view(View::Workspace);
                    }
                    s.focus_pane(&pane.id);
                });
                self.focused_view = None;
            }
            C::FocusDir(dir) => {
                if let Some(to) = self.resolve_pane(&PaneRef::Focused, cx).and_then(|from| self.pane_in_direction(&from, dir, cx)) {
                    self.update_state(cx, |s| s.focus_pane(&to));
                    self.focused_view = None;
                }
            }
            C::SwapDir(dir) => {
                let Some(from) = self.resolve_pane(&PaneRef::Focused, cx) else { return };
                if let Some(to) = self.pane_in_direction(&from, dir, cx) {
                    self.update_state(cx, |s| {
                        if let Some(w) = s.active_mut() {
                            w.layout = w.layout.as_ref().map(|l| swap_leaves(l, &from, &to));
                        }
                    });
                }
            }
            C::Resize { target, axis, delta } => {
                if let Some(id) = self.resolve_pane(&target, cx) {
                    self.update_layout(cx, |layout, _, _| *layout = layout.as_ref().map(|l| resize_pane(l, &id, axis, delta)));
                }
            }
            C::Zoom(target) => {
                let Some(id) = self.resolve_pane(&target, cx).filter(|id| self.state.read(cx).pane(id).is_some_and(|p| !p.archived)) else { return };
                self.update_state(cx, |s| s.focus_pane(&id));
                self.focused_view = None;
                self.update_layout(cx, |_, zoomed, _| *zoomed = if zoomed.as_ref() == Some(&id) { None } else { Some(id.clone()) });
            }
            C::Archive { target, restore } => {
                let Some(pane) = self.resolve_pane(&target, cx).and_then(|id| self.state.read(cx).pane(&id).cloned()) else { return };
                if pane.archived != restore {
                    return;
                }
                let name = self.state.read(cx).provider_name(&pane.provider);
                self.update_state(cx, |s| {
                    if restore {
                        s.unarchive_pane(&pane.id);
                        s.log_activity(&pane.workspace_id, format!("Restored {name} from the archive"), Some(pane.provider.clone()));
                    } else {
                        s.archive_pane(&pane.id);
                        s.log_activity(&pane.workspace_id, format!("Archived {name}"), Some(pane.provider.clone()));
                    }
                });
                if restore {
                    self.execute(C::Focus(PaneRef::Id(pane.id)), window, cx);
                }
            }
            C::ToggleArchiveFocused => {
                let s = self.state.read(cx);
                let Some(restore) = s.focused_pane_id.as_deref().and_then(|id| s.pane(id)).map(|p| p.archived) else { return };
                self.execute(C::Archive { target: PaneRef::Focused, restore }, window, cx);
            }
            C::InterruptAll => {
                let panes: Vec<Pane> = self.state.read(cx).active().map(|w| w.panes.iter().filter(|p| !p.archived).cloned().collect()).unwrap_or_default();
                for pane in &panes {
                    if let Some(session) = SESSIONS.get(&pane.id) {
                        session.send(if pane.provider == "shell" { "\x03" } else { "\x1b" });
                    }
                }
                self.update_state(cx, |s| {
                    let ws = s.active_workspace_id.clone();
                    s.log_activity(&ws, format!("Stopped all {} sessions", panes.len()), None);
                });
            }
            C::CloseAll => {
                let ids: Vec<String> = self.state.read(cx).active().map(|w| w.panes.iter().filter(|p| !p.archived).map(|p| p.id.clone()).collect()).unwrap_or_default();
                for id in &ids {
                    self.close_pane(id, cx);
                }
            }
            C::Interrupt(target) => {
                let Some(pane) = self.resolve_pane(&target, cx).and_then(|id| self.state.read(cx).pane(&id).cloned()) else { return };
                if let Some(session) = SESSIONS.get(&pane.id) {
                    session.send(if pane.provider == "shell" { "\x03" } else { "\x1b" });
                }
            }
            C::Approve(target) => {
                let s = self.state.read(cx);
                let Some(pane) = self.resolve_pane(&target, cx).and_then(|id| s.pane(&id).cloned()) else { return };
                let input = s.providers.iter().find(|p| p.id == pane.provider).and_then(|p| p.approve_input.clone()).unwrap_or_else(|| "\r".to_string());
                if let Some(session) = SESSIONS.get(&pane.id) {
                    session.send(&input);
                }
            }
            C::Broadcast { text, submit, pane_ids } => {
                let targets = pane_ids.unwrap_or_else(|| {
                    self.state.read(cx).active().map(|w| w.panes.iter().filter(|p| !p.archived && p.provider != "shell").map(|p| p.id.clone()).collect()).unwrap_or_default()
                });
                for id in targets {
                    self.type_into(&id, &text, submit, cx);
                }
            }
            C::Search => {
                let id = self.resolve_pane(&PaneRef::Focused, cx);
                self.update_state(cx, |s| s.search_pane_id = if s.search_pane_id == id { None } else { id });
            }
            C::SendText { target, text, submit } => {
                if let Some(id) = self.resolve_pane(&target, cx) {
                    self.type_into(&id, &text, submit, cx);
                }
            }
            C::ShowChanges(target) => {
                if let Some(pane) = self.worktree_of(&target, cx) {
                    self.update_state(cx, |s| {
                        s.diff_pane_id = Some(pane.id.clone());
                        s.palette_query = None;
                        s.cheat_sheet_open = false;
                    });
                }
            }
            C::MergeWorktree(target) => {
                if let Some(pane) = self.worktree_of(&target, cx) {
                    self.merge_pane(pane, window, cx);
                }
            }
            C::DiscardWorktree(target) => {
                if let Some(pane) = self.worktree_of(&target, cx) {
                    self.discard_pane(pane, window, cx);
                }
            }
            C::EditorOpen { target, editor } => {
                let path = match target.and_then(|t| self.resolve_pane(&t, cx)).and_then(|id| self.state.read(cx).pane(&id).cloned()) {
                    Some(p) => p.worktree.map_or(p.cwd, |t| t.path),
                    None => self.state.read(cx).active().map_or_else(|| "~".to_string(), |w| w.root_dir.clone()),
                };
                let editor = editor.or_else(|| self.state.read(cx).editor.clone());
                self.open_in_editor(path, editor, cx);
            }
            C::NextAttention => self.next_attention(cx),
            C::LayoutEqualize => self.update_layout(cx, |layout, _, _| *layout = layout.as_ref().map(equalize)),
            C::LayoutPreset(preset) => self.update_layout(cx, |layout, zoomed, index| {
                *index = match preset {
                    Some(p) => PRESETS.iter().position(|x| *x == p).unwrap_or(0),
                    None => (*index + 1) % PRESETS.len(),
                };
                *zoomed = None;
                if let Some(ids) = layout.as_ref().map(leaves) {
                    *layout = build_preset(PRESETS[*index], &ids);
                }
            }),
            C::WorkspaceSwitchIndex(n) => {
                let id = self.state.read(cx).workspaces.get(n.wrapping_sub(1)).map(|w| w.id.clone());
                if let Some(id) = id {
                    self.execute(C::WorkspaceSwitch(id), window, cx);
                }
            }
            C::WorkspaceSwitch(id) => {
                self.update_state(cx, |s| {
                    s.set_active_workspace(&id);
                    s.set_view(View::Workspace);
                });
                self.focused_view = None;
            }
            C::WorkspaceCreate => {
                let paths = cx.prompt_for_paths(PathPromptOptions { files: false, directories: true, multiple: false, prompt: Some("Choose".into()) });
                cx.spawn(async move |this, cx| {
                    if let Ok(Ok(Some(paths))) = paths.await {
                        if let Some(dir) = paths.first() {
                            let path = dir.display().to_string();
                            let name = dir.file_name().map_or(path.clone(), |n| n.to_string_lossy().to_string());
                            let _ = this.update(cx, |this, cx| this.update_state(cx, |s| {
                                s.create_workspace(&name, &path);
                            }));
                        }
                    }
                })
                .detach();
            }
            C::WorkspaceRemove(id) => {
                let s = self.state.read(cx);
                let Some(ws) = s.workspaces.iter().find(|w| w.id == id) else { return };
                let panes = match ws.panes.len() {
                    0 => String::new(),
                    1 => " Its 1 pane will be closed.".to_string(),
                    n => format!(" Its {n} panes will be closed."),
                };
                let detail = format!("Remove workspace “{}” from winfer?{panes} Files and worktrees stay on disk.", ws.name);
                self.confirm("Remove workspace", detail, "Remove", window, cx, move |this, cx| {
                    this.close_all_of(&id, cx);
                    this.close_side_terminal(&id, cx);
                    this.update_state(cx, |s| s.remove_workspace(&id));
                    this.focused_view = None;
                });
            }
            C::ToggleWorktrees => self.update_state(cx, |s| {
                if let Some(w) = s.active_mut() {
                    w.use_worktrees = !w.use_worktrees;
                }
            }),
            C::BrowserOpen { url, new_tab } => {
                self.update_state(cx, |s| {
                    s.browser_open = true;
                    s.side_panel = Some(SidePanel::Browser);
                });
                self.browser.update(cx, |b, cx| b.open_url(url, new_tab, None, cx)).detach();
            }
            C::BrowserNewTab => self.browser.update(cx, |b, _| b.new_tab()),
            C::BrowserCloseTab => self.browser.update(cx, |b, _| b.close_current()),
            C::BrowserHistory(delta) => self.browser.update(cx, |b, cx| b.go(delta, cx)),
            C::BrowserPick => {
                self.update_state(cx, |s| {
                    s.browser_open = true;
                    s.side_panel = Some(SidePanel::Browser);
                });
                self.browser.update(cx, |b, cx| b.toggle_pick(cx));
            }
            C::LiveVoice => self.toggle_voice(window, cx),
            C::CheckUpdates => self.check_updates(false, cx),
            C::DownloadUpdate => self.download_update(cx),
            C::InstallUpdate => self.install_update(cx),
            C::ShowReleaseNotes(available) => self.show_release_notes(available, cx),
            C::BrowserUndock => self.toggle_browser_window(window, cx),
            C::BrowserFocusUrl => {
                self.update_state(cx, |s| {
                    s.browser_open = true;
                    s.side_panel = Some(SidePanel::Browser);
                });
                if let Some(handle) = self.browser_window {
                    // In its own window: bring that forward and focus the address bar there.
                    let browser = self.browser.clone();
                    cx.defer(move |cx| {
                        let _ = handle.update(cx, |_, window, cx| {
                            window.activate_window();
                            browser.update(cx, |b, cx| b.focus_address(window, cx));
                        });
                    });
                } else {
                    self.browser.update(cx, |b, cx| b.focus_address(window, cx));
                }
            }
            C::ToggleSidebar => self.update_state(cx, |s| s.sidebar_open = !s.sidebar_open),
            C::ToggleBrowser => self.update_state(cx, |s| s.browser_open = !s.browser_open),
            C::SidePanel { panel, show } => self.update_state(cx, |s| {
                if !show && s.browser_open && s.side_panel() == panel {
                    s.browser_open = false;
                } else {
                    s.side_panel = Some(panel);
                    s.browser_open = true;
                }
            }),
            C::Palette(query) => {
                if self.state.read(cx).palette_query.is_some() {
                    self.close_palette(cx);
                } else {
                    self.open_palette(query.unwrap_or_default(), window, cx);
                }
            }
            C::CheatSheet => self.update_state(cx, |s| {
                s.cheat_sheet_open = !s.cheat_sheet_open;
                if s.cheat_sheet_open {
                    s.palette_query = None;
                    s.settings_open = false;
                    s.overlay = Overlay::None;
                }
            }),
            C::Settings(page) => self.update_state(cx, |s| match page {
                Some(page) => {
                    s.settings_open = true;
                    s.settings_page = page;
                }
                None => s.settings_open = !s.settings_open,
            }),
            // Choosing a theme ends following the system and becomes the pick for its mode.
            C::Theme(id) => self.update_state(cx, |s| {
                s.theme_mode = "manual".into();
                s.pick_theme(&id, false);
            }),
            C::Overlay { overlay, provider } => {
                let missing = match overlay {
                    _ => None,
                };
                if let Some(name) = missing {
                    return self.execute(C::Unavailable(name), window, cx);
                }
                let s = self.state.read(cx);
                let without_worktrees = s.active().is_some_and(|w| !w.use_worktrees);
                if overlay == Overlay::NewPane && s.overlay != Overlay::NewPane && without_worktrees {
                    return self.execute(C::Spawn { provider, count: 1, prompt: None, worktree: None, model: None }, window, cx);
                }
                let same = s.overlay == overlay && (overlay != Overlay::NewPane || s.new_pane_provider == provider);
                self.update_state(cx, |s| {
                    if same {
                        s.overlay = Overlay::None;
                        s.new_pane_provider = None;
                    } else {
                        s.overlay = overlay;
                        s.new_pane_provider = provider;
                        s.palette_query = None;
                        s.cheat_sheet_open = false;
                    }
                });
            }
            C::TerminalZoom(delta) => self.update_state(cx, |s| {
                let size = if delta == 0 { crate::model::DEFAULT_TERMINAL_FONT_SIZE } else { s.terminal_font_size + delta as f64 };
                s.set_terminal_font_size(size);
            }),
            C::View(target) => self.update_state(cx, |s| {
                let next = match target {
                    ViewTarget::Set(v) => v,
                    ViewTarget::Toggle if s.view() == View::Overview => View::Workspace,
                    ViewTarget::Toggle => View::Overview,
                };
                s.set_view(next);
            }),
            C::RaceResults(race) => self.open_race_results(race, cx),
            C::HandOff { pane, to } => self.continue_in(&pane, &to, cx),
            C::RedeemCodexReset => self.redeem_codex_reset(cx),
            C::PluginTool { id, args } => self.run_plugin_tool(&id, args, window, cx),
            C::RecipeLoad(id) => self.load_recipe(&id, window, cx),
            C::Unavailable(name) => self.toast(format!("{name} isn't in the native build yet."), cx),
        }
        cx.notify();
    }

    fn next_attention(&mut self, cx: &mut Context<Self>) {
        let s = self.state.read(cx);
        let all: Vec<Pane> = s.workspaces.iter().flat_map(|w| w.layout.as_ref().map(leaves).unwrap_or_default().into_iter().filter_map(|id| w.panes.iter().find(|p| p.id == id).cloned())).collect();
        let focused = s.focused_pane_id.clone();
        let rank = |p: &Pane| if p.status == AgentStatus::NeedsInput { 0 } else if p.unseen { 1 } else { 2 };
        let start = all.iter().position(|p| Some(&p.id) == focused.as_ref()).map_or(-1, |i| i as i64);
        let len = all.len().max(1) as i64;
        let distance = |i: usize| {
            let d = (i as i64 - start).rem_euclid(len);
            if d == 0 { len } else { d }
        };
        let next = all.iter().enumerate().filter(|(_, p)| rank(p) < 2 && Some(&p.id) != focused.as_ref()).min_by_key(|(i, p)| (rank(p), distance(*i))).map(|(_, p)| p.clone());
        let Some(next) = next else { return };
        self.update_state(cx, |s| {
            if next.workspace_id != s.active_workspace_id {
                s.set_active_workspace(&next.workspace_id);
            }
            s.focus_pane(&next.id);
        });
        self.focused_view = None;
    }

    pub(super) fn open_in_editor(&mut self, path: String, editor: Option<String>, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let result = tauri::async_runtime::spawn(async move {
                let list = winfer_backend::editors::editors_list().await;
                let list = serde_json::to_value(list).unwrap_or(Value::Null);
                let chosen = editor.or_else(|| list.as_array()?.first()?["id"].as_str().map(str::to_string)).ok_or_else(|| "No code editor found.".to_string())?;
                winfer_backend::editors::editor_open(chosen, path).await.map_err(|e| e.to_string())
            })
            .await;
            let failure = match result {
                Ok(Ok(_)) => None,
                Ok(Err(err)) => Some(err),
                Err(err) => Some(err.to_string()),
            };
            if let Some(err) = failure {
                let _ = this.update(cx, |this, cx| this.toast(err, cx));
            }
        })
        .detach();
    }

    /// Handles an app shortcut; true if the key press was one.
    pub(super) fn handle_shortcut(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(chord) = event_chord(&event.keystroke) else { return false };
        let s = self.state.read(cx);
        // ⌘1–9 pick a model while a chat pane's model picker is open.
        if chord.len() == 6 && chord.starts_with("Meta+") && chord.ends_with(|c: char| c.is_ascii_digit()) {
            let picker_open = s.focused_pane_id.as_ref().and_then(|id| self.code_models.get(id)).is_some_and(|m| m.read(cx).state.sheet == Some(crate::code::state::Sheet::Model));
            if picker_open {
                return false;
            }
        }
        let Some(binding) = BINDINGS.iter().find(|b| chord_for(b, &s.shortcuts).as_deref() == Some(chord.as_str())) else { return false };
        if event.is_held && !binding.repeat {
            return true;
        }
        if let Some(cmd) = (binding.command)(s) {
            self.execute(cmd, window, cx);
        }
        true
    }
}

/// The backend's type for a pane's worktree (its fields are private; same JSON).
pub(super) fn worktree_info(info: &crate::model::WorktreeInfo) -> Result<winfer_backend::worktree::WorktreeInfo, String> {
    serde_json::to_value(info).and_then(serde_json::from_value).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::{BINDINGS, canonical, chord_for, event_chord, format_chord, slug};
    use gpui_kit::{Keystroke, Modifiers};
    use std::collections::HashMap;

    #[test]
    fn binding_ids_match_the_tauri_app() {
        assert_eq!(slug("New pane (default agent)"), "new-pane-default-agent");
        let focus: Vec<_> = BINDINGS.iter().filter(|b| b.id == "focus-pane").collect();
        assert_eq!(focus.len(), 9);
        assert_eq!(focus[2].digit, Some(3));
    }

    #[test]
    fn chords_are_canonical() {
        let parts: Vec<String> = ["Meta", "T", "Shift"].map(String::from).to_vec();
        assert_eq!(canonical(&parts), "Shift+Meta+T");
        let stroke = Keystroke { modifiers: Modifiers { platform: true, shift: true, ..Default::default() }, key: "t".into(), key_char: None };
        assert_eq!(event_chord(&stroke).as_deref(), Some("Shift+Meta+T"));
    }

    #[test]
    fn every_default_chord_is_unique_and_typeable() {
        let mut seen = std::collections::HashSet::new();
        for b in BINDINGS.iter() {
            let chord = chord_for(b, &HashMap::new()).unwrap();
            let key = chord.rsplit('+').next().filter(|k| !k.is_empty()).unwrap_or("+");
            assert!(!key.is_empty(), "{} has no key", b.title);
            // The chord a keystroke produces must be the chord the binding listens for.
            let (key_name, mods) = (chord.rsplit_once('+').map_or(chord.as_str(), |(_, k)| k), &chord);
            let stroke = Keystroke {
                modifiers: Modifiers { control: mods.contains("Ctrl"), alt: mods.contains("Alt"), shift: mods.contains("Shift"), platform: mods.contains("Meta"), ..Default::default() },
                key: match key_name { "Left" => "left", "Right" => "right", "Up" => "up", "Down" => "down", "Enter" => "enter", k => k }.to_lowercase(),
                key_char: None,
            };
            assert_eq!(event_chord(&stroke).as_deref(), Some(chord.as_str()), "{}", b.title);
            assert!(seen.insert(chord.clone()), "{chord} is bound twice ({})", b.title);
        }
    }

    #[test]
    fn overrides_apply_and_switch_off() {
        let b = BINDINGS.iter().find(|b| b.id == "focus-pane" && b.digit == Some(2)).unwrap();
        let mut o = HashMap::new();
        o.insert("focus-pane".to_string(), "Ctrl+#".to_string());
        assert_eq!(chord_for(b, &o).as_deref(), Some("Ctrl+2"));
        o.insert("focus-pane".to_string(), String::new());
        assert_eq!(chord_for(b, &o), None);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn mac_notation() {
        assert_eq!(format_chord("Ctrl+Alt+Shift+Meta+Left"), "⌃⌥⇧⌘←");
    }
}
