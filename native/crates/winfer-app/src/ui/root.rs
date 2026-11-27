//! The main window (port of `src/app/AppShell.tsx`): title bar, sidebar, tiled panes, side panel.

use crate::backend;
use crate::code::session::CodeModel;
use crate::ui::code_view::{CodeView, OpenLink};
use crate::model::layout::{self, DropSide, LayoutNode, SplitDir, leaves};
use crate::model::{AddPane, AgentStatus, AppState, Pane, SidePanel, View};
use crate::status;
use crate::terminal::{self, Event as SessionEvent, Events, SESSIONS};
use crate::theme::{self, Colors};
use crate::ui::browser::{BrowserPanel, ToggleWindow};
use crate::ui::viewer::ViewerPanel;
use crate::ui::android::AndroidPanel;
use crate::ui::terminal_view::{self, TerminalView};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use gpui_kit::assets::IconName;
use gpui_kit::component::{Icon, Theme as ComponentTheme, ThemeMode};
use serde_json::Value;
use std::collections::HashMap;
use std::time::Duration;
use tauri::Manager;
use winfer_backend::{providers, store};

#[path = "commands.rs"]
pub mod commands;
#[path = "agent_api.rs"]
mod agent_api;
#[path = "overlays.rs"]
mod overlays;
#[path = "welcome.rs"]
mod welcome;
#[path = "dialogs.rs"]
mod dialogs;
#[path = "settings.rs"]
mod settings;
#[path = "review.rs"]
mod review;
#[path = "usage.rs"]
pub(crate) mod usage;
#[path = "new_menu.rs"]
mod new_menu;
#[path = "pane_note.rs"]
mod pane_note;
#[path = "orchestration.rs"]
mod orchestration;
#[path = "progress.rs"]
mod progress;
#[path = "plugins.rs"]
mod plugins;
#[path = "menu.rs"]
mod menu;
#[path = "title_swap.rs"]
mod title_swap;
#[path = "sidebar.rs"]
mod sidebar;
#[path = "pane_header.rs"]
pub(super) mod pane_header;
#[path = "worktrees.rs"]
mod worktrees;
#[path = "accounts.rs"]
mod accounts;
#[path = "usage_page.rs"]
mod usage_page;
#[path = "settings_more.rs"]
mod settings_more;
#[path = "settings_resets.rs"]
pub mod settings_resets;
#[path = "settings_plugins.rs"]
mod settings_plugins;
#[path = "settings_skills.rs"]
mod settings_skills;
#[path = "settings_harness.rs"]
mod settings_harness;
#[path = "settings_connect.rs"]
mod settings_connect;
#[path = "connect_ui.rs"]
mod connect_ui;
#[path = "connect_folder.rs"]
mod connect_folder;
#[path = "settings_code.rs"]
mod settings_code;
#[path = "settings_kit.rs"]
mod settings_kit;
#[path = "settings_agents.rs"]
mod settings_agents;
#[path = "settings_system.rs"]
mod settings_system;
#[path = "model_logos.rs"]
mod model_logos;
#[path = "mobile.rs"]
mod mobile;
#[path = "mobile_panel.rs"]
mod mobile_panel;
#[path = "updates_ui.rs"]
mod updates_ui;
#[path = "voice_tools.rs"]
pub mod voice_tools;
#[path = "voice_ui.rs"]
mod voice_ui;
#[path = "home.rs"]
mod home;
#[path = "home_follow.rs"]
mod home_follow;
#[path = "home_view.rs"]
mod home_view;

use agent_api::PendingQuestions;
use commands::Command;
use overlays::Entry;

actions!(winfer, [ToggleSidePanel, Quit, Hide, HideOthers, Minimize, Zoom]);

const GAP: f32 = 8.0;
const MIN_SIDEBAR: f32 = 180.0;
const MAX_SIDEBAR: f32 = 520.0;
const MIN_SIDE_PANEL: f32 = 280.0;

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("cmd-q", Quit, None),
        KeyBinding::new("cmd-h", Hide, None),
        KeyBinding::new("cmd-alt-h", HideOthers, None),
        KeyBinding::new("cmd-m", Minimize, None),
        KeyBinding::new("cmd-c", terminal_view::Copy, Some("Terminal")),
        KeyBinding::new("cmd-v", terminal_view::Paste, Some("Terminal")),
        KeyBinding::new("tab", terminal_view::SendTab, Some("Terminal")),
        KeyBinding::new("shift-tab", terminal_view::SendBackTab, Some("Terminal")),
    ]);
    cx.on_action(|_: &Quit, cx| cx.quit());
    cx.on_action(|_: &Hide, cx| cx.hide());
    cx.on_action(|_: &HideOthers, cx| cx.hide_other_apps());
    cx.on_action(|_: &Minimize, cx| {
        if let Some(window) = cx.active_window() {
            let _ = window.update(cx, |_, window, _| window.minimize_window());
        }
    });
    cx.on_action(|_: &Zoom, cx| {
        if let Some(window) = cx.active_window() {
            let _ = window.update(cx, |_, window, _| window.zoom_window());
        }
    });
    // The macOS menu bar: the standard app and window items (Hide, Minimize, Quit) and their keys.
    cx.set_menus(vec![
        Menu::new("winfer").items([
            MenuItem::os_submenu("Services", SystemMenuType::Services),
            MenuItem::separator(),
            MenuItem::action("Hide winfer", Hide),
            MenuItem::action("Hide Others", HideOthers),
            MenuItem::separator(),
            MenuItem::action("Quit winfer", Quit),
        ]),
        // Without an Edit menu, macOS never sends cut:/copy:/paste:/selectAll: to a focused WKWebView
        // (the plugin pages), so Cmd+V etc. do nothing there. GPUI inputs get the same items as actions.
        Menu::new("Edit").items({
            use gpui_kit::component::input;
            [
                MenuItem::os_action("Undo", input::Undo, OsAction::Undo),
                MenuItem::os_action("Redo", input::Redo, OsAction::Redo),
                MenuItem::separator(),
                MenuItem::os_action("Cut", input::Cut, OsAction::Cut),
                MenuItem::os_action("Copy", input::Copy, OsAction::Copy),
                MenuItem::os_action("Paste", input::Paste, OsAction::Paste),
                MenuItem::os_action("Select All", input::SelectAll, OsAction::SelectAll),
            ]
        }),
        Menu::new("Window").items([MenuItem::action("Minimize", Minimize), MenuItem::action("Zoom", Zoom)]),
    ]);
}

pub fn open_main_window(cx: &mut App) {
    let bounds = Bounds::centered(None, size(px(1440.0), px(900.0)), cx);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: Some(TitlebarOptions {
            title: Some("winfer".into()),
            appears_transparent: true,
            traffic_light_position: Some(point(px(12.0), px(12.0))),
        }),
        ..Default::default()
    };
    cx.open_window(options, |window, cx| {
        let view = cx.new(|cx| Root::new(window, cx));
        cx.new(|cx| gpui_kit::component::Root::new(view, window, cx))
    })
    .expect("failed to open window");
    cx.activate(true);
}

/// The browser panel as the content of its own window.
struct UndockedBrowser {
    browser: Entity<BrowserPanel>,
    state: Entity<AppState>,
    root: WeakEntity<Root>,
    /// The main window, where app shortcuts pressed here are run.
    main: AnyWindowHandle,
}

impl Render for UndockedBrowser {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = theme::colors(&self.state.read(cx).theme);
        div()
            .size_full()
            .bg(c.panel)
            .text_color(c.fg)
            .font_family(crate::fonts::sans(cx))
            // App shortcuts (⌘L, ⌘⇧U, ⌘⇧C, …) work from this window too.
            .capture_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                let root = this.root.clone();
                let handled = this.main.update(cx, |_, window, cx| root.update(cx, |r, cx| r.handle_shortcut(event, window, cx)).unwrap_or(false));
                if handled.unwrap_or(false) {
                    cx.stop_propagation();
                }
            }))
            .child(self.browser.clone())
    }
}

/// Where the undocked browser window was last, kept across launches (like the Tauri app's).
mod browser_bounds {
    use gpui_kit::*;
    use serde::{Deserialize, Serialize};
    use tauri::Manager;

    /// Logical (point) coordinates.
    #[derive(Serialize, Deserialize)]
    struct Saved {
        x: f32,
        y: f32,
        width: f32,
        height: f32,
    }

    fn path() -> Option<std::path::PathBuf> {
        crate::backend::app().path().app_data_dir().ok().map(|d| d.join("browser-window.json"))
    }

    /// The saved place, if it's still on a connected screen.
    pub fn load(cx: &App) -> Option<Bounds<Pixels>> {
        let b: Saved = serde_json::from_slice(&std::fs::read(path()?).ok()?).ok()?;
        if !(b.width >= 100.0 && b.height >= 100.0 && b.x.is_finite() && b.y.is_finite()) {
            return None;
        }
        let bounds = Bounds::new(point(px(b.x), px(b.y)), size(px(b.width), px(b.height)));
        cx.displays().iter().any(|d| d.bounds().intersects(&bounds)).then_some(bounds)
    }

    pub fn save(window: &Window) {
        let b = window.window_bounds().get_bounds();
        let saved = Saved { x: f32::from(b.origin.x), y: f32::from(b.origin.y), width: f32::from(b.size.width), height: f32::from(b.size.height) };
        if saved.width < 100.0 || saved.height < 100.0 {
            return;
        }
        if let (Some(path), Ok(json)) = (path(), serde_json::to_vec(&saved)) {
            let _ = std::fs::write(path, json);
        }
    }
}

struct Rect {
    id: String,
    bounds: Bounds<Pixels>,
}

struct Gutter {
    path: Vec<usize>,
    index: usize,
    dir: SplitDir,
    bounds: Bounds<Pixels>,
    span: f32,
}

struct GutterDrag {
    path: Vec<usize>,
    index: usize,
    dir: SplitDir,
    span: f32,
    start: Point<Pixels>,
    sizes: Vec<f64>,
}

const PANE_DRAG_THRESHOLD: f32 = 6.0;

enum Drag {
    Sidebar,
    SidePanel,
    Gutter(GutterDrag),
    Pane { id: String, start: Point<Pixels>, at: Point<Pixels>, active: bool },
}

fn place(node: &LayoutNode, bounds: Bounds<Pixels>, path: &mut Vec<usize>, rects: &mut Vec<Rect>, gutters: &mut Vec<Gutter>) {
    match node {
        LayoutNode::Leaf { pane_id } => rects.push(Rect { id: pane_id.clone(), bounds }),
        LayoutNode::Split { dir, sizes, children } => {
            let gaps = GAP * (children.len() as f32 - 1.0);
            let span = if *dir == SplitDir::Row { bounds.size.width } else { bounds.size.height } / px(1.0) - gaps;
            let mut offset = 0.0;
            for (i, child) in children.iter().enumerate() {
                let len = span * sizes[i] as f32;
                let child_bounds = if *dir == SplitDir::Row {
                    Bounds::new(point(bounds.origin.x + px(offset), bounds.origin.y), size(px(len), bounds.size.height))
                } else {
                    Bounds::new(point(bounds.origin.x, bounds.origin.y + px(offset)), size(bounds.size.width, px(len)))
                };
                path.push(i);
                place(child, child_bounds, path, rects, gutters);
                path.pop();
                if i + 1 < children.len() {
                    let gutter = if *dir == SplitDir::Row {
                        Bounds::new(point(child_bounds.right(), bounds.origin.y), size(px(GAP), bounds.size.height))
                    } else {
                        Bounds::new(point(bounds.origin.x, child_bounds.bottom()), size(bounds.size.width, px(GAP)))
                    };
                    gutters.push(Gutter { path: path.clone(), index: i, dir: *dir, bounds: gutter, span });
                }
                offset += len + GAP;
            }
        }
    }
}

fn drop_side(bounds: Bounds<Pixels>, at: Point<Pixels>) -> DropSide {
    let x = (at.x - bounds.left()) / bounds.size.width;
    let y = (at.y - bounds.top()) / bounds.size.height;
    let edge = 0.25;
    if x < edge {
        DropSide::Left
    } else if x > 1.0 - edge {
        DropSide::Right
    } else if y < edge {
        DropSide::Top
    } else if y > 1.0 - edge {
        DropSide::Bottom
    } else {
        DropSide::Center
    }
}

fn status_color(status: AgentStatus, c: &Colors) -> Hsla {
    match status {
        AgentStatus::Idle => c.subtle,
        AgentStatus::Working => c.success,
        AgentStatus::NeedsInput => c.warning,
        AgentStatus::Done => c.accent,
        AgentStatus::Error => c.danger,
    }
}

pub struct Root {
    state: Entity<AppState>,
    events: Events,
    terminals: HashMap<String, Entity<TerminalView>>,
    /// winfer code panes shown as a chat: the process side and the view, and which panes chose it.
    code_models: HashMap<String, Entity<CodeModel>>,
    code_views: HashMap<String, Entity<CodeView>>,
    code_choice: HashMap<String, bool>,
    /// The browser panel's own window, while it's undocked.
    browser_window: Option<AnyWindowHandle>,
    updates: updates_ui::UpdatesUi,
    /// Settings → Diagnostics: the list of usage data sent (telemetry.rs).
    pub(super) usage_info_open: bool,
    voice: voice_ui::VoiceUi,
    /// Live voice's orchestrators, one per workspace that has used one.
    home: home::Home,
    /// The tile area's bounds as of the last frame.
    tile_bounds: Bounds<Pixels>,
    drag: Option<Drag>,
    save_task: Option<Task<()>>,
    last_saved: String,
    focused_view: Option<String>,
    /// Holds the keyboard when no pane or field does, so app shortcuts always reach `handle_shortcut`.
    focus: FocusHandle,
    /// Tracks the main workspace area so modals can pull focus out of panes behind them.
    workspace_focus: FocusHandle,
    focus_outside: u8,
    browser: Entity<BrowserPanel>,
    viewer: Entity<ViewerPanel>,
    android: Entity<AndroidPanel>,
    side_open: Vec<String>,
    side_shown: Option<String>,
    applied_theme: Option<String>,
    toasts: Vec<commands::Toast>,
    questions: std::collections::VecDeque<PendingQuestions>,
    question_serial: u64,
    question_input: Option<Entity<gpui_kit::component::input::InputState>>,
    question_picked: Vec<usize>,
    palette_input: Option<Entity<gpui_kit::component::input::InputState>>,
    palette_entries: Vec<Entry>,
    palette_selected: usize,
    bg_notes: HashMap<String, Vec<String>>,
    bg_tasks: HashMap<String, Vec<pane_header::BgTask>>,
    bg_popover: Option<(String, Point<Pixels>)>,
    bg_ticking: bool,
    #[cfg(debug_assertions)]
    dev_layer_opened: bool,
    usage: usage::UsageStore,
    wakes_loaded: bool,
    review: review::ReviewStore,
    review_opened: bool,
    history: Option<review::HistoryUi>,
    settings: Option<settings::SettingsUi>,
    recording: Option<String>,
    suspend_saving: bool,
    orchestration: orchestration::Orchestration,
    progress_ui: progress::ProgressUi,
    plugins: plugins::PluginHost,
    menu: Option<menu::OpenMenu>,
    sidebar: sidebar::SidebarUi,
    meters: pane_header::PaneMeters,
    search: Option<pane_header::SearchBox>,
    worktrees: worktrees::WorktreesUi,
    failover: accounts::Failover,
    usage_page: usage_page::UsagePage,
    more: settings_more::MoreSettings,
    plugin_page: settings_plugins::PluginsPage,
    skill_page: settings_skills::SkillsPage,
    harness_page: settings_harness::HarnessPage,
    connect_page: settings_connect::ConnectPage,
    connect: connect_ui::ConnectClient,
    mobile: mobile::MobileBridge,
    new_pane: Option<dialogs::NewPaneDialog>,
    model_dialog: Option<dialogs::ModelDialog>,
    diff_dialog: Option<dialogs::DiffDialog>,
    models: HashMap<String, Vec<dialogs::ModelInfo>>,
    new_menu: new_menu::NewMenu,
    pane_note: Option<pane_note::NoteUi>,
    welcome: welcome::WelcomeUi,
}

impl Root {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        // The system switching between light and dark re-renders, which re-checks the theme.
        cx.observe_window_appearance(window, |_, _, cx| cx.notify()).detach();
        let (tx, rx) = async_channel::unbounded::<SessionEvent>();
        let state = cx.new(|_| AppState::new());
        cx.observe(&state, |this, _, cx| {
            this.schedule_save(cx);
            cx.notify();
        })
        .detach();

        let forward = tx.clone();
        backend::app().listen_any(move |event, payload| {
            if matches!(event, "pty://exit" | "agent://status" | "notification://action" | "mobile://request" | "mobile://changed" | "connect://changed" | "connect://peer" | "connect://message" | "mcp://call" | "bg://note" | "bg://task" | "usage://report") || event.starts_with("browser://") {
                let _ = forward.try_send(SessionEvent::Backend(event.to_string(), payload.clone()));
            }
        });

        cx.spawn_in(window, async move |this, cx| {
            while let Ok(event) = rx.recv().await {
                if this.update_in(cx, |this, window, cx| this.on_session_event(event, window, cx)).is_err() {
                    break;
                }
            }
        })
        .detach();

        // Winfer Connect's once-a-second chores (with a window, to open tiles).
        cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                if this.update_in(cx, |this, window, cx| this.tick_connect(window, cx)).is_err() {
                    break;
                }
            }
        })
        .detach();

        let ticker = state.clone();
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                let finished = cx.update(|cx| status::tick(&ticker, cx));
                if this.update(cx, |this, cx| {
                    for id in &finished {
                        this.agent_finished(id, cx);
                    }
                    this.tick_progress(cx);
                    this.tick_meters(cx);
                    this.tick_worktrees(cx);
                    this.tick_failover(cx);
                    this.deliver_bg_notes(cx);
                    this.tick_home(cx);
                    this.expire_questions(cx);
                    this.tick_phone_panel(cx);
                }).is_err() {
                    break;
                }
            }
        })
        .detach();

        let loading = state.clone();
        cx.spawn(async move |_, cx| {
            let snapshot = backend::background(|| store::state_load(backend::app().state()).map(|s| serde_json::to_value(s).unwrap_or(Value::Null))).await;
            let snapshot = snapshot.ok().and_then(Result::ok).unwrap_or(Value::Null);
            let _ = loading.update(cx, |s, cx| {
                s.hydrate(snapshot);
                theme::set_palette(&s.theme);
                // Development aid: open panes running these providers at launch.
                // Development aid: open (and switch to) a workspace for this folder.
                #[cfg(debug_assertions)]
                if let Ok(dir) = std::env::var("WINFER_NATIVE_WORKSPACE") {
                    let name = dir.rsplit('/').find(|p| !p.is_empty()).unwrap_or("workspace").to_string();
                    let id = s.create_workspace(&name, &dir);
                    s.set_active_workspace(&id);
                    if !s.expanded_workspaces.contains(&id) {
                        s.expanded_workspaces.push(id);
                    }
                }
                #[cfg(debug_assertions)]
                if let Ok(list) = std::env::var("WINFER_NATIVE_SPAWN") {
                    for provider in list.split(',').filter(|p| !p.is_empty()) {
                        s.add_pane(provider, AddPane { prompt: std::env::var("WINFER_NATIVE_PROMPT").ok(), ..AddPane::default() });
                    }
                }
                // Development aid: give the spawned panes these statuses (working:64:Writing tests,
                // needs::Run prisma migrate?, done:100, …) to check how the UI shows them.
                #[cfg(debug_assertions)]
                if let Ok(list) = std::env::var("WINFER_NATIVE_DEMO_STATUS") {
                    let ids: Vec<String> = s.workspaces.iter().find(|w| w.id == s.active_workspace_id).map(|w| w.panes.iter().map(|p| p.id.clone()).collect()).unwrap_or_default();
                    for (id, spec) in ids.iter().zip(list.split(',')) {
                        let mut parts = spec.splitn(3, ':');
                        let status = match parts.next() { Some("working") => AgentStatus::Working, Some("needs") => AgentStatus::NeedsInput, Some("done") => AgentStatus::Done, Some("error") => AgentStatus::Error, _ => AgentStatus::Idle };
                        let percent = parts.next().and_then(|p| p.parse().ok());
                        let step = parts.next().map(str::to_string);
                        if let Some(p) = s.workspaces.iter_mut().flat_map(|w| w.panes.iter_mut()).find(|p| p.id == *id) {
                            p.status = status;
                        }
                        let e = s.progress.entry(id.clone()).or_default();
                        e.percent = percent;
                        e.step = step;
                    }
                }
                cx.notify();
            });
            crate::telemetry::app_ready();
            let _ = loading.read_with(cx, |s, _| {
                crate::telemetry::settings_drift(&s.snapshot()["ui"], &crate::model::AppState::default_ui());
                crate::telemetry::workspace_size(crate::telemetry::WorkspaceSize {
                    workspaces: s.workspaces.len(),
                    panes: s.all_panes().count(),
                    max_panes: s.workspaces.iter().map(|w| w.panes.len()).max().unwrap_or(0),
                });
            });
            let found = tauri::async_runtime::spawn(providers::providers_list()).await;
            if let Ok(found) = found {
                let found = found.into_iter().map(|p| crate::model::AgentProvider { info: p.provider, installed: p.installed }).collect();
                let _ = loading.update(cx, |s, cx| {
                    s.set_providers(found);
                    cx.notify();
                });
            }
        })
        .detach();

        cx.defer_in(window, |this, _, cx| {
            this.start_usage(cx);
            this.start_updates(cx);
            if crate::telemetry::take_first_run_notice() {
                this.toast_action("winfer sends anonymous usage data (no prompts, code or paths) to improve the app", "Settings", Command::Settings(Some("diagnostics".into())), cx);
            }
            this.reload_plugins(cx);
        });
        window.activate_window();
        let panel_state = state.clone();
        let browser = cx.new(|cx| BrowserPanel::new(panel_state, window, cx));
        cx.subscribe_in(&browser, window, |this, _, _: &ToggleWindow, window, cx| this.toggle_browser_window(window, cx)).detach();
        let viewer_state = state.clone();
        let viewer = cx.new(|cx| ViewerPanel::new(viewer_state, cx));
        let android_state = state.clone();
        let android = cx.new(|cx| AndroidPanel::new(android_state, cx));
        Self { android, orchestration: Default::default(), progress_ui: Default::default(), plugins: Default::default(), menu: None, sidebar: Default::default(), meters: Default::default(), search: None, worktrees: Default::default(), failover: Default::default(), usage_page: Default::default(), more: Default::default(), plugin_page: Default::default(), skill_page: Default::default(), harness_page: Default::default(), connect_page: Default::default(), connect: Default::default(), mobile: Default::default(), wakes_loaded: false, usage: Default::default(), review: review::ReviewStore { queue_tests: true, ..Default::default() }, review_opened: false, history: None, settings: None, recording: None, suspend_saving: false, viewer, side_open: Vec::new(), side_shown: None, #[cfg(debug_assertions)] dev_layer_opened: false, new_pane: None, model_dialog: None, diff_dialog: None, models: HashMap::new(), new_menu: Default::default(), pane_note: None, welcome: Default::default(), toasts: Vec::new(), questions: Default::default(), question_serial: 0, question_input: None, question_picked: Vec::new(), palette_input: None, palette_entries: Vec::new(), palette_selected: 0, bg_notes: HashMap::new(), bg_tasks: HashMap::new(), bg_popover: None, bg_ticking: false, applied_theme: None, browser, state, events: tx, terminals: HashMap::new(), code_models: HashMap::new(), code_views: HashMap::new(), code_choice: HashMap::new(), browser_window: None, updates: Default::default(), usage_info_open: false, voice: Default::default(), home: Default::default(), tile_bounds: Bounds::default(), drag: None, save_task: None, last_saved: String::new(), focused_view: None, focus: cx.focus_handle(), workspace_focus: cx.focus_handle(), focus_outside: 0 }
    }

    fn schedule_save(&mut self, cx: &mut Context<Self>) {
        if !self.state.read(cx).hydrated || self.suspend_saving {
            return;
        }
        self.save_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(400)).await;
            let _ = this.update(cx, |this, cx| {
                let state = this.state.read(cx);
                let snapshot = state.snapshot();
                crate::telemetry::workspace_size(crate::telemetry::WorkspaceSize {
                    workspaces: state.workspaces.len(),
                    panes: state.all_panes().count(),
                    max_panes: state.workspaces.iter().map(|w| w.panes.len()).max().unwrap_or(0),
                });
                let json = snapshot.to_string();
                if json == this.last_saved {
                    return;
                }
                this.last_saved = json;
                backend::background(move || {
                    if let Ok(snapshot) = serde_json::from_value(snapshot) {
                        let _ = store::state_save(backend::app().state(), snapshot);
                    }
                });
            });
        }));
    }

    fn on_session_event(&mut self, event: SessionEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            SessionEvent::Dirty(id) => {
                if let Some(view) = self.terminals.get(&id) {
                    view.update(cx, |_, cx| cx.notify());
                } else if let Some(session) = SESSIONS.get(&id) {
                    session.painted();
                }
            }
            SessionEvent::Title(id, raw) => {
                self.state.update(cx, |s, cx| {
                    let Some(pane) = s.pane(&id) else { return };
                    let provider = s.providers.iter().find(|p| p.id == pane.provider);
                    let name = provider.map_or(String::new(), |p| p.name.clone());
                    let command = provider.and_then(|p| p.command.clone()).unwrap_or_default();
                    let title = crate::model::clean_agent_title(&raw, &[&name, &command, &pane.provider]);
                    if let Some(title) = title {
                        if let Some(pane) = s.pane_mut(&id).filter(|p| p.agent_title.as_ref() != Some(&title)) {
                            pane.agent_title = Some(title);
                            cx.notify();
                        }
                    }
                });
            }
            SessionEvent::Notice(id, message) => status::notice(&self.state, &id, message, cx),
            SessionEvent::Output(id) => {
                status::output(&self.state, &id, cx);
                self.check_limit_output(&id, cx);
            }
            SessionEvent::DevUrl(id, url) => self.state.update(cx, |s, cx| {
                if let Some(p) = s.pane_mut(&id).filter(|p| p.dev_url.as_ref() != Some(&url)) {
                    p.dev_url = Some(url);
                    cx.notify();
                }
            }),
            SessionEvent::OpenLink { pane_id, link, editor } => self.open_link(pane_id, link, editor, window, cx),
            SessionEvent::Clipboard(text) => cx.write_to_clipboard(ClipboardItem::new_string(text)),
            SessionEvent::FirstOutput(id) => self.state.update(cx, |s, cx| {
                s.starting.remove(&id);
                cx.notify();
            }),
            SessionEvent::StartingProcess(id) => self.state.update(cx, |s, cx| {
                s.starting.insert(id, "process");
                cx.notify();
            }),
            SessionEvent::Worktree { pane_id, info } => self.state.update(cx, |s, cx| {
                if let Some(p) = s.pane_mut(&pane_id) {
                    p.wants_worktree = false;
                    if let Some(info) = info {
                        p.cwd = info.path.clone();
                        p.worktree = Some(info);
                    }
                }
                cx.notify();
            }),
            SessionEvent::Spawned { pane_id, busy, task } => {
                let status = if busy { AgentStatus::Working } else { AgentStatus::Idle };
                if self.state.read(cx).pane(&pane_id).is_some_and(|p| p.status == AgentStatus::Idle) {
                    self.state.update(cx, |s, cx| {
                        s.set_pane_status(&pane_id, status, true);
                        cx.notify();
                    });
                }
                if let Some(task) = task {
                    status::set_task(&self.state, &pane_id, &task, cx);
                }
            }
            SessionEvent::SpawnFailed(id) => {
                // An orchestrator that couldn't start: the next request tries again.
                self.orchestrator_exited(&id, 1, cx);
                self.state.update(cx, |s, cx| {
                    s.starting.remove(&id);
                    s.set_pane_status(&id, AgentStatus::Error, true);
                    cx.notify();
                })
            }
            SessionEvent::Backend(event, payload) if event.starts_with("browser://") => {
                self.browser.update(cx, |b, cx| b.on_event(&event, &payload, cx));
            }
            SessionEvent::Backend(event, payload) => match event.as_str() {
                "agent://status" => {
                    if status::hook(&self.state, &payload, cx) {
                        if let Some(id) = payload["paneId"].as_str() {
                            self.agent_finished(id, cx);
                        }
                    }
                    if let Some(id) = payload["paneId"].as_str() {
                        self.on_meter_status(id, cx);
                    }
                }
                "mcp://call" => self.on_mcp_call(payload, window, cx),
                "notification://action" => self.on_notification_action(&payload, window, cx),
                "mobile://request" => self.on_mobile_request(&payload, window, cx),
                "mobile://changed" => self.on_mobile_changed(cx),
                "connect://changed" => self.on_connect_changed(window, cx),
                "connect://peer" => self.on_connect_peer(&payload, cx),
                "connect://message" => self.on_connect_message(&payload, window, cx),
                "usage://report" => {
                    self.note_pane_report(&payload);
                    self.on_usage_report(&payload, cx);
                }
                "bg://note" => {
                    let (id, text) = (payload["paneId"].as_str().unwrap_or_default().to_string(), payload["text"].as_str().unwrap_or_default().to_string());
                    if !self.orchestrator_note(&id, text.clone(), cx) {
                        self.bg_notes.entry(id).or_default().push(text);
                        self.deliver_bg_notes(cx);
                    }
                }
                "bg://task" => {
                    self.on_bg_task(&payload, cx);
                }
                "pty://exit" => {
                    let id = payload["paneId"].as_str().unwrap_or_default().to_string();
                    let code = payload["code"].as_u64().unwrap_or(0) as u32;
                    let pid = payload["pid"].as_u64().map(|p| p as u32);
                    crate::connect_term::exited(&id, code);
                    if terminal::on_exit(&id, code, pid) && !self.orchestrator_exited(&id, code, cx) {
                        if let Some(model) = self.code_models.get(&id) {
                            model.update(cx, |m, cx| m.on_exit(code, cx));
                        }
                        let next = if code == 0 { AgentStatus::Done } else { AgentStatus::Error };
                        status::report(&self.state, &id, next, Some(format!("exited with code {code}")), cx);
                        if code == 0 {
                            self.agent_finished(&id, cx);
                        }
                    }
                    self.state.update(cx, |s, cx| {
                        s.starting.remove(&id);
                        cx.notify();
                    });
                }
                _ => {}
            },
        }
    }

    fn terminal(&mut self, id: &str, cx: &mut Context<Self>) -> Entity<TerminalView> {
        if let Some(view) = self.terminals.get(id) {
            return view.clone();
        }
        let session = SESSIONS.get_or_create(id, &self.events);
        let (state, events) = (self.state.clone(), self.events.clone());
        let account = self.state.read(cx).pane(id).and_then(|p| self.account_for_start(&p.provider));
        let view = cx.new(|cx| {
            let mut v = TerminalView::new(session, state, events, cx);
            v.start_account = account;
            v
        });
        self.terminals.insert(id.to_string(), view.clone());
        view
    }

    /// Whether a winfer pane shows the chat instead of the TUI; decided once per pane so flipping
    /// the setting doesn't swap a running session's process.
    pub(super) fn uses_code(&mut self, provider: &str, id: &str, cx: &App) -> bool {
        if provider != "winfer" {
            return false;
        }
        if let Some(v) = self.code_choice.get(id) {
            return *v;
        }
        let on = self.state.read(cx).rest.get("codeWeb").and_then(|g| g["web"].as_bool()).unwrap_or(true) || (cfg!(debug_assertions) && std::env::var_os("WINFER_NATIVE_CODE_WEB").is_some());
        self.code_choice.insert(id.to_string(), on);
        on
    }

    /// The chat's process side, started on first use.
    pub(super) fn code_model(&mut self, id: &str, cx: &mut Context<Self>) -> Option<Entity<CodeModel>> {
        if let Some(m) = self.code_models.get(id) {
            return Some(m.clone());
        }
        let s = self.state.read(cx);
        let mut pane = s.pane(id)?.clone();
        let settings = terminal::SpawnSettings::of(s, &pane);
        if pane.account_id.is_none() {
            pane.account_id = self.account_for_start(&pane.provider);
        }
        let session = SESSIONS.get_or_create(id, &self.events);
        session.resize(200, 50);
        let (events, app) = (self.events.clone(), self.state.clone());
        let phase = if pane.wants_worktree && pane.worktree.is_none() { "worktree" } else { "process" };
        self.state.update(cx, |s, cx| {
            s.starting.insert(pane.id.clone(), phase);
            cx.notify();
        });
        let model = cx.new(|cx| CodeModel::new(pane, settings, session, events, app, cx));
        self.code_models.insert(id.to_string(), model.clone());
        let pane_id = id.to_string();
        cx.observe(&model, move |this, _, cx| this.on_code_changed(&pane_id, cx)).detach();
        Some(model)
    }

    fn code_view(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) -> Option<Entity<CodeView>> {
        if let Some(v) = self.code_views.get(id) {
            return Some(v.clone());
        }
        let model = self.code_model(id, cx)?;
        let app = self.state.clone();
        let view = cx.new(|cx| CodeView::new(model, app, window, cx));
        let pane = id.to_string();
        cx.subscribe_in(&view, window, move |this, _, OpenLink(href), window, cx| {
            let link = if href.starts_with("http://") || href.starts_with("https://") {
                terminal::Link::Web(href.clone())
            } else {
                let (name, line) = crate::code::links::split_line(href);
                // Agents write spaces in link targets as %20: look up the real path.
                let path = crate::ui::viewer::percent_decode(name.trim_start_matches("file://"));
                let is_remote = this.state.read(cx).pane(&pane).is_some_and(|p| p.remote.is_some());
                // A local file the chat named: straight into the viewer.
                if !is_remote && std::path::Path::new(&path).is_absolute() && std::path::Path::new(&path).is_file() {
                    this.viewer.update(cx, |v, cx| v.open(path, line, false, cx));
                    return;
                }
                terminal::Link::File { path, line }
            };
            this.open_link(pane.clone(), link, false, window, cx);
        })
        .detach();
        self.code_views.insert(id.to_string(), view.clone());
        Some(view)
    }

    /// Moves the browser panel into a window of its own, or back into the side panel.
    /// `window` is the main window (shortcuts pressed in the browser's window are run there).
    pub(super) fn toggle_browser_window(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(handle) = self.browser_window.take() {
            // Deferred: this may run from a key press in that very window, which is busy until it returns.
            cx.defer(move |cx| {
                let _ = handle.update(cx, |_, window, _| {
                    browser_bounds::save(window);
                    window.remove_window();
                });
            });
            self.browser.update(cx, |b, cx| {
                b.undocked = false;
                cx.notify();
            });
            self.update_state(cx, |s| {
                s.browser_open = true;
                s.side_panel = Some(SidePanel::Browser);
            });
            cx.notify();
            return;
        }
        let (browser, root, state, main) = (self.browser.clone(), cx.entity().downgrade(), self.state.clone(), window.window_handle());
        let bounds = browser_bounds::load(cx).unwrap_or_else(|| Bounds::centered(None, size(px(1100.0), px(800.0)), cx));
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            window_min_size: Some(size(px(360.0), px(300.0))),
            titlebar: Some(TitlebarOptions { title: Some("winfer browser".into()), ..Default::default() }),
            ..Default::default()
        };
        let opened = cx.open_window(options, move |window, cx| {
            let (browser, root, state) = (browser.clone(), root.clone(), state.clone());
            window.on_window_should_close(cx, {
                let root = root.clone();
                move |window, cx| {
                    // Closed by its own close button: the browser goes back into the side panel.
                    browser_bounds::save(window);
                    let _ = root.update(cx, |this, cx| {
                        this.browser_window = None;
                        this.browser.update(cx, |b, cx| {
                            b.undocked = false;
                            cx.notify();
                        });
                        cx.notify();
                    });
                    true
                }
            });
            let view = cx.new(|_| UndockedBrowser { browser, state, root, main });
            cx.new(|cx| gpui_kit::component::Root::new(view, window, cx))
        });
        if let Ok(handle) = opened {
            self.browser_window = Some(handle.into());
            self.browser.update(cx, |b, cx| {
                b.undocked = true;
                cx.notify();
            });
            self.update_state(cx, |s| {
                s.browser_open = true;
                s.side_panel = Some(SidePanel::Browser);
            });
            cx.notify();
        }
    }

    /// Brings the undocked browser window to the front.
    pub(super) fn raise_browser_window(&self, cx: &mut Context<Self>) {
        let Some(handle) = self.browser_window else { return };
        // Deferred: this may run from a key press in that very window.
        cx.defer(move |cx| {
            let _ = handle.update(cx, |_, window, _| window.activate_window());
        });
    }

    fn forget_code(&mut self, id: &str) {
        self.code_models.remove(id);
        self.code_views.remove(id);
    }

    fn add_pane(&mut self, provider: Option<String>, opts: AddPane, cx: &mut Context<Self>) {
        self.state.update(cx, |s, cx| {
            let provider = provider.unwrap_or_else(|| s.default_provider_id());
            s.add_pane(&provider, opts);
            cx.notify();
        });
    }

    /// Runs what the user chose in an actionable notification.
    fn on_notification_action(&mut self, payload: &Value, window: &mut Window, cx: &mut Context<Self>) {
        if self.on_connect_notification(payload, window, cx) || self.on_remote_notification(payload, cx) {
            return;
        }
        let id = payload["paneId"].as_str().unwrap_or_default().to_string();
        let Some(pane) = self.state.read(cx).pane(&id).cloned() else { return };
        let name = self.state.read(cx).provider_name(&pane.provider);
        let target = commands::PaneRef::Id(id.clone());
        let text = payload["text"].as_str().filter(|t| !t.is_empty()).map(str::to_string);
        let log = match (payload["action"].as_str().unwrap_or_default(), text) {
            ("approve", _) => {
                self.execute(Command::Approve(target), window, cx);
                Some(format!("Approved {name} from a notification"))
            }
            ("decline", _) => {
                // Esc cancels the pending request in Claude Code and Codex.
                self.execute(Command::Interrupt(target), window, cx);
                Some(format!("Declined {name} from a notification"))
            }
            ("reply", Some(text)) => {
                let short: String = text.chars().take(80).collect();
                self.execute(Command::SendText { target, text, submit: true }, window, cx);
                Some(format!("Answered {name}: {short}"))
            }
            _ => {
                window.activate_window();
                cx.activate(true);
                self.execute(Command::Focus(target), window, cx);
                None
            }
        };
        if let Some(text) = log {
            self.update_state(cx, |s| s.log_activity(&pane.workspace_id, text, Some(pane.provider.clone())));
        }
    }

    fn close_pane(&mut self, id: &str, cx: &mut Context<Self>) {
        // Home notes its worktree while the pane is still there.
        self.settle_home_tasks(cx);
        terminal::close(id);
        self.terminals.remove(id);
        self.forget_code(id);
        self.code_choice.remove(id);
        let id = id.to_string();
        self.state.update(cx, |s, cx| {
            s.remove_pane(&id);
            cx.notify();
        });
        self.settle_home_tasks(cx);
    }

    fn open_link(&mut self, pane_id: String, link: terminal::Link, editor: bool, window: &mut Window, cx: &mut Context<Self>) {
        match link {
            terminal::Link::Web(url) => self.execute(Command::BrowserOpen { url, new_tab: true }, window, cx),
            terminal::Link::File { path, line } => {
                let (remote, cwd, roots) = {
                    let s = self.state.read(cx);
                    match s.pane(&pane_id) {
                        Some(p) if p.remote.is_some() => {
                            let r = p.remote.clone().unwrap();
                            let mirror = self.connect.mirrors.get(&r.peer);
                            let rpane = mirror.and_then(|m| m.panes.iter().find(|rp| rp.id == r.pane));
                            let ws_root = rpane
                                .and_then(|rp| mirror?.workspace_roots.get(&rp.workspace_id).cloned())
                                .or_else(|| {
                                    s.workspaces
                                        .iter()
                                        .find(|w| w.id == p.workspace_id)
                                        .and_then(|w| w.remote.as_ref())
                                        .and_then(|rw| mirror?.workspace_roots.get(&rw.workspace).cloned())
                                });
                            let cwd = self
                                .code_models
                                .get(&pane_id)
                                .and_then(|m| m.read(cx).state.cwd.clone())
                                .filter(|c| !c.is_empty())
                                .or_else(|| rpane.map(|rp| rp.cwd.clone()).filter(|c| !c.is_empty()))
                                .or_else(|| ws_root.clone())
                                .unwrap_or_else(|| "~".to_string());
                            let roots = ws_root.map(|root| vec![root]);
                            (Some(r), cwd, roots)
                        }
                        Some(p) => (None, p.cwd.clone(), s.workspaces.iter().find(|w| w.id == p.workspace_id).map(|w| vec![w.root_dir.clone()])),
                        None => (None, self.side_terminal_pane(&pane_id, cx).map_or_else(|| "~".to_string(), |p| p.cwd), None),
                    }
                };
                let is_absolute = path.starts_with('/') || path.starts_with('~');
                if let Some(remote) = remote {
                    cx.spawn(async move |this, cx| {
                        let query = path.clone();
                        let res = winfer_core::connect_files::call(
                            &remote.peer,
                            "resolve",
                            serde_json::json!({ "cwd": cwd, "path": query, "roots": roots }),
                        )
                        .await;
                        let _ = this.update(cx, |this, cx| match res {
                            Ok(v) => match v["path"].as_str() {
                                Some(found) => this.viewer.update(cx, |v, cx| v.open_remote(&remote.peer, found.to_string(), line, false, cx)),
                                None if is_absolute => this.toast(format!("Couldn't find {path} on {}", remote.machine), cx),
                                None => {}
                            },
                            Err(e) => this.toast(e, cx),
                        });
                    })
                    .detach();
                    return;
                }
                cx.spawn(async move |this, cx| {
                    let query = path.clone();
                    let found = tauri::async_runtime::spawn(async move { winfer_backend::files::files_resolve(cwd, vec![query], roots).await }).await.ok().and_then(|r| r.into_iter().next().flatten());
                    let _ = this.update(cx, |this, cx| {
                        let Some(found) = found else {
                            if is_absolute {
                                this.toast(format!("Couldn't find {path}"), cx);
                            }
                            return;
                        };
                        if editor {
                            let chosen = this.state.read(cx).editor.clone();
                            this.open_in_editor(found, chosen, cx);
                        } else if found.ends_with(".html") || found.ends_with(".htm") || found.ends_with(".svg") {
                            this.update_state(cx, |s| {
                                s.browser_open = true;
                                s.side_panel = Some(SidePanel::Browser);
                            });
                            this.browser.update(cx, |b, cx| b.open_url(format!("file://{found}"), true, None, cx)).detach();
                        } else {
                            this.viewer.update(cx, |v, cx| v.open(found, line, false, cx));
                        }
                    });
                })
                .detach();
            }
        }
    }

    fn side_terminal_pane(&self, pane_id: &str, cx: &App) -> Option<Pane> {
        let workspace = pane_id.strip_prefix("side-terminal-")?;
        let w = self.state.read(cx).workspaces.iter().find(|w| w.id == workspace)?;
        Some(Pane { id: pane_id.to_string(), workspace_id: w.id.clone(), provider: "shell".to_string(), title: "Terminal".to_string(), cwd: w.root_dir.clone(), ..Default::default() })
    }

    fn side_terminal(&mut self, workspace: &str, cx: &mut Context<Self>) -> Option<Entity<TerminalView>> {
        let id = format!("side-terminal-{workspace}");
        let pane = self.side_terminal_pane(&id, cx)?;
        let view = self.terminal(&id, cx);
        view.update(cx, |v, _| v.own_pane = Some(pane));
        Some(view)
    }

    pub(super) fn close_side_terminal(&mut self, workspace: &str, cx: &mut Context<Self>) {
        let id = format!("side-terminal-{workspace}");
        terminal::close(&id);
        self.terminals.remove(&id);
        self.side_open.retain(|w| w != workspace);
        if self.side_shown.as_deref() == Some(workspace) {
            self.side_shown = self.side_open.last().cloned();
        }
        cx.notify();
    }

    fn render_side_terminal(&mut self, c: &Colors, cx: &mut Context<Self>) -> AnyElement {
        let c = *c;
        let active = self.state.read(cx).active_workspace_id.clone();
        self.side_open.retain(|id| self.state.read(cx).workspaces.iter().any(|w| w.id == *id));
        if self.side_open.is_empty() && !active.is_empty() {
            self.side_open.push(active.clone());
            self.side_shown = Some(active.clone());
        }
        let shown = self.side_shown.clone().filter(|w| self.side_open.contains(w)).or_else(|| self.side_open.last().cloned());
        let Some(workspace) = shown else { return div().into_any_element() };
        let Some(view) = self.side_terminal(&workspace, cx) else { return div().into_any_element() };
        let s = self.state.read(cx);
        let cwd = s.workspaces.iter().find(|w| w.id == workspace).map_or(String::new(), |w| w.root_dir.clone());
        let tabs: Vec<(String, String)> = self.side_open.iter().filter_map(|id| s.workspaces.iter().find(|w| w.id == *id).map(|w| (id.clone(), w.name.clone()))).collect();
        let show_tabs = tabs.len() > 1 || active != workspace;
        let offer_active = !self.side_open.contains(&active);
        let mut strip = div().flex().items_center().gap_1().h(px(32.0)).px_1p5();
        for (id, name) in tabs {
            let selected = id == workspace;
            let (show, close) = (id.clone(), id.clone());
            strip = strip.child(
                div().id(SharedString::from(format!("side-tab-{id}"))).flex().items_center().gap_1p5().max_w(px(160.0)).rounded_md().px_2().h(px(24.0)).text_xs().cursor_pointer()
                    .text_color(if selected { c.fg } else { c.muted }).when(selected, |d| d.bg(c.active))
                    .on_click(cx.listener(move |this, _, _, cx| { this.side_shown = Some(show.clone()); cx.notify(); }))
                    .child(div().truncate().child(name))
                    .child(
                        div().id(SharedString::from(format!("side-tab-x-{id}"))).cursor_pointer()
                            .on_click(cx.listener(move |this, _, _, cx| { cx.stop_propagation(); this.close_side_terminal(&close, cx); }))
                            .child(Icon::new(IconName::X).size(px(11.0)).text_color(c.subtle)),
                    ),
            );
        }
        let add_active = active.clone();
        let strip = strip.when(offer_active, |d| {
            d.child(
                div().id("side-add").rounded_sm().p_1().cursor_pointer().hover(move |d| d.bg(c.hover))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.side_open.push(add_active.clone());
                        this.side_shown = Some(add_active.clone());
                        cx.notify();
                    }))
                    .child(Icon::new(IconName::Plus).size(px(14.0)).text_color(c.subtle)),
            )
        });
        let restart_id = workspace.clone();
        let mono = crate::fonts::mono(cx);
        div()
            .size_full()
            .flex()
            .flex_col()
            .when(show_tabs, |d| d.child(strip))
            .child(
                div().flex().items_center().gap_2().px_3().py_1p5().text_xs().text_color(c.subtle)
                    .child(Icon::new(IconName::FolderOpen).size(px(12.0)))
                    .child(div().flex_1().min_w_0().truncate().font_family(mono).text_size(px(11.0)).child(cwd))
                    .child(
                        div().id("side-restart").rounded_sm().p_1().cursor_pointer().hover(move |d| d.bg(c.hover))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let id = format!("side-terminal-{restart_id}");
                                this.restart_pane(&id, cx);
                            }))
                            .child(Icon::new(IconName::RotateCw).size(px(12.0))),
                    ),
            )
            .child(div().flex_1().min_h_0().rounded_b(px(15.0)).overflow_hidden().py_1p5().pr_1().pl_2p5().child(view))
            .into_any_element()
    }

    fn render_side_panel(&mut self, c: &Colors, cx: &mut Context<Self>) -> AnyElement {
        let c = *c;
        let panel = self.state.read(cx).side_panel();
        let tab = |this: &Self, cx: &mut Context<Self>, id: &'static str, icon: IconName, label: &'static str, target: SidePanel| {
            let _ = this;
            let active = panel == target;
            div().id(id).flex().items_center().gap_1p5().rounded_md().px_2().py_1().text_xs().cursor_pointer()
                .text_color(if active { c.fg } else { c.muted }).when(active, |d| d.bg(c.active)).hover(move |d| d.bg(c.hover).text_color(c.fg))
                .on_click(cx.listener(move |this, _, window, cx| this.execute(Command::SidePanel { panel: target, show: true }, window, cx)))
                .child(Icon::new(icon).size(px(13.0)))
                .child(label)
        };
        let header = div().flex().items_center().gap_1().pl_1().pr_2().py_1()
            .child(tab(self, cx, "side-browser", IconName::Globe, "Browser", SidePanel::Browser).rounded_tl_xl())
            .child(tab(self, cx, "side-device", IconName::Smartphone, "Devices", SidePanel::Device))
            .child(tab(self, cx, "side-terminal", IconName::SquareTerminal, "Terminal", SidePanel::Terminal))
            .child(tab(self, cx, "side-viewer", IconName::FileText, "Files", SidePanel::Viewer))
            .child(div().flex_1())
            .children(match panel {
                SidePanel::Browser => self.render_panel_actions("browser", c, cx),
                SidePanel::Device => self.render_panel_actions("android", c, cx),
                _ => Vec::new(),
            });
        let body: AnyElement = match panel {
            SidePanel::Browser if self.browser_window.is_some() => div().size_full().flex().flex_col().items_center().justify_center().gap_2().text_xs().text_color(c.muted)
                .child(Icon::new(IconName::AppWindow).size(px(20.0)))
                .child("The browser is in its own window.")
                .child(div().flex().gap_2()
                    .child(div().id("show-browser-window").rounded_md().border_1().border_color(c.edge).px_3().py_1().cursor_pointer().text_color(c.fg).hover(move |d| d.bg(c.hover))
                        .on_click(cx.listener(|this, _, _, cx| this.raise_browser_window(cx))).child("Show window"))
                    .child(div().id("dock-browser").rounded_md().border_1().border_color(c.edge).px_3().py_1().cursor_pointer().text_color(c.fg).hover(move |d| d.bg(c.hover))
                        .on_click(cx.listener(|this, _, window, cx| this.toggle_browser_window(window, cx))).child("Dock it back")))
                .into_any_element(),
            SidePanel::Browser => self.browser.clone().into_any_element(),
            SidePanel::Viewer => self.viewer.clone().into_any_element(),
            SidePanel::Terminal => self.render_side_terminal(&c, cx),
            SidePanel::Device => self.android.clone().into_any_element(),
        };
        div().size_full().flex().flex_col().child(header).child(div().flex_1().min_h_0().rounded_b(px(15.0)).overflow_hidden().child(body)).into_any_element()
    }

    /// Restarts a pane's process in place (a different model), resuming its session.
    fn restart_pane(&mut self, id: &str, cx: &mut Context<Self>) {
        terminal::close(id);
        self.terminals.remove(id);
        self.forget_code(id);
        self.focused_view = None;
        cx.notify();
    }

    fn sync_focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.modal_layer_open(cx) {
            if self.workspace_focus.contains_focused(window, cx) {
                self.focus.focus(window, cx);
                self.focused_view = None;
            }
            return;
        }
        if self.search.is_some() || self.sidebar.renaming.is_some() {
            return;
        }
        let focused = self.state.read(cx).focused_pane_id.clone();
        if focused == self.focused_view {
            return;
        }
        self.focused_view = focused.clone();
        // Not a pane: Home's chats focus their own composer.
        if let Some(id) = focused.filter(|id| !crate::voice::is_orchestrator(id)) {
            let provider = self.state.read(cx).pane(&id).map(|p| p.provider.clone()).unwrap_or_default();
            if self.uses_code(&provider, &id, cx) {
                if let Some(view) = self.code_view(&id, window, cx) {
                    view.update(cx, |v, cx| v.focus_composer(window, cx));
                }
            } else {
                let view = self.terminal(&id, cx);
                view.read(cx).focus.clone().focus(window, cx);
            }
        }
    }

    fn mouse_moved(&mut self, event: &MouseMoveEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.drag.is_some() && event.pressed_button != Some(MouseButton::Left) {
            self.drag = None;
            cx.notify();
            return;
        }
        match self.drag.as_mut() {
            Some(Drag::Sidebar) => {
                let width = (event.position.x / px(1.0)).clamp(MIN_SIDEBAR, MAX_SIDEBAR) as f64;
                self.state.update(cx, |s, cx| {
                    s.sidebar_width = width.round();
                    cx.notify();
                });
            }
            Some(Drag::SidePanel) => {
                let right = window.viewport_size().width / px(1.0) - 8.0;
                let width = ((right - event.position.x / px(1.0)) as f64).max(MIN_SIDE_PANEL as f64);
                self.state.update(cx, |s, cx| {
                    s.browser_width = width.round();
                    cx.notify();
                });
            }
            Some(Drag::Gutter(drag)) => {
                let moved = if drag.dir == SplitDir::Row { event.position.x - drag.start.x } else { event.position.y - drag.start.y };
                let delta = ((moved / px(1.0)) / drag.span) as f64;
                let mut sizes = drag.sizes.clone();
                let (a, b) = (drag.index, drag.index + 1);
                let d = delta.clamp(0.08 - sizes[a], sizes[b] - 0.08);
                sizes[a] += d;
                sizes[b] -= d;
                let path = drag.path.clone();
                let overview = self.state.read(cx).view() == View::Overview;
                self.state.update(cx, |s, cx| {
                    if overview {
                        s.overview.layout = s.overview.layout.as_ref().map(|l| layout::set_sizes_at(l, &path, &sizes));
                    } else if let Some(w) = s.active_mut() {
                        w.layout = w.layout.as_ref().map(|l| layout::set_sizes_at(l, &path, &sizes));
                    }
                    cx.notify();
                });
            }
            Some(Drag::Pane { start, at, active, .. }) => {
                *at = event.position;
                if !*active {
                    let dx = (event.position.x - start.x) / px(1.0);
                    let dy = (event.position.y - start.y) / px(1.0);
                    if dx.hypot(dy) >= PANE_DRAG_THRESHOLD {
                        *active = true;
                        gpui_kit::base::TextSelection::clear(window, cx);
                    }
                }
                cx.notify();
            }
            None => {}
        }
    }

    fn mouse_released(&mut self, event: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        let drag = self.drag.take();
        if let Some(Drag::Pane { id, active: true, .. }) = drag {
            let overview = self.state.read(cx).view() == View::Overview;
            let tree = {
                let s = self.state.read(cx);
                if overview { s.overview.layout.clone() } else { s.active().and_then(|w| w.layout.clone()) }
            };
            if let Some(tree) = tree {
                let mut rects = Vec::new();
                place(&tree, self.tile_bounds, &mut Vec::new(), &mut rects, &mut Vec::new());
                if let Some(target) = rects.iter().find(|r| r.id != id && r.bounds.contains(&event.position)) {
                    let moved = layout::move_pane(&tree, &id, &target.id, drop_side(target.bounds, event.position));
                    self.state.update(cx, |s, cx| {
                        if overview {
                            s.overview.layout = Some(moved);
                        } else if let Some(w) = s.active_mut() {
                            w.layout = Some(moved);
                        }
                        cx.notify();
                    });
                }
            }
        }
        cx.notify();
    }

    /// Minimize, maximize/restore and close, drawn where the OS doesn't (the title bar is ours):
    /// Windows, and Linux windows with client-side decorations. macOS keeps its traffic lights.
    /// On Windows the buttons are only hit-test areas (the OS handles the clicks); on Linux they act.
    fn render_window_controls(&self, c: &Colors, window: &Window) -> Option<AnyElement> {
        let windows = cfg!(target_os = "windows");
        let linux = cfg!(target_os = "linux");
        if !windows && !(linux && matches!(window.window_decorations(), Decorations::Client { .. })) {
            return None;
        }
        let c = *c;
        let supported = window.window_controls();
        let button = move |id: &'static str, icon: IconName, area: WindowControlArea| {
            let close = area == WindowControlArea::Close;
            div()
                .id(id)
                .flex()
                .items_center()
                .justify_center()
                .w(px(46.0))
                .h_full()
                .flex_shrink_0()
                .text_color(c.muted)
                .hover(move |d| if close { d.bg(c.danger).text_color(white()) } else { d.bg(c.hover).text_color(c.fg) })
                .when(windows, |d| d.window_control_area(area))
                .when(!windows, |d| {
                    d.on_mouse_down(MouseButton::Left, |_, window, cx| {
                        window.prevent_default();
                        cx.stop_propagation();
                    })
                    .on_click(move |_, window, cx| {
                        cx.stop_propagation();
                        match area {
                            WindowControlArea::Min => window.minimize_window(),
                            WindowControlArea::Max => window.zoom_window(),
                            _ => window.remove_window(),
                        }
                    })
                })
                .child(Icon::new(icon).size(px(12.0)))
        };
        Some(
            div()
                .id("window-controls")
                .flex()
                .items_center()
                .h_full()
                .flex_shrink_0()
                .when(supported.minimize, |d| d.child(button("window-minimize", IconName::WindowMinimize, WindowControlArea::Min)))
                .when(supported.maximize, |d| {
                    d.child(if window.is_maximized() {
                        button("window-restore", IconName::WindowRestore, WindowControlArea::Max)
                    } else {
                        button("window-maximize", IconName::WindowMaximize, WindowControlArea::Max)
                    })
                })
                .child(button("window-close", IconName::WindowClose, WindowControlArea::Close))
                .into_any_element(),
        )
    }

    fn render_title_bar(&self, c: &Colors, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Room for macOS's traffic lights; elsewhere the window controls sit at the right edge.
        let mac = cfg!(target_os = "macos");
        let controls = self.render_window_controls(c, window);
        // Moves the window on Windows (the OS drags and snaps it, and a double click maximizes it).
        // Only on gaps between controls: over a button it would take the click for a title bar drag.
        let drag = || div().flex_1().h_full().when(cfg!(target_os = "windows"), |d| d.window_control_area(WindowControlArea::Drag));
        let s = self.state.read(cx);
        let name: SharedString = s.active().map_or(String::new(), |w| w.name.clone()).into();
        let view = s.view();
        let live_on = self.live_voice_on();
        let all: Vec<&Pane> = s.all_panes().collect();
        let sessions = all.iter().filter(|p| !p.archived).count();
        let archived = all.iter().filter(|p| p.archived).count();
        let waiting = all.iter().any(|p| !p.archived && p.status == AgentStatus::NeedsInput);
        let c = *c;
        let tab = move |id: &'static str, icon: IconName, label: SharedString, active: bool| {
            div()
                .id(id)
                .flex()
                .items_center()
                .gap_1p5()
                .rounded_md()
                .px_2p5()
                .py_1()
                .text_xs()
                .text_color(if active { c.fg } else { c.muted })
                .when(active, |d| d.bg(c.active))
                .hover(move |d| d.bg(c.hover).text_color(c.fg))
                .cursor_pointer()
                .child(Icon::new(icon).size(px(12.0)))
                .child(label)
        };
        let set = |v: View| cx.listener(move |this: &mut Self, _: &ClickEvent, _, cx| this.state.update(cx, |s, cx| { s.set_view(v.clone()); cx.notify(); }));
        div()
            .id("title-bar")
            .h(px(40.0))
            .flex_shrink_0()
            .flex()
            .items_center()
            .pl_3()
            .when(controls.is_none(), |d| d.pr_3())
            .gap_2()
            .text_color(c.subtle)
            .on_mouse_down(MouseButton::Left, |_, window, _| window.start_window_move())
            .child(
                div().flex_1().h_full().flex().items_center().pl(px(if mac { 68.0 } else { 0.0 }))
                    .child(
                        div().id("toggle-sidebar").rounded_sm().p_1().hover(move |d| d.bg(c.hover).text_color(c.fg)).cursor_pointer()
                            .on_click(cx.listener(|this, _, window, cx| this.execute(Command::ToggleSidebar, window, cx)))
                            .child(Icon::new(IconName::PanelLeft).size(px(15.0))),
                    )
                    .child(drag()),
            )
            .child(
                div().flex().items_center().gap_0p5().rounded_lg().border_1().border_color(c.edge).bg(c.panel).p_0p5()
                    .child(tab("tab-workspace", IconName::LayoutGrid, name, view == View::Workspace).on_click(set(View::Workspace)))
                    .child(
                        tab("tab-overview", IconName::Layers, "Overview".into(), view == View::Overview)
                            .child(div().text_size(px(10.0)).text_color(c.subtle).child(sessions.to_string()))
                            .when(waiting, |d| d.child(div().size_1p5().rounded_full().bg(c.warning)))
                            .on_click(set(View::Overview)),
                    )
                    .when(archived > 0 || view == View::Archive, |d| {
                        d.child(
                            tab("tab-archive", IconName::Archive, "Archive".into(), view == View::Archive)
                                .child(div().text_size(px(10.0)).text_color(c.subtle).child(archived.to_string()))
                                .on_click(set(View::Archive)),
                        )
                    })
                    .children(self.render_page_tabs(c, cx)),
            )
            .child(
                div().flex_1().h_full().flex().items_center().justify_end().gap_2()
                    .child(drag())
                    .child(self.render_usage_meter(c, cx))
                    .child(
                        div().id("live-voice").rounded_md().border_1().px_1p5().py_1().cursor_pointer().hover(move |d| d.bg(c.hover).text_color(c.fg))
                            .text_color(if live_on { c.accent } else { c.subtle })
                            .tooltip(crate::ui::code_view::tip(if live_on { "End live voice conversation" } else { "Start live voice conversation" }))
                            .on_click(cx.listener(|this, _, window, cx| this.execute(Command::LiveVoice, window, cx)))
                            .child(Icon::new(IconName::AudioLines).size(px(13.0))),
                    )
                    .child(
                        div().id("new-pane").flex().items_center().gap_1().rounded_md().border_1().border_color(c.edge).px_2().py_1().text_xs().cursor_pointer()
                            .hover(move |d| d.bg(c.hover).text_color(c.fg))
                            .tooltip(crate::ui::code_view::tip("New pane"))
                            .on_click(cx.listener(|this, _, _, cx| this.toggle_new_menu(cx)))
                            .child(Icon::new(IconName::Plus).size(px(13.0)))
                            .child("New"),
                    )
                    .child(
                        div().id("toggle-side").rounded_sm().p_1().hover(move |d| d.bg(c.hover).text_color(c.fg)).cursor_pointer()
                            .on_click(cx.listener(|_, _, window, cx| window.dispatch_action(Box::new(ToggleSidePanel), cx)))
                            .child(Icon::new(IconName::PanelRight).size(px(15.0))),
                    ),
            )
            // `.children` of an Option: nothing on macOS.
            .children(controls.map(|controls| div().ml_2().h_full().child(controls)))
    }



    fn render_pane(&mut self, pane: &Pane, _index: usize, zoomed: bool, bounds: Bounds<Pixels>, origin: Point<Pixels>, c: &Colors, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let code = if self.uses_code(&pane.provider, &pane.id, cx) { self.code_view(&pane.id, window, cx) } else { None };
        let is_code = code.is_some();
        let sky = code.as_ref().map(|view| {
            div()
                .absolute()
                .inset_0()
                .rounded_2xl()
                .overflow_hidden()
                .child(view.read(cx).sky_view())
        });
        let body: AnyElement = match code {
            Some(view) => view.into_any_element(),
            None => self.terminal(&pane.id, cx).into_any_element(),
        };
        let header = self.render_header(pane, bounds.size.width / px(1.0), zoomed, c, cx);
        let search = if is_code { None } else { self.render_search(&pane.id, *c, cx) };
        let s = self.state.read(cx);
        let starting = s.starting.get(&pane.id).map(|p| if *p == "worktree" { "Creating worktree…".to_string() } else { format!("Starting {}…", s.provider_name(&pane.provider)) });
        let dragged = matches!(&self.drag, Some(Drag::Pane { id, active: true, .. }) if *id == pane.id);
        let focused = s.focused_pane_id.as_deref() == Some(&pane.id);
        let c = *c;
        let pane_id = pane.id.clone();
        let pane_ws_id = pane.workspace_id.clone();
        let body_pane_id = pane.id.clone();
        let body_ws_id = pane.workspace_id.clone();
        let body_wrap = if is_code {
            div().flex_1().min_h_0().rounded_b(px(15.0)).overflow_hidden().child(body)
        } else {
            div().flex_1().min_h_0().rounded_b(px(15.0)).overflow_hidden().pt_1p5().pb_1p5().pl_2p5().pr_1().child(body)
        }
        .capture_any_mouse_down(cx.listener(move |this, e: &MouseDownEvent, _, cx| {
            if e.button == MouseButton::Left {
                let s = this.state.read(cx);
                if s.focused_pane_id.as_deref() != Some(body_pane_id.as_str()) || s.active_workspace_id != body_ws_id {
                    this.update_state(cx, |s| s.focus_pane(&body_pane_id));
                }
            }
        }));
        div()
            .absolute()
            .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| {
                let s = this.state.read(cx);
                if s.focused_pane_id.as_deref() != Some(pane_id.as_str()) || s.active_workspace_id != pane_ws_id {
                    this.update_state(cx, |s| s.focus_pane(&pane_id));
                }
            }))
            .left(bounds.origin.x - origin.x)
            .top(bounds.origin.y - origin.y)
            .w(bounds.size.width)
            .h(bounds.size.height)
            .flex()
            .flex_col()
            .overflow_hidden()
            .rounded_2xl()
            .bg(c.panel)
            .when(dragged, |d| d.opacity(0.4))
            .children(sky)
            .child(header)
            .child(body_wrap)
            .children(search)
            .when_some(starting, |d, text| {
                d.child(
                    div().absolute().top(px(32.0)).left_0().right_0().bottom_0().flex().items_center().justify_center().child(
                        div().rounded_md().border_1().border_color(c.edge).bg(c.raised).px_3().py_1p5().text_xs().text_color(c.muted).child(text),
                    ),
                )
            })
            // Paint the rounded border on top so inner views (chat/terminal/sky) can never square off the bottom corners.
            .child(div().absolute().top_0().left_0().size_full().rounded_2xl().when(focused, |d| d.border_1().border_color(Hsla { a: 0.75, ..c.accent })).when(!focused, |d| d.border_1().border_color(c.edge)))
    }

    fn render_tiles(&mut self, tree: Option<LayoutNode>, zoomed: Option<String>, c: &Colors, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let bounds = self.tile_bounds;
        let origin = bounds.origin;
        let mut area = div().relative().size_full();
        let Some(tree) = tree else {
            return area.child(self.render_welcome(*c, window, cx));
        };
        if bounds.size.width <= px(0.0) {
            return area;
        }
        let order = leaves(&tree);
        let mut rects = Vec::new();
        let mut gutters = Vec::new();
        match &zoomed {
            Some(id) if order.contains(id) => rects.push(Rect { id: id.clone(), bounds }),
            _ => place(&tree, bounds, &mut Vec::new(), &mut rects, &mut gutters),
        }
        for rect in rects {
            let Some(pane) = self.state.read(cx).pane(&rect.id).cloned() else { continue };
            let index = order.iter().position(|id| *id == rect.id).unwrap_or(0) + 1;
            let element = self.render_pane(&pane, index, zoomed.as_ref() == Some(&rect.id), rect.bounds, origin, c, window, cx);
            area = area.child(element);
        }
        for (i, gutter) in gutters.into_iter().enumerate() {
            let cursor = if gutter.dir == SplitDir::Row { CursorStyle::ResizeLeftRight } else { CursorStyle::ResizeUpDown };
            let (path, index, dir, span) = (gutter.path, gutter.index, gutter.dir, gutter.span);
            let g = gutter.bounds;
            area = area.child(
                div().id(("gutter", i)).absolute().left(g.origin.x - origin.x).top(g.origin.y - origin.y).w(g.size.width).h(g.size.height).cursor(cursor)
                    .on_mouse_down(MouseButton::Left, cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                        let overview = this.state.read(cx).view() == View::Overview;
                        let s = this.state.read(cx);
                        let tree = if overview { s.overview.layout.clone() } else { s.active().and_then(|w| w.layout.clone()) };
                        let Some(tree) = tree else { return };
                        let node = path.iter().fold(&tree, |n, i| match n {
                            LayoutNode::Split { children, .. } => &children[*i],
                            leaf => leaf,
                        });
                        if let LayoutNode::Split { sizes, .. } = node {
                            gpui_kit::base::GlobalState::suppress_text_selection(cx);
                            this.drag = Some(Drag::Gutter(GutterDrag { path: path.clone(), index, dir, span, start: e.position, sizes: sizes.clone() }));
                            cx.notify();
                        }
                    })),
            );
        }
        area
    }

    fn render_archive(&mut self, c: &Colors, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let bounds = self.tile_bounds;
        let archived: Vec<Pane> = self.state.read(cx).all_panes().filter(|p| p.archived).cloned().collect();
        let mut area = div().relative().size_full();
        let Some(tree) = layout::build_preset(layout::Preset::Grid, &archived.iter().map(|p| p.id.clone()).collect::<Vec<_>>()) else {
            return area.child(div().size_full().flex().items_center().justify_center().text_sm().text_color(c.subtle).child("Nothing archived"));
        };
        let mut rects = Vec::new();
        place(&tree, bounds, &mut Vec::new(), &mut rects, &mut Vec::new());
        for (i, rect) in rects.into_iter().enumerate() {
            let Some(pane) = archived.iter().find(|p| p.id == rect.id).cloned() else { continue };
            let element = self.render_pane(&pane, i + 1, false, rect.bounds, bounds.origin, c, window, cx);
            area = area.child(element);
        }
        area
    }

    fn drop_indicator(&self, c: &Colors, cx: &App) -> Option<impl IntoElement + use<>> {
        let Some(Drag::Pane { id, at, active: true, .. }) = &self.drag else { return None };
        let s = self.state.read(cx);
        let tree = if s.view() == View::Overview { s.overview.layout.clone() } else { s.active().and_then(|w| w.layout.clone()) }?;
        let mut rects = Vec::new();
        place(&tree, self.tile_bounds, &mut Vec::new(), &mut rects, &mut Vec::new());
        let target = rects.iter().find(|r| r.id != *id && r.bounds.contains(at))?;
        let side = drop_side(target.bounds, *at);
        let b = target.bounds;
        let (mut origin, mut extent) = (b.origin, b.size);
        match side {
            DropSide::Left => extent.width /= 2.0,
            DropSide::Right => {
                extent.width /= 2.0;
                origin.x += extent.width;
            }
            DropSide::Top => extent.height /= 2.0,
            DropSide::Bottom => {
                extent.height /= 2.0;
                origin.y += extent.height;
            }
            DropSide::Center => {}
        }
        let tile = self.tile_bounds.origin;
        Some(div().absolute().left(origin.x - tile.x).top(origin.y - tile.y).w(extent.width).h(extent.height).rounded_md().bg(Hsla { a: 0.2, ..c.accent }).border_1().border_color(c.accent))
    }
}

impl Root {
    /// Closes the topmost modal layer; true if there was one.
    fn close_top_layer(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.drag.take().is_some() {
            cx.notify();
            return true;
        }
        // The skill list in Settings → WinferCode closes before Settings does.
        if self.state.read(cx).settings_open && self.close_skill_list(window, cx) {
            return true;
        }
        if self.usage_info_open {
            self.usage_info_open = false;
            cx.notify();
            return true;
        }
        if self.close_release_notes(cx) || self.close_bg_popover(cx) || self.close_welcome_picker(cx) || self.close_pane_note(cx) || self.close_menu(cx) || self.cancel_rename(cx) || self.close_search(cx) || self.close_plugin_layer(cx) || self.close_orchestration_layer(cx) {
            return true;
        }
        if self.usage.popover.take().is_some() {
            cx.notify();
            return true;
        }
        if self.new_menu.open {
            self.new_menu = Default::default();
            cx.notify();
            return true;
        }
        let s = self.state.read(cx);
        if s.palette_query.is_some() {
            self.close_palette(cx);
        } else if s.cheat_sheet_open {
            self.update_state(cx, |s| s.cheat_sheet_open = false);
            self.focused_view = None;
        } else if s.settings_open {
            self.update_state(cx, |s| s.settings_open = false);
            self.focused_view = None;
        } else if s.diff_pane_id.is_some() {
            self.update_state(cx, |s| s.diff_pane_id = None);
        } else if matches!(s.overlay, crate::model::Overlay::NewPane | crate::model::Overlay::Model | crate::model::Overlay::Review | crate::model::Overlay::History | crate::model::Overlay::Branch) {
            self.update_state(cx, |s| {
                s.overlay = crate::model::Overlay::None;
                s.new_pane_provider = None;
            });
        } else {
            return false;
        }
        true
    }

    /// Widgets from the component library and the terminal follow the app's theme.
    fn apply_theme(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Following the system: show the theme picked for its light or dark setting.
        if self.state.read(cx).theme_mode == "system" {
            let dark = matches!(window.appearance(), WindowAppearance::Dark | WindowAppearance::VibrantDark);
            let want = self.state.read(cx).theme_for_system(dark);
            if want != self.state.read(cx).theme {
                self.state.update(cx, |s, cx| {
                    s.theme = want;
                    cx.notify();
                });
            }
        }
        let id = self.state.read(cx).theme.clone();
        if self.applied_theme.as_ref() == Some(&id) {
            return;
        }
        self.applied_theme = Some(id.clone());
        theme::set_palette(&id);
        let def = theme::theme_by_id(&id);
        let mode = if def.mode == "light" { ThemeMode::Light } else { ThemeMode::Dark };
        ComponentTheme::change(mode, Some(window), cx);
        let c = theme::colors(&id);
        let t = ComponentTheme::global_mut(cx);
        t.background = c.panel;
        t.foreground = c.fg;
        t.border = c.border;
        t.input = c.border;
        t.muted = c.hover;
        t.muted_foreground = c.muted;
        t.primary = c.accent;
        t.primary_foreground = c.on_accent;
        t.popover = c.raised;
        t.popover_foreground = c.fg;
        t.accent = c.hover;
        t.accent_foreground = c.fg;
        t.selection = Hsla { a: 0.35, ..c.accent };
        t.caret = c.fg;
        t.table_head = c.hover;
        t.table_head_foreground = c.muted;
        t.table_hover = c.hover;
        t.table_row_border = c.border;
        let hl = std::sync::Arc::make_mut(&mut t.highlight_theme);
        hl.style.editor_background = Some(c.panel);
        hl.style.editor_gutter_background = Some(c.panel);
        hl.style.editor_foreground = Some(c.fg);
        hl.style.editor_active_line = Some(c.hover);
        hl.style.editor_line_number = Some(c.subtle);
        hl.style.editor_active_line_number = Some(c.fg);
        ComponentTheme::sync_base(cx);
        window.refresh();
    }
}

impl Render for Root {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.apply_theme(window, cx);
        // A dialog that closed takes its focused field with it: without this the keyboard would
        // belong to nothing and shortcuts would stop until the next click. A field that was just
        // focused isn't on screen until it has rendered once, so only a second frame with focus
        // outside the window's tree counts as lost.
        if self.focus.contains_focused(window, cx) {
            self.focus_outside = 0;
        } else {
            self.focus_outside += 1;
            if self.focus_outside >= 2 {
                self.focus_outside = 0;
                self.focus.focus(window, cx);
            } else {
                cx.notify();
            }
        }
        // Development aid: open a layer at launch, to look at it without driving the keyboard.
        #[cfg(debug_assertions)]
        // Development aid: hand the orchestrator a request at launch, as if said to live voice
        // (once the agents are detected, so winfer code is known to be there).
        #[cfg(debug_assertions)]
        if self.state.read(cx).hydrated && !self.state.read(cx).providers.is_empty() {
            static SENT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
            if let Ok(request) = std::env::var("WINFER_NATIVE_ORCHESTRATE")
                && !SENT.swap(true, std::sync::atomic::Ordering::Relaxed)
            {
                let result = self.orchestrate(&request, false, cx);
                eprintln!("[orchestrate] {result:?}");
                if let Err(e) = result {
                    self.toast(e, cx);
                }
            }
        }
        #[cfg(debug_assertions)]
        if self.state.read(cx).hydrated && !std::mem::replace(&mut self.dev_layer_opened, true) {
            match std::env::var("WINFER_NATIVE_LAYER").as_deref() {
                Ok("palette") => self.execute(Command::Palette(None), window, cx),
                Ok("new-pane") => self.update_state(cx, |s| s.overlay = crate::model::Overlay::NewPane),
                Ok("shortcuts") => self.execute(Command::CheatSheet, window, cx),
                Ok("branch") => self.update_state(cx, |s| s.overlay = crate::model::Overlay::Branch),
                Ok("race") => self.update_state(cx, |s| s.overlay = crate::model::Overlay::Race),
                Ok("abtest") => self.update_state(cx, |s| s.overlay = crate::model::Overlay::AbTest),
                Ok("broadcast") => self.update_state(cx, |s| s.overlay = crate::model::Overlay::Broadcast),
                Ok("recipes") => self.update_state(cx, |s| s.overlay = crate::model::Overlay::Recipes),
                Ok("sessions") => self.update_state(cx, |s| s.overlay = crate::model::Overlay::Progress),
                // Development aid: some settings changed from their defaults, to see the reset marks.
                Ok("changed") => {
                    self.update_state(cx, |s| {
                        s.set_terminal_font_size(15.0);
                        s.default_provider = Some("codex".into());
                        s.show_pane_cost = true;
                        s.agent_models.insert("claude".into(), "opus".into());
                    });
                    self.execute(Command::Settings(Some("agents".into())), window, cx);
                }
                // Development aid: the light theme with Settings open (corners, colors).
                Ok("light-settings") => {
                    self.update_state(cx, |s| s.theme = "light".into());
                    self.execute(Command::Settings(Some("general".into())), window, cx);
                }
                // Development aid: a settings page open at launch (`settings:connect`).
                Ok(layer) if layer.starts_with("settings:") => {
                    let page = layer.trim_start_matches("settings:").to_string();
                    self.execute(Command::Settings(Some(page)), window, cx);
                }
                Ok("new-menu") => {
                    self.new_menu.open = true;
                    cx.notify();
                }
                Ok("install") => {
                    self.execute(Command::Settings(Some("agents".into())), window, cx);
                    self.scroll_settings_to_install(cx);
                }
                Ok("review") => self.demo_reviews(cx),
                Ok("usage") => self.demo_usage(cx),
                Ok("usage-limits") => {
                    self.demo_usage(cx);
                    self.usage.popover = None;
                    self.usage.snapshots.remove("claude");
                    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_millis() as f64);
                    for (id, sh, wh) in [("a1", 3.5, 50.0), ("a2", 0.3, 95.0), ("a3", 1.2, 140.0)] {
                        if let Some(s) = self.usage.snapshots.get_mut(id) {
                            if let Some(w) = s.windows.get_mut(0) { w.resets_at = Some(now + sh * 3_600_000.0); }
                            if let Some(w) = s.windows.get_mut(1) { w.resets_at = Some(now + wh * 3_600_000.0); }
                        }
                    }
                    self.execute(Command::Settings(Some("usage".into())), window, cx);
                    self.settings_ui_for_dev(String::new(), window, cx);
                    if let Some(s) = self.settings.as_mut() {
                        s.page_tabs.insert("usage", "limits");
                    }
                }
                Ok("history") => {
                    if let Some((id, name)) = self.state.read(cx).active().map(|w| (w.id.clone(), w.name.clone())) {
                        review::demo_history(&id, &name);
                    }
                    self.update_state(cx, |s| s.overlay = crate::model::Overlay::History);
                }
                Ok("whats-new") => self.updates.preview(),
                Ok("updates-checking") => {
                    self.updates.preview_checking();
                    self.execute(Command::Settings(Some("updates".into())), window, cx);
                }
                Ok("updates-installing") => {
                    self.updates.preview_installing(28 * 1_048_576, 44 * 1_048_576);
                    self.execute(Command::Settings(Some("updates".into())), window, cx);
                }
                Ok("undock") => self.execute(Command::BrowserUndock, window, cx),
                Ok("browser") => self.execute(Command::SidePanel { panel: SidePanel::Browser, show: true }, window, cx),
                Ok("side-terminal") => self.execute(Command::SidePanel { panel: SidePanel::Terminal, show: true }, window, cx),
                Ok(other) if other.starts_with("settings-search:") => {
                    self.execute(Command::Settings(Some("general".into())), window, cx);
                    self.settings_ui_for_dev(other["settings-search:".len()..].to_string(), window, cx);
                }
                // Development aid: Settings → WinferCode on one of its tabs (interface, models, …, mcp),
                // optionally scrolled down (`code-tab:efficiency@500`).
                Ok(other) if other.starts_with("code-tab:") => {
                    let (tab, scroll) = other["code-tab:".len()..].split_once('@').unwrap_or((&other["code-tab:".len()..], "0"));
                    self.harness_page.ui.tab = settings_code::tab_named(tab);
                    self.execute(Command::Settings(Some("winfer-code".into())), window, cx);
                    self.scroll_settings_for_dev(scroll.parse().unwrap_or(0.0), cx);
                }
                // Development aid: a tabbed settings page on one of its tabs, optionally scrolled
                // down (`settings-tab:usage:spend@300`).
                Ok(other) if other.starts_with("settings-tab:") => {
                    let rest = &other["settings-tab:".len()..];
                    let (target, scroll) = rest.split_once('@').unwrap_or((rest, "0"));
                    let (page, tab) = target.split_once(':').unwrap_or((target, ""));
                    let page: &'static str = Box::leak(page.to_string().into_boxed_str());
                    self.execute(Command::Settings(Some(page.into())), window, cx);
                    self.settings_ui_for_dev(String::new(), window, cx);
                    if let Some(s) = self.settings.as_mut().filter(|_| !tab.is_empty()) {
                        s.page_tabs.insert(page, Box::leak(tab.to_string().into_boxed_str()));
                    }
                    self.scroll_settings_for_dev(scroll.parse().unwrap_or(0.0), cx);
                }
                Ok(other) if other.starts_with("settings:") => self.execute(Command::Settings(Some(other["settings:".len()..].to_string())), window, cx),
                Ok(other) if other.starts_with("page:") => self.update_state(cx, |s| s.set_view(View::Page(other["page:".len()..].to_string()))),
                Ok(other) if other.starts_with("file:") => {
                    let path = other["file:".len()..].to_string();
                    self.viewer.update(cx, |v, cx| v.open(path, None, false, cx));
                }
                _ => {}
            }
        }
        if self.state.read(cx).hydrated && !std::mem::replace(&mut self.wakes_loaded, true) {
            self.load_wakes(cx);
            self.home_on_launch(cx);
        }
        self.sync_dialogs(window, cx);
        self.sync_orchestration(window, cx);
        self.sync_plugins(window, cx);
        self.sync_search(window, cx);
        self.sync_branch_dialog(window, cx);
        self.sync_mobile(cx);
        self.sync_focus(window, cx);
        let (theme_id, view, sidebar_open, side_open, side_width, tree, zoomed) = {
            let s = self.state.read(cx);
            let (tree, zoomed) = match s.view() {
                View::Overview => (s.overview.layout.clone(), s.overview.zoomed_pane_id.clone()),
                _ => s.active().map_or((None, None), |w| (w.layout.clone(), w.zoomed_pane_id.clone())),
            };
            (s.theme.clone(), s.view(), s.sidebar_open, s.browser_open, s.browser_width as f32, tree, zoomed)
        };
        let c = theme::colors(&theme_id);
        // Home's opening brief: when the user comes to Home after a while away.
        let home_on = view == View::Home;
        if home_on != self.home.shown {
            self.home.shown = home_on;
            if home_on {
                self.home_opened(cx);
            } else {
                self.home_closed();
            }
        }
        let tiles = match view {
            View::Home => self.render_home(&c, window, cx),
            View::Archive => self.render_archive(&c, window, cx),
            View::Page(key) => self.render_plugin_page(&key, &c, cx),
            _ => self.render_tiles(tree, zoomed, &c, window, cx),
        };
        let indicator = self.drop_indicator(&c, cx);
        let device_on = side_open && self.state.read(cx).side_panel() == SidePanel::Device;
        self.android.update(cx, |a, cx| a.set_visible(device_on, cx));
        let viewer_on = side_open && self.state.read(cx).side_panel() == SidePanel::Viewer && !self.modal_layer_open(cx);
        self.viewer.update(cx, |v, cx| v.set_html_allowed(viewer_on, window, cx));
        let mut side_panel = side_open.then(|| self.render_side_panel(&c, cx));
        let palette = self.render_palette(&c, cx);
        let sheet = self.render_cheat_sheet(&c, cx);
        let question = self.render_question(&c, cx);
        let new_pane = self.render_new_pane(&c, cx);
        let model = self.render_model_dialog(&c, cx);
        let folder = self.render_remote_folder(&c, cx);
        let diff = self.render_diff(&c, cx);
        let settings = self.render_settings(&c, window, cx);
        let review = self.render_review(&c, window, cx);
        let history = self.render_history(&c, window, cx);
        let race = self.render_race(&c, cx);
        let results = self.render_race_results(&c, cx);
        let broadcast = self.render_broadcast(&c, cx);
        let recipes = self.render_recipes(&c, cx);
        let sessions = self.render_sessions(&c, cx);
        let agent_bar = self.render_agent_bar(&c, window, cx);
        let usage_popover = self.render_usage_popover(&c, cx);
        let new_menu = self.render_new_menu(&c, cx);
        let pane_note = self.render_pane_note(&c, cx);
        let bg_popover = self.render_bg_popover(&c, window, cx);
        let apps_menu = self.render_apps_menu(&c, cx);
        let branch_dialog = self.render_branch_dialog(&c, cx);
        let context_menu = self.render_menu(&c, window, cx);
        let approval = self.render_approval(&c, cx);
        let whats_new = self.render_whats_new(&c, cx);
        let usage_info = self.render_usage_info(&c, cx);
        let voice_hud = self.render_voice_hud(&c, cx);
        let toasts = self.render_toasts(&c, cx);
        let drag_shield = match &self.drag {
            Some(Drag::Pane { active: true, .. }) => Some(CursorStyle::ClosedHand),
            Some(Drag::Sidebar | Drag::SidePanel) => Some(CursorStyle::ResizeLeftRight),
            Some(Drag::Gutter(g)) => Some(if g.dir == SplitDir::Row { CursorStyle::ResizeLeftRight } else { CursorStyle::ResizeUpDown }),
            _ => None,
        }
        .map(|cursor| {
            div()
                .id("drag-shield")
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .occlude()
                .cursor(cursor)
                .on_mouse_move(cx.listener(Self::mouse_moved))
                .on_mouse_up(MouseButton::Left, cx.listener(Self::mouse_released))
                .on_mouse_up_out(MouseButton::Left, cx.listener(Self::mouse_released))
        });
        let measure = cx.entity();
        // Full sidebar with its resize handle, or the collapsed icon rail.
        let sidebar = if sidebar_open {
            div().flex().h_full().child(self.render_sidebar(&c, cx)).child(
                div().id("sidebar-resize").w(px(8.0)).flex_shrink_0().cursor(CursorStyle::ResizeLeftRight)
                    .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                        gpui_kit::base::GlobalState::suppress_text_selection(cx);
                        this.drag = Some(Drag::Sidebar);
                        cx.notify();
                    })),
            ).into_any_element()
        } else {
            div().flex().h_full().child(self.render_sidebar_rail(&c, cx)).child(div().w(px(8.0)).flex_shrink_0()).into_any_element()
        };
        div()
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .bg(c.app)
            .text_color(c.fg)
            .font_family(crate::fonts::sans(cx))
            .track_focus(&self.focus)
            .key_context("Root")
            .on_mouse_move(cx.listener(Self::mouse_moved))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::mouse_released))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::mouse_released))
            .capture_any_mouse_down(cx.listener(|this, _: &MouseDownEvent, _, cx| {
                this.note_user_activity();
                // A click that reaches GPUI is outside the plugin and HTML webviews: keys belong to GPUI again.
                this.reclaim_keyboard();
                this.viewer.read(cx).reclaim_keyboard();
            }))
            .capture_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                this.note_user_activity();
                if this.record_shortcut(event, cx) || this.paste_welcome_images_from_key(event, cx) {
                    cx.stop_propagation();
                } else if this.handle_shortcut(event, window, cx) {
                    cx.stop_propagation();
                } else if event.keystroke.key == "escape" && this.close_top_layer(window, cx) {
                    cx.stop_propagation();
                }
            }))
            // Cmd/Ctrl+V in a focused text box is matched as the Input's `Paste` binding before any key-down
            // listener runs, so images for the welcome task box are taken here, ahead of the Input's text paste.
            .capture_action(cx.listener(|this, _: &gpui_kit::component::input::Paste, _, cx| {
                if this.welcome_showing(cx) && this.paste_welcome_images(cx) {
                    cx.stop_propagation();
                }
            }))
            .on_action(cx.listener(|this, _: &ToggleSidePanel, window, cx| this.execute(Command::ToggleBrowser, window, cx)))
            .child(self.render_title_bar(&c, window, cx))
            .child(
                div()
                    .track_focus(&self.workspace_focus)
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .px_2()
                    .pb_2()
                    .child(sidebar)
                    .child(
                        div().flex_1().min_w_0().min_h_0().flex().flex_col().children(agent_bar).child(div()
                            .relative()
                            .flex_1()
                            .min_w_0()
                            .min_h_0()
                            .child(
                                canvas(
                                    move |bounds, _, cx| {
                                        measure.update(cx, |this, cx| {
                                            if this.tile_bounds != bounds {
                                                this.tile_bounds = bounds;
                                                cx.notify();
                                            }
                                        });
                                    },
                                    |_, _, _, _| {},
                                )
                                .absolute()
                                .size_full(),
                            )
                            .child(tiles)
                            .children(indicator)),
                    )
                    .when(side_open, |d| {
                        d.child(
                            div().id("side-resize").w(px(8.0)).flex_shrink_0().cursor(CursorStyle::ResizeLeftRight)
                                .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                                    gpui_kit::base::GlobalState::suppress_text_selection(cx);
                                    this.drag = Some(Drag::SidePanel);
                                    cx.notify();
                                })),
                        )
                        .child(
                            div()
                                .relative()
                                .w(px(side_width.max(MIN_SIDE_PANEL)))
                                .flex_shrink_0()
                                .overflow_hidden()
                                .rounded_2xl()
                                .bg(c.panel)
                                .child(side_panel.take().unwrap())
                                // Paint the rounded border on top so inner views (browser/terminal/files) can never square off the bottom corners.
                                .child(div().absolute().top_0().left_0().size_full().rounded_2xl().border_1().border_color(c.edge)),
                        )
                    }),
            )
            .children(usage_popover)
            .children(new_menu)
            .children(pane_note)
            .children(bg_popover)
            .children(settings)
            .children(review)
            .children(history)
            .children(race)
            .children(results)
            .children(broadcast)
            .children(recipes)
            .children(sessions)
            .children(new_pane)
            .children(model)
            .children(folder)
            .children(diff)
            .children(palette)
            .children(sheet)
            .children(question)
            .children(branch_dialog)
            .children(apps_menu)
            .children(context_menu)
            .children(approval)
            .children(whats_new)
            .children(usage_info)
            .children(voice_hud)
            .children(toasts)
            .children(drag_shield)
    }
}
