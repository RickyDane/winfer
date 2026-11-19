//! The winfer code chat pane: transcript, approvals, plan, composer and footer
//! (port of `features/code/CodePane.tsx`, `Transcript.tsx` and `Composer.tsx`).
//! Panels that open over the transcript live in `code_sheets.rs`.

use crate::code::commands::{self, Effect};
use crate::code::names::{backend_of, model_name};
use crate::code::protocol::*;
use crate::code::session::{CodeEvent, CodeModel};
use crate::code::state::{Attachment, Item, Sheet, now_ms, plan_counts};
use crate::code::toolbody::{self, Section};
use crate::fonts;
use crate::model::AppState;
use crate::theme::{self, Colors};
use base64::Engine;
use gpui_kit::assets::IconName;
use gpui_kit::component::Icon;
use gpui_kit::component::input::{InputEvent, Textarea, TextareaState};
use gpui_kit::component::text::TextView;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

/// What an item's renderer needs from the view: a snapshot taken at render time.
#[derive(Clone)]
pub(super) struct Env {
    pub c: Colors,
    pub mono: SharedString,
    pub scale: f32,
    pub compact: bool,
    pub view: WeakEntity<CodeView>,
    pub flipped: Rc<HashSet<String>>,
    pub expand_thinking: bool,
    pub expand_shell: bool,
    pub images: Rc<RefCell<HashMap<String, Vec<Arc<Image>>>>>,
    pub now: u64,
    pub cwd: Option<String>,
    /// When each row first appeared (ms), to play its entrance once.
    pub born: Rc<HashMap<String, u64>>,
}

impl Env {
    fn fresh(&self, key: &str) -> bool {
        self.born.get(key).is_some_and(|b| *b > 0 && self.now.saturating_sub(*b) < 600)
    }

    fn text(&self) -> Pixels {
        px(13.0 * self.scale)
    }

    fn small(&self) -> Pixels {
        px(11.5 * self.scale)
    }

        fn open(&self, key: &str, default: bool) -> bool {
        default != self.flipped.contains(key)
    }
}

pub struct CodeView {
    pub(super) model: Entity<CodeModel>,
    pub(super) app: Entity<AppState>,
    list: ListState,
    sigs: Vec<u64>,
    input: Entity<TextareaState>,
    seen_rev: u64,
    flipped: HashSet<String>,
    images: Rc<RefCell<HashMap<String, Vec<Arc<Image>>>>>,
    popup: usize,
    /// Per approval id and question: the picked option indexes.
    picks: HashMap<String, Vec<Vec<usize>>>,
    asked_files: bool,
    ticking: bool,
    born: HashMap<String, u64>,
    seeded: bool,
    pub(super) picker: super::code_pickers::PickerState,
    /// An image open full size.
    preview: Option<Arc<Image>>,
}

/// A link clicked in the transcript: a web address or a file path (with `:line`).
pub struct OpenLink(pub String);

impl EventEmitter<OpenLink> for CodeView {}

impl Focusable for CodeView {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.input.read(cx).focus_handle(cx)
    }
}

pub(super) fn tip(text: impl Into<SharedString>) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
    let t: SharedString = text.into();
    move |window, cx| Tooltip::new(t.clone()).build(window, cx)
}

pub(super) fn elapsed(ms: u64) -> String {
    let s = ms / 1000;
    if s < 60 {
        format!("{s}s")
    } else if s < 3600 {
        format!("{}m {:02}s", s / 60, s % 60)
    } else {
        format!("{}h {:02}m", s / 3600, s % 3600 / 60)
    }
}

pub(super) fn tokens(n: u64) -> String {
    if n >= 1_000_000 { format!("{:.1}M", n as f64 / 1e6) } else if n >= 1000 { format!("{:.1}k", n as f64 / 1e3) } else { n.to_string() }
}

fn tool_icon(name: &str) -> IconName {
    match name {
        "Read" => IconName::FileText,
        "Grep" => IconName::Search,
        "Glob" => IconName::FolderSearch,
        "Bash" => IconName::SquareTerminal,
        "Edit" | "Write" => IconName::FilePen,
        "Agent" => IconName::Bot,
        "WebFetch" | "WebSearch" => IconName::Globe,
        _ => IconName::Wrench,
    }
}

fn signature(item: &Item, env_flipped: &HashSet<String>, now: u64) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    item.key().hash(&mut h);
    env_flipped.contains(item.key()).hash(&mut h);
    match item {
        Item::User { text, images, .. } => (text.len(), images.len()).hash(&mut h),
        Item::Text { text, streaming, .. } => (text.len(), *streaming).hash(&mut h),
        Item::Reasoning { text, streaming, ended, .. } => {
            (text.len(), *streaming, ended.is_some()).hash(&mut h);
            if *streaming {
                (now / 1000).hash(&mut h);
            }
        }
        Item::Tool { output, ok, diff, ended, .. } => {
            (output.len(), *ok, diff.as_ref().map(String::len), ended.is_some()).hash(&mut h);
            if ended.is_none() {
                (now / 1000).hash(&mut h);
            }
        }
        Item::Notice { text, .. } => text.len().hash(&mut h),
        Item::Worked { ms, .. } => ms.hash(&mut h),
    }
    h.finish()
}

fn data_image(url: &str) -> Option<Arc<Image>> {
    let (head, body) = url.split_once(',')?;
    let format = match head.strip_prefix("data:")?.split(';').next()? {
        "image/png" => ImageFormat::Png,
        "image/jpeg" | "image/jpg" => ImageFormat::Jpeg,
        "image/gif" => ImageFormat::Gif,
        "image/webp" => ImageFormat::Webp,
        _ => return None,
    };
    let bytes = base64::engine::general_purpose::STANDARD.decode(body).ok()?;
    Some(Arc::new(Image::from_bytes(format, bytes)))
}

impl CodeView {
    pub fn new(model: Entity<CodeModel>, app: Entity<AppState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| TextareaState::new(window, cx).auto_grow(1, 10).soft_wrap(true).submit_on_enter(true).placeholder("Message winfer code…  (/ for commands, @ for files)"));
        cx.subscribe_in(&input, window, |this, input, event: &InputEvent, window, cx| match event {
            InputEvent::PressEnter { shift: false, secondary: false } => this.enter(window, cx),
            InputEvent::Change => {
                let text = input.read(cx).value().to_string();
                this.model.update(cx, |m, _| m.set_draft(text.clone()));
                this.popup = 0;
                if text.contains('@') && !this.asked_files {
                    this.asked_files = true;
                    this.model.update(cx, |m, cx| m.send(Cmd::Files, cx));
                }
                cx.notify();
            }
            _ => {}
        })
        .detach();
        cx.observe(&model, |_, _, cx| cx.notify()).detach();
        cx.subscribe(&model, |_, _, _: &CodeEvent, _| {}).detach();
        let list = ListState::new(0, ListAlignment::Bottom, px(800.0));
        list.set_follow_mode(FollowMode::Tail);
        Self { model, app, list, sigs: Vec::new(), input, seen_rev: 0, flipped: if cfg!(debug_assertions) && std::env::var("WINFER_NATIVE_CODE_DEMO").is_ok_and(|v| v == "diff") { ["t1", "t2", "t3", "t4"].map(String::from).into() } else { HashSet::new() }, images: Rc::new(RefCell::new(HashMap::new())), popup: 0, picks: HashMap::new(), asked_files: false, ticking: false, born: HashMap::new(), seeded: false, picker: Default::default(), preview: None }
    }

    pub fn focus_composer(&self, window: &mut Window, cx: &mut App) {
        self.input.read(cx).focus_handle(cx).focus(window, cx);
    }

    fn draft(&self, cx: &App) -> String {
        self.input.read(cx).value().to_string()
    }

    fn set_input(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        let text = text.to_string();
        self.input.update(cx, |s, cx| s.set_value(text, window, cx));
    }

    /// The composer's Enter: completes a half-typed command, answers a question, or sends.
    fn enter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.draft(cx);
        let t = text.trim().to_string();
        if t.starts_with('/') && !t.contains(char::is_whitespace) {
            let matches = commands::matching(&t);
            if !matches.iter().any(|(c, _)| *c == t) {
                if let Some((c, _)) = matches.get(self.popup.min(matches.len().saturating_sub(1))) {
                    self.set_input(&format!("{c} "), window, cx);
                    return;
                }
            }
        }
        if let Some((_, path)) = self.mention(&text, cx) {
            let at = text.rfind('@').unwrap_or(0);
            self.set_input(&format!("{}@{path} ", &text[..at]), window, cx);
            return;
        }
        let model = self.model.clone();
        // A pending question takes the typed text as its answer.
        let question = model.read(cx).state.approvals.iter().find(|a| !a.questions.is_empty()).cloned();
        if let (Some(a), false) = (question, t.is_empty()) {
            model.update(cx, |m, cx| m.send(Cmd::Answer { id: a.id.clone(), answers: vec![t.clone()] }, cx));
            self.set_input("", window, cx);
            return;
        }
        if t.is_empty() && model.read(cx).attachments.is_empty() {
            return;
        }
        let effect = model.update(cx, |m, cx| m.run(&t, cx));
        self.set_input("", window, cx);
        self.model.update(cx, |m, _| m.set_draft(String::new()));
        if let Some(Effect::Copy(text)) = effect {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
            self.model.update(cx, |m, cx| {
                m.state.notice("Copied the last reply.");
                cx.notify();
            });
        } else if let Some(Effect::Sidebar(on)) = effect {
            self.model.update(cx, |m, cx| {
                m.plan_sidebar = on.unwrap_or(!m.plan_sidebar);
                let on = m.plan_sidebar;
                m.state.notice(if on { "Plan sidebar on (shows when the pane is wide enough)" } else { "Plan sidebar off" });
                cx.notify();
            });
        }
        self.after_command(cx);
    }

    /// Runs a command picked from the help sheet.
    pub(super) fn run_from_sheet(&mut self, cmd: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.set_input(cmd, window, cx);
        self.enter(window, cx);
    }

    fn after_command(&mut self, cx: &mut Context<Self>) {
        let sheet = self.model.read(cx).state.sheet;
        match sheet {
            Some(Sheet::Usage) => {
                let tokens = self.model.read(cx).state.usage.context;
                self.model.update(cx, |m, cx| m.send(Cmd::ContextBreakdown { tokens }, cx));
            }
            _ => {}
        }
    }

    /// `@mention` being typed at the end of the text: (query, first match).
    fn mention(&self, text: &str, cx: &App) -> Option<(String, String)> {
        let at = text.rfind('@')?;
        let query = &text[at + 1..];
        if query.contains(char::is_whitespace) || (at > 0 && !text[..at].ends_with(char::is_whitespace)) {
            return None;
        }
        let files = self.model.read(cx).state.files.clone()?;
        let q = query.to_lowercase();
        let hit = files.iter().find(|f| f.to_lowercase().contains(&q))?;
        Some((query.to_string(), hit.clone()))
    }

    fn mention_matches(&self, text: &str, cx: &App) -> Vec<String> {
        let Some(at) = text.rfind('@') else { return Vec::new() };
        let query = &text[at + 1..];
        if query.contains(char::is_whitespace) || (at > 0 && !text[..at].ends_with(char::is_whitespace)) {
            return Vec::new();
        }
        let q = query.to_lowercase();
        self.model.read(cx).state.files.as_ref().map_or(Vec::new(), |f| f.iter().filter(|p| p.to_lowercase().contains(&q)).take(8).cloned().collect())
    }

    pub(super) fn set_sheet(&mut self, sheet: Option<Sheet>, cx: &mut Context<Self>) {
        self.model.update(cx, |m, cx| {
            m.state.set_sheet(sheet);
            cx.notify();
        });
        if sheet == Some(Sheet::Usage) {
            self.after_command(cx);
        }
        if matches!(sheet, Some(Sheet::Model | Sheet::SubagentModel)) && self.model.read(cx).state.models.is_empty() {
            // The process lists models on its own; nothing to ask for.
        }
    }

    pub(super) fn send(&mut self, cmd: Cmd, cx: &mut Context<Self>) {
        self.model.update(cx, |m, cx| m.send(cmd, cx));
    }

    fn toggle(&mut self, key: &str, cx: &mut Context<Self>) {
        if !self.flipped.remove(key) {
            self.flipped.insert(key.to_string());
        }
        cx.notify();
    }

    fn attach_paths(&mut self, paths: Vec<std::path::PathBuf>, cx: &mut Context<Self>) {
        for path in paths {
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let mime = match path.extension().and_then(|e| e.to_str()).map(str::to_lowercase).as_deref() {
                Some("png") => "image/png",
                Some("jpg" | "jpeg") => "image/jpeg",
                Some("gif") => "image/gif",
                Some("webp") => "image/webp",
                _ => "",
            };
            let attachment = if mime.is_empty() {
                Attachment { data: String::new(), mime: String::new(), name, path: Some(path.to_string_lossy().into_owned()) }
            } else {
                let Ok(bytes) = std::fs::read(&path) else { continue };
                Attachment { data: base64::engine::general_purpose::STANDARD.encode(bytes), mime: mime.into(), name, path: None }
            };
            self.model.update(cx, |m, _| m.attachments.push(attachment));
        }
        cx.notify();
    }

    fn pick_files(&mut self, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions { files: true, directories: false, multiple: true, prompt: Some("Attach".into()) });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = paths.await {
                let _ = this.update(cx, |this, cx| this.attach_paths(paths, cx));
            }
        })
        .detach();
    }

    fn tick(&mut self, cx: &mut Context<Self>) {
        if self.ticking {
            return;
        }
        self.ticking = true;
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                let keep = this.update(cx, |this, cx| {
                    let busy = this.model.read(cx).state.busy;
                    if busy {
                        cx.notify();
                    } else {
                        this.ticking = false;
                    }
                    busy
                });
                if !keep.unwrap_or(false) {
                    break;
                }
            }
        })
        .detach();
    }

    fn prefs(&self, cx: &App) -> (f32, bool, bool, bool, bool, bool, bool, bool) {
        let s = self.app.read(cx);
        let get = |k: &str, d: bool| s.rest.get("codeWeb").and_then(|v| v[k].as_bool()).unwrap_or(d);
        let scale = s.rest.get("codeWeb").and_then(|v| v["textScale"].as_f64()).unwrap_or(1.0) as f32;
        (scale, get("compact", false), get("expandThinking", false), get("expandShell", false), get("showCost", true), get("showContext", true), get("showRate", true), get("showPlan", true))
    }

    /// Splices what changed since the last frame into the list state.
    fn sync_list(&mut self, items: &[Item], now: u64) {
        let sigs: Vec<u64> = items.iter().map(|i| signature(i, &self.flipped, now)).collect();
        let first = sigs.iter().zip(&self.sigs).position(|(a, b)| a != b).unwrap_or(sigs.len().min(self.sigs.len()));
        if first < self.sigs.len() || sigs.len() != self.sigs.len() {
            self.list.splice(first..self.sigs.len(), sigs.len() - first);
        }
        // Rows present on the first frame (a resumed session) don't animate in.
        let stamp = if self.seeded || items.is_empty() { now } else { 0 };
        for i in items {
            self.born.entry(i.key().to_string()).or_insert(stamp);
        }
        self.seeded |= !items.is_empty();
        self.sigs = sigs;
    }
}

/// The transcript and composer share one centered column.
pub(super) const COLUMN: f32 = 780.0;

fn alpha(c: Hsla, a: f32) -> Hsla {
    Hsla { a, ..c }
}

fn ease_out(t: f32) -> f32 {
    1.0 - (1.0 - t).powi(4)
}

/// 0..1 breathing curve for a repeating animation's progress.
fn breathe(t: f32) -> f32 {
    0.5 + 0.5 * (t * std::f32::consts::TAU).sin()
}

/// Fades an element in and lifts it into place, `delay` ms after it mounts.
pub(super) fn rise<E: Styled + IntoElement + 'static>(el: E, id: impl Into<ElementId>, delay: u64, lift: f32) -> AnimationElement<E> {
    const MS: f32 = 380.0;
    let total = delay as f32 + MS;
    el.with_animation(id, Animation::new(Duration::from_millis(total as u64)), move |d, t| {
        let e = ease_out(((t * total - delay as f32) / MS).clamp(0.0, 1.0));
        d.opacity(e).relative().top(px(lift * (1.0 - e)))
    })
}

fn spinner(color: Hsla, size: f32, id: impl Into<ElementId>) -> AnimationElement<Icon> {
    Icon::new(IconName::LoaderCircle).size(px(size)).text_color(color)
        .with_animation(id, Animation::new(Duration::from_millis(900)).repeat().with_max_fps(30.0), |i, t| i.rotate(radians(t * std::f32::consts::TAU)))
}

/// Three dots pulsing in turn.
fn dots(color: Hsla, id: &str) -> AnimationElement<Div> {
    div().flex().items_center().gap(px(3.0)).with_animation(SharedString::from(id.to_string()), Animation::new(Duration::from_millis(1100)).repeat().with_max_fps(30.0), move |d, t| {
        d.children((0..3).map(|i| div().size(px(4.0)).rounded_full().bg(color).opacity(0.25 + 0.75 * breathe((t + 1.0 - i as f32 * 0.16) % 1.0))))
    })
}

/// A column-width wrapper: content centered, never wider than `COLUMN`.
fn col(el: impl IntoElement) -> Div {
    div().w_full().flex().justify_center().px_4().child(div().w_full().max_w(px(COLUMN)).child(el))
}

fn pill(c: Colors, id: impl Into<ElementId>, icon: Option<AnyElement>, label: impl Into<SharedString>) -> Stateful<Div> {
    let mut d = div().id(id).h(px(26.0)).flex().items_center().gap_1p5().rounded_lg().px_2().text_size(px(12.0)).text_color(c.muted).cursor_pointer().hover(move |d| d.bg(c.hover).text_color(c.fg));
    if let Some(i) = icon {
        d = d.child(i);
    }
    d.child(label.into())
}

/// Plays the entrance animation on rows that appeared in the last moment.
fn enter(env: &Env, key: &str, el: Div) -> AnyElement {
    if env.fresh(key) { rise(el, SharedString::from(format!("in-{key}")), 0, 10.0).into_any_element() } else { el.into_any_element() }
}

fn user_row(env: &Env, text: &str, images: &[String], key: &str, first: bool) -> AnyElement {
    let c = env.c;
    let decoded: Vec<Arc<Image>> = {
        let mut cache = env.images.borrow_mut();
        cache.entry(key.to_string()).or_insert_with(|| images.iter().filter_map(|u| data_image(u)).collect()).clone()
    };
    let top = if first { 0.0 } else if env.compact { 10.0 } else { 22.0 };
    enter(env, key, div().w_full().flex().flex_col().items_end().pt(px(top)).pb(px(6.0)).child(
        div().max_w(relative(0.82)).flex().flex_col().gap_2().rounded_2xl().bg(alpha(c.accent, 0.13)).border_1().border_color(alpha(c.accent, 0.28)).px_3p5().py_2p5().text_size(env.text()).line_height(relative(1.5)).text_color(c.fg)
            .children(decoded.into_iter().enumerate().map(|(i, image)| {
                let (view, full) = (env.view.clone(), image.clone());
                div().id(("user-img", i)).max_w(px(300.0)).max_h(px(220.0)).rounded_lg().overflow_hidden().cursor_pointer()
                    .on_click(move |_, _, cx| {
                        let full = full.clone();
                        let _ = view.update(cx, |v, cx| {
                            v.preview = Some(full);
                            cx.notify();
                        });
                    })
                    .child(gpui_kit::img(image).max_h(px(220.0)))
            }))
            .when(!text.is_empty(), |d| d.child(div().whitespace_normal().child(text.to_string()))),
    ))
}

fn text_row(env: &Env, key: &str, text: &str, _streaming: bool) -> AnyElement {
    let c = env.c;
    enter(env, key, div().w_full().py(px(if env.compact { 4.0 } else { 8.0 })).px_0p5().text_size(px(13.5 * env.scale)).line_height(relative(1.62)).text_color(c.fg)
        .child(TextView::markdown(SharedString::from(format!("cw-md-{key}")), SharedString::from(crate::code::links::linkify(text, env.cwd.as_deref()))).selectable(true).on_link_click({
            let view = env.view.clone();
            move |href, _, _, cx| {
                let href = href.to_string();
                let _ = view.update(cx, |_, cx| cx.emit(OpenLink(href)));
            }
        })))
}

fn reasoning_row(env: &Env, key: &str, text: &str, streaming: bool, started: u64, ended: Option<u64>) -> AnyElement {
    let c = env.c;
    let open = env.open(key, streaming || env.expand_thinking);
    let secs = ended.unwrap_or(env.now).saturating_sub(started);
    let label = if streaming { "Thinking…".to_string() } else { format!("Thought for {}", elapsed(secs)) };
    let label = if started == 0 && !streaming { "Thought".to_string() } else { label };
    let (view, k) = (env.view.clone(), key.to_string());
    let icon = Icon::new(IconName::Brain).size(px(13.0)).text_color(if streaming { c.accent } else { c.subtle });
    let name = div().text_size(env.small()).text_color(if streaming { c.accent } else { c.muted }).child(label);
    let head = div().id(SharedString::from(format!("r-{key}"))).flex().items_center().gap_1p5().h(px(26.0)).px_1p5().ml(px(-6.0)).rounded_lg().cursor_pointer().hover(move |d| d.bg(c.hover))
        .on_click(move |_, _, cx| {
            let _ = view.update(cx, |v, cx| v.toggle(&k, cx));
        })
        .child(if streaming { icon.with_animation(SharedString::from(format!("rp-{key}")), Animation::new(Duration::from_millis(1600)).repeat().with_max_fps(30.0), |i, t| i.opacity(0.45 + 0.55 * breathe(t))).into_any_element() } else { icon.into_any_element() })
        .child(if streaming { name.with_animation(SharedString::from(format!("rn-{key}")), Animation::new(Duration::from_millis(1600)).repeat().with_max_fps(30.0), |d, t| d.opacity(0.55 + 0.45 * breathe(t))).into_any_element() } else { name.into_any_element() })
        .child(Icon::new(if open { IconName::ChevronDown } else { IconName::ChevronRight }).size(px(11.0)).text_color(c.subtle));
    enter(env, key, div().w_full().flex().flex_col().py(px(2.0)).child(div().flex().child(head))
        .when(open, |d| d.child(div().ml(px(7.0)).mt_1().mb_1().pl_3().border_l_2().border_color(c.border_strong).py_0p5().text_size(env.small()).line_height(relative(1.6)).text_color(c.muted).whitespace_normal().child(text.trim().to_string()))))
}

fn diff_block(env: &Env, diff: &str, max: usize) -> Div {
    let c = env.c;
    let mut block = div().flex().flex_col().rounded_lg().border_1().border_color(c.border).bg(c.app).overflow_hidden().py_1().font_family(env.mono.clone()).text_size(env.small());
    let (mut o, mut n) = (0usize, 0usize);
    let mut shown = 0;
    let lines: Vec<&str> = diff.split('\n').collect();
    for line in &lines {
        if line.starts_with("---") || line.starts_with("+++") {
            continue;
        }
        if shown >= max {
            break;
        }
        let (bg, fg, sign, num) = if let Some(h) = line.strip_prefix("@@") {
            let nums: Vec<usize> = h.split(|ch: char| !ch.is_ascii_digit()).filter(|s| !s.is_empty()).filter_map(|s| s.parse().ok()).collect();
            if nums.len() >= 2 {
                o = nums[0];
                n = if nums.len() >= 3 { nums[2] } else { nums[1] };
            }
            (alpha(c.accent, 0.08), c.accent, "", String::new())
        } else if line.starts_with('+') {
            n += 1;
            (alpha(c.success, 0.14), c.fg, "+", (n - 1).to_string())
        } else if line.starts_with('-') {
            o += 1;
            (alpha(c.danger, 0.14), c.fg, "−", (o - 1).to_string())
        } else {
            o += 1;
            n += 1;
            (transparent_black(), c.muted, "", (n - 1).to_string())
        };
        let sign_color = if sign == "+" { c.success } else if sign == "−" { c.danger } else { fg };
        let body = if line.starts_with("@@") { line.to_string() } else { line.chars().skip(1).collect() };
        block = block.child(
            div().flex().w_full().bg(bg).child(div().w(px(40.0)).flex_shrink_0().pr_2().text_right().text_color(c.subtle).child(num)).child(div().w(px(14.0)).flex_shrink_0().text_color(sign_color).child(sign))
                .child(div().flex_1().min_w_0().whitespace_nowrap().overflow_hidden().pr_2().text_color(fg).child(if body.is_empty() { " ".to_string() } else { body })),
        );
        shown += 1;
    }
    if lines.len() > max {
        block = block.child(div().px_3().py_1().text_color(c.subtle).child(format!("… {} more lines", lines.len() - max)));
    }
    block
}

fn diff_counts(diff: &str) -> (usize, usize) {
    let add = diff.lines().filter(|l| l.starts_with('+') && !l.starts_with("+++")).count();
    let del = diff.lines().filter(|l| l.starts_with('-') && !l.starts_with("---")).count();
    (add, del)
}

/// Output (and the command that made it) as a terminal: the last 200 lines.
fn terminal_block(env: &Env, command: Option<&str>, output: &str) -> Div {
    let c = env.c;
    let out = output.trim_end();
    let shown: String = if out.len() > 6000 { format!("…{}", out[out.len() - 6000..].trim_start_matches(|ch: char| !ch.is_ascii())) } else { out.to_string() };
    let lines: Vec<String> = shown.lines().map(str::to_string).collect();
    let skip = lines.len().saturating_sub(200);
    div().rounded_lg().border_1().border_color(c.border).bg(c.app).px_3().py_2().flex().flex_col().font_family(env.mono.clone()).text_size(env.small()).line_height(relative(1.5)).text_color(c.muted)
        .when_some(command, |d, cmd| d.child(div().whitespace_normal().text_color(c.fg).child(format!("$ {cmd}"))))
        .when(skip > 0, |d| d.child(div().text_color(c.subtle).child(format!("… {skip} earlier lines"))))
        .children(lines.into_iter().skip(skip).map(|l| div().whitespace_normal().child(if l.is_empty() { " ".to_string() } else { l })))
}

/// File contents with line numbers.
fn code_block(env: &Env, text: &str, max: usize) -> Div {
    let c = env.c;
    let lines: Vec<&str> = text.lines().collect();
    let width = lines.len().min(max).to_string().len().max(2);
    div().rounded_lg().border_1().border_color(c.border).bg(c.app).py_1p5().flex().flex_col().overflow_hidden().font_family(env.mono.clone()).text_size(env.small()).line_height(relative(1.5))
        .children(lines.iter().take(max).enumerate().map(|(i, l)| {
            div().flex().gap_3().px_3()
                .child(div().w(px(8.0 * width as f32)).flex_shrink_0().text_color(c.subtle).child(format!("{:>width$}", i + 1)))
                .child(div().flex_1().min_w_0().whitespace_normal().text_color(c.fg).child(if l.is_empty() { " ".to_string() } else { l.to_string() }))
        }))
        .when(lines.len() > max, |d| d.child(div().px_3().text_color(c.subtle).child(format!("… {} more lines", lines.len() - max))))
}

/// `name  value` rows: a tool's arguments.
fn fields_block(env: &Env, rows: Vec<(String, String)>) -> Div {
    let c = env.c;
    div().rounded_lg().border_1().border_color(c.border).bg(c.app).px_3().py_2().flex().flex_col().gap_1().text_size(env.small())
        .children(rows.into_iter().map(|(k, v)| {
            div().flex().gap_3().items_start()
                .child(div().w(px(84.0)).flex_shrink_0().text_color(c.subtle).child(k))
                .child(div().flex_1().min_w_0().whitespace_normal().font_family(env.mono.clone()).text_color(c.fg).child(v))
        }))
}

/// The send button: a small painted nebula (`ui::nebula`) in a round, glowing frame; ember-red while
/// it can interrupt, dim when there's nothing to send.
fn nebula_send(c: Colors, stop: bool, ready: bool, animate: bool, icon: IconName) -> Stateful<Div> {
    use super::nebula::{self, Kind};
    let live = ready || stop;
    let kind = if stop { Kind::Stop } else if ready { Kind::Ready } else { Kind::Idle };
    let glow = if stop { theme::hex("#fb7185") } else { theme::hex("#a855f7") };
    let mut shadows = vec![BoxShadow { color: alpha(gpui_kit::white(), if live { 0.14 } else { 0.05 }), offset: point(px(0.0), px(1.0)), blur_radius: px(1.0), spread_radius: px(0.0), inset: true }];
    if live {
        shadows.push(BoxShadow { color: alpha(glow, 0.4), offset: point(px(0.0), px(2.0)), blur_radius: px(12.0), spread_radius: px(0.0), inset: false });
    }
    // While the session works the clouds drift: a loop of pre-painted frames, each faded into the
    // next so the motion is smooth at 60 fps.
    let face: AnyElement = if stop && animate {
        div().absolute().top_0().left_0().size_full().rounded_full().overflow_hidden()
            .with_animation("send-drift", Animation::new(Duration::from_millis(6000)).repeat().with_max_fps(60.0), move |d, t| {
                let frames = nebula::working_frames();
                let pos = (t * frames.len() as f32).min(frames.len() as f32 - 0.001);
                let (i, mix) = (pos as usize, pos.fract());
                d.child(gpui_kit::img(frames[i].clone()).size_full().rounded_full().object_fit(ObjectFit::Cover))
                    .child(gpui_kit::img(frames[(i + 1) % frames.len()].clone()).absolute().top_0().left_0().size_full().rounded_full().object_fit(ObjectFit::Cover).opacity(mix))
            })
            .into_any_element()
    } else {
        gpui_kit::img(nebula::image(kind)).absolute().top_0().left_0().size_full().rounded_full().object_fit(ObjectFit::Cover).into_any_element()
    };
    div().id("cw-send").relative().size(px(32.0)).flex_shrink_0().flex().items_center().justify_center().rounded_full()
        .shadow(shadows)
        .child(face)
        .child(Icon::new(icon).size(px(14.0)).text_color(if live { gpui_kit::white() } else { c.subtle }))
        .when(live, |d| d.cursor_pointer().hover(|d| d.opacity(0.9)))
}

fn tool_tint(c: Colors, name: &str) -> Hsla {
    match name {
        "Read" | "Grep" | "Glob" => c.accent,
        "Edit" | "Write" => c.success,
        "Bash" => c.warning,
        "Agent" => c.claude,
        "WebFetch" | "WebSearch" => c.accent,
        _ => c.muted,
    }
}

fn tool_row(env: &Env, key: &str, name: &str, title: &str, input: &serde_json::Value, output: &str, ok: Option<bool>, diff: Option<&str>, started: u64, ended: Option<u64>) -> AnyElement {
    let c = env.c;
    let running = ended.is_none() && ok.is_none();
    let default_open = running && name == "Bash" && env.expand_shell;
    let open = env.open(key, default_open);
    let tint = if ok == Some(false) { c.danger } else { tool_tint(c, name) };
    let status: AnyElement = match ok {
        Some(true) => Icon::new(IconName::Check).size(px(13.0)).text_color(c.success).into_any_element(),
        Some(false) => Icon::new(IconName::X).size(px(13.0)).text_color(c.danger).into_any_element(),
        None => spinner(c.accent, 13.0, SharedString::from(format!("sp-{key}"))).into_any_element(),
    };
    let right = match (ended, started) {
        (Some(e), s) if s > 0 && e >= s => Some(elapsed(e - s)),
        (None, s) if s > 0 => Some(elapsed(env.now.saturating_sub(s))),
        _ => None,
    };
    let counts = diff.map(diff_counts).filter(|(a, d)| a + d > 0);
    let (view, k) = (env.view.clone(), key.to_string());
    let head = div().id(SharedString::from(format!("t-{key}"))).w_full().flex().items_center().gap_2().h(px(34.0)).px_2p5().cursor_pointer().hover(move |d| d.bg(alpha(c.hover, 0.6)))
        .on_click(move |_, _, cx| {
            let _ = view.update(cx, |v, cx| v.toggle(&k, cx));
        })
        .child(div().size(px(22.0)).flex_shrink_0().flex().items_center().justify_center().rounded_md().bg(alpha(tint, 0.14)).child(Icon::new(tool_icon(name)).size(px(13.0)).text_color(tint)))
        .child(div().flex_shrink_0().text_size(env.small()).text_color(c.fg).child(name.to_string()))
        .child(div().flex_1().min_w_0().truncate().font_family(env.mono.clone()).text_size(px(11.5 * env.scale)).text_color(c.muted).child(title.to_string()))
        .when_some(counts, |d, (a, r)| d.child(div().flex().flex_shrink_0().gap_1().text_size(px(11.0)).font_family(env.mono.clone()).child(div().text_color(c.success).child(format!("+{a}"))).child(div().text_color(c.danger).child(format!("−{r}")))))
        .when_some(right, |d, r| d.child(div().flex_shrink_0().text_size(px(11.0)).text_color(c.subtle).child(r)))
        .child(div().w(px(14.0)).flex_shrink_0().flex().items_center().justify_center().child(status))
        .child(Icon::new(if open { IconName::ChevronDown } else { IconName::ChevronRight }).size(px(11.0)).text_color(c.subtle));
    let mut card = div().w_full().rounded_xl().border_1().border_color(if running { alpha(c.accent, 0.4) } else { c.border }).bg(c.raised).overflow_hidden().child(head);
    if open {
        let secs = toolbody::sections(name, input, output, diff);
        let mut body = div().border_t_1().border_color(c.border).p_2().flex().flex_col().gap_2();
        if secs.is_empty() {
            body = body.child(div().px_1().py_0p5().text_size(env.small()).text_color(c.subtle).child(if running { "Waiting for output…" } else { "No details for this call." }));
        }
        let mut it = secs.into_iter().peekable();
        while let Some(sec) = it.next() {
            body = body.child(match sec {
                Section::Diff(d) => diff_block(env, &d, 400).into_any_element(),
                Section::Code(t) => code_block(env, &t, 300).into_any_element(),
                Section::Fields(f) => fields_block(env, f).into_any_element(),
                Section::Command(cmd) => {
                    // The command and its output read as one terminal.
                    let out = it.next_if(|s| matches!(s, Section::Output(_)));
                    let out = out.and_then(|s| if let Section::Output(o) = s { Some(o) } else { None });
                    terminal_block(env, Some(&cmd), out.as_deref().unwrap_or("")).into_any_element()
                }
                Section::Output(o) => terminal_block(env, None, &o).into_any_element(),
            });
        }
        card = card.child(body);
    }
    let card: AnyElement = if running {
        let id = SharedString::from(format!("glow-{key}"));
        card.with_animation(id, Animation::new(Duration::from_millis(1800)).repeat().with_max_fps(30.0), move |d, t| d.border_color(alpha(c.accent, 0.18 + 0.4 * breathe(t)))).into_any_element()
    } else {
        card.into_any_element()
    };
    enter(env, key, div().w_full().py(px(if env.compact { 1.5 } else { 3.0 })).child(card))
}

fn notice_row(env: &Env, key: &str, text: &str, error: bool) -> AnyElement {
    let c = env.c;
    let (icon, color) = if error { (IconName::TriangleAlert, c.danger) } else { (IconName::Info, c.subtle) };
    enter(env, key, div().w_full().flex().justify_center().py_1p5().child(
        div().max_w(relative(0.9)).flex().items_start().gap_1p5().rounded_xl().px_3().py_1p5().border_1().border_color(if error { alpha(c.danger, 0.4) } else { c.border }).bg(if error { alpha(c.danger, 0.08) } else { c.raised }).text_size(env.small()).line_height(relative(1.5)).text_color(if error { c.danger } else { c.muted })
            .child(div().mt(px(2.0)).child(Icon::new(icon).size(px(12.0)).text_color(color)))
            .child(div().min_w_0().whitespace_normal().child(text.to_string())),
    ))
}

fn worked_row(env: &Env, key: &str, ms: u64) -> AnyElement {
    let c = env.c;
    enter(env, key, div().w_full().flex().items_center().gap_3().pt_3().pb_1().text_size(px(11.0)).text_color(c.subtle)
        .child(div().h(px(1.0)).flex_1().bg(c.border))
        .child(div().flex().items_center().gap_1().child(Icon::new(IconName::Check).size(px(11.0)).text_color(c.success)).child(format!("Worked for {}", elapsed(ms))))
        .child(div().h(px(1.0)).flex_1().bg(c.border)))
}

pub(super) fn render_item(env: &Env, item: &Item, first: bool) -> AnyElement {
    match item {
        Item::User { key, text, images, .. } => user_row(env, text, images, key, first),
        Item::Text { key, text, streaming } => text_row(env, key, text, *streaming),
        Item::Reasoning { key, text, streaming, started, ended } => reasoning_row(env, key, text, *streaming, *started, *ended),
        Item::Tool { key, name, title, input, output, ok, diff, started, ended } => tool_row(env, key, name, title, input, output, *ok, diff.as_deref(), *started, *ended),
        Item::Notice { key, text, error, .. } => notice_row(env, key, text, *error),
        Item::Worked { key, ms, .. } => worked_row(env, key, *ms),
    }
}

const SUGGESTIONS: [(IconName, &str, &str); 4] = [
    (IconName::Compass, "Explore this codebase", "Give me a tour of this project: structure, entry points and how it fits together."),
    (IconName::Bug, "Find and fix a bug", "Look for a likely bug in this project and fix it, explaining what was wrong."),
    (IconName::FlaskConical, "Write tests", "Find the least-tested important code here and add good tests for it."),
    (IconName::Lightbulb, "Suggest improvements", "Review this project and suggest the three most valuable improvements."),
];

impl CodeView {
    fn welcome(&self, c: Colors, cx: &mut Context<Self>) -> AnyElement {
        let m = self.model.read(cx);
        let st = &m.state;
        let spec = &st.model;
        let name = if spec.is_empty() { "…".to_string() } else { model_name(spec, st.actual_model.as_deref()) };
        let backend = backend_of(spec).map_or("", |b| b.1);
        let ready = st.session.is_some() && st.exited.is_none();
        let status = if let Some(e) = st.exited.clone() { e } else if st.session.is_none() { "Starting…".to_string() } else { format!("{name} · {backend} · {}", st.mode.label()) };
        let cwd = st.cwd.clone().filter(|_| st.session.is_some()).map(|cwd| format!("{cwd}{}", st.branch.as_ref().map_or(String::new(), |b| format!("  ⎇ {b}"))));
        let mono = fonts::mono(cx);
        let mut grid = div().w_full().max_w(px(520.0)).flex().flex_wrap().gap_2().justify_center();
        for (i, (icon, title, prompt)) in SUGGESTIONS.into_iter().enumerate() {
            let card = div().id(("cw-suggest", i)).w(px(250.0)).flex().items_center().gap_2p5().rounded_xl().border_1().border_color(c.border).bg(c.raised).px_3().py_2p5().cursor_pointer().text_size(px(12.5)).text_color(c.muted)
                .hover(move |d| d.bg(c.hover).border_color(alpha(c.accent, 0.5)).text_color(c.fg))
                .when(!ready, |d| d.opacity(0.5))
                .on_click(cx.listener(move |this, _, window, cx| {
                    if !ready {
                        return;
                    }
                    this.set_input(prompt, window, cx);
                    this.enter(window, cx);
                }))
                .child(div().size(px(24.0)).flex_shrink_0().flex().items_center().justify_center().rounded_md().bg(alpha(c.accent, 0.13)).child(Icon::new(icon).size(px(13.0)).text_color(c.accent)))
                .child(title);
            grid = grid.child(rise(card, SharedString::from(format!("sg-{i}")), 220 + i as u64 * 70, 10.0));
        }
        div().size_full().flex().flex_col().items_center().justify_center().gap_4().px_4().text_color(c.subtle)
            .child(rise(div().size(px(52.0)).flex().items_center().justify_center().rounded_2xl().bg(alpha(c.accent, 0.14)).border_1().border_color(alpha(c.accent, 0.35)).child(Icon::new(IconName::Sparkles).size(px(24.0)).text_color(c.accent)), "wl-logo", 0, 8.0))
            .child(rise(div().flex().flex_col().items_center().gap_1().child(div().text_size(px(22.0)).text_color(c.fg).child("What should we build?")).child(div().text_size(px(12.5)).child(status))
                .when_some(cwd, |d, cwd| d.child(div().mt_1().px_2().py_0p5().rounded_md().bg(c.raised).border_1().border_color(c.border).text_size(px(11.0)).font_family(mono).child(cwd))), "wl-title", 80, 8.0))
            .child(grid)
            .child(rise(div().text_size(px(11.0)).child("Type a message below, or / for commands and @ for files"), "wl-hint", 560, 4.0))
            .into_any_element()
    }

    fn render_plan(&self, c: Colors, cx: &mut Context<Self>) -> Option<AnyElement> {
        let m = self.model.read(cx);
        let st = &m.state;
        if st.todos.is_empty() {
            return None;
        }
        let (done, total) = plan_counts(&st.todos);
        let open = !self.flipped.contains("plan");
        let mut list = div().flex().flex_col().gap_1p5().px_3().pb_2p5().pt_0p5();
        for t in &st.todos {
            let (icon, color) = match t.status.as_str() {
                "completed" => (IconName::CircleCheck, c.success),
                "in_progress" => (IconName::LoaderCircle, c.accent),
                _ => (IconName::CircleDashed, c.subtle),
            };
            let glyph: AnyElement = if t.status == "in_progress" { spinner(color, 13.0, SharedString::from(format!("plan-sp-{}", t.content.len()))).into_any_element() } else { Icon::new(icon).size(px(13.0)).text_color(color).into_any_element() };
            list = list.child(div().flex().items_start().gap_2().text_size(px(12.5)).line_height(relative(1.45)).text_color(if t.status == "completed" { c.subtle } else { c.fg })
                .child(div().mt(px(2.0)).child(glyph))
                .child(div().flex_1().whitespace_normal().child(t.content.clone())));
        }
        let frac = if total == 0 { 0.0 } else { done as f32 / total as f32 };
        let head = div().id("cw-plan-head").flex().items_center().gap_2().h(px(34.0)).px_3().cursor_pointer().text_size(px(12.0)).text_color(c.muted)
            .on_click(cx.listener(|this, _, _, cx| this.toggle("plan", cx)))
            .child(Icon::new(IconName::ListTodo).size(px(13.0)).text_color(c.accent))
            .child(div().text_color(c.fg).child("Plan"))
            .child(div().w(px(56.0)).h(px(4.0)).rounded_full().bg(c.border).overflow_hidden().child(div().h_full().w(relative(frac)).rounded_full().bg(c.success)))
            .child(div().flex_1().text_color(c.subtle).child(format!("{done} of {total}")))
            .child(Icon::new(if open { IconName::ChevronDown } else { IconName::ChevronUp }).size(px(12.0)).text_color(c.subtle));
        Some(col(div().mb_2().rounded_xl().border_1().border_color(c.border).bg(c.raised).overflow_hidden().child(head).when(open, |d| d.child(list))).into_any_element())
    }

    fn render_approval(&mut self, a: &Approval, c: Colors, mono: SharedString, cx: &mut Context<Self>) -> AnyElement {
        let (id, model) = (a.id.clone(), self.model.clone());
        let decide = |label: &'static str, key_hint: &'static str, decision: Decision, primary: bool, key: String, model: Entity<CodeModel>, id: String| {
            div().id(SharedString::from(key)).h(px(30.0)).flex().items_center().gap_1p5().px_3().rounded_lg().text_size(px(12.5)).cursor_pointer()
                .when(primary, |d| d.bg(c.accent).text_color(c.on_accent).hover(|d| d.opacity(0.88)))
                .when(!primary, |d| d.border_1().border_color(c.border_strong).text_color(c.fg).hover(move |d| d.bg(c.hover)))
                .on_click(move |_, _, cx| {
                    let (id, decision) = (id.clone(), decision.clone());
                    model.update(cx, |m, cx| m.send(Cmd::Approve { id, decision }, cx));
                })
                .child(label)
                .child(div().text_size(px(10.0)).opacity(0.55).child(key_hint))
        };
        let mut body = div().flex_1().min_w_0().p_3().flex().flex_col().gap_2()
            .child(div().flex().items_center().gap_2().text_size(px(12.5)).text_color(c.warning).child(Icon::new(IconName::ShieldQuestionMark).size(px(15.0))).child(if a.questions.is_empty() { format!("{} wants to run", a.tool) } else { "winfer code is asking".to_string() }));
        if a.questions.is_empty() {
            body = body.child(div().rounded_lg().bg(c.app).border_1().border_color(c.border).px_3().py_2().font_family(mono.clone()).text_size(px(12.0)).text_color(c.fg).whitespace_normal().child(a.title.clone()));
            if !a.detail.trim().is_empty() && a.detail != a.title {
                body = body.child(div().font_family(mono.clone()).text_size(px(11.5)).text_color(c.muted).max_h(px(140.0)).overflow_hidden().whitespace_normal().child(a.detail.clone()));
            }
            if let Some(d) = a.diff.as_deref().filter(|d| !d.trim().is_empty()) {
                let env = Env { c, mono: mono.clone(), scale: 1.0, compact: true, view: cx.entity().downgrade(), flipped: Rc::new(HashSet::new()), expand_thinking: false, expand_shell: false, images: self.images.clone(), now: 0, cwd: None, born: Rc::new(HashMap::new()) };
                body = body.child(diff_block(&env, d, 40));
            }
            body = body.child(div().flex().gap_2().pt_0p5().child(decide("Allow", "Y", Decision::Allow, true, format!("allow-{id}"), model.clone(), id.clone())).child(decide("Always allow", "A", Decision::Always, false, format!("always-{id}"), model.clone(), id.clone())).child(decide("Deny", "N", Decision::Deny, false, format!("deny-{id}"), model.clone(), id.clone())));
        } else {
            let picks = self.picks.entry(a.id.clone()).or_insert_with(|| vec![Vec::new(); a.questions.len()]).clone();
            for (qi, q) in a.questions.iter().enumerate() {
                let mut options = div().flex().flex_wrap().gap_2();
                for (oi, o) in q.options.iter().enumerate() {
                    let on = picks.get(qi).is_some_and(|p| p.contains(&oi));
                    let (aid, multi) = (a.id.clone(), q.multi);
                    options = options.child(div().id(SharedString::from(format!("q-{}-{qi}-{oi}", a.id))).flex().flex_col().gap_0p5().rounded_lg().border_1().px_3().py_1p5().cursor_pointer().text_size(px(12.5))
                        .when(on, |d| d.border_color(c.accent).bg(alpha(c.accent, 0.14)).text_color(c.fg))
                        .when(!on, |d| d.border_color(c.border_strong).text_color(c.muted).hover(move |d| d.bg(c.hover).text_color(c.fg)))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(p) = this.picks.get_mut(&aid).and_then(|p| p.get_mut(qi)) {
                                if let Some(at) = p.iter().position(|x| *x == oi) {
                                    p.remove(at);
                                } else if multi {
                                    p.push(oi);
                                } else {
                                    *p = vec![oi];
                                }
                            }
                            cx.notify();
                        }))
                        .child(o.label.clone())
                        .when(!o.description.is_empty(), |d| d.child(div().text_size(px(11.0)).text_color(c.subtle).child(o.description.clone()))));
                }
                body = body.child(div().flex().flex_col().gap_1p5().child(div().text_size(px(10.5)).text_color(c.subtle).child(q.header.to_uppercase())).child(div().text_size(px(13.0)).text_color(c.fg).whitespace_normal().child(q.question.clone())).child(options));
            }
            let ready = a.questions.iter().enumerate().all(|(i, _)| picks.get(i).is_some_and(|p| !p.is_empty()));
            let (aid, qs) = (a.id.clone(), a.questions.clone());
            body = body.child(div().flex().items_center().gap_3().pt_0p5()
                .child(div().id(SharedString::from(format!("answer-{}", a.id))).h(px(30.0)).flex().items_center().px_3().rounded_lg().text_size(px(12.5)).bg(c.accent).text_color(c.on_accent)
                    .when(ready, |d| d.cursor_pointer().hover(|d| d.opacity(0.88))).when(!ready, |d| d.opacity(0.4))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !ready { return; }
                        let picks = this.picks.remove(&aid).unwrap_or_default();
                        let answers: Vec<String> = qs.iter().enumerate().map(|(i, q)| picks.get(i).map_or(String::new(), |p| p.iter().filter_map(|o| q.options.get(*o).map(|o| o.label.clone())).collect::<Vec<_>>().join(", "))).collect();
                        let id = aid.clone();
                        this.send(Cmd::Answer { id, answers }, cx);
                    }))
                    .child("Answer"))
                .child(div().text_size(px(11.0)).text_color(c.subtle).child("or type an answer below")));
        }
        let card = div().w_full().flex().mb_2().rounded_xl().border_1().border_color(alpha(c.warning, 0.55)).bg(c.raised).shadow_md().child(body);
        col(rise(card, SharedString::from(format!("appr-{}", a.id)), 0, 14.0)).into_any_element()
    }

    fn render_popup(&mut self, text: &str, c: Colors, cx: &mut Context<Self>) -> Option<AnyElement> {
        let t = text.trim_start();
        let mut rows: Vec<(String, String, String)> = Vec::new();
        if t.starts_with('/') && !t.contains(char::is_whitespace) {
            rows = commands::matching(t).into_iter().map(|(cmd, d)| (cmd.to_string(), d.to_string(), format!("{cmd} "))).collect();
        } else if let Some(at) = text.rfind('@') {
            let prefix = text[..at].to_string();
            rows = self.mention_matches(text, cx).into_iter().map(|f| (f.clone(), String::new(), format!("{prefix}@{f} "))).collect();
        }
        if rows.is_empty() {
            return None;
        }
        let selected = self.popup.min(rows.len() - 1);
        let mut list = div().flex().flex_col().max_h(px(236.0)).overflow_hidden().p_1();
        for (i, (label, hint, insert)) in rows.into_iter().enumerate() {
            list = list.child(div().id(("cw-pop", i)).flex().items_center().gap_3().px_2p5().h(px(28.0)).rounded_lg().cursor_pointer().text_size(px(12.5)).when(i == selected, |d| d.bg(alpha(c.accent, 0.14))).hover(move |d| d.bg(c.hover))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.set_input(&insert, window, cx);
                    this.focus_composer(window, cx);
                }))
                .child(div().font_family(fonts::mono(cx)).text_color(if i == selected { c.accent } else { c.fg }).child(label))
                .child(div().flex_1().min_w_0().truncate().text_size(px(11.5)).text_color(c.subtle).child(hint)));
        }
        Some(col(rise(div().mb_2().rounded_xl().border_1().border_color(c.border_strong).bg(c.raised).shadow_md().child(list), "cw-popup", 0, 6.0)).into_any_element())
    }

    /// Spinner row above the composer while a turn runs.
    fn render_status(&self, c: Colors, cx: &App) -> Option<AnyElement> {
        let st = &self.model.read(cx).state;
        if !st.busy {
            return None;
        }
        let secs = st.turn_started.map(|s| elapsed(now_ms().saturating_sub(s)));
        let rate = st.tokens_per_second();
        Some(col(rise(div().h(px(26.0)).flex().items_center().gap_2().px_1().text_size(px(11.5)).text_color(c.subtle)
            .child(dots(c.accent, "busy"))
            .child(div().text_color(c.muted).child("Working"))
            .when_some(secs, |d, s| d.child(s))
            .when_some(rate, |d, r| d.child(format!("· {r:.0} tok/s")))
            .child(div().text_color(c.subtle).child("· esc to interrupt")), "cw-status", 0, 4.0)).into_any_element())
    }

    fn render_composer(&mut self, c: Colors, cx: &mut Context<Self>) -> AnyElement {
        let (_, _, _, _, show_cost, show_context, _, _) = self.prefs(cx);
        let m = self.model.read(cx);
        let st = &m.state;
        let busy = st.busy;
        let attachments = m.attachments.clone();
        let animate_send = self.app.read(cx).rest.get("codeWeb").and_then(|v| v["sendAnimation"].as_bool()).unwrap_or(true);
        // Fast mode: the model's fast twin, on the backends that list one (the GPT/Codex models).
        // Codex lists a twin only for models with a fast tier; the button still shows on every Codex
        // model (dimmed when unlisted), since Codex maps any `-fast` model to its fast service tier.
        let fast = crate::code::names::fast_spec(&st.model, &st.models).map(|twin| (twin, true)).or_else(|| crate::code::names::fast_twin(&st.model).map(|t| (t, false))).map(|(twin, listed)| {
            let on = crate::code::names::is_fast(&st.model);
            (on, listed, if on { crate::code::names::base_spec(&st.model) } else { twin })
        });
        let model_label = if st.model.is_empty() { "model".to_string() } else { model_name(&st.model, st.actual_model.as_deref()) };
        let effort = if st.effort.is_empty() || st.effort == "default" { "default".to_string() } else { st.effort.clone() };
        let (mode, branch, cost, pct, window_tokens, context) = (st.mode, st.branch.clone(), st.usage.cost, st.context_fraction(), st.usage.window, st.usage.context);
        let draft_empty = self.draft(cx).trim().is_empty();
        let stop = busy && draft_empty;
        let can_send = !draft_empty || !attachments.is_empty();
        let v = cx.entity();
        let open = |sheet: Sheet| { let v = v.clone(); move |_: &ClickEvent, _: &mut Window, cx: &mut App| { let _ = v.update(cx, |this, cx| this.set_sheet(Some(sheet), cx)); } };
        let model_backend = crate::code::names::backend_of(&st.model).map(|b| b.0);
        let effort_id = if st.effort.is_empty() { "default".to_string() } else { st.effort.clone() };
        let open_sheet = st.sheet;
        let model_pop = (open_sheet == Some(Sheet::Model)).then(|| self.model_popover(c, cx));
        let effort_pop = (open_sheet == Some(Sheet::Effort)).then(|| self.effort_popover(c, cx));
        let mode_pop = (open_sheet == Some(Sheet::Mode)).then(|| self.mode_popover(c, cx));
        let mode_color = if matches!(mode, Mode::Yolo | Mode::YoloPlan) { c.warning } else { c.muted };

        let mut card = div().flex().flex_col().rounded_2xl().border_1().border_color(c.border_strong).bg(c.raised).shadow_md();
        if !attachments.is_empty() {
            let mut chips = div().flex().flex_wrap().gap_1p5().px_3().pt_2p5();
            for (i, a) in attachments.iter().enumerate() {
                chips = chips.child(div().id(("cw-att", i)).flex().items_center().gap_1p5().h(px(24.0)).pl_2().pr_1().rounded_lg().bg(c.hover).text_size(px(11.5)).text_color(c.muted)
                    .child(Icon::new(if a.mime.starts_with("image/") { IconName::Image } else { IconName::File }).size(px(12.0)))
                    .child(a.name.clone())
                    .child(div().id(("cw-att-x", i)).size(px(16.0)).flex().items_center().justify_center().rounded_md().cursor_pointer().hover(move |d| d.bg(c.active)).on_click(cx.listener(move |this, _, _, cx| { this.model.update(cx, |m, _| { if i < m.attachments.len() { m.attachments.remove(i); } }); cx.notify(); })).child(Icon::new(IconName::X).size(px(10.0)))));
            }
            card = card.child(chips);
        }
        card = card.child(div().px_3().pt_2p5().pb_1().child(Textarea::new(&self.input).appearance(false)));
        let toolbar = div().flex().items_center().gap_0p5().px_1p5().pb_1p5().h(px(36.0))
            .child(div().id("cw-attach").size(px(28.0)).flex().items_center().justify_center().rounded_lg().cursor_pointer().text_color(c.subtle).hover(move |d| d.bg(c.hover).text_color(c.fg)).tooltip(tip("Attach files or images")).on_click(cx.listener(|this, _, _, cx| this.pick_files(cx))).child(Icon::new(IconName::Paperclip).size(px(15.0))))
            .child(div().w(px(1.0)).h(px(14.0)).mx_1().bg(c.border))
            .child(div().relative().children(mode_pop).child(pill(c, "cw-mode", Some(super::code_pickers::mode_glyph(mode_color, mode, 14.0)), mode.label()).text_color(mode_color).when(open_sheet == Some(Sheet::Mode), |d| d.bg(c.hover).text_color(c.fg)).on_click(cx.listener(|this, _, _, cx| this.toggle_picker(Sheet::Mode, cx)))))
            .child(div().relative().children(model_pop).child(pill(c, "cw-model", model_backend.map(|b| super::code_pickers::mark(c, b, 14.0, 1.0)), model_label).when(open_sheet == Some(Sheet::Model), |d| d.bg(c.hover).text_color(c.fg)).on_click(cx.listener(|this, _, _, cx| this.toggle_picker(Sheet::Model, cx)))))
            .when_some(fast, |d, (on, listed, target)| d.child(
                pill(c, "cw-fast", Some(Icon::new(IconName::Zap).size(px(13.0)).text_color(if on { c.warning } else { c.subtle }).into_any_element()), "Fast")
                    .when(on, |d| d.text_color(c.warning).bg(alpha(c.warning, 0.12)))
                    .when(!on && !listed, |d| d.opacity(0.6))
                    .tooltip(tip(if on { "Fast mode is on: about 2x speed, higher usage. Click to turn off" } else if listed { "Turn on fast mode: about 2x speed, higher usage" } else { "Try fast mode. Codex doesn't list a fast tier for this model, so it may not apply" }))
                    .on_click(cx.listener(move |this, _, window, cx| this.pick_model(&target, window, cx))),
            ))
            .child(div().relative().children(effort_pop).child(pill(c, "cw-effort", Some(super::code_pickers::effort_glyph(c.muted, &effort_id, 14.0)), effort).when(open_sheet == Some(Sheet::Effort), |d| d.bg(c.hover).text_color(c.fg)).on_click(cx.listener(|this, _, _, cx| this.toggle_picker(Sheet::Effort, cx)))))
            .when_some(branch, |d, b| d.child(div().id("cw-branch").h(px(26.0)).max_w(px(160.0)).flex().items_center().gap_1p5().px_2().rounded_lg().text_size(px(12.0)).text_color(c.subtle)
                .tooltip(tip(format!("Branch {b}")))
                .child(Icon::new(IconName::GitBranch).size(px(13.0)).flex_shrink_0())
                .child(div().min_w_0().truncate().child(b))))
            .child(div().flex_1())
            .child(div().id("cw-usage").h(px(26.0)).flex().items_center().gap_2().px_2().mr_1().rounded_lg().cursor_pointer().text_size(px(11.0)).text_color(c.subtle).hover(move |d| d.bg(c.hover))
                .tooltip(tip(format!("{} / {} tokens", tokens(context), tokens(window_tokens))))
                .on_click(open(Sheet::Usage))
                .when(show_cost, |d| d.when_some(cost, |d, cost| d.child(format!("${cost:.2}"))))
                .when(show_context, |d| d.child(div().w(px(40.0)).h(px(4.0)).rounded_full().bg(c.border).overflow_hidden().child(div().h_full().w(relative(pct)).rounded_full().bg(if pct > 0.85 { c.danger } else if pct > 0.65 { c.warning } else { c.accent }))).child(format!("{}%", (pct * 100.0).round()))))
            
            .child(nebula_send(c, stop, can_send, animate_send, if stop { IconName::Square } else { IconName::ArrowUp })
                .tooltip(tip(if stop { "Interrupt (Esc)" } else if busy { "Steer the running turn (Enter)" } else { "Send (Enter)" }))
                .on_click(cx.listener(move |this, _, window, cx| if stop { this.send(Cmd::Interrupt, cx) } else { this.enter(window, cx) })));
        card = card.child(toolbar);
        col(div().pt_1().pb_2().child(card)).into_any_element()
    }
}

impl Render for CodeView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme_id = self.app.read(cx).theme.clone();
        let c = theme::colors(&theme_id);
        let mono = fonts::mono(cx);
        let (scale, compact, expand_thinking, expand_shell, .., show_plan) = self.prefs(cx);
        let now = now_ms();
        // Text typed from outside (send_to_pane, broadcast) shows up in the composer.
        let (rev, draft) = { let m = self.model.read(cx); (m.draft_rev, m.draft.clone()) };
        if rev != self.seen_rev {
            self.seen_rev = rev;
            self.set_input(&draft, window, cx);
        }
        let (items, approvals, busy, exited, sheet) = {
            let m = self.model.read(cx);
            (m.state.items.clone(), m.state.approvals.clone(), m.state.busy, m.state.exited.clone(), m.state.sheet)
        };
        if busy {
            self.tick(cx);
        }
        self.ensure_picker(window, cx);
        self.sync_list(&items, now);
        let env = Env { c, mono: mono.clone(), scale, compact, view: cx.entity().downgrade(), flipped: Rc::new(self.flipped.clone()), expand_thinking, expand_shell, images: self.images.clone(), now, cwd: self.model.read(cx).state.cwd.clone(), born: Rc::new(self.born.clone()) };
        let model = self.model.clone();
        let transcript: AnyElement = if items.is_empty() {
            self.welcome(c, cx)
        } else {
            let list_env = env.clone();
            let count = items.len();
            list(self.list.clone(), move |ix, _, cx| {
                let item = model.read(cx).state.items.get(ix).cloned();
                match item {
                    Some(item) => div().w_full().flex().justify_center().px_4().child(div().w_full().max_w(px(COLUMN)).pt(px(if ix == 0 { 16.0 } else { 0.0 })).pb(px(if ix + 1 == count { 12.0 } else { 0.0 })).child(render_item(&list_env, &item, ix == 0))).into_any_element(),
                    None => div().into_any_element(),
                }
            }).size_full().into_any_element()
        };
        let text = self.draft(cx);
        let popup = self.render_popup(&text, c, cx);
        let plan = if show_plan { self.render_plan(c, cx) } else { None };
        let approval_cards: Vec<AnyElement> = approvals.iter().take(1).map(|a| self.render_approval(a, c, mono.clone(), cx)).collect();
        let more_approvals = approvals.len().saturating_sub(1);
        let status = self.render_status(c, cx);
        let composer = self.render_composer(c, cx);
        let preview = self.preview.clone().map(|image| {
            let scrim = div().id("cw-lightbox").absolute().top_0().left_0().size_full().flex().items_center().justify_center().bg(alpha(black(), 0.82)).p_6().cursor_pointer()
                .on_click(cx.listener(|this, _, _, cx| {
                    this.preview = None;
                    cx.notify();
                }))
                .child(gpui_kit::img(image).max_w_full().max_h_full().object_fit(ObjectFit::Contain));
            rise(scrim, "cw-lightbox-in", 0, 0.0)
        });
        let sheet_el = sheet.filter(|s| !matches!(s, Sheet::Model | Sheet::Effort | Sheet::Mode)).map(|s| self.render_sheet(s, c, mono.clone(), window, cx));
        let exited_note = exited.map(|e| col(div().mb_2().rounded_xl().border_1().border_color(alpha(c.danger, 0.5)).bg(alpha(c.danger, 0.08)).px_3().py_2().text_size(px(12.5)).text_color(c.danger).child(e)));
        div().id("code-pane").relative().size_full().flex().flex_col().bg(c.panel).text_color(c.fg).font_family(fonts::sans(cx))
            .on_key_down(cx.listener(|this, e: &KeyDownEvent, window, cx| {
                let k = e.keystroke.key.as_str();
                let digit = k.parse::<usize>().ok().filter(|n| (1..=9).contains(n));
                if let (true, Some(n)) = (e.keystroke.modifiers.platform && !e.keystroke.modifiers.shift && !e.keystroke.modifiers.alt && !e.keystroke.modifiers.control && this.model.read(cx).state.sheet == Some(Sheet::Model), digit) {
                    if this.pick_nth(n - 1, window, cx) {
                        cx.stop_propagation();
                    }
                } else if k == "escape" && this.preview.take().is_some() {
                    cx.notify();
                } else if k == "escape" {
                    if this.model.read(cx).state.sheet.is_some() {
                        this.set_sheet(None, cx);
                    } else if !this.draft(cx).is_empty() && this.draft(cx).trim_start().starts_with('/') {
                        this.set_input("", window, cx);
                    } else if this.model.read(cx).state.busy {
                        this.send(Cmd::Interrupt, cx);
                    }
                } else if k == "tab" && e.keystroke.modifiers.shift {
                    let cur = this.model.read(cx).state.mode;
                    let next = Mode::ALL[(Mode::ALL.iter().position(|m| *m == cur).unwrap_or(0) + 1) % Mode::ALL.len()];
                    this.send(Cmd::SetMode(next), cx);
                } else if this.draft(cx).is_empty() {
                    let first = this.model.read(cx).state.approvals.first().filter(|a| a.questions.is_empty()).map(|a| a.id.clone());
                    if let Some(id) = first {
                        let decision = match k { "y" => Some(Decision::Allow), "a" => Some(Decision::Always), "n" => Some(Decision::Deny), _ => None };
                        if let Some(decision) = decision {
                            this.send(Cmd::Approve { id, decision }, cx);
                            cx.stop_propagation();
                        }
                    }
                }
            }))
            .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| this.attach_paths(paths.paths().to_vec(), cx)))
            .child(div().flex_1().min_h_0().relative().child(transcript))
            .children(status)
            .children(plan)
            .children(approval_cards)
            .when(more_approvals > 0, |d| d.child(col(div().pb_2().px_1().text_size(px(11.0)).text_color(c.subtle).child(format!("{more_approvals} more waiting")))))
            .children(exited_note)
            .children(popup)
            .child(composer)
            .children(sheet_el)
            .children(preview)
    }
}
