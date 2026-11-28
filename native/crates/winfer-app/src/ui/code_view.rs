//! The winfer code chat pane: transcript, approvals, plan, composer and footer
//! (port of `features/code/CodePane.tsx`, `Transcript.tsx` and `Composer.tsx`).
//! Panels that open over the transcript live in `code_sheets.rs`.

use crate::code::bg_note::BgNote;
use crate::code::commands::{self, Effect};
use crate::code::names::{backend_of, model_name};
use crate::code::protocol::*;
use crate::code::session::{CodeEvent, CodeModel};
use crate::code::state::{Attachment, Item, Sheet, now_ms, plan_counts, plan_summary};
use crate::code::toolbody::{self, Section};
use crate::fonts;
use crate::model::AppState;
use crate::theme::{self, Colors};
use base64::Engine;
use gpui_kit::assets::IconName;
use gpui_kit::base::InteractiveElementExt as _;
use gpui_kit::component::{Icon, Sizable, Size as KitSize};
use gpui_kit::component::input::{Enter as InputEnter, Escape as InputEscape, IndentInline, InputEvent, MoveDown, MoveEnd, MoveHome, MovePageDown, MovePageUp, MoveUp, OutdentInline, Position, Textarea, TextareaState};
use gpui_kit::component::scroll::{Scrollbar, ScrollbarAxis};
use gpui_kit::component::text::{TextView, TextViewStyle};
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
    /// Rows currently animating an open (`true`) or close (`false`) toggle, with a monotonic id.
    pub toggled: Rc<HashMap<String, (u64, bool)>>,
    pub expand_thinking: bool,
    pub expand_shell: bool,
    pub images: Rc<RefCell<HashMap<String, Vec<Arc<Image>>>>>,
    pub wg_scrolls: Rc<RefCell<HashMap<String, ScrollHandle>>>,
    pub now: u64,
    pub cwd: Option<String>,
    pub remote: bool,
    /// When each row first appeared (ms), to play its entrance once.
    pub born: Rc<HashMap<String, u64>>,
    /// Width of a transcript row's content, in px.
    pub width: f32,
    /// Height of the code pane, in px.
    pub height: f32,
    /// In Home: the task numbers and machine names its text links.
    pub refs: Option<Rc<super::root::home_nav::Refs>>,
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

    pub(super) fn open(&self, key: &str, default: bool) -> bool {
        default != self.flipped.contains(key)
    }
}

/// A push-to-talk dictation in progress: what the draft was and where the words go.
struct Dictating {
    base: String,
    at: usize,
    started: u64,
    stopping: bool,
    /// Something was heard or went wrong; else a held press that ends says it heard nothing.
    heard: bool,
}

pub struct CodeView {
    dictation: Option<Dictating>,
    pub(super) model: Entity<CodeModel>,
    pub(super) app: Entity<AppState>,
    list: ListState,
    rows: Rc<Vec<DisplayRow>>,
    sigs: Vec<u64>,
    input: Entity<TextareaState>,
    seen_rev: u64,
    flipped: HashSet<String>,
    toggled: HashMap<String, (u64, bool)>,
    toggle_seq: u64,
    /// A card was opened at the bottom: scroll to the end once it is laid out so the whole body shows.
    scroll_end_pending: bool,
    pane_size: Size<Pixels>,
    /// Ctrl/⌘ + wheel distance not yet turned into a text size step.
    zoom_accum: Pixels,
    images: Rc<RefCell<HashMap<String, Vec<Arc<Image>>>>>,
    wg_scrolls: Rc<RefCell<HashMap<String, ScrollHandle>>>,
    popup: usize,
    popup_scroll: ScrollHandle,
    popup_closed_at: Option<usize>,
    /// Per approval id and question: the picked option indexes.
    picks: HashMap<String, Vec<Vec<usize>>>,
    asked_files: bool,
    ticking: bool,
    sky: Entity<SpaceSkyView>,
    born: HashMap<String, u64>,
    seeded: bool,
    pub(super) picker: super::code_pickers::PickerState,
    pub(super) usage_sheet: super::code_usage_sheet::UsageSheetState,
    pub(super) task_log_scroll: super::code_sheets::TaskLogScroll,
    /// An image open full size.
    preview: Option<Arc<Image>>,
    /// Key of the newest user message; a new one snaps the transcript back to the bottom.
    last_user: Option<String>,
    /// Row cap the composer currently grows to (follows the pane height).
    composer_rows: usize,
    /// How many of the newest rows are shown; older ones hide behind a "Load more" row.
    visible: usize,
    /// Rows currently hidden above the "Load more" row (0 = no such row).
    hidden: usize,
    /// Item count last frame: any change is new activity and re-caps the transcript.
    seen_items: usize,
    /// "Load more" was clicked: keep the reader's place when the rows are prepended.
    load_pending: bool,
    /// Pane-level focus handle used while a blocking sheet/lightbox is open so keys don't hit the composer.
    focus: FocusHandle,
    restore_composer_focus: bool,
    /// `plan_done_at` a hide timer is already running for.
    plan_hide_armed: Option<u64>,
    /// In Home: the task numbers and machine names its text links (see `home_nav::link_refs`).
    refs: Option<Rc<super::root::home_nav::Refs>>,
    /// The composer hint set by `set_placeholder` (None: the default one).
    hint: Option<String>,
}

/// How long a fully completed plan stays docked above the composer.
const PLAN_HIDE_MS: u64 = 15_000;

/// Rows shown initially and added per "Load more".
const PAGE_ROWS: usize = 30;

/// A link clicked in the transcript: a web address or a file path (with `:line`).
pub struct OpenLink(pub String);

impl EventEmitter<OpenLink> for CodeView {}

impl Focusable for CodeView {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.input.read(cx).focus_handle(cx)
    }
}

/// winfer code's text scale moved by `by`, clamped to 80–150% and rounded to 5% steps.
pub(crate) fn text_scale_step(scale: f64, by: f64) -> f64 {
    (((scale + by).clamp(0.8, 1.5) / 0.05).round() * 0.05 * 100.0).round() / 100.0
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
        "Swarm" => IconName::Network,
        "Swarm post" | "Swarm board" => IconName::MessageSquareText,
        "Swarm wait" => IconName::Clock,
        "Workflow" => IconName::Workflow,
        "WebFetch" | "WebSearch" => IconName::Globe,
        _ => IconName::Wrench,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum DisplayRow {
    Item(usize),
    /// Home's app events (messages it sent, by item index) with whether the orchestrator answered
    /// "[silent]"; drawn by `home_events`.
    Events { key: String, events: Vec<(usize, bool)> },
    WorkGroup {
        key: String,
        start: usize,
        end: usize,
        active: bool,
    },
    /// `visualize` calls made in a row (only work between them), drawn as one report.
    Report { key: String, items: Vec<usize> },
    Working,
}

impl DisplayRow {
    fn key<'a>(&'a self, items: &'a [Item]) -> &'a str {
        match self {
            DisplayRow::Item(ix) => items.get(*ix).map_or("", Item::key),
            DisplayRow::WorkGroup { key, .. } | DisplayRow::Report { key, .. } | DisplayRow::Events { key, .. } => key.as_str(),
            DisplayRow::Working => "cw-working",
        }
    }
}

fn is_work_item(item: &Item) -> bool {
    matches!(item, Item::Tool { .. } | Item::Reasoning { .. }) && !is_chart(item)
}

/// A `visualize` call drawn as a chart: its own row, never folded into a work group.
fn is_chart(item: &Item) -> bool {
    matches!(item, Item::Tool { name, input, ok, .. } if crate::code::viz::report_of(name, input, *ok).is_some())
}

/// Adds item `k` as a row of its own. A chart joins the report still `open` (one with only work
/// since it); anything else closes that report.
fn push_row(rows: &mut Vec<DisplayRow>, items: &[Item], k: usize, open: &mut Option<usize>) {
    if !is_chart(&items[k]) {
        *open = None;
        rows.push(DisplayRow::Item(k));
        return;
    }
    if let Some(DisplayRow::Report { items: charts, .. }) = open.and_then(|r| rows.get_mut(r)) {
        charts.push(k);
        return;
    }
    *open = Some(rows.len());
    rows.push(DisplayRow::Report { key: format!("rep-{}", items[k].key()), items: vec![k] });
}

pub(super) fn build_display_rows(items: &[Item], busy: bool, compact_tools: bool) -> Vec<DisplayRow> {
    if !compact_tools {
        let (mut out, mut open) = (Vec::new(), None);
        for k in 0..items.len() {
            push_row(&mut out, items, k, &mut open);
        }
        if busy {
            out.push(DisplayRow::Working);
        }
        return out;
    }

    let mut rows = Vec::new();
    let mut open: Option<usize>;
    let mut i = 0;
    while i < items.len() {
        if matches!(items[i], Item::User { .. }) {
            rows.push(DisplayRow::Item(i));
            i += 1;
            if i == items.len() && busy {
                rows.push(DisplayRow::Working);
            }
            continue;
        }

        let turn_start = i;
        open = None;
        while i < items.len() && !matches!(items[i], Item::User { .. }) {
            i += 1;
        }
        let turn_end = i;
        let is_last_turn = turn_end == items.len();

        // Each run of work between text, notices and charts gets its own group; the turn's total
        // "Worked for" row stays where it is. A finished run of one step shows that step directly.
        let mut k = turn_start;
        while k < turn_end {
            if !is_work_item(&items[k]) {
                push_row(&mut rows, items, k, &mut open);
                k += 1;
                continue;
            }
            let start = k;
            while k < turn_end && is_work_item(&items[k]) {
                k += 1;
            }
            let active = busy && is_last_turn && k == turn_end;
            // Not `push_row`: work between charts keeps their report open.
            rows.push(if k - start == 1 && !active {
                DisplayRow::Item(start)
            } else {
                DisplayRow::WorkGroup { key: format!("wg-{}", items[start].key()), start, end: k - 1, active }
            });
        }
        if is_last_turn && busy {
            let live_group = matches!(rows.last(), Some(DisplayRow::WorkGroup { active: true, .. }));
            let streaming_text = matches!(items.get(turn_end.wrapping_sub(1)), Some(Item::Text { streaming: true, .. }));
            if !live_group && !streaming_text {
                rows.push(DisplayRow::Working);
            }
        }
    }

    rows
}

fn group_duration_ms(items: &[Item], start: usize, end: usize) -> Option<u64> {
    let mut min_start = u64::MAX;
    let mut max_end = 0u64;
    for it in items.get(start..=end).unwrap_or(&[]) {
        match it {
            Item::Tool { started, ended, .. } | Item::Reasoning { started, ended, .. } => {
                if *started > 0 {
                    min_start = min_start.min(*started);
                }
                if let Some(e) = ended {
                    max_end = max_end.max(*e);
                }
            }
            _ => {}
        }
    }
    if min_start < u64::MAX && max_end >= min_start {
        Some((max_end - min_start).max(1000))
    } else {
        None
    }
}

fn truncate_chars(s: &str, max: usize) -> String {
    let clean: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if clean.chars().count() <= max {
        clean
    } else {
        let mut out: String = clean.chars().take(max.saturating_sub(1)).collect();
        while out.ends_with(' ') {
            out.pop();
        }
        out.push('…');
        out
    }
}

fn file_label(raw: &str) -> String {
    let first = raw.split_whitespace().next().unwrap_or("");
    let base = first.rsplit(['/', '\\']).next().unwrap_or(first);
    truncate_chars(base, 36)
}

fn host_label(raw: &str) -> String {
    let s = raw.trim();
    let stripped = s.strip_prefix("https://").or_else(|| s.strip_prefix("http://")).unwrap_or(s);
    let stripped = stripped.strip_prefix("www.").unwrap_or(stripped);
    let host = stripped.split('/').next().unwrap_or(stripped);
    truncate_chars(host, 36)
}

fn capitalize_first(s: String) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) => format!("{}{}", c.to_uppercase(), chars.as_str()),
        None => s,
    }
}

/// Short, subtle status description for the active compact row (no raw tool chrome).
fn describe_step(step: Option<&Item>) -> String {
    capitalize_first(describe_step_raw(step))
}

fn describe_step_raw(step: Option<&Item>) -> String {
    match step {
        None => "Working…".into(),
        Some(Item::Reasoning { streaming: true, .. }) => "Thinking…".into(),
        Some(Item::Reasoning { streaming: false, .. }) => "Working…".into(),
        Some(Item::Tool { name, title, input, .. }) => {
            let desc = input.get("description").and_then(|v| v.as_str()).map(str::trim).unwrap_or("");
            let title = title.trim();
            let lower = name.to_ascii_lowercase();
            match lower.as_str() {
                "read" | "ls" | "list" => {
                    let f = file_label(title);
                    if f.is_empty() { "Reading file".into() } else { format!("Reading {f}") }
                }
                "edit" | "multiedit" | "notebookedit" => {
                    let f = file_label(title);
                    if f.is_empty() { "Editing file".into() } else { format!("Editing {f}") }
                }
                "write" => {
                    let f = file_label(title);
                    if f.is_empty() { "Writing file".into() } else { format!("Writing {f}") }
                }
                "grep" => if title.is_empty() { "Searching files".into() } else { format!("Searching {}", truncate_chars(title, 36)) },
                "glob" | "find" => if title.is_empty() { "Finding files".into() } else { format!("Finding {}", truncate_chars(title, 36)) },
                "bash" | "shell" | "powershell" => {
                    if !desc.is_empty() {
                        truncate_chars(desc, 48)
                    } else {
                        let cmd = title.lines().next().map(str::trim).unwrap_or("");
                        if cmd.is_empty() { "Running command".into() } else { format!("Running {}", truncate_chars(cmd, 40)) }
                    }
                }
                "agent" | "task" => {
                    if !desc.is_empty() {
                        truncate_chars(desc, 48)
                    } else if !title.is_empty() {
                        format!("Running agent · {}", truncate_chars(title, 34))
                    } else {
                        "Running subagent".into()
                    }
                }
                "websearch" | "codesearch" => if title.is_empty() { "Searching the web".into() } else { format!("Searching web · {}", truncate_chars(title, 34)) },
                "webfetch" | "fetch" => {
                    let h = host_label(title);
                    if h.is_empty() { "Fetching page".into() } else { format!("Fetching {h}") }
                }
                "todowrite" | "updateplan" | "todos" | "taskcreate" | "taskupdate" => "Updating plan".into(),
                "askuserquestion" | "question" => "Asking a question".into(),
                "exitplanmode" => "Presenting plan".into(),
                "background" => if title.is_empty() { "Managing background task".into() } else { truncate_chars(title, 48) },
                n if n.ends_with(" · visualize") || n == "visualize" => {
                    if !title.is_empty() {
                        format!("Visualizing {}", truncate_chars(title, 36))
                    } else {
                        "Creating visualization".into()
                    }
                }
                _ => {
                    if !desc.is_empty() {
                        truncate_chars(desc, 48)
                    } else if !title.is_empty() {
                        truncate_chars(title, 48)
                    } else if !name.is_empty() {
                        format!("Running {}", truncate_chars(name, 36))
                    } else {
                        "Working…".into()
                    }
                }
            }
        }
        _ => "Working…".into(),
    }
}

/// Maximum height of an expanded compacted work group: 40% of the pane height.
fn work_group_max_h(pane_h: f32) -> f32 {
    let h = if pane_h <= 1.0 { 600.0 } else { pane_h };
    (h * 0.4).max(80.0).round()
}

fn row_signature(row: &DisplayRow, items: &[Item], env_flipped: &HashSet<String>, env_toggled: &HashMap<String, (u64, bool)>, now: u64, wg_max_h: u32) -> u64 {
    match row {
        DisplayRow::Item(ix) => items.get(*ix).map_or(0, |i| signature(i, env_flipped, env_toggled, now)),
        DisplayRow::Working => u64::MAX,
        DisplayRow::Events { key, events } => {
            let mut h = std::collections::hash_map::DefaultHasher::new();
            (key, events, env_flipped.contains(key), env_toggled.get(key)).hash(&mut h);
            for (ix, _) in events {
                if let Some(i) = items.get(*ix) {
                    signature(i, env_flipped, env_toggled, now).hash(&mut h);
                    for n in 0..4 {
                        let k = format!("evrep-{}-{n}", i.key());
                        (env_flipped.contains(&k), env_toggled.get(&k)).hash(&mut h);
                    }
                }
            }
            h.finish()
        }
        DisplayRow::Report { key, items: charts } => {
            let mut h = std::collections::hash_map::DefaultHasher::new();
            key.hash(&mut h);
            for ix in charts {
                items.get(*ix).map(|i| signature(i, env_flipped, env_toggled, now)).hash(&mut h);
            }
            h.finish()
        }
        DisplayRow::WorkGroup { key, start, end, active } => {
            let mut h = std::collections::hash_map::DefaultHasher::new();
            key.hash(&mut h);
            (*start, *end, *active).hash(&mut h);
            let open = env_flipped.contains(key);
            open.hash(&mut h);
            env_toggled.get(key).hash(&mut h);
            if open || env_toggled.contains_key(key) {
                wg_max_h.hash(&mut h);
                for it in items.get(*start..=*end).unwrap_or(&[]) {
                    signature(it, env_flipped, env_toggled, now).hash(&mut h);
                }
            } else {
                let slice = items.get(*start..=*end).unwrap_or(&[]);
                let mut tool_count = 0usize;
                let mut last_work: Option<&Item> = None;
                for it in slice {
                    match it {
                        Item::Tool { .. } => {
                            tool_count += 1;
                            last_work = Some(it);
                        }
                        Item::Reasoning { .. } => {
                            last_work = Some(it);
                        }
                        _ => {}
                    }
                }
                (tool_count, group_duration_ms(items, *start, *end)).hash(&mut h);
                if *active {
                    describe_step(last_work).hash(&mut h);
                }
            }
            h.finish()
        }
    }
}

fn signature(item: &Item, env_flipped: &HashSet<String>, env_toggled: &HashMap<String, (u64, bool)>, now: u64) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    item.key().hash(&mut h);
    env_flipped.contains(item.key()).hash(&mut h);
    env_toggled.get(item.key()).hash(&mut h);
    match item {
        Item::User { text, images, .. } => (text.len(), images.len()).hash(&mut h),
        Item::Text { text, streaming, .. } => (text.len(), *streaming).hash(&mut h),
        Item::Reasoning { text, streaming, ended, .. } => {
            (text.len(), *streaming, ended.is_some()).hash(&mut h);
            if *streaming {
                (now / 1000).hash(&mut h);
            }
        }
        Item::Tool { name, title, input, output, ok, diff, ended, .. } => {
            let desc = input.get("description").and_then(|v| v.as_str()).unwrap_or("");
            let input_keys = input.as_object().map_or(0, serde_json::Map::len);
            (name.as_str(), title.as_str(), desc, input_keys, output.len(), *ok, diff.as_ref().map(String::len), ended.is_some()).hash(&mut h);
            if ended.is_none() {
                (now / 1000).hash(&mut h);
            }
        }
        Item::Notice { text, .. } => text.len().hash(&mut h),
        Item::Handoff { from_model, from_actual, to_model, .. } => {
            (from_model.as_str(), from_actual.as_deref(), to_model.as_str(), model_name(from_model, from_actual.as_deref()), model_name(to_model, None)).hash(&mut h);
        }
        Item::Worked { ms, .. } => ms.hash(&mut h),
    }
    h.finish()
}

pub(super) fn data_image(url: &str) -> Option<Arc<Image>> {
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

fn encode_png(bytes: &[u8]) -> Option<Vec<u8>> {
    let img = image::load_from_memory(bytes).ok()?;
    let mut out = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png).ok()?;
    Some(out)
}

pub(super) fn attachment_from_path(path: &std::path::Path) -> Option<Attachment> {
    if path.is_dir() {
        return None;
    }
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).filter(|n| !n.is_empty())?;
    let ext = path.extension().and_then(|e| e.to_str()).map(str::to_lowercase);
    let direct_mime = match ext.as_deref() {
        Some("png") => Some("image/png"),
        Some("jpg" | "jpeg") => Some("image/jpeg"),
        Some("gif") => Some("image/gif"),
        Some("webp") => Some("image/webp"),
        _ => None,
    };
    if let Some(mime) = direct_mime {
        let bytes = std::fs::read(path).ok()?;
        return Some(Attachment { data: base64::engine::general_purpose::STANDARD.encode(bytes), mime: mime.into(), name, path: None });
    }
    if matches!(ext.as_deref(), Some("bmp" | "tif" | "tiff")) {
        if let Ok(raw) = std::fs::read(path) {
            if let Some(png) = encode_png(&raw) {
                return Some(Attachment { data: base64::engine::general_purpose::STANDARD.encode(png), mime: "image/png".into(), name, path: None });
            }
        }
    }
    Some(Attachment { data: String::new(), mime: String::new(), name, path: Some(path.to_string_lossy().into_owned()) })
}

pub(super) fn attachment_from_clipboard_image(img: &Image, existing: usize) -> Option<Attachment> {
    let (mime, ext, bytes) = match img.format {
        ImageFormat::Png => ("image/png", "png", img.bytes.clone()),
        ImageFormat::Jpeg => ("image/jpeg", "jpg", img.bytes.clone()),
        ImageFormat::Gif => ("image/gif", "gif", img.bytes.clone()),
        ImageFormat::Webp => ("image/webp", "webp", img.bytes.clone()),
        _ => ("image/png", "png", encode_png(&img.bytes)?),
    };
    if bytes.is_empty() {
        return None;
    }
    let name = if existing == 0 { format!("pasted-image.{ext}") } else { format!("pasted-image-{}.{ext}", existing + 1) };
    Some(Attachment { data: base64::engine::general_purpose::STANDARD.encode(bytes), mime: mime.into(), name, path: None })
}

impl CodeView {
    pub fn new(model: Entity<CodeModel>, app: Entity<AppState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let home = crate::voice::home_pane(&model.read(cx).pane_id) == Some(crate::voice::HomePane::Conductor);
        let placeholder = if home { "Ask Home…  (@ for workspaces, / for commands)" } else { "Message WinferCode…  (/ for commands, @ for files)" };
        let input = cx.new(|cx| TextareaState::new(window, cx).auto_grow(1, 3).soft_wrap(true).submit_on_enter(true).placeholder(placeholder));
        cx.subscribe_in(&input, window, |this, input, event: &InputEvent, window, cx| match event {
            InputEvent::Focus => {
                let pane_id = this.model.read(cx).pane_id.clone();
                // Home's chats run under ids no workspace lists: claiming the pane focus for one
                // would make Root's focus sync hand the keyboard to a terminal for that id.
                if crate::voice::is_orchestrator(&pane_id) {
                    return;
                }
                this.app.update(cx, |s, cx| {
                    if s.focused_pane_id.as_deref() != Some(pane_id.as_str())
                        || s.pane(&pane_id).is_some_and(|p| p.workspace_id != s.active_workspace_id)
                    {
                        s.focus_pane(&pane_id);
                        cx.notify();
                    }
                });
            }
            InputEvent::PressEnter { shift: false, secondary: false } => this.enter(window, cx),
            InputEvent::Change => {
                let text = input.read(cx).value().to_string();
                this.model.update(cx, |m, _| m.set_draft(text.clone()));
                this.popup = 0;
                this.popup_closed_at = None;
                this.popup_scroll.set_offset(point(px(0.0), px(0.0)));
                if text.contains('@') && !this.asked_files {
                    this.asked_files = true;
                    this.model.update(cx, |m, cx| m.send(Cmd::Files, cx));
                }
                cx.notify();
            }
            _ => {}
        })
        .detach();
        cx.observe(&model, |this, _, cx| {
            this.arm_plan_hide(cx);
            cx.notify();
        })
        .detach();
        cx.observe(&app, |_, _, cx| cx.notify()).detach();
        cx.subscribe(&model, |_, _, _: &CodeEvent, _| {}).detach();
        let list = ListState::new(0, ListAlignment::Bottom, px(800.0));
        list.set_follow_mode(FollowMode::Tail);
        let sky = cx.new(|cx| SpaceSkyView::new(model.clone(), app.clone(), cx));
        Self { dictation: None, model, app, list, rows: Rc::new(Vec::new()), sigs: Vec::new(), input, seen_rev: 0, flipped: if cfg!(debug_assertions) && std::env::var("WINFER_NATIVE_CODE_DEMO").is_ok_and(|v| v == "diff") { ["t1", "t2", "t3", "t4"].map(String::from).into() } else { HashSet::new() }, toggled: HashMap::new(), toggle_seq: 0, scroll_end_pending: false, pane_size: Size::default(), zoom_accum: Pixels::ZERO, images: Rc::new(RefCell::new(HashMap::new())), wg_scrolls: Rc::new(RefCell::new(HashMap::new())), popup: 0, popup_scroll: ScrollHandle::new(), popup_closed_at: None, picks: HashMap::new(), asked_files: false, ticking: false, sky, born: HashMap::new(), seeded: false, picker: Default::default(), usage_sheet: Default::default(), task_log_scroll: Default::default(), preview: None, last_user: None, composer_rows: 3, visible: PAGE_ROWS, hidden: 0, seen_items: 0, load_pending: false, focus: cx.focus_handle(), restore_composer_focus: false, plan_hide_armed: None, refs: None, hint: None }
    }

    /// Once the plan is complete, re-render after `PLAN_HIDE_MS` so `render_plan` drops it.
    fn arm_plan_hide(&mut self, cx: &mut Context<Self>) {
        let Some(done_at) = self.model.read(cx).state.plan_done_at else {
            self.plan_hide_armed = None;
            return;
        };
        if self.plan_hide_armed == Some(done_at) {
            return;
        }
        self.plan_hide_armed = Some(done_at);
        let wait = (done_at + PLAN_HIDE_MS).saturating_sub(now_ms()) + 50;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(wait)).await;
            let _ = this.update(cx, |_, cx| cx.notify());
        })
        .detach();
    }

    pub fn sky_view(&self) -> Entity<SpaceSkyView> {
        self.sky.clone()
    }

    /// True when a full-pane modal sheet or image lightbox is covering the chat.
    fn has_blocking_modal(&self, cx: &App) -> bool {
        self.preview.is_some()
            || self.model.read(cx).state.sheet.is_some_and(|s| !matches!(s, Sheet::Model | Sheet::Effort | Sheet::Mode))
    }

    pub fn focus_composer(&self, window: &mut Window, cx: &mut App) {
        if self.has_blocking_modal(cx) {
            self.focus.focus(window, cx);
            return;
        }
        // `TextareaState::focus` also (re)starts the caret blink; focusing the bare handle of a
        // freshly created pane can leave the caret hidden.
        self.input.update(cx, |input, cx| input.focus(window, cx));
    }

    fn cycle_mode(&mut self, cx: &mut Context<Self>) {
        let cur = self.model.read(cx).state.mode;
        let next = Mode::ALL[(Mode::ALL.iter().position(|m| *m == cur).unwrap_or(0) + 1) % Mode::ALL.len()];
        self.send(Cmd::SetMode(next), cx);
    }

    fn draft(&self, cx: &App) -> String {
        self.input.read(cx).value().to_string()
    }

    /// Nothing typed in the composer (Home's Esc goes back only then).
    pub(super) fn composer_empty(&self, cx: &App) -> bool {
        self.draft(cx).is_empty()
    }

    /// Home's task numbers and machine names, linked in this chat's text.
    pub(super) fn set_refs(&mut self, refs: super::root::home_nav::Refs, cx: &mut Context<Self>) {
        if self.refs.as_deref() != Some(&refs) {
            self.refs = Some(Rc::new(refs));
            cx.notify();
        }
    }

    /// The composer's hint, e.g. who it talks to when Home shows a task's agent.
    pub(super) fn set_placeholder(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.hint.as_deref() != Some(text) {
            self.hint = Some(text.to_string());
            let text = text.to_string();
            self.input.update(cx, |i, cx| i.set_placeholder(text, window, cx));
        }
    }

    fn set_input(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        let text = text.to_string();
        let line = text.matches('\n').count() as u32;
        let col = text.rsplit('\n').next().map_or(0, |s| s.encode_utf16().count()) as u32;
        self.popup = 0;
        self.popup_closed_at = None;
        self.popup_scroll.set_offset(point(px(0.0), px(0.0)));
        self.input.update(cx, |s, cx| {
            s.set_value(text.clone(), window, cx);
            if !text.is_empty() {
                s.set_cursor_position(Position::new(line, col), window, cx);
            }
        });
    }

    /// The composer's Enter: completes a half-typed command, answers a question, or sends.
    fn enter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.draft(cx);
        let t = text.trim().to_string();
        if t.starts_with('/') && !t.contains(char::is_whitespace) {
            let skills = self.model.read(cx).state.skills.clone();
            let matches = commands::matching_with_skills(&t, &skills);
            if let Some((c, _)) = matches.get(self.popup.min(matches.len().saturating_sub(1))) {
                if *c != t {
                    self.set_input(&format!("{c} "), window, cx);
                    return;
                }
            }
        }
        let mentions = self.mention_matches(&text, cx);
        if let Some((path, _)) = mentions.get(self.popup.min(mentions.len().saturating_sub(1))) {
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
                self.usage_sheet.open(super::code_usage_sheet::UsageTab::Overview);
                let tokens = self.model.read(cx).state.usage.context;
                if tokens > 0 {
                    self.model.update(cx, |m, cx| m.send(Cmd::ContextBreakdown { tokens }, cx));
                }
            }
            Some(Sheet::Compaction) => {
                self.usage_sheet.open(super::code_usage_sheet::UsageTab::Compaction);
                let tokens = self.model.read(cx).state.usage.context;
                if tokens > 0 {
                    self.model.update(cx, |m, cx| m.send(Cmd::ContextBreakdown { tokens }, cx));
                }
            }
            _ => {}
        }
    }

    /// `@mention` being typed at the end of the text: `(at_offset, query)`.
    fn mention_at(text: &str) -> Option<(usize, &str)> {
        let at = text.rfind('@')?;
        let query = &text[at + 1..];
        if query.contains(char::is_whitespace) || (at > 0 && !text[..at].ends_with(char::is_whitespace)) {
            return None;
        }
        Some((at, query))
    }

    /// What `@` offers: the workspaces (Home's chat), else the project's files. `(token, detail)`.
    fn mention_matches(&self, text: &str, cx: &App) -> Vec<(String, String)> {
        let Some((at, query)) = Self::mention_at(text) else { return Vec::new() };
        if self.popup_closed_at == Some(at) {
            return Vec::new();
        }
        let q = query.to_lowercase();
        let m = self.model.read(cx);
        if !m.mentions.is_empty() {
            // Starts-with first, then the rest that contain it (e.g. "web" finds "mac-mini/web").
            let (mut first, rest): (Vec<_>, Vec<_>) = m.mentions.iter().filter(|x| x.token.contains(&q)).partition(|x| x.token.starts_with(&q));
            first.extend(rest);
            return first.into_iter().map(|x| (x.token.clone(), x.detail.clone())).collect();
        }
        m.state.files.as_ref().map_or(Vec::new(), |f| f.iter().filter(|p| p.to_lowercase().contains(&q)).take(200).map(|p| (p.clone(), String::new())).collect())
    }

    fn popup_rows(&self, text: &str, cx: &App) -> Vec<(String, String, String)> {
        let t = text.trim_start();
        if t.starts_with('/') && !t.contains(char::is_whitespace) {
            let skills = &self.model.read(cx).state.skills;
            return commands::matching_with_skills(t, skills)
                .into_iter()
                .map(|(cmd, d)| {
                    let insert = format!("{cmd} ");
                    (cmd, d, insert)
                })
                .collect();
        }
        if let Some((at, _)) = Self::mention_at(text) {
            let prefix = &text[..at];
            return self
                .mention_matches(text, cx)
                .into_iter()
                .map(|(f, detail)| {
                    let insert = format!("{prefix}@{f} ");
                    (f, detail, insert)
                })
                .collect();
        }
        Vec::new()
    }

    /// PageUp/PageDown/Home/End scroll the transcript. Home/End only do so while the composer is
    /// empty, so they still move the caret in a draft (on macOS they always scroll, as in Cocoa).
    fn scroll_key(&mut self, key: &str, cx: &mut Context<Self>) -> bool {
        let state = &self.model.read(cx).state;
        if state.items.is_empty() || state.sheet.is_some() || self.preview.is_some() {
            return false;
        }
        let page = (self.list.viewport_bounds().size.height - px(48.0)).max(px(48.0));
        match key {
            "pageup" => self.list.scroll_by(-page),
            "pagedown" => self.list.scroll_by(page),
            "home" | "end" if !cfg!(target_os = "macos") && !self.draft(cx).is_empty() => return false,
            "home" => self.list.scroll_to(ListOffset { item_ix: 0, offset_in_item: px(0.0) }),
            "end" => self.list.scroll_to_end(),
            _ => return false,
        }
        cx.notify();
        true
    }

    fn move_popup(&mut self, down: bool, window: &Window, cx: &mut Context<Self>) -> bool {
        if !self.input.read(cx).focus_handle(cx).is_focused(window) {
            return false;
        }
        let len = self.popup_rows(&self.draft(cx), cx).len();
        if len == 0 {
            return false;
        }
        let cur = self.popup.min(len - 1);
        self.popup = if down { (cur + 1) % len } else { (cur + len - 1) % len };
        self.popup_scroll.scroll_to_item(self.popup);
        cx.notify();
        true
    }

    fn accept_popup(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if !self.input.read(cx).focus_handle(cx).is_focused(window) {
            return false;
        }
        let rows = self.popup_rows(&self.draft(cx), cx);
        if rows.is_empty() {
            return false;
        }
        let selected = self.popup.min(rows.len() - 1);
        let insert = rows[selected].2.clone();
        self.set_input(&insert, window, cx);
        self.focus_composer(window, cx);
        true
    }

    fn accept_popup_on_enter(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if !self.input.read(cx).focus_handle(cx).is_focused(window) {
            return false;
        }
        let text = self.draft(cx);
        let t = text.trim();
        if t.starts_with('/') && !t.contains(char::is_whitespace) {
            let skills = self.model.read(cx).state.skills.clone();
            let matches = commands::matching_with_skills(t, &skills);
            if let Some((c, _)) = matches.get(self.popup.min(matches.len().saturating_sub(1))) {
                if c != t {
                    self.set_input(&format!("{c} "), window, cx);
                    return true;
                }
            }
            return false;
        }
        let mentions = self.mention_matches(&text, cx);
        if let Some((path, _)) = mentions.get(self.popup.min(mentions.len().saturating_sub(1))) {
            let at = text.rfind('@').unwrap_or(0);
            self.set_input(&format!("{}@{path} ", &text[..at]), window, cx);
            return true;
        }
        false
    }

    fn dismiss_mention_popup(&mut self, window: &Window, cx: &mut Context<Self>) -> bool {
        if !self.input.read(cx).focus_handle(cx).is_focused(window) {
            return false;
        }
        let text = self.draft(cx);
        if let Some((at, _)) = Self::mention_at(&text) {
            if !self.mention_matches(&text, cx).is_empty() {
                self.popup_closed_at = Some(at);
                cx.notify();
                return true;
            }
        }
        false
    }

    pub(super) fn set_sheet(&mut self, sheet: Option<Sheet>, cx: &mut Context<Self>) {
        self.model.update(cx, |m, cx| {
            m.state.set_sheet(sheet);
            cx.notify();
        });
        if matches!(sheet, Some(Sheet::Usage | Sheet::Compaction)) {
            self.after_command(cx);
        }
        if matches!(sheet, Some(Sheet::Model | Sheet::SubagentModel)) && self.model.read(cx).state.models.is_empty() {
            // The process lists models on its own; nothing to ask for.
        }
    }

    pub(super) fn send(&mut self, cmd: Cmd, cx: &mut Context<Self>) {
        self.model.update(cx, |m, cx| m.send(cmd, cx));
    }

    pub(super) fn toggle(&mut self, key: &str, was_open: bool, cx: &mut Context<Self>) {
        if !self.flipped.remove(key) {
            self.flipped.insert(key.to_string());
        }
        let opening = !was_open;
        let at_end = self.list.is_following_tail() || self.list.is_scrolled_to_end() == Some(true) || self.list.logical_scroll_top().item_ix >= self.list.item_count();
        self.scroll_end_pending = opening && at_end;
        if opening {
            if let Some(handle) = self.wg_scrolls.borrow().get(key) {
                handle.set_offset(point(px(0.0), px(0.0)));
            }
        }
        // At the bottom the bottom-aligned list has no anchor and lays out from the end, so a growing
        // card would push everything above it up. Pin the current top so it pushes content below it down.
        if self.list.logical_scroll_top().item_ix >= self.list.item_count() {
            let top = -self.list.scroll_px_offset_for_scrollbar().y;
            self.list.scroll_to_end();
            let end = -self.list.scroll_px_offset_for_scrollbar().y;
            self.list.scroll_by(top - end);
        }
        self.list.pause_following_tail();
        self.toggle_seq = self.toggle_seq.wrapping_add(1);
        let seq = self.toggle_seq;
        let k = key.to_string();
        self.toggled.insert(k.clone(), (seq, opening));
        cx.notify();
        let wait = if opening { 230 } else { 175 };
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(wait)).await;
            let _ = this.update(cx, |this, cx| {
                if this.toggled.get(&k).is_some_and(|(s, _)| *s == seq) {
                    this.toggled.remove(&k);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn attach_paths(&mut self, paths: Vec<std::path::PathBuf>, cx: &mut Context<Self>) {
        let mut added = false;
        for path in paths {
            if let Some(attachment) = attachment_from_path(&path) {
                self.model.update(cx, |m, _| m.attachments.push(attachment));
                added = true;
            }
        }
        if added {
            cx.notify();
        }
    }

    /// Attaches copied files/documents or clipboard images from `item` to the composer.
    /// Returns `true` if clipboard media/files were attached (so the textarea shouldn't also paste text).
    fn paste_clipboard_item(&mut self, item: &ClipboardItem, cx: &mut Context<Self>) -> bool {
        let mut paths = Vec::new();
        let mut images = Vec::new();
        for entry in &item.entries {
            match entry {
                ClipboardEntry::ExternalPaths(ext) => paths.extend(ext.paths().iter().cloned()),
                ClipboardEntry::Image(img) => images.push(img),
                ClipboardEntry::String(_) => {}
            }
        }
        if !paths.is_empty() {
            let before = self.model.read(cx).attachments.len();
            self.attach_paths(paths, cx);
            return self.model.read(cx).attachments.len() > before;
        }
        if !images.is_empty() {
            let mut added = false;
            for img in images {
                let count = self.model.read(cx).attachments.iter().filter(|a| a.name.starts_with("pasted-image")).count();
                if let Some(att) = attachment_from_clipboard_image(img, count) {
                    self.model.update(cx, |m, _| m.attachments.push(att));
                    added = true;
                }
            }
            if added {
                cx.notify();
                return true;
            }
        }
        false
    }

    /// Pastes copied files/documents or clipboard images into the composer's attachments.
    /// Returns `true` if clipboard media/files were attached (so the textarea shouldn't also paste text).
    fn paste_attachments(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(item) = cx.read_from_clipboard() else { return false };
        self.paste_clipboard_item(&item, cx)
    }

    /// Mic button pressed: listen, and show what's heard in the composer as it comes.
    fn start_dictation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.dictation.is_some() || !crate::dictation::supported() {
            return;
        }
        let base = self.draft(cx);
        let at = base.chars().count();
        self.dictation = Some(Dictating { base, at, started: now_ms(), stopping: false, heard: false });
        let (tx, rx) = async_channel::unbounded();
        crate::dictation::start(tx);
        self.tick(cx);
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            while let Ok(ev) = rx.recv().await {
                let ended = ev == crate::dictation::Event::Ended;
                if this.update_in(cx, |this, window, cx| this.on_dictation(ev, window, cx)).is_err() {
                    crate::dictation::cancel();
                    break;
                }
                if ended {
                    break;
                }
            }
        })
        .detach();
    }

    /// Mic button released (or the pointer let go elsewhere): finish and keep the text.
    fn stop_dictation(&mut self, cx: &mut Context<Self>) {
        if let Some(d) = self.dictation.as_mut().filter(|d| !d.stopping) {
            d.stopping = true;
            crate::dictation::stop();
            cx.notify();
        }
    }

    fn on_dictation(&mut self, ev: crate::dictation::Event, window: &mut Window, cx: &mut Context<Self>) {
        use crate::dictation::Event;
        match ev {
            Event::Text { text, .. } => {
                if let Some(d) = self.dictation.as_mut() {
                    d.heard |= !text.trim().is_empty();
                    let (draft, _) = crate::dictation::insert(&d.base, d.at, &text);
                    self.set_input(&draft, window, cx);
                }
            }
            Event::Error(msg) => {
                if let Some(d) = self.dictation.as_mut() {
                    d.heard = true;
                }
                self.model.update(cx, |m, cx| {
                    m.state.notice(format!("Dictation: {msg}"));
                    cx.notify();
                });
            }
            Event::Ended => {
                // A quick tap is likely a miss; a real hold that caught nothing gets a hint.
                if self.dictation.take().is_some_and(|d| !d.heard && now_ms().saturating_sub(d.started) > 1200) {
                    self.model.update(cx, |m, cx| {
                        m.state.notice("Dictation: didn't hear anything. Check the microphone in System Settings → Sound → Input.");
                        cx.notify();
                    });
                }
                self.focus_composer(window, cx);
            }
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
                    let busy = this.live(cx);
                    if busy {
                        this.poll_task_log(cx);
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

    /// Something on screen counts time: a running turn, a subagent still running in the background, or dictation.
    fn live(&self, cx: &App) -> bool {
        let st = &self.model.read(cx).state;
        let tree = st.tree_agents(now_ms());
        let tree_needs_tick = !tree.is_empty() && !tree.iter().all(|t| t.waiting());
        st.busy || st.tasks.iter().any(|t| t.ev.agent.is_some() && t.running()) || st.live_log_task().is_some() || self.dictation.is_some() || tree_needs_tick
    }

    /// Asks again for the tail of a running task whose output sheet is open (called each tick).
    fn poll_task_log(&mut self, cx: &mut Context<Self>) {
        let id = self.model.read(cx).state.live_log_task().map(|t| t.ev.id.clone());
        if let Some(id) = id {
            self.model.update(cx, |m, cx| m.send(Cmd::TaskLog(id), cx));
        }
    }

    fn prefs(&self, cx: &App) -> (f32, bool, bool, bool, bool, bool, bool, bool, bool) {
        let s = self.app.read(cx);
        let get = |k: &str, d: bool| s.rest.get("codeWeb").and_then(|v| v[k].as_bool()).unwrap_or(d);
        let scale = s.rest.get("codeWeb").and_then(|v| v["textScale"].as_f64()).unwrap_or(1.0) as f32;
        (scale, get("compact", false), get("expandThinking", false), get("expandShell", false), get("showCost", true), get("showContext", true), get("showRate", true), get("showPlan", true), get("showBgNotes", true))
    }

    /// Steps `codeWeb.textScale` (Settings → winfer code → Text size) by 5% per wheel notch.
    fn zoom_by_wheel(&mut self, delta: Pixels, cx: &mut Context<Self>) {
        const NOTCH: f32 = 20.0;
        self.zoom_accum += delta;
        let steps = (self.zoom_accum / px(NOTCH)).trunc();
        if steps == 0.0 {
            return;
        }
        self.zoom_accum -= px(steps * NOTCH);
        let scale = self.prefs(cx).0 as f64;
        let next = text_scale_step(scale, steps as f64 * 0.05);
        if next == scale {
            return;
        }
        self.app.update(cx, |s, cx| {
            let entry = s.rest.entry("codeWeb".to_string()).or_insert_with(|| serde_json::json!({}));
            if !entry.is_object() {
                *entry = serde_json::json!({});
            }
            entry["textScale"] = serde_json::json!(next);
            cx.notify();
        });
    }

    fn compact_tools(&self, cx: &App) -> bool {
        if cfg!(debug_assertions) && std::env::var("WINFER_NATIVE_CODE_DEMO").is_ok_and(|v| v == "diff") {
            return false;
        }
        self.app.read(cx).rest.get("codeWeb").and_then(|v| v["compactTools"].as_bool()).unwrap_or(true)
    }

    fn load_more(&mut self, cx: &mut Context<Self>) {
        self.visible += PAGE_ROWS;
        self.load_pending = true;
        cx.notify();
    }

    /// Splices what changed since the last frame into the list state.
    fn sync_list(&mut self, items: &[Item], busy: bool, compact_tools: bool, now: u64, home: bool) {
        let mut rows = build_display_rows(items, busy, compact_tools);
        if home {
            rows = super::home_events::fold(items, rows);
        }
        if items.len() != self.seen_items {
            self.seen_items = items.len();
            self.visible = PAGE_ROWS;
            self.load_pending = false;
        }
        let (old_hidden, old_lm) = (self.hidden, (self.hidden > 0) as usize);
        let hidden = rows.len().saturating_sub(self.visible);
        rows.drain(..hidden);
        self.hidden = hidden;
        let lm = (hidden > 0) as usize;
        let wg_max_h = work_group_max_h(self.pane_size.height / px(1.0)) as u32;
        let mut sigs: Vec<u64> = Vec::with_capacity(rows.len() + lm);
        if hidden > 0 {
            sigs.push(0x10AD_0000_0000_0000 ^ hidden as u64);
        }
        sigs.extend(rows.iter().map(|r| row_signature(r, items, &self.flipped, &self.toggled, now, wg_max_h)));
        let first = sigs.iter().zip(&self.sigs).position(|(a, b)| a != b).unwrap_or(sigs.len().min(self.sigs.len()));
        if first < self.sigs.len() || sigs.len() != self.sigs.len() {
            // Only re-measure the rows that changed, so rows after a toggled card keep their heights.
            let max_suffix = sigs.len().min(self.sigs.len()) - first;
            let suffix = sigs.iter().rev().zip(self.sigs.iter().rev()).take(max_suffix).take_while(|(a, b)| a == b).count();
            let old_range = first..self.sigs.len() - suffix;
            let top = self.list.logical_scroll_top();
            let count = sigs.len() - suffix - first;
            self.list.splice(old_range.clone(), count);
            // `splice` resets the anchor to the top of a replaced row; when rows were only re-rendered in place,
            // keep the offset within it instead.
            if old_range.contains(&top.item_ix) && old_range.len() == count {
                self.list.scroll_to(top);
            }
            if std::mem::take(&mut self.load_pending) && old_hidden > hidden {
                // Older rows were prepended: stay on the row that was at the top.
                let shift = old_hidden - hidden;
                let at_lm = top.item_ix < old_lm;
                self.list.scroll_to(ListOffset { item_ix: top.item_ix.saturating_sub(old_lm) + shift + lm, offset_in_item: if at_lm { px(0.0) } else { top.offset_in_item } });
            }
        }
        // Rows present on the first frame (a resumed session) don't animate in.
        let stamp = if self.seeded || items.is_empty() { now } else { 0 };
        for i in items {
            self.born.entry(i.key().to_string()).or_insert(stamp);
        }
        for r in &rows {
            let k = r.key(items);
            if !k.is_empty() {
                self.born.entry(k.to_string()).or_insert(stamp);
            }
        }
        self.seeded |= !items.is_empty();
        self.rows = Rc::new(rows);
        self.sigs = sigs;
    }
}

/// The transcript and composer share one centered column.
pub(super) const COLUMN: f32 = 780.0;

fn mix_color(base: Hsla, over: Hsla, t: f32) -> Hsla {
    let (a, b) = (Rgba::from(base), Rgba::from(over));
    let inv = 1.0 - t;
    Rgba { r: a.r * inv + b.r * t, g: a.g * inv + b.g * t, b: a.b * inv + b.b * t, a: a.a * inv + b.a * t }.into()
}

pub(super) fn alpha(c: Hsla, a: f32) -> Hsla {
    Hsla { a, ..c }
}

pub(super) fn ease_out(t: f32) -> f32 {
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

/// A column-width wrapper: content centered, never wider than `COLUMN`.
fn col(el: impl IntoElement) -> Div {
    col_inset(el, false)
}

fn col_inset(el: impl IntoElement, tight: bool) -> Div {
    let wrap = div().w_full().min_w_0().flex().justify_center();
    let wrap = if tight { wrap.px_2() } else { wrap.px_4() };
    wrap.child(div().w_full().min_w_0().child(el))
}

/// Edge-aligned dock wrapper so the composer (and its attached strips) matches left/right/bottom pane margins.
fn dock_inset(el: impl IntoElement, tight: bool) -> Div {
    let edge = if tight { px(6.0) } else { px(8.0) };
    div().w_full().min_w_0().px(edge).child(el)
}

fn pill(c: Colors, id: impl Into<ElementId>, icon: Option<AnyElement>, label: Option<SharedString>, h: f32) -> Stateful<Div> {
    let icon_only = label.is_none();
    let mut d = div().id(id).h(px(h)).flex().items_center().justify_center().rounded_lg().text_size(px(12.0)).text_color(c.muted).cursor_pointer().hover(move |d| d.bg(c.hover).text_color(c.fg));
    d = if icon_only { d.px_1p5() } else { d.gap_1p5().px_2() };
    if let Some(i) = icon {
        d = d.child(i);
    }
    if let Some(text) = label {
        // Fixed line box + 1px nudge: glyphs without descenders ("Sonnet") otherwise ride high next to the icon.
        d = d.child(div().min_w_0().truncate().line_height(px(14.0)).pt(px(1.0)).child(text));
    }
    d
}

/// Chevron that smoothly rotates 0 ↔ 90° when a row is toggled open or closed.
pub(super) fn toggle_chevron(key: &str, open: bool, toggled: Option<(u64, bool)>, color: Hsla) -> AnyElement {
    let icon = Icon::new(IconName::ChevronRight).size(px(11.0)).text_color(color);
    if let Some((seq, opening)) = toggled {
        let dur = if opening { 200 } else { 160 };
        let id = SharedString::from(format!("ch-{key}-{seq}"));
        icon.with_animation(id, Animation::new(Duration::from_millis(dur)), move |i, t| {
            let e = ease_out(t);
            let frac = if opening { e } else { 1.0 - e };
            i.rotate(radians(frac * std::f32::consts::FRAC_PI_2))
        })
        .into_any_element()
    } else if open {
        icon.rotate(radians(std::f32::consts::FRAC_PI_2)).into_any_element()
    } else {
        icon.into_any_element()
    }
}

/// Subtle fade + vertical glide for a tool/reasoning body opening or closing.
pub(super) fn toggle_body(key: &str, toggled: Option<(u64, bool)>, body: Div) -> AnyElement {
    if let Some((seq, opening)) = toggled {
        let dur = if opening { 210 } else { 160 };
        let id = SharedString::from(format!("tb-{key}-{seq}"));
        let animated = body.with_animation(id, Animation::new(Duration::from_millis(dur)), move |d, t| {
            let e = ease_out(t);
            if opening {
                d.opacity(e).relative().top(px(-5.0 * (1.0 - e)))
            } else {
                d.opacity(1.0 - e).relative().top(px(-4.0 * e))
            }
        });
        div().overflow_hidden().child(animated).into_any_element()
    } else {
        body.into_any_element()
    }
}

/// Responsive layout flags for the composer as the pane shrinks horizontally or vertically.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ComposerLayout {
    tight_x: bool,
    show_branch: bool,
    show_meter: bool,
    show_labels: bool,
    show_model_label: bool,
    show_cost: bool,
    show_token_metrics: bool,
    show_rate_inline: bool,
    show_usage: bool,
    compact_v: bool,
    ultra_v: bool,
    single_row: bool,
}

fn composer_layout(w: f32, h: f32) -> ComposerLayout {
    let w = if w <= 1.0 { 800.0 } else { w };
    let h = if h <= 1.0 { 600.0 } else { h };
    let compact_v = h < 380.0;
    let ultra_v = h < 270.0;
    let single_row = ultra_v && w >= 420.0;
    ComposerLayout {
        tight_x: w < 440.0,
        show_branch: w >= 620.0 && !ultra_v,
        show_meter: w >= 540.0 && !ultra_v,
        show_labels: w >= 500.0 && !ultra_v,
        show_model_label: w >= 360.0 && !single_row,
        show_cost: w >= 420.0 && !ultra_v,
        show_token_metrics: w >= 560.0 && !ultra_v,
        show_rate_inline: w >= 660.0 && !ultra_v,
        show_usage: w >= 260.0 && !single_row,
        compact_v,
        ultra_v,
        single_row,
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct WelcomeLayout {
    compact: bool,
    show_cwd: bool,
    short_cwd: bool,
    suggestions: usize,
    two_col: bool,
    compact_cards: bool,
    show_hint: bool,
}

fn welcome_layout(w: f32, h: f32) -> WelcomeLayout {
    let w = if w <= 1.0 { 800.0 } else { w };
    let h = if h <= 1.0 { 600.0 } else { h };
    let two_col = w >= 540.0;
    let compact = w < 440.0 || h < 520.0;
    let compact_cards = !two_col || h < 560.0;
    let suggestions = if w < 220.0 || h < 360.0 {
        0
    } else if two_col {
        if h >= 410.0 { 4 } else { 0 }
    } else if h >= 500.0 {
        4
    } else if h >= 410.0 {
        2
    } else {
        0
    };
    WelcomeLayout {
        compact,
        show_cwd: h >= 260.0,
        short_cwd: w < 460.0 || h < 440.0,
        suggestions,
        two_col,
        compact_cards,
        show_hint: w >= 420.0 && h >= 520.0,
    }
}

/// Plays the entrance animation on rows that appeared in the last moment.
pub(super) fn enter(env: &Env, key: &str, el: Div) -> AnyElement {
    if env.fresh(key) { rise(el, SharedString::from(format!("in-{key}")), 0, 10.0).into_any_element() } else { el.into_any_element() }
}

fn user_row(env: &Env, text: &str, images: &[String], key: &str, first: bool) -> AnyElement {
    let c = env.c;
    let decoded: Vec<Arc<Image>> = {
        let mut cache = env.images.borrow_mut();
        cache.entry(key.to_string()).or_insert_with(|| images.iter().filter_map(|u| data_image(u)).collect()).clone()
    };
    let top = if first { 0.0 } else if env.compact { 10.0 } else { 22.0 };
    let full = list_wraps(text, 13.0 * env.scale, env.width * 0.82 - 30.0);
    enter(env, key, div().w_full().flex().flex_col().items_end().pt(px(top)).pb(px(6.0)).child(
        div().max_w(relative(0.82)).when(full, |d| d.w(relative(0.82))).flex().flex_col().gap_2().rounded_2xl().bg(alpha(c.accent, 0.13)).border_1().border_color(alpha(c.accent, 0.28)).px_3p5().py_2p5().text_size(env.text()).line_height(relative(1.5)).text_color(c.fg)
            // Only add the image row when there are images: an empty row still takes a flex gap and pushes the text down.
            .when(!decoded.is_empty(), |d| d.child(div().flex().flex_row().flex_wrap().items_start().gap_1p5().children(decoded.into_iter().enumerate().map(|(i, image)| {
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
            }))))
            .when(!text.is_empty(), |d| d.child(
                // Selectable like assistant text (mark, then ⌘/Ctrl+C). Keep the user's line breaks.
                TextView::markdown(SharedString::from(format!("cw-user-{key}")), SharedString::from(text.replace('\n', "  \n"))).selectable(true),
            )),
    ))
}

/// Whether a user message has a markdown list that will wrap in a bubble `inner` px wide (text `font` px).
/// A shrink-to-fit bubble sizes a wrapping list without its bullet column, so the text runs past the
/// bubble's edge; such a bubble takes its full width instead. Width is estimated generously per character.
fn list_wraps(text: &str, font: f32, inner: f32) -> bool {
    let is_item = |l: &str| {
        let l = l.trim_start();
        let digits = l.len() - l.trim_start_matches(|c: char| c.is_ascii_digit()).len();
        ["- ", "* ", "+ "].iter().any(|m| l.starts_with(m)) || (digits > 0 && (l[digits..].starts_with(". ") || l[digits..].starts_with(") ")))
    };
    if !text.lines().any(is_item) {
        return false;
    }
    let longest = text.lines().map(|l| l.chars().count()).max().unwrap_or(0) as f32;
    longest * font * 0.6 + font * 2.8 >= inner * 0.8
}

/// Markdown tables in the app's palette: no fill of their own, the card edge, and a header tinted like hovered rows.
pub(super) fn md_style(c: Colors) -> TextViewStyle {
    let mut table = StyleRefinement::default().bg(gpui::transparent_black());
    table.border_color = Some(c.edge);
    let mut head = StyleRefinement::default().bg(c.hover).text_color(c.muted);
    head.text.font_weight = Some(FontWeight::MEDIUM);
    TextViewStyle::default().table(table).table_head(head)
}

fn text_row(env: &Env, key: &str, text: &str, _streaming: bool) -> AnyElement {
    let c = env.c;
    let body = if env.remote {
        crate::code::links::linkify_remote(text)
    } else {
        crate::code::links::linkify(text, env.cwd.as_deref())
    };
    // In Home: "Task 3" drills in; a paired machine's name is a chip.
    let body = match env.refs.as_deref() {
        Some(refs) => super::root::home_nav::link_refs(&body, refs),
        None => body,
    };
    let chips = env.refs.is_some() && body.contains(super::root::home_nav::MACHINE_ICON);
    let md = TextView::markdown(SharedString::from(format!("cw-md-{key}")), SharedString::from(body));
    let md = if chips {
        let rgba = gpui::Rgba::from(c.accent);
        let hex = format!("#{:02x}{:02x}{:02x}", (rgba.r * 255.0) as u8, (rgba.g * 255.0) as u8, (rgba.b * 255.0) as u8);
        let icon = Arc::new(Image::from_bytes(ImageFormat::Svg, super::root::home_nav::machine_icon(&hex).into_bytes()));
        md.image_source(move |uri| if uri.as_ref() == super::root::home_nav::MACHINE_ICON { ImageSource::Image(icon.clone()) } else { ImageSource::from(uri.clone()) })
    } else {
        md
    };
    enter(env, key, div().w_full().py(px(if env.compact { 4.0 } else { 8.0 })).px_0p5().text_size(px(13.5 * env.scale)).line_height(relative(1.62)).text_color(c.fg)
        .child(md.style(md_style(c)).selectable(true).on_link_click({
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
    let toggled = env.toggled.get(key).copied();
    let show_body = open || matches!(toggled, Some((_, false)));
    let secs = ended.unwrap_or(env.now).saturating_sub(started);
    let label = if streaming { "Thinking…".to_string() } else { format!("Thought for {}", elapsed(secs)) };
    let label = if started == 0 && !streaming { "Thought".to_string() } else { label };
    let (view, k) = (env.view.clone(), key.to_string());
    let icon = Icon::new(IconName::Brain).size(px(13.0)).text_color(if streaming { c.accent } else { c.subtle });
    let name = div().text_size(env.small()).text_color(if streaming { c.accent } else { c.muted }).child(label);
    let head = div().id(SharedString::from(format!("r-{key}"))).flex().items_center().gap_1p5().h(px(26.0)).px_1p5().rounded_lg().cursor_pointer().hover(move |d| d.bg(c.hover))
        .on_click(move |_, _, cx| {
            let _ = view.update(cx, |v, cx| v.toggle(&k, open, cx));
        })
        .child(if streaming { icon.with_animation(SharedString::from(format!("rp-{key}")), Animation::new(Duration::from_millis(1600)).repeat().with_max_fps(30.0), |i, t| i.opacity(0.45 + 0.55 * breathe(t))).into_any_element() } else { icon.into_any_element() })
        .child(if streaming { name.with_animation(SharedString::from(format!("rn-{key}")), Animation::new(Duration::from_millis(1600)).repeat().with_max_fps(30.0), |d, t| d.opacity(0.55 + 0.45 * breathe(t))).into_any_element() } else { name.into_any_element() })
        .child(toggle_chevron(key, open, toggled, c.subtle));
    enter(env, key, div().w_full().flex().flex_col().py(px(2.0)).child(div().flex().child(head))
        .when(show_body, |d| {
            let body = div().ml(px(7.0)).mt_1().mb_1().pl_3().py_0p5().text_size(env.small()).line_height(relative(1.6)).text_color(c.muted).whitespace_normal().child(text.trim().to_string());
            d.child(toggle_body(key, toggled, body))
        }))
}

fn diff_block(env: &Env, diff: &str, max: usize) -> Div {
    let c = env.c;
    let mut block = div().flex().flex_col().rounded_lg().border_1().border_color(c.edge).bg(c.app).overflow_hidden().py_1().font_family(env.mono.clone()).text_size(env.small());
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
    div().rounded_lg().border_1().border_color(c.edge).bg(c.app).px_3().py_2().flex().flex_col().font_family(env.mono.clone()).text_size(env.small()).line_height(relative(1.5)).text_color(c.muted)
        .when_some(command, |d, cmd| d.child(div().whitespace_normal().text_color(c.fg).child(format!("$ {cmd}"))))
        .when(skip > 0, |d| d.child(div().text_color(c.subtle).child(format!("… {skip} earlier lines"))))
        .children(lines.into_iter().skip(skip).map(|l| div().whitespace_normal().child(if l.is_empty() { " ".to_string() } else { l })))
}

/// File contents with line numbers.
fn code_block(env: &Env, text: &str, max: usize) -> Div {
    let c = env.c;
    let lines: Vec<&str> = text.lines().collect();
    let width = lines.len().min(max).to_string().len().max(2);
    div().rounded_lg().border_1().border_color(c.edge).bg(c.app).py_1p5().flex().flex_col().overflow_hidden().font_family(env.mono.clone()).text_size(env.small()).line_height(relative(1.5))
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
    div().rounded_lg().border_1().border_color(c.edge).bg(c.app).px_3().py_2().flex().flex_col().gap_1().text_size(env.small())
        .children(rows.into_iter().map(|(k, v)| {
            div().flex().gap_3().items_start()
                .child(div().w(px(84.0)).flex_shrink_0().text_color(c.subtle).child(k))
                .child(div().flex_1().min_w_0().whitespace_normal().font_family(env.mono.clone()).text_color(c.fg).child(v))
        }))
}

/// The send button: a small painted nebula (`ui::nebula`) in a round, glowing frame; ember-red while
/// it can interrupt, dim when there's nothing to send.
pub(crate) fn nebula_send(c: Colors, stop: bool, ready: bool, animate: bool, icon: IconName, sz: f32) -> Stateful<Div> {
    use super::nebula::{self, Kind};
    let live = ready || stop;
    let light = c.light;
    let kind = if stop { Kind::Stop } else if ready { Kind::Ready } else { Kind::Idle };
    let glow = if stop { theme::hex("#fb7185") } else if light { theme::hex("#818cf8") } else { theme::hex("#a855f7") };
    let mut shadows = vec![BoxShadow {
        color: alpha(gpui_kit::white(), if light { if live { 0.35 } else { 0.6 } } else if live { 0.14 } else { 0.05 }),
        offset: point(px(0.0), px(1.0)),
        blur_radius: px(1.0),
        spread_radius: px(0.0),
        inset: true,
    }];
    if live {
        shadows.push(BoxShadow {
            color: alpha(glow, 0.18),
            offset: point(px(0.0), px(2.0)),
            blur_radius: px(8.0),
            spread_radius: px(0.0),
            inset: false,
        });
    }
    // While the session works the clouds drift: a loop of pre-painted frames, each faded into the
    // next so the motion is smooth at 60 fps.
    let face: AnyElement = if stop && animate {
        div().absolute().top_0().left_0().size_full().rounded_full().overflow_hidden()
            .with_animation("send-drift", Animation::new(Duration::from_millis(6000)).repeat().with_max_fps(60.0), move |d, t| {
                let frames = nebula::working_frames(light);
                let pos = (t * frames.len() as f32).min(frames.len() as f32 - 0.001);
                let (i, mix) = (pos as usize, pos.fract());
                d.child(gpui_kit::img(frames[i].clone()).size_full().rounded_full().object_fit(ObjectFit::Cover))
                    .child(gpui_kit::img(frames[(i + 1) % frames.len()].clone()).absolute().top_0().left_0().size_full().rounded_full().object_fit(ObjectFit::Cover).opacity(mix))
            })
            .into_any_element()
    } else {
        gpui_kit::img(nebula::image(kind, light)).absolute().top_0().left_0().size_full().rounded_full().object_fit(ObjectFit::Cover).into_any_element()
    };
    let icon_sz = (sz * 0.44).clamp(11.0, 14.0);
    let icon_color = if live { gpui_kit::white() } else if light { c.muted } else { c.subtle };
    div().id("cw-send").relative().size(px(sz)).flex_shrink_0().flex().items_center().justify_center().rounded_full()
        .when(light && !live, |d| d.border_1().border_color(alpha(c.border_strong, 0.7)))
        .shadow(shadows)
        .child(face)
        .child(Icon::new(icon).size(px(icon_sz)).text_color(icon_color))
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

fn tool_row(env: &Env, key: &str, name: &str, title: &str, input: &serde_json::Value, output: &str, ok: Option<bool>, diff: Option<&str>, started: u64, ended: Option<u64>, link_up: bool, link_down: bool) -> AnyElement {
    let c = env.c;
    let running = ended.is_none() && ok.is_none();
    let default_open = running && name == "Bash" && env.expand_shell;
    let open = env.open(key, default_open);
    let toggled = env.toggled.get(key).copied();
    let show_body = tool_body_shown(env, key, name, ok, ended);
    let link_down = link_down && !show_body;
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
    let dbg_k = key.to_string();
    let head = div().id(SharedString::from(format!("t-{key}"))).debug_selector(move || format!("t-{dbg_k}")).w_full().flex().items_center().gap_2().h(px(34.0)).px_2p5().rounded_t_xl().when(!show_body, |d| d.rounded_b_xl()).cursor_pointer().hover(move |d| d.bg(alpha(c.hover, 0.6)))
        .on_click(move |_, _, cx| {
            let _ = view.update(cx, |v, cx| v.toggle(&k, open, cx));
        })
        .child(div().size(px(22.0)).flex_shrink_0().flex().items_center().justify_center().rounded_md().bg(alpha(tint, 0.14)).child(Icon::new(tool_icon(name)).size(px(13.0)).text_color(tint)))
        .child(div().flex_shrink_0().text_size(env.small()).text_color(c.fg).child(name.to_string()))
        .child(div().flex_1().min_w_0().truncate().font_family(env.mono.clone()).text_size(px(11.5 * env.scale)).text_color(c.muted).child(title.to_string()))
        .when_some(counts, |d, (a, r)| d.child(div().flex().flex_shrink_0().gap_1().text_size(px(11.0)).font_family(env.mono.clone()).child(div().text_color(c.success).child(format!("+{a}"))).child(div().text_color(c.danger).child(format!("−{r}")))))
        .when_some(right, |d, r| d.child(div().flex_shrink_0().text_size(px(11.0)).text_color(c.subtle).child(r)))
        .child(div().w(px(14.0)).flex_shrink_0().flex().items_center().justify_center().child(status))
        .child(toggle_chevron(key, open, toggled, c.subtle));
    let mut card = div().w_full().rounded_xl().overflow_hidden().child(head);
    if show_body {
        let secs = toolbody::sections(name, input, output, diff);
        let mut body = div().p_2().flex().flex_col().gap_2();
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
        card = card.child(toggle_body(key, toggled, body));
    }
    let card: AnyElement = if running {
        let id = SharedString::from(format!("glow-{key}"));
        card.with_animation(id, Animation::new(Duration::from_millis(1800)).repeat().with_max_fps(30.0), move |d, _t| d).into_any_element()
    } else {
        card.into_any_element()
    };
    // Connector segments from the row's edges to the icon badge (22px, centred in the 34px head, 10px in).
    let pad = if env.compact { 1.5 } else { 3.0 };
    let (x, gap) = (px(10.0 + 11.0 - 0.5), pad + 6.0);
    let seg = || div().absolute().left(x).w(px(1.0)).h(px(gap)).bg(c.border);
    enter(env, key, div().relative().w_full().py(px(pad)).child(card)
        .when(link_up, |d| d.child(seg().top_0()))
        .when(link_down, |d| d.child(seg().bottom_0())))
}

/// `visualize` calls as the one report they make up.
fn report_row<'a>(env: &Env, key: &str, charts: impl Iterator<Item = &'a Item>) -> AnyElement {
    let reports: Vec<_> = charts
        .filter_map(|it| match it {
            Item::Tool { name, input, ok, .. } => crate::code::viz::report_of(name, input, *ok),
            _ => None,
        })
        .collect();
    let view = env.view.clone();
    let (theme, all) = (super::viz::file_theme(&env.c), reports.clone());
    let on_export: Rc<dyn Fn(crate::code::viz_svg::Format, &mut App)> = Rc::new(move |format, cx| {
        let view = view.clone();
        super::viz::export(all.clone(), theme.clone(), format, cx, move |result, cx| {
            let text = match result {
                Ok(path) => format!("Saved the report to {}", path.display()),
                Err(e) => format!("Couldn't export the report: {e}"),
            };
            let _ = view.update(cx, |v, cx| v.model.update(cx, |m, cx| {
                m.state.notice(text);
                cx.notify();
            }));
        });
    });
    let (view, theme, all) = (env.view.clone(), super::viz::file_theme(&env.c), reports.clone());
    let on_copy: Rc<dyn Fn(&mut App)> = Rc::new(move |cx| {
        let view = view.clone();
        super::viz::copy_image(all.clone(), theme.clone(), cx, move |result, cx| {
            let text = match result {
                Ok(()) => "Copied the report as an image".to_string(),
                Err(e) => format!("Couldn't copy the report: {e}"),
            };
            let _ = view.update(cx, |v, cx| v.model.update(cx, |m, cx| {
                m.state.notice(text);
                cx.notify();
            }));
        });
    });
    let style = super::viz::Style { c: env.c, mono: env.mono.clone(), scale: env.scale, width: env.width, on_export: Some(on_export), on_copy: Some(on_copy) };
    enter(env, key, super::viz::report(&style, key, &reports))
}

/// A finished/stopped background task: one line, click for its command and output.
fn bg_note_row(env: &Env, key: &str, note: &BgNote) -> AnyElement {
    let c = env.c;
    let open = env.open(key, false);
    let toggled = env.toggled.get(key).copied();
    let show_body = open || matches!(toggled, Some((_, false)));
    let (icon, color) = if note.failed() { (IconName::X, c.danger) } else if note.code.is_none() { (IconName::Info, c.subtle) } else { (IconName::Check, c.success) };
    let (view, k) = (env.view.clone(), key.to_string());
    let head = div().id(SharedString::from(format!("bg-{key}"))).flex().items_center().gap_1p5().h(px(26.0)).rounded_lg().cursor_pointer().hover(move |d| d.bg(c.hover))
        .on_click(move |_, _, cx| {
            let _ = view.update(cx, |v, cx| v.toggle(&k, open, cx));
        })
        .child(Icon::new(icon).size(px(12.0)).text_color(color))
        .child(div().text_size(env.small()).text_color(if note.failed() { c.danger } else { c.muted }).child(note.title()))
        .child(toggle_chevron(key, open, toggled, c.subtle));
    let body = div().flex().flex_col().gap_1().mt_1().mb_1().px_3().py_1p5().rounded_lg().border_1().border_color(c.edge).bg(c.raised).font_family(env.mono.clone()).text_size(env.small()).text_color(c.muted).whitespace_normal()
        .child(div().text_color(c.subtle).child(note.cmd.clone()))
        .when(!note.output.is_empty(), |d| d.child(note.output.clone()));
    enter(env, key, div().w_full().flex().flex_col().items_start().py(px(2.0)).child(head)
        .when(show_body, |d| d.child(div().max_w(relative(0.9)).child(toggle_body(key, toggled, body)))))
}

fn notice_row(env: &Env, key: &str, text: &str, error: bool) -> AnyElement {
    if !error {
        if let Some(note) = crate::code::bg_note::parse(text) {
            return bg_note_row(env, key, &note);
        }
    }
    let c = env.c;
    let (icon, color) = if error { (IconName::TriangleAlert, c.danger) } else { (IconName::Info, c.subtle) };
    enter(env, key, div().w_full().flex().justify_center().py_1p5().child(
        div().max_w(relative(0.9)).flex().items_start().gap_1p5().rounded_xl().px_3().py_1p5().border_1().border_color(if error { alpha(c.danger, 0.4) } else { c.border }).bg(if error { alpha(c.danger, 0.08) } else { c.raised }).text_size(env.small()).line_height(relative(1.5)).text_color(if error { c.danger } else { c.muted })
            .child(div().mt(px(2.0)).child(Icon::new(icon).size(px(12.0)).text_color(color)))
            .child(div().min_w_0().whitespace_normal().child(text.to_string())),
    ))
}

fn handoff_row(env: &Env, key: &str, from_model: &str, from_actual: Option<&str>, to_model: &str) -> AnyElement {
    let c = env.c;
    let from_backend = crate::code::names::backend_of(from_model).map(|b| b.0);
    let to_backend = crate::code::names::backend_of(to_model).map(|b| b.0);
    let from_label = model_name(from_model, from_actual);
    let to_label = model_name(to_model, None);
    let from_color = from_backend.map_or(c.fg, |b| super::code_pickers::backend_color(c, b));
    let to_color = to_backend.map_or(c.fg, |b| super::code_pickers::backend_color(c, b));
    let label_size = px(11.5 * env.scale);
    let icon_size = 13.0 * env.scale;
    let mut from_chip = div().flex().items_center().gap_1().text_color(from_color).font_weight(FontWeight::MEDIUM);
    if let Some(b) = from_backend {
        from_chip = from_chip.child(super::code_pickers::mark(c, b, icon_size, 1.0));
    }
    from_chip = from_chip.child(from_label.clone());
    let mut to_chip = div().flex().items_center().gap_1().text_color(to_color).font_weight(FontWeight::MEDIUM);
    if let Some(b) = to_backend {
        to_chip = to_chip.child(super::code_pickers::mark(c, b, icon_size, 1.0));
    }
    to_chip = to_chip.child(to_label.clone());
    let tooltip_text = format!("Conversation context handed off from {from_label} to {to_label}");
    enter(
        env,
        key,
        div()
            .w_full()
            .flex()
            .items_center()
            .gap_3()
            .py_2()
            .text_size(label_size)
            .text_color(c.muted)
            .child(div().h(px(1.0)).flex_1().bg(c.border))
            .child(
                div()
                    .id(SharedString::from(format!("cw-handoff-{key}")))
                    .flex_shrink_0()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .justify_center()
                    .gap_2()
                    .tooltip(tip(tooltip_text))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1p5()
                            .child(Icon::new(IconName::ArrowRightLeft).size(px(12.0 * env.scale)).text_color(c.muted))
                            .child("Context handoff"),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1p5()
                            .child(from_chip)
                            .child(Icon::new(IconName::ArrowRight).size(px(11.0 * env.scale)).text_color(c.subtle))
                            .child(to_chip),
                    ),
            )
            .child(div().h(px(1.0)).flex_1().bg(c.border)),
    )
}

fn worked_row(env: &Env, key: &str, ms: u64) -> AnyElement {
    let c = env.c;
    enter(env, key, div().w_full().flex().items_center().gap_3().pt_3().pb_1().text_size(px(11.0)).text_color(c.subtle)
        .child(div().h(px(1.0)).flex_1().bg(c.border))
        .child(div().flex().items_center().gap_1().child(Icon::new(IconName::Check).size(px(11.0)).text_color(c.success)).child(format!("Worked for {}", elapsed(ms.max(1000)))))
        .child(div().h(px(1.0)).flex_1().bg(c.border)))
}

fn working_row(env: &Env) -> AnyElement {
    let pad = if env.compact { 1.5 } else { 3.0 };
    div()
        .w_full()
        .py(px(pad))
        .flex()
        .items_center()
        .gap_1p5()
        .h(px(28.0))
        .child(div().flex_shrink_0().flex().items_center().justify_center().child(spinner(env.c.muted, 12.0, "cw-working-sp")))
        .child(
            div()
                .text_size(px(12.5 * env.scale))
                .text_color(env.c.muted)
                .child("Working…")
                .with_animation("cw-working-pulse", Animation::new(Duration::from_millis(1800)).repeat().with_max_fps(30.0), |d, t| {
                    d.opacity(0.48 + 0.52 * breathe(t))
                }),
        )
        .into_any_element()
}

/// Height of the soft edge fade on an expanded compacted work group, in px.
const WORK_GROUP_FADE_H: f32 = 64.0;
/// Scroll distance (px) over which an edge fade ramps from invisible to full strength.
const WORK_GROUP_FADE_RAMP: f32 = 32.0;

/// Returns `(top_strength, bottom_strength)` in `0.0..=1.0` for a vertical scroll container.
/// `offset_y` is `<= 0.0` (`0` at the top, `-max_offset_y` at the bottom) and `max_offset_y >= 0.0`.
fn scroll_edge_fade_strengths(offset_y: f32, max_offset_y: f32) -> (f32, f32) {
    if max_offset_y <= 0.5 {
        return (0.0, 0.0);
    }
    let scrolled_top = (-offset_y).clamp(0.0, max_offset_y);
    let remaining_bottom = (max_offset_y - scrolled_top).max(0.0);
    (
        (scrolled_top / WORK_GROUP_FADE_RAMP).clamp(0.0, 1.0),
        (remaining_bottom / WORK_GROUP_FADE_RAMP).clamp(0.0, 1.0),
    )
}

fn paint_scroll_edge_fades(bounds: Bounds<Pixels>, handle: &ScrollHandle, color: Hsla, window: &mut Window) {
    paint_scroll_edge_fades_with(bounds, handle, color, EdgeFade { height: WORK_GROUP_FADE_H, max_share: 0.42, alphas: [1.0, 0.80, 0.48, 0.16, 0.0] }, window);
}

/// How tall and strong [`paint_scroll_edge_fades_with`] paints: at most `height` px or `max_share` of
/// the box, with `alphas` from the clipped edge inwards.
pub(crate) struct EdgeFade {
    pub height: f32,
    pub max_share: f32,
    pub alphas: [f32; 5],
}

pub(crate) fn paint_scroll_edge_fades_with(bounds: Bounds<Pixels>, handle: &ScrollHandle, color: Hsla, fade: EdgeFade, window: &mut Window) {
    let h = bounds.size.height / px(1.0);
    if h <= 4.0 {
        return;
    }
    let (top_s, bot_s) = scroll_edge_fade_strengths(handle.offset().y / px(1.0), handle.max_offset().y / px(1.0));
    let fade_h = fade.height.min(h * fade.max_share).round();
    if fade_h <= 1.0 {
        return;
    }
    // 4-band eased falloff (outer clipped edge -> inner clear edge) for a tall, soft feathered dissolve.
    let ys = [0.0, (fade_h * 0.22).round(), (fade_h * 0.48).round(), (fade_h * 0.76).round(), fade_h];
    #[allow(non_snake_case)]
    let ALPHAS = fade.alphas;

    if top_s > 0.001 {
        let top_y = bounds.origin.y / px(1.0);
        for i in 0..4 {
            let (y0, y1) = (top_y + ys[i], top_y + ys[i + 1]);
            if y1 <= y0 {
                continue;
            }
            let band = Bounds::new(point(bounds.origin.x, px(y0)), size(bounds.size.width, px(y1 - y0)));
            window.paint_quad(fill(
                band,
                linear_gradient(
                    180.0,
                    linear_color_stop(alpha(color, ALPHAS[i] * top_s), 0.0),
                    linear_color_stop(alpha(color, ALPHAS[i + 1] * top_s), 1.0),
                ),
            ));
        }
    }

    if bot_s > 0.001 {
        let bot_y = (bounds.origin.y + bounds.size.height) / px(1.0);
        for i in 0..4 {
            let (y0, y1) = (bot_y - ys[i + 1], bot_y - ys[i]);
            if y1 <= y0 {
                continue;
            }
            let band = Bounds::new(point(bounds.origin.x, px(y0)), size(bounds.size.width, px(y1 - y0)));
            window.paint_quad(fill(
                band,
                linear_gradient(
                    180.0,
                    linear_color_stop(alpha(color, ALPHAS[i + 1] * bot_s), 0.0),
                    linear_color_stop(alpha(color, ALPHAS[i] * bot_s), 1.0),
                ),
            ));
        }
    }
}

/// Vertically scrollable body for an expanded compacted work group: caps at `max_h`,
/// shows a vertical scrollbar, and paints soft scroll-reactive top/bottom edge fades.
#[derive(IntoElement)]
struct WorkGroupScroll {
    id: ElementId,
    max_h: Pixels,
    fade_color: Hsla,
    handle: ScrollHandle,
    view: WeakEntity<CodeView>,
    children: Vec<AnyElement>,
}

/// Whether a wheel delta over an expanded work group should scroll the transcript instead:
/// the group overflows but already sits at the end the wheel pushes toward. `ScrollBounce`
/// would otherwise keep the whole gesture (bouncing) until the pointer leaves the group.
fn wheel_passes_work_group(offset_y: f32, max_y: f32, dy: f32) -> bool {
    if max_y <= 0.5 {
        return false; // Not scrollable: the group doesn't claim the wheel, the list gets it already.
    }
    if dy > 0.0 {
        offset_y >= -0.5
    } else if dy < 0.0 {
        offset_y <= -max_y + 0.5
    } else {
        false
    }
}

impl RenderOnce for WorkGroupScroll {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let root_id = self.id;
        let handle = self.handle;
        let fade_handle = handle.clone();
        let fade_color = self.fade_color;
        let (edge_handle, view) = (handle.clone(), self.view);

        // Registered before the ScrollBounce paints, so in the capture phase it runs before the
        // group claims the wheel: at the group's start or end the transcript list scrolls on.
        let edge_pass = canvas(
            |bounds, window, _| window.insert_hitbox(bounds, HitboxBehavior::Normal),
            move |_, hitbox, window, _| {
                window.on_mouse_event(move |e: &ScrollEvent, phase, window, cx| {
                    if phase != DispatchPhase::Capture || e.modifiers.control || e.modifiers.platform || !hitbox.should_handle_scroll(window) {
                        return;
                    }
                    let d = e.delta.pixel_delta(window.line_height());
                    if d.x.abs() > d.y.abs() || !wheel_passes_work_group(edge_handle.offset().y / px(1.0), edge_handle.max_offset().y / px(1.0), d.y / px(1.0)) {
                        return;
                    }
                    cx.stop_propagation();
                    let _ = view.update(cx, |v, cx| {
                        // Scrollbar coordinates clamp, and reaching the end resumes following the tail.
                        let y = v.list.scroll_px_offset_for_scrollbar().y + d.y;
                        v.list.set_offset_from_scrollbar(point(px(0.0), y));
                        cx.notify();
                    });
                });
            },
        )
        .absolute()
        .inset_0();

        let content = div()
            .id((root_id.clone(), "content"))
            .flex_none()
            .w_full()
            .h_auto()
            .min_h_full()
            .flex()
            .flex_col()
            .pr_2()
            .children(self.children);

        let scroll_area = div()
            .id((root_id.clone(), "area"))
            .size_full()
            .flex()
            .flex_col()
            .overflow_y_scroll()
            .lock_scroll_axis()
            .track_scroll(&handle)
            .child(content);

        let fade_overlay = canvas(
            |_, _, _| {},
            move |bounds, _, window, _| {
                paint_scroll_edge_fades(bounds, &fade_handle, fade_color, window);
            },
        )
        .absolute()
        .inset_0();

        div()
            .id(root_id.clone())
            .size_full()
            .max_h(self.max_h)
            .overflow_hidden()
            .relative()
            .flex()
            .flex_col()
            .child(edge_pass)
            .child(gpui_kit::base::ScrollBounce::new((root_id.clone(), "bounce"), &handle, scroll_area))
            .child(fade_overlay)
            .when(!window.is_inspector_picking(cx), |d| {
                d.child(
                    div().absolute().inset_0().child(
                        Scrollbar::new(&handle)
                            .id((root_id, "scrollbar"))
                            .axis(ScrollbarAxis::Vertical)
                            .viewport_from_layout(),
                    ),
                )
            })
    }
}

fn work_group_row(env: &Env, items: &[Item], key: &str, start: usize, end: usize, active: bool) -> AnyElement {
    let c = env.c;
    let open = env.open(key, false);
    let toggled = env.toggled.get(key).copied();
    let show_body = open || matches!(toggled, Some((_, false)));
    let slice = items.get(start..=end).unwrap_or(&[]);

    let mut tool_count = 0usize;
    let mut step_count = 0usize;
    let mut last_step: Option<&Item> = None;
    for it in slice {
        match it {
            Item::Tool { .. } => {
                tool_count += 1;
                step_count += 1;
                last_step = Some(it);
            }
            Item::Reasoning { .. } => {
                step_count += 1;
                last_step = Some(it);
            }
            _ => {}
        }
    }

    let (view, k) = (env.view.clone(), key.to_string());
    let group = SharedString::from(format!("wg-grp-{key}"));
    let chev = div()
        .flex_shrink_0()
        .flex()
        .items_center()
        .opacity(0.55)
        .group_hover(group.clone(), |d| d.opacity(1.0))
        .child(toggle_chevron(key, open, toggled, c.fg));
    let head: AnyElement = if active {
        let label = div()
            .min_w_0()
            .truncate()
            .child(describe_step(last_step))
            .with_animation(SharedString::from(format!("wg-live-{key}")), Animation::new(Duration::from_millis(1800)).repeat().with_max_fps(30.0), |d, t| {
                d.opacity(0.52 + 0.48 * breathe(t))
            });
        div()
            .w_full()
            .flex()
            .child(
                div()
                    .id(SharedString::from(format!("wg-{key}")))
                    .group(group.clone())
                    .max_w_full()
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .h(px(28.0))
                    .cursor_pointer()
                    .text_size(px(12.5 * env.scale))
                    .text_color(c.muted)
                    .hover(move |d| d.text_color(c.fg))
                    .on_click(move |_, _, cx| {
                        let _ = view.update(cx, |v, cx| v.toggle(&k, open, cx));
                    })
                    // Nudged up 1px: the label's glyphs sit above its line box's centre, so a
                    // box-centred spinner reads low next to them.
                    .child(div().flex_shrink_0().flex().items_center().justify_center().relative().top(px(-1.0)).opacity(0.6).group_hover(group, |d| d.opacity(1.0)).child(spinner(c.fg, 12.0, SharedString::from(format!("wg-sp-{key}")))))
                    .child(label)
                    .child(chev),
            )
            .into_any_element()
    } else {
        let count = if tool_count > 0 { tool_count } else { step_count };
        let unit = match (tool_count > 0, count == 1) {
            (true, true) => "tool call",
            (true, false) => "tool calls",
            (false, true) => "step",
            (false, false) => "steps",
        };
        let label = match group_duration_ms(items, start, end) {
            Some(ms) => format!("Worked for {} · {count} {unit}", elapsed(ms.max(1000))),
            None => format!("Worked · {count} {unit}"),
        };
        div()
            .flex()
            .child(
                div()
                    .id(SharedString::from(format!("wg-{key}")))
                    .group(group)
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .h(px(28.0))
                    .cursor_pointer()
                    .text_size(px(12.5 * env.scale))
                    .text_color(c.muted)
                    .hover(move |d| d.text_color(c.fg))
                    .on_click(move |_, _, cx| {
                        let _ = view.update(cx, |v, cx| v.toggle(&k, open, cx));
                    })
                    .child(label)
                    .child(chev),
            )
            .into_any_element()
    };

    let pad = if env.compact { 1.5 } else { 3.0 };
    let mut wrap = div().w_full().flex().flex_col().py(px(pad)).child(head);
    if show_body {
        let cap = work_group_max_h(env.height);
        let handle = env.wg_scrolls.borrow_mut().entry(key.to_string()).or_default().clone();
        let children: Vec<AnyElement> = (start..=end)
            .filter_map(|ix| {
                let item = items.get(ix)?;
                let prev = if ix > start { items.get(ix - 1) } else { None };
                let next = if ix < end { items.get(ix + 1) } else { None };
                Some(render_item(env, item, false, prev, next))
            })
            .collect();
        let list = WorkGroupScroll {
            id: ElementId::Name(SharedString::from(format!("wg-scroll-{key}"))),
            max_h: px(cap),
            fade_color: c.panel,
            handle,
            view: env.view.clone(),
            children,
        };
        let dbg_wg = key.to_string();
        let body = div().w_full().flex().flex_col().pt_1().debug_selector(move || format!("wg-body-{dbg_wg}")).child(list);
        wrap = wrap.child(toggle_body(key, toggled, body));
    }
    enter(env, key, wrap)
}

/// Whether a tool row currently shows its body (open, or animating closed).
fn tool_body_shown(env: &Env, key: &str, name: &str, ok: Option<bool>, ended: Option<u64>) -> bool {
    let running = ended.is_none() && ok.is_none();
    let open = env.open(key, running && name == "Bash" && env.expand_shell);
    open || matches!(env.toggled.get(key), Some((_, false)))
}

pub(super) fn render_item(env: &Env, item: &Item, first: bool, prev: Option<&Item>, next: Option<&Item>) -> AnyElement {
    // Consecutive tool calls get a line joining their icons; an expanded body breaks it.
    let link_up = matches!(prev, Some(Item::Tool { key, name, ok, ended, .. }) if !tool_body_shown(env, key, name, *ok, *ended));
    let link_down = matches!(next, Some(Item::Tool { .. }));
    match item {
        Item::User { key, text, images, .. } => user_row(env, text, images, key, first),
        Item::Text { key, text, streaming } => text_row(env, key, text, *streaming),
        Item::Reasoning { key, text, streaming, started, ended } => reasoning_row(env, key, text, *streaming, *started, *ended),
        Item::Tool { key, .. } if is_chart(item) => report_row(env, key, std::iter::once(item)),
        Item::Tool { key, name, title, input, output, ok, diff, started, ended } => tool_row(env, key, name, title, input, output, *ok, diff.as_deref(), *started, *ended, link_up, link_down),
        Item::Notice { key, text, error, .. } => notice_row(env, key, text, *error),
        Item::Handoff { key, from_model, from_actual, to_model } => handoff_row(env, key, from_model, from_actual.as_deref(), to_model),
        Item::Worked { key, ms, .. } => worked_row(env, key, *ms),
    }
}

/// Home's orchestrator starts from what's going on, not from a project.
const HOME_SUGGESTIONS: [(IconName, &str, &str); 4] = [
    (IconName::Bell, "What needs me?", "What needs me right now across my workspaces and machines: agents waiting, reviews ready, open tasks, usage left?"),
    (IconName::GitMerge, "What changed today?", "Summarize what changed today in each workspace: commits, merged work and what the agents did."),
    (IconName::Sparkles, "Build something", "I want to build something. Ask me what and where, then start it as a task."),
    (IconName::ListTodo, "Status of my tasks", "Give me the status of every open task and what's next for each."),
];

const SUGGESTIONS: [(IconName, &str, &str); 4] = [
    (IconName::Compass, "Explore this codebase", "Give me a tour of this project: structure, entry points and how it fits together."),
    (IconName::Bug, "Find and fix a bug", "Look for a likely bug in this project and fix it, explaining what was wrong."),
    (IconName::FlaskConical, "Write tests", "Find the least-tested important code here and add good tests for it."),
    (IconName::Lightbulb, "Suggest improvements", "Review this project and suggest the three most valuable improvements."),
];

/// Deterministic scatter matching `Welcome.tsx` so stars never jump between frames.
fn sky_rand(i: u32, n: u32) -> f32 {
    let v = ((i as f64 * 12.9898 + n as f64 * 78.233).sin() * 43758.5453).fract();
    ((v + 1.0).fract()) as f32
}

/// Keeps painted stars/streaks (plus their `pad` half-extent) strictly inside the pane's rounded
/// corners on all four corners.
fn inside_rounded_corners(x: f32, y: f32, w: f32, h: f32, r: f32, pad: f32) -> bool {
    if x < pad || x > w - pad || y < pad || y > h - pad {
        return false;
    }
    let dy = if y < r {
        r - y
    } else if y > h - r {
        y - (h - r)
    } else {
        return true;
    };
    let dx = if x < r {
        r - x
    } else if x > w - r {
        x - (w - r)
    } else {
        return true;
    };
    let max_d = (r - pad).max(0.0);
    dx * dx + dy * dy <= max_d * max_d
}

/// Computes `image_bounds` for `window.paint_image` so the inner `SKY_INNER_W x SKY_INNER_H` sky
/// covers `bounds` (`ObjectFit::Cover`) while the `SKY_PAD`-texel margin sits outside `bounds` on
/// all four sides, keeping `sub_tile` strictly inside the atlas tile on every edge while letting
/// GPUI's sprite SDF clip all four corners to `Corners::all(px(16.0))`.
fn sky_image_bounds(bounds: Bounds<Pixels>) -> Bounds<Pixels> {
    let cover = ObjectFit::Cover.get_bounds(
        bounds,
        size(DevicePixels(super::nebula::SKY_INNER_W as i32), DevicePixels(super::nebula::SKY_INNER_H as i32)),
    );
    let pad_x = cover.size.width * (super::nebula::SKY_PAD as f32 / super::nebula::SKY_INNER_W as f32);
    let pad_y = cover.size.height * (super::nebula::SKY_PAD as f32 / super::nebula::SKY_INNER_H as f32);
    Bounds::new(
        point(cover.origin.x - pad_x, cover.origin.y - pad_y),
        size(cover.size.width + pad_x * 2.0, cover.size.height + pad_y * 2.0),
    )
}

/// Full-pane animated space welcome sky rendered behind both the floating pane header and
/// `CodeView` so the nebula and stars span the entire rounded pane without a top/bottom seam.
pub struct SpaceSkyView {
    model: Entity<CodeModel>,
    app: Entity<AppState>,
    ticking: bool,
}

impl SpaceSkyView {
    pub fn new(model: Entity<CodeModel>, app: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        cx.observe(&model, |_, _, cx| cx.notify()).detach();
        cx.observe(&app, |_, _, cx| cx.notify()).detach();
        Self { model, app, ticking: false }
    }

    fn tick(&mut self, cx: &mut Context<Self>) {
        if self.ticking {
            return;
        }
        self.ticking = true;
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_millis(45)).await;
                let keep = this.update(cx, |this, cx| {
                    let empty = this.model.read(cx).state.items.is_empty();
                    if empty {
                        cx.notify();
                    } else {
                        this.ticking = false;
                    }
                    empty
                });
                if !keep.unwrap_or(false) {
                    break;
                }
            }
        })
        .detach();
    }
}

impl Render for SpaceSkyView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.model.read(cx).state.items.is_empty() {
            return div().into_any_element();
        }
        self.tick(cx);
        let c = theme::colors(&self.app.read(cx).theme);
        render_space_sky(c, now_ms())
    }
}

/// Space-themed welcome background matching the Tauri web UI: soft nebula + Milky Way glow (dark mode),
/// three depths of twinkling stars with 4-point glints on bright stars, and periodic shooting stars.
fn render_space_sky(c: Colors, now: u64) -> AnyElement {
    let t = (now % 86_400_000) as f32 / 1000.0;
    let light = c.light;
    let star_canvas = canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            let w = bounds.size.width / px(1.0);
            let h = bounds.size.height / px(1.0);
            if w <= 4.0 || h <= 4.0 {
                return;
            }
            let ox = bounds.origin.x / px(1.0);
            let oy = bounds.origin.y / px(1.0);
            let sky = super::nebula::sky_backdrop_render(light, c.accent);
            let _ = window.paint_image(bounds, sky_image_bounds(bounds), Corners::all(px(16.0)), sky, 0, false);
            let white_star = hsla(0.0, 0.0, 1.0, 1.0);
            let cool_star = crate::theme::hex("#b9c8ff");
            let warm_star = crate::theme::hex("#ffe2b8");

            // Three depths: far dust (110), mid stars (42), near bright stars with glint (7).
            let layers: [(u32, u32, f32, f32, f32, f32, bool); 3] = [
                (110, 0, 0.7, 0.8, if light { 0.42 } else { 0.30 }, 1.0, false),
                (42, 300, 1.1, 1.2, if light { 0.70 } else { 0.58 }, -1.5, false),
                (7, 600, 2.0, 1.2, if light { 0.95 } else { 0.92 }, 0.0, true),
            ];
            for (count, seed, min_s, spread, peak, drift_dir, near) in layers {
                let drift_x = if drift_dir != 0.0 { (t * 0.035 * drift_dir).sin() * 7.0 } else { 0.0 };
                let drift_y = if drift_dir != 0.0 { (t * 0.025 * drift_dir).cos() * 3.0 } else { 0.0 };
                for k in 0..count {
                    let i = k + seed;
                    let sx = sky_rand(i, 1) * w + drift_x;
                    let sy = sky_rand(i, 2) * h + drift_y;
                    let sz = min_s + sky_rand(i, 3) * spread;
                    let pad = if near { 7.0 } else { sz * 0.5 + 1.0 };
                    if !inside_rounded_corners(sx, sy, w, h, 16.0, pad) {
                        continue;
                    }
                    let delay = sky_rand(i, 4) * 6.0;
                    let period = 2.5 + sky_rand(i, 5) * 4.0;
                    let wave = 0.5 - 0.5 * ((t + delay) * std::f32::consts::TAU / period).cos();
                    let a = wave * peak;
                    if a < 0.02 {
                        continue;
                    }
                    let base = if light {
                        mix_color(c.accent, c.fg, 0.45)
                    } else {
                        match (sky_rand(i, 6) * 6.0) as u32 {
                            3 | 4 => cool_star,
                            5 => warm_star,
                            _ => white_star,
                        }
                    };
                    let half = sz * 0.5;
                    window.paint_quad(
                        fill(Bounds::new(point(px(ox + sx - half), px(oy + sy - half)), size(px(sz), px(sz))), alpha(base, a))
                            .corner_radii(Corners::all(px(half))),
                    );
                    if near {
                        // Soft halo + 4-point cross glint on the brightest stars.
                        let halo = sz * 3.0;
                        window.paint_quad(
                            fill(Bounds::new(point(px(ox + sx - halo * 0.5), px(oy + sy - halo * 0.5)), size(px(halo), px(halo))), alpha(base, a * 0.20))
                                .corner_radii(Corners::all(px(halo * 0.5))),
                        );
                        let glint_len = 13.0;
                        let ga = a * 0.48;
                        window.paint_quad(fill(
                            Bounds::new(point(px(ox + sx - glint_len * 0.5), px(oy + sy - 0.5)), size(px(glint_len), px(1.0))),
                            alpha(base, ga),
                        ));
                        window.paint_quad(fill(
                            Bounds::new(point(px(ox + sx - 0.5), px(oy + sy - glint_len * 0.5)), size(px(1.0), px(glint_len))),
                            alpha(base, ga),
                        ));
                    }
                }
            }

            // Periodic deterministic shooting star every ~5.4 s.
            let cycle = 5.4_f32;
            let shot_idx = (t / cycle).floor() as u32;
            let phase = t % cycle;
            let dur = 0.85 + sky_rand(shot_idx + 900, 1) * 0.55;
            if phase < dur {
                let p = phase / dur;
                let fade = if p < 0.14 {
                    p / 0.14
                } else if p > 0.65 {
                    ((1.0 - p) / 0.35).max(0.0)
                } else {
                    1.0
                };
                let leftward = sky_rand(shot_idx + 900, 2) < 0.5;
                let tilt = 14.0 + sky_rand(shot_idx + 900, 3) * 26.0;
                let angle = (if leftward { 180.0 - tilt } else { tilt }).to_radians();
                let start_x = (if leftward { 0.38 + sky_rand(shot_idx + 900, 4) * 0.55 } else { 0.07 + sky_rand(shot_idx + 900, 4) * 0.55 }) * w;
                let start_y = (0.06 + sky_rand(shot_idx + 900, 5) * 0.45) * h;
                let dist = (170.0 + sky_rand(shot_idx + 900, 6) * 220.0).min(w * 0.7);
                let tail_len = 65.0 + sky_rand(shot_idx + 900, 7) * 75.0;
                let head_d = p * dist;
                let (dx, dy) = (angle.cos(), angle.sin());
                let segs = 22;
                for s in 0..segs {
                    let frac = s as f32 / (segs - 1) as f32; // 0 = tail end, 1 = head
                    let d = head_d - (1.0 - frac) * tail_len * (0.25 + 0.75 * p);
                    if d < 0.0 {
                        continue;
                    }
                    let px_x = start_x + dx * d;
                    let px_y = start_y + dy * d;
                    let dot = 1.2 + frac * 1.4;
                    if !inside_rounded_corners(px_x, px_y, w, h, 16.0, dot * 0.5 + 1.0) {
                        continue;
                    }
                    let seg_a = fade * frac * frac * if light { 0.55 } else { 0.92 };
                    let col = if light { c.accent } else if frac > 0.78 { white_star } else { cool_star };
                    window.paint_quad(
                        fill(Bounds::new(point(px(ox + px_x - dot * 0.5), px(oy + px_y - dot * 0.5)), size(px(dot), px(dot))), alpha(col, seg_a))
                            .corner_radii(Corners::all(px(dot * 0.5))),
                    );
                }
            }
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full();

    div()
        .relative()
        .size_full()
        .rounded_2xl()
        .overflow_hidden()
        .child(star_canvas)
        .into_any_element()
}

impl CodeView {
    fn welcome(&self, c: Colors, cx: &mut Context<Self>) -> AnyElement {
        let lay = welcome_layout(self.pane_size.width / px(1.0), self.pane_size.height / px(1.0));
        let m = self.model.read(cx);
        let home = crate::voice::home_pane(&m.pane_id) == Some(crate::voice::HomePane::Conductor);
        let st = &m.state;
        let spec = &st.model;
        let name = if spec.is_empty() { "…".to_string() } else { model_name(spec, st.actual_model.as_deref()) };
        let backend = backend_of(spec).map_or("", |b| b.1);
        let ready = st.session.is_some() && st.exited.is_none();
        let status = if let Some(e) = st.exited.clone() {
            e
        } else if st.session.is_none() {
            "Starting…".to_string()
        } else if lay.compact || backend.is_empty() {
            format!("{name} · {}", st.mode.label())
        } else {
            format!("{name} · {backend} · {}", st.mode.label())
        };
        let cwd_full = st.cwd.clone().filter(|_| st.session.is_some());
        let cwd_label = cwd_full.as_deref().map(|dir| {
            let shown = if lay.short_cwd {
                std::path::Path::new(dir).file_name().and_then(|n| n.to_str()).unwrap_or(dir)
            } else {
                dir
            };
            match &st.branch {
                Some(b) => format!("{shown}  ⎇ {b}"),
                None => shown.to_string(),
            }
        });
        let mono = fonts::mono(cx);

        let suggestions = (lay.suggestions > 0).then(|| {
            let mut grid = div()
                .w_full()
                .max_w(px(if lay.two_col { 520.0 } else { 300.0 }))
                .flex()
                .when(lay.two_col, |d| d.flex_wrap().justify_center().gap_2())
                .when(!lay.two_col, |d| d.flex_col().items_center().gap_1p5());
            for (i, (icon, title, prompt)) in (if home { HOME_SUGGESTIONS } else { SUGGESTIONS }).into_iter().take(lay.suggestions).enumerate() {
                let icon_box = if lay.compact_cards { 20.0 } else { 24.0 };
                let icon_sz = if lay.compact_cards { 11.5 } else { 13.0 };
                let card = div()
                    .id(("cw-suggest", i))
                    .when(lay.two_col, |d| d.w(px(250.0)))
                    .when(!lay.two_col, |d| d.w_full())
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap(px(if lay.compact_cards { 8.0 } else { 10.0 }))
                    .rounded(px(if lay.compact_cards { 9.0 } else { 12.0 }))
                    .border_1()
                    .border_color(c.edge)
                    .bg(c.raised)
                    .px(px(if lay.compact_cards { 10.0 } else { 12.0 }))
                    .py(px(if lay.compact_cards { 6.0 } else { 10.0 }))
                    .cursor_pointer()
                    .text_size(px(if lay.compact_cards { 11.5 } else { 12.5 }))
                    .text_color(c.muted)
                    .hover(move |d| d.bg(c.hover).text_color(c.fg))
                    .when(!ready, |d| d.opacity(0.5))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if !ready {
                            return;
                        }
                        this.set_input(prompt, window, cx);
                        this.enter(window, cx);
                    }))
                    .child(div().size(px(icon_box)).flex_shrink_0().flex().items_center().justify_center().rounded_md().bg(alpha(c.accent, 0.13)).child(Icon::new(icon).size(px(icon_sz)).text_color(c.accent)))
                    .child(div().flex_1().min_w_0().truncate().child(title));
                grid = grid.child(rise(card, SharedString::from(format!("sg-{i}")), 220 + i as u64 * 70, 10.0));
            }
            grid
        });

        let cwd_pill = if lay.show_cwd && !home {
            cwd_label.map(|label| {
                let full_tip = cwd_full.clone().unwrap_or_else(|| label.clone());
                div()
                    .id("wl-cwd")
                    .mt_1()
                    .max_w_full()
                    .overflow_hidden()
                    .px_2()
                    .py_0p5()
                    .rounded_md()
                    .bg(c.raised)
                    .text_size(px(if lay.compact { 10.5 } else { 11.0 }))
                    .font_family(mono)
                    .tooltip(tip(full_tip))
                    .child(div().truncate().child(label))
            })
        } else {
            None
        };

        let header_block = rise(
            div()
                .w_full()
                .min_w_0()
                .flex()
                .flex_col()
                .items_center()
                .gap_1()
                .child(div().w_full().text_center().text_size(px(if lay.compact { 16.0 } else { 22.0 })).font_weight(FontWeight::MEDIUM).text_color(c.fg).child(if home { "What's next?" } else { "What should we build?" }))
                .child(div().w_full().min_w_0().text_center().truncate().text_size(px(if lay.compact { 11.5 } else { 12.5 })).child(status))
                .children(cwd_pill),
            "wl-title",
            80,
            8.0,
        );

        let hint = lay.show_hint.then(|| {
            rise(div().text_center().text_size(px(11.0)).child("Type a message below, or / for commands and @ for files"), "wl-hint", 560, 4.0)
        });

        div()
            .size_full()
            .overflow_hidden()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(if lay.compact { 10.0 } else { 16.0 }))
            .px(px(if lay.compact { 12.0 } else { 16.0 }))
            .py(px(if lay.compact { 8.0 } else { 16.0 }))
            .text_color(c.subtle)
            .child(header_block)
            .children(suggestions)
            .children(hint)
            .into_any_element()
    }

    fn render_plan(&self, c: Colors, cx: &mut Context<Self>) -> Option<AnyElement> {
        let m = self.model.read(cx);
        let st = &m.state;
        if st.todos.is_empty() || st.plan_done_at.is_some_and(|at| now_ms().saturating_sub(at) >= PLAN_HIDE_MS) {
            return None;
        }
        let (done, total) = plan_counts(&st.todos);
        let open = !self.flipped.contains("plan");
        let mut list = div().flex().flex_col().gap_1p5().px_3().pb_2p5().pt_0p5();
        // Several steps can be in progress at once, so each spinner is keyed by its place.
        for (i, t) in st.todos.iter().enumerate() {
            let (icon, color) = match t.status.as_str() {
                "completed" => (IconName::CircleCheck, c.success),
                "in_progress" => (IconName::LoaderCircle, c.accent),
                "skipped" => (IconName::CircleSlash, c.subtle),
                "failed" => (IconName::CircleX, c.danger),
                _ => (IconName::CircleDashed, c.subtle),
            };
            let glyph: AnyElement = if t.status == "in_progress" { spinner(color, 13.0, SharedString::from(format!("plan-sp-{i}"))).into_any_element() } else { Icon::new(icon).size(px(13.0)).text_color(color).into_any_element() };
            list = list.child(div().flex().items_start().gap_2().text_size(px(12.5)).line_height(relative(1.45)).text_color(if matches!(t.status.as_str(), "completed" | "skipped") { c.subtle } else { c.fg })
                .child(div().mt(px(2.0)).child(glyph))
                .child(div().flex_1().whitespace_normal().child(t.content.clone())));
        }
        let frac = if total == 0 { 0.0 } else { done as f32 / total as f32 };
        let head = div().id("cw-plan-head").flex().items_center().gap_2().h(px(34.0)).px_3().cursor_pointer().text_size(px(12.0)).text_color(c.muted)
            .on_click(cx.listener(move |this, _, _, cx| this.toggle("plan", open, cx)))
            .child(Icon::new(IconName::ListTodo).size(px(13.0)).text_color(c.accent))
            .child(div().text_color(c.fg).child("Plan"))
            .child(div().w(px(56.0)).h(px(3.0)).rounded_full().bg(c.border).overflow_hidden().child(div().h_full().w(relative(frac)).rounded_full().bg(c.success)))
            .child(div().flex_1().text_color(c.subtle).child(plan_summary(&st.todos)))
            .child(Icon::new(if open { IconName::ChevronDown } else { IconName::ChevronUp }).size(px(12.0)).text_color(c.subtle));
        Some(dock_inset(div().mb_2().rounded_xl().bg(c.raised).overflow_hidden().child(head).when(open, |d| d.child(list)), false).into_any_element())
    }

    /// Out of usage, above the composer: when the session continues by itself and how many
    /// messages wait to go after it, with a button that stops waiting (like Esc).
    fn render_limit_wait(&self, c: Colors, cx: &mut Context<Self>) -> Option<AnyElement> {
        let st = &self.model.read(cx).state;
        if st.paused_until.is_none() && st.queued == 0 {
            return None;
        }
        let (title, detail) = match st.paused_until {
            Some(at) => ("Out of usage", format!("continues {}", super::root::usage::short_when(at as f64, now_ms() as f64))),
            None => ("Sending queued messages", String::new()),
        };
        let queued = match st.queued {
            0 => String::new(),
            1 => "1 message queued".to_string(),
            n => format!("{n} messages queued"),
        };
        let info = [detail, queued].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · ");
        let stop = div().id("cw-limit-stop").flex_shrink_0().h(px(22.0)).px_2().flex().items_center().rounded_md().cursor_pointer().text_color(c.muted)
            .hover(move |d| d.bg(c.hover).text_color(c.fg))
            .tooltip(tip(if st.paused_until.is_some() { "Stop waiting and drop the queued messages (Esc)" } else { "Drop the queued messages (Esc)" }))
            .on_click(cx.listener(|this, _, _, cx| this.send(Cmd::Interrupt, cx)))
            .child(if st.paused_until.is_some() { "Stop waiting" } else { "Drop queue" });
        let row = div().flex().items_center().gap_2().h(px(34.0)).px_3().text_size(px(12.0)).text_color(c.muted)
            .child(Icon::new(IconName::Clock).size(px(13.0)).text_color(c.warning))
            .child(div().text_color(c.fg).child(title))
            .child(div().flex_1().min_w_0().truncate().text_color(c.subtle).child(info))
            .child(stop);
        Some(dock_inset(div().mb_2().rounded_xl().bg(c.raised).overflow_hidden().child(row), false).into_any_element())
    }

    /// The live subagent / swarm tree above the composer: the main agent (or swarm coordinator)
    /// and the subagents/workers it started, each with its status, type/name, model, tool count
    /// and time. A row opens its live output; its chevron shows what it did last.
    fn render_agent_tree(&self, c: Colors, mono: SharedString, cx: &mut Context<Self>) -> Option<AnyElement> {
        let st = &self.model.read(cx).state;
        let now = now_ms();
        let agents = st.tree_agents(now);
        let board = st.swarm_board();
        if agents.is_empty() {
            return None;
        }
        let is_swarm = board.is_some();
        let running = agents.iter().filter(|t| t.running()).count();
        let waiting = agents.iter().filter(|t| t.waiting()).count();
        let failed = agents.iter().filter(|t| t.ev.status.as_deref() == Some("failed")).count();
        let stopped = agents.iter().filter(|t| t.ev.status.as_deref() == Some("stopped")).count();
        let finished = agents.len().saturating_sub(running + waiting + failed + stopped);
        let open = !self.flipped.contains("agent-tree");
        let mut counts = Vec::new();
        if running > 0 { counts.push(format!("{running} {}", if is_swarm { "working" } else { "running" })); }
        if waiting > 0 { counts.push(format!("{waiting} waiting")); }
        if finished > 0 { counts.push(format!("{finished} done")); }
        if stopped > 0 { counts.push(format!("{stopped} stopped")); }
        if failed > 0 { counts.push(format!("{failed} failed")); }
        let heading = match board.and_then(|b| b.ev.title.as_deref()).filter(|t| !t.is_empty() && *t != "Swarm") {
            Some(title) => format!("Swarm · {title}"),
            None if is_swarm => "Swarm".to_string(),
            None => "Subagents".to_string(),
        };
        let board_btn = board.map(|b| {
            let bid = b.ev.id.clone();
            let posts = b.ev.tools.unwrap_or(0);
            let label = if posts > 0 { format!("Board · {posts}") } else { "Board".to_string() };
            div()
                .id("cw-swarm-board-btn")
                .flex_shrink_0()
                .flex()
                .items_center()
                .gap_1()
                .h(px(20.0))
                .px_1p5()
                .rounded_md()
                .border_1()
                .border_color(c.edge)
                .bg(c.hover)
                .text_size(px(11.0))
                .text_color(c.fg)
                .cursor_pointer()
                .hover(move |d| d.border_color(c.accent).text_color(c.accent))
                .tooltip(tip("Open live swarm message board"))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    let id = bid.clone();
                    this.model.update(cx, |m, cx| { m.state.open_task_log(&id); m.flush(cx); cx.notify(); });
                }))
                .child(Icon::new(IconName::MessageSquareText).size(px(11.0)).text_color(c.accent))
                .child(label)
        });
        let head = div().id("cw-agents-head").flex().items_center().gap_2().h(px(34.0)).px_3().cursor_pointer().text_size(px(12.0)).text_color(c.muted)
            .on_click(cx.listener(move |this, _, _, cx| this.toggle("agent-tree", open, cx)))
            .child(Icon::new(if is_swarm { IconName::Network } else { IconName::Workflow }).size(px(13.0)).text_color(c.accent))
            .child(div().max_w(px(220.0)).truncate().text_color(c.fg).child(heading))
            .child(div().flex_1().min_w_0().truncate().text_color(c.subtle).child(counts.join(" · ")))
            .children(board_btn)
            .child(Icon::new(if open { IconName::ChevronDown } else { IconName::ChevronUp }).size(px(12.0)).text_color(c.subtle));
        if !open {
            return Some(dock_inset(div().mb_2().rounded_xl().bg(c.raised).overflow_hidden().child(head), false).into_any_element());
        }
        let branch = |last: bool| div().w(px(14.0)).flex_shrink_0().font_family(mono.clone()).text_color(alpha(c.subtle, 0.7)).child(if last { "└" } else { "├" });
        let rail = |last: bool| div().w(px(14.0)).flex_shrink_0().font_family(mono.clone()).text_color(alpha(c.subtle, 0.7)).child(if last { " " } else { "│" });
        let main_model = model_name(&st.model, None);
        let mut list = div().flex().flex_col().px_3().pb_2p5().text_size(px(12.0));
        if let Some(b) = board {
            if let Some(last_post) = b.ev.last.clone().filter(|s| !s.is_empty()) {
                let bid = b.ev.id.clone();
                list = list.child(
                    div()
                        .id("cw-swarm-last-post")
                        .flex()
                        .items_center()
                        .gap_1p5()
                        .h(px(22.0))
                        .mb_1()
                        .px_2()
                        .rounded_md()
                        .bg(alpha(c.hover, 0.75))
                        .cursor_pointer()
                        .hover(move |d| d.bg(c.hover))
                        .tooltip(tip("Open live swarm message board"))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let id = bid.clone();
                            this.model.update(cx, |m, cx| { m.state.open_task_log(&id); m.flush(cx); cx.notify(); });
                        }))
                        .child(Icon::new(IconName::MessageSquareText).size(px(11.0)).text_color(c.accent))
                        .child(div().flex_1().min_w_0().truncate().font_family(mono.clone()).text_size(px(11.0)).text_color(c.muted).child(last_post)),
                );
            }
        }
        list = list.child(div().flex().items_center().gap_1p5().h(px(22.0))
            .child(Icon::new(IconName::Bot).size(px(13.0)).text_color(c.accent))
            .child(div().text_color(c.fg).child(if is_swarm { "Coordinator" } else { "Main agent" }))
            .child(div().text_color(c.subtle).child(main_model)));
        let n = agents.len();
        for (i, t) in agents.iter().enumerate() {
            let last = i + 1 == n;
            let id = t.ev.id.clone();
            let key = format!("agt-{id}");
            let expanded = self.flipped.contains(&key);
            let live = t.running();
            let waiting = t.waiting();
            // Only a task that really runs spins; waiting workers show a calm clock; any other status is finished.
            let glyph: AnyElement = match t.ev.status.as_deref() {
                _ if live => spinner(c.accent, 13.0, SharedString::from(format!("agt-sp-{id}"))).into_any_element(),
                Some("waiting") => Icon::new(IconName::Clock).size(px(13.0)).text_color(c.subtle).into_any_element(),
                Some("failed") => Icon::new(IconName::CircleX).size(px(13.0)).text_color(c.danger).into_any_element(),
                Some("stopped") => Icon::new(IconName::CircleSlash).size(px(13.0)).text_color(c.subtle).into_any_element(),
                _ => Icon::new(IconName::CircleCheck).size(px(13.0)).text_color(c.success).into_any_element(),
            };
            let kind = t.ev.agent.clone().unwrap_or_default();
            let title = t.ev.description.clone().or(t.ev.title.clone()).unwrap_or_else(|| kind.clone());
            let mut meta = Vec::new();
            if waiting { meta.push("waiting".to_string()); }
            if let Some(m) = &t.ev.model { meta.push(model_name(m, None)); }
            if let Some(n) = t.ev.tools { meta.push(format!("{n} tool{}", if n == 1 { "" } else { "s" })); }
            meta.push(elapsed(t.elapsed_ms(now)));
            let has_lines = !t.recent_lines.is_empty();
            let (id_log, id_stop, key_toggle) = (id.clone(), id.clone(), key.clone());
            let row = div().id(SharedString::from(format!("agt-row-{id}"))).flex().items_center().gap_1p5().h(px(24.0)).rounded_md().cursor_pointer().hover(move |d| d.bg(c.hover))
                .tooltip(tip("Open its output"))
                .on_click(cx.listener(move |this, _, _, cx| {
                    let id = id_log.clone();
                    this.model.update(cx, |m, cx| { m.state.open_task_log(&id); m.flush(cx); cx.notify(); });
                }))
                .child(branch(last))
                .child(glyph)
                .child(div().flex_shrink_0().text_color(if live { c.fg } else { c.muted }).child(kind))
                .child(div().flex_1().min_w_0().truncate().text_color(c.subtle).child(title))
                .child(div().flex_shrink_0().font_family(mono.clone()).text_size(px(11.0)).text_color(c.subtle).child(meta.join(" · ")))
                .when(live && !is_swarm, |d| d.child(div().id(SharedString::from(format!("agt-stop-{id}"))).flex_shrink_0().p_0p5().rounded_md().hover(move |d| d.bg(alpha(c.danger, 0.14)))
                    .tooltip(tip("Stop this subagent"))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |this, _, _, cx| { cx.stop_propagation(); this.send(Cmd::StopTask(id_stop.clone()), cx); }))
                    .child(Icon::new(IconName::CircleStop).size(px(12.0)).text_color(c.danger))))
                .when(has_lines, |d| d.child(div().id(SharedString::from(format!("agt-more-{id}"))).flex_shrink_0().p_0p5().rounded_md().hover(move |d| d.bg(c.hover))
                    .tooltip(tip(if expanded { "Hide its recent steps" } else { "Show its recent steps" }))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |this, _, _, cx| { cx.stop_propagation(); this.toggle(&key_toggle, expanded, cx); }))
                    .child(Icon::new(if expanded { IconName::ChevronDown } else { IconName::ChevronRight }).size(px(12.0)).text_color(c.subtle))));
            list = list.child(row);
            // Collapsed, a running subagent (or waiting swarm worker) shows the step it's on; expanded, its last few.
            let lines: Vec<String> = if expanded {
                t.recent_lines.clone()
            } else if live || (is_swarm && waiting) {
                t.recent_lines.last().cloned().into_iter().collect()
            } else {
                Vec::new()
            };
            let k = lines.len();
            for (j, line) in lines.into_iter().enumerate() {
                let current = live && j + 1 == k;
                list = list.child(div().flex().items_center().gap_1p5().h(px(20.0)).text_size(px(11.0))
                    .child(rail(last))
                    .child(div().w(px(13.0)).flex_shrink_0().text_color(alpha(c.subtle, 0.7)).child("·"))
                    .child(div().flex_1().min_w_0().truncate().font_family(mono.clone()).text_color(if current { c.muted } else { c.subtle }).child(line)));
            }
        }
        let cap = (self.pane_size.height / px(1.0) * 0.35).clamp(96.0, 280.0);
        let body = div().id("cw-agents-body").max_h(px(cap)).overflow_y_scroll().child(list);
        Some(dock_inset(rise(div().mb_2().rounded_xl().bg(c.raised).overflow_hidden().child(head).child(body), "cw-agents", 0, 6.0), false).into_any_element())
    }

    fn render_approval(&mut self, a: &Approval, c: Colors, mono: SharedString, cx: &mut Context<Self>) -> AnyElement {
        let (id, model) = (a.id.clone(), self.model.clone());
        let decide = |label: &'static str, key_hint: &'static str, decision: Decision, primary: bool, key: String, model: Entity<CodeModel>, id: String| {
            div().id(SharedString::from(key)).h(px(30.0)).flex().items_center().gap_1p5().px_3().rounded_lg().text_size(px(12.5)).cursor_pointer()
                .when(primary, |d| d.bg(c.accent).text_color(c.on_accent).hover(|d| d.opacity(0.88)))
                .when(!primary, |d| d.border_1().border_color(c.edge).text_color(c.fg).hover(move |d| d.bg(c.hover)))
                .on_click(move |_, _, cx| {
                    let (id, decision) = (id.clone(), decision.clone());
                    model.update(cx, |m, cx| m.send(Cmd::Approve { id, decision }, cx));
                })
                .child(label)
                .child(div().text_size(px(10.0)).opacity(0.55).child(key_hint))
        };
        let head = div().flex().items_center().gap_2().text_size(px(12.5)).text_color(c.warning).child(Icon::new(IconName::ShieldQuestionMark).size(px(15.0))).child(if a.questions.is_empty() { format!("{} wants to run", a.tool) } else { "WinferCode is asking".to_string() });
        let mut body = div().flex().flex_col().gap_2();
        let actions;
        if a.questions.is_empty() {
            body = body.child(div().rounded_lg().bg(c.app).border_1().border_color(c.edge).px_3().py_2().font_family(mono.clone()).text_size(px(12.0)).text_color(c.fg).whitespace_normal().child(a.title.clone()));
            if !a.detail.trim().is_empty() && a.detail != a.title {
                body = body.child(div().font_family(mono.clone()).text_size(px(11.5)).text_color(c.muted).max_h(px(140.0)).overflow_hidden().whitespace_normal().child(a.detail.clone()));
            }
            if let Some(d) = a.diff.as_deref().filter(|d| !d.trim().is_empty()) {
                let env = Env { c, mono: mono.clone(), scale: 1.0, compact: true, view: cx.entity().downgrade(), flipped: Rc::new(HashSet::new()), toggled: Rc::new(HashMap::new()), expand_thinking: false, expand_shell: false, images: self.images.clone(), wg_scrolls: self.wg_scrolls.clone(), now: 0, cwd: None, remote: false, born: Rc::new(HashMap::new()), width: 600.0, height: self.pane_size.height / px(1.0), refs: None };
                body = body.child(diff_block(&env, d, 400));
            }
            actions = div().flex().gap_2().child(decide("Allow", "Y", Decision::Allow, true, format!("allow-{id}"), model.clone(), id.clone())).child(decide("Always allow", "A", Decision::Always, false, format!("always-{id}"), model.clone(), id.clone())).child(decide("Deny", "N", Decision::Deny, false, format!("deny-{id}"), model.clone(), id.clone()));
        } else {
            let picks = self.picks.entry(a.id.clone()).or_insert_with(|| vec![Vec::new(); a.questions.len()]).clone();
            for (qi, q) in a.questions.iter().enumerate() {
                let mut options = div().flex().flex_wrap().gap_2();
                for (oi, o) in q.options.iter().enumerate() {
                    let on = picks.get(qi).is_some_and(|p| p.contains(&oi));
                    let (aid, multi) = (a.id.clone(), q.multi);
                    options = options.child(div().id(SharedString::from(format!("q-{}-{qi}-{oi}", a.id))).debug_selector(move || format!("cw-q-{qi}-{oi}")).flex().flex_col().gap_0p5().rounded_lg().border_1().px_3().py_1p5().cursor_pointer().text_size(px(12.5))
                        .when(on, |d| d.bg(alpha(c.accent, 0.14)).text_color(c.fg))
                        .when(!on, |d| d.text_color(c.muted).hover(move |d| d.bg(c.hover).text_color(c.fg)))
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
            actions = div().flex().items_center().gap_3()
                .child(div().id(SharedString::from(format!("answer-{}", a.id))).debug_selector(|| "cw-answer".to_string()).h(px(30.0)).flex().items_center().px_3().rounded_lg().text_size(px(12.5)).bg(c.accent).text_color(c.on_accent)
                    .when(ready, |d| d.cursor_pointer().hover(|d| d.opacity(0.88))).when(!ready, |d| d.opacity(0.4))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !ready { return; }
                        let picks = this.picks.remove(&aid).unwrap_or_default();
                        let answers: Vec<String> = qs.iter().enumerate().map(|(i, q)| picks.get(i).map_or(String::new(), |p| p.iter().filter_map(|o| q.options.get(*o).map(|o| o.label.clone())).collect::<Vec<_>>().join(", "))).collect();
                        let id = aid.clone();
                        this.send(Cmd::Answer { id, answers }, cx);
                    }))
                    .child("Answer"))
                .child(div().text_size(px(11.0)).text_color(c.subtle).child("or type an answer below"));
        }
        // Keep the card compact: content scrolls within a height cap tied to the pane, actions stay pinned.
        let cap = (self.pane_size.height / px(1.0) * 0.4).clamp(120.0, 320.0);
        let scroll = div().id(SharedString::from(format!("appr-scroll-{}", a.id))).min_h_0().max_h(px(cap)).overflow_y_scroll().child(body);
        let card = div().debug_selector(|| "cw-approval".to_string()).w_full().flex().flex_col().gap_2().mb_2().p_3().min_w_0().rounded_xl().border_1().border_color(alpha(c.warning, 0.55)).bg(c.raised).shadow_md().child(head).child(scroll).child(actions);
        dock_inset(rise(card, SharedString::from(format!("appr-{}", a.id)), 0, 14.0), false).into_any_element()
    }

    fn render_popup(&mut self, text: &str, c: Colors, cx: &mut Context<Self>) -> Option<AnyElement> {
        let rows = self.popup_rows(text, cx);
        if rows.is_empty() {
            return None;
        }
        let selected = self.popup.min(rows.len() - 1);
        let mut list = div()
            .id("cw-popup-list")
            .flex()
            .flex_col()
            .max_h(px(236.0))
            .overflow_y_scroll()
            .track_scroll(&self.popup_scroll)
            .p_1();
        for (i, (label, hint, insert)) in rows.into_iter().enumerate() {
            list = list.child(
                div()
                    .id(("cw-pop", i))
                    .flex()
                    .items_center()
                    .gap_3()
                    .px_2p5()
                    .h(px(28.0))
                    .flex_shrink_0()
                    .rounded_lg()
                    .cursor_pointer()
                    .text_size(px(12.5))
                    .when(i == selected, |d| d.bg(alpha(c.accent, 0.14)))
                    .hover(move |d| d.bg(c.hover))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.set_input(&insert, window, cx);
                        this.focus_composer(window, cx);
                    }))
                    .child(
                        div()
                            .font_family(fonts::mono(cx))
                            .text_color(if i == selected { c.accent } else { c.fg })
                            .when(hint.is_empty(), |d| d.flex_1().min_w_0().truncate())
                            .child(label),
                    )
                    .when(!hint.is_empty(), |d| {
                        d.child(div().flex_1().min_w_0().truncate().text_size(px(11.5)).text_color(c.subtle).child(hint))
                    }),
            );
        }
        Some(dock_inset(rise(div().mb_2().rounded_xl().border_1().border_color(c.edge).bg(c.raised).shadow_md().overflow_hidden().child(list), "cw-popup", 0, 6.0), false).into_any_element())
    }

    /// While dictating: a pulsing dot, the time, and how to finish.
    fn render_dictation(&self, c: Colors, cx: &App) -> Option<AnyElement> {
        let d = self.dictation.as_ref()?;
        let lay = composer_layout(self.pane_size.width / px(1.0), self.pane_size.height / px(1.0));
        let dot = div().size(px(7.0)).rounded_full().bg(c.danger)
            .with_animation("cw-dictate-dot", Animation::new(Duration::from_millis(1200)).repeat(), |d, t| d.opacity(0.35 + 0.65 * breathe(t)));
        let row = div().h(px(26.0)).flex().items_center().gap_2().px(if lay.tight_x { px(8.0) } else { px(12.0) }).text_size(px(11.5)).text_color(c.subtle)
            .child(dot)
            .child(div().text_color(c.fg).child(if d.stopping { "Finishing…" } else { "Listening…" }))
            .child(div().font_family(fonts::mono(cx)).child(elapsed(now_ms().saturating_sub(d.started))))
            .child(div().flex_1())
            .when(!d.stopping, |r| r.child(div().child("release to insert")));
        Some(dock_inset(rise(row, "cw-dictate", 0, 4.0), lay.compact_v || lay.tight_x).into_any_element())
    }

    /// Spinner row above the composer while a turn runs: elapsed time, token in/out, cached %, tok/s.
    fn render_status(&self, c: Colors, cx: &App) -> Option<AnyElement> {
        let (_, _, _, _, _, _, show_rate, ..) = self.prefs(cx);
        let st = &self.model.read(cx).state;
        if !st.busy {
            return None;
        }
        let lay = composer_layout(self.pane_size.width / px(1.0), self.pane_size.height / px(1.0));
        let mono = fonts::mono(cx);
        let secs = st.turn_started.map(|s| elapsed(now_ms().saturating_sub(s)));
        let turn_in = st.turn_input();
        let turn_cached = st.turn_cached();
        let turn_out = st.turn_output();
        let rate = st.tokens_per_second();
        let cache_pct = if turn_in > 0 && turn_cached > 0 { Some((((turn_cached as f64 / turn_in as f64) * 100.0).round() as u64).min(100)) } else { None };
        // "Working" itself trails the transcript; this row starts with the elapsed time.
        let pad_x = if lay.tight_x { px(8.0) } else { px(12.0) };
        let row = div().h(px(26.0)).flex().items_center().gap_1p5().px(pad_x).text_size(px(11.5)).text_color(c.subtle)
            .when_some(secs.clone(), |d, s| d.child(div().font_family(mono.clone()).child(s)))
            .when(turn_in > 0, |d| {
                let in_label = if let (Some(pct), false) = (cache_pct, lay.tight_x) {
                    format!("↑ {} in ({pct}% cached)", tokens(turn_in))
                } else {
                    format!("↑ {} in", tokens(turn_in))
                };
                d.when(secs.is_some(), |d| d.child("·")).child(div().font_family(mono.clone()).child(in_label))
            })
            .when(secs.is_some() || turn_in > 0, |d| d.child("·"))
            .child(div().font_family(mono.clone()).child(format!("↓ {} out", tokens(turn_out))))
            .when(show_rate, |d| d.when_some(rate, |d, r| d.child("·").child(div().font_family(mono.clone()).text_color(c.muted).child(format!("{r:.0} tok/s")))))
            .child(div().flex_1())
            .child(div().flex_shrink_0().text_color(c.subtle).child(if lay.tight_x { "esc" } else { "esc to interrupt" }));
        Some(dock_inset(rise(row, "cw-status", 0, 4.0), lay.compact_v || lay.tight_x).into_any_element())
    }

    fn render_composer(&mut self, c: Colors, cx: &mut Context<Self>) -> AnyElement {
        let (_, _, _, _, show_cost, show_context, show_rate, ..) = self.prefs(cx);
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
        let (mode, branch, cost, pct, window_tokens, context, total_in, total_cached, total_out) = (
            st.mode,
            st.branch.clone(),
            st.usage.cost,
            st.context_fraction(),
            st.usage.window,
            st.usage.context,
            st.usage.input,
            st.usage.cached,
            st.usage.output,
        );
        // The context meter only means something once the session has started (the agent reported usage).
        let show_context = show_context && context > 0;
        let done_rate = (!busy).then_some(st.last_rate).flatten();
        let has_done_metrics = !busy && (total_in > 0 || total_out > 0);
        let draft_empty = self.draft(cx).trim().is_empty();
        let stop = busy && draft_empty;
        let can_send = !draft_empty || !attachments.is_empty();
        let v = cx.entity();
        let open = |sheet: Sheet| { let v = v.clone(); move |_: &ClickEvent, _: &mut Window, cx: &mut App| { let _ = v.update(cx, |this, cx| this.set_sheet(Some(sheet), cx)); } };
        let model_backend = crate::code::names::backend_of(&st.model).map(|b| b.0);
        let ultra_supported = crate::code::names::supports_ultracode(&st.model);
        let ultra_on = st.ultracode && ultra_supported;
        let ultra_ignite_seq = ultra_on.then(|| self.ultra_igniting()).flatten();
        let ultra_cool_seq = (!ultra_on && ultra_supported).then(|| self.ultra_cooling()).flatten();
        let effort_id = if st.effort.is_empty() { "default".to_string() } else { st.effort.clone() };
        let open_sheet = st.sheet;
        let model_pop = (open_sheet == Some(Sheet::Model)).then(|| self.model_popover(c, cx));
        let effort_pop = (open_sheet == Some(Sheet::Effort)).then(|| self.effort_popover(c, cx));
        let mode_pop = (open_sheet == Some(Sheet::Mode)).then(|| self.mode_popover(c, cx));
        let mode_color = if matches!(mode, Mode::Yolo | Mode::YoloPlan) { c.warning } else { c.muted };

        let lay = composer_layout(self.pane_size.width / px(1.0), self.pane_size.height / px(1.0));
        let pill_h = if lay.ultra_v { 22.0 } else if lay.compact_v { 24.0 } else { 26.0 };
        let send_sz = if lay.ultra_v { 24.0 } else if lay.compact_v { 26.0 } else { 32.0 };
        let attach_sz = if lay.ultra_v { 22.0 } else if lay.compact_v { 24.0 } else { 28.0 };
        let attach_btn = div().id("cw-attach").size(px(attach_sz)).flex_shrink_0().flex().items_center().justify_center().rounded_lg().cursor_pointer().text_color(c.subtle).hover(move |d| d.bg(c.hover).text_color(c.fg)).tooltip(tip("Attach files or images")).on_click(cx.listener(|this, _, _, cx| this.pick_files(cx))).child(Icon::new(IconName::Paperclip).size(px(if lay.ultra_v { 13.0 } else { 15.0 })));
        let mode_btn = div().relative().flex_shrink_0().children(mode_pop).child(
            pill(c, "cw-mode", Some(super::code_pickers::mode_glyph(mode_color, mode, 14.0)), lay.show_labels.then(|| SharedString::from(mode.label())), pill_h)
                .text_color(mode_color)
                .tooltip(tip(format!("Mode: {} · Shift+Tab to cycle", mode.label())))
                .when(open_sheet == Some(Sheet::Mode), |d| d.bg(c.hover).text_color(c.fg))
                .on_click(cx.listener(|this, _, _, cx| this.toggle_picker(Sheet::Mode, cx))),
        );
        let model_tip = format!("Model: {model_label}");
        let model_btn = div().relative().min_w_0().children(model_pop).child(
            pill(c, "cw-model", model_backend.map(|b| super::code_pickers::mark(c, b, 14.0, 1.0)), lay.show_model_label.then(|| SharedString::from(model_label)), pill_h)
                .max_w(px(if lay.show_labels { 200.0 } else { 130.0 }))
                .tooltip(tip(model_tip))
                .when(open_sheet == Some(Sheet::Model), |d| d.bg(c.hover).text_color(c.fg))
                .on_click(cx.listener(|this, _, _, cx| this.toggle_picker(Sheet::Model, cx))),
        );
        let fast_btn = fast.map(|(on, listed, target)| {
            pill(c, "cw-fast", Some(Icon::new(IconName::Zap).size(px(13.0)).text_color(if on { c.warning } else { c.subtle }).into_any_element()), lay.show_labels.then(|| SharedString::from("Fast")), pill_h)
                .flex_shrink_0()
                .when(on, |d| d.text_color(c.warning).bg(alpha(c.warning, 0.12)))
                .when(!on && !listed, |d| d.opacity(0.6))
                .tooltip(tip(if on { "Fast mode is on: about 2x speed, higher usage. Click to turn off" } else if listed { "Turn on fast mode: about 2x speed, higher usage" } else { "Try fast mode. Codex doesn't list a fast tier for this model, so it may not apply" }))
                .on_click(cx.listener(move |this, _, window, cx| this.pick_model(&target, window, cx)))
        });
        let effort_tip = if ultra_on {
            match model_backend {
                Some("codex") => "Ultracode ON · Multi-agent v2 + Ultra reasoning".to_string(),
                Some("agy") => format!("Reasoning effort: {} · Ultracode ON (Agent swarm)", super::code_pickers::effort_label(&effort_id)),
                _ => format!("Reasoning effort: {} · Ultracode ON (Dynamic workflows)", super::code_pickers::effort_label(&effort_id)),
            }
        } else {
            format!("Reasoning effort: {}", super::code_pickers::effort_label(&effort_id))
        };
        let effort_btn = div().relative().flex_shrink_0().children(effort_pop).child({
            let (glyph, label) = if ultra_on {
                let bolt_tint = if c.light { theme::hex("#7c3aed") } else { theme::hex("#7dd3fc") };
                let lbl = lay.show_labels.then(|| {
                    if model_backend == Some("codex") || effort == "default" {
                        SharedString::from("Ultracode")
                    } else {
                        SharedString::from(format!("{effort} · Ultra"))
                    }
                });
                (super::code_pickers::effort_glyph(bolt_tint, "ultra", 14.0), lbl)
            } else {
                (super::code_pickers::effort_glyph(c.muted, &effort_id, 14.0), lay.show_labels.then(|| SharedString::from(effort)))
            };
            pill(c, "cw-effort", Some(glyph), label, pill_h)
                .tooltip(tip(effort_tip))
                .when(open_sheet == Some(Sheet::Effort), |d| d.bg(c.hover).text_color(c.fg))
                .on_click(cx.listener(|this, _, _, cx| this.toggle_picker(Sheet::Effort, cx)))
        });
        let mic_btn = crate::dictation::supported().then(|| {
            let on = self.dictation.as_ref().is_some_and(|d| !d.stopping);
            let mic_sz = attach_sz;
            div().id("cw-mic").size(px(mic_sz)).flex_shrink_0().flex().items_center().justify_center().rounded_lg().cursor_pointer()
                .when(!on, |d| d.text_color(c.subtle).hover(move |d| d.bg(c.hover).text_color(c.fg)))
                .when(on, |d| d.text_color(c.danger).bg(alpha(c.danger, 0.16)))
                .tooltip(tip(if on { "Listening · release to insert" } else { "Hold to dictate" }))
                .on_mouse_down(MouseButton::Left, cx.listener(|this, _, window, cx| {
                    cx.stop_propagation();
                    this.start_dictation(window, cx);
                }))
                .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _, cx| this.stop_dictation(cx)))
                .on_mouse_up_out(MouseButton::Left, cx.listener(|this, _, _, cx| this.stop_dictation(cx)))
                .child(Icon::new(IconName::Mic).size(px(if lay.ultra_v { 13.0 } else { 15.0 })))
        });
        let send_btn = nebula_send(c, stop, can_send, animate_send, if stop { IconName::Square } else { IconName::ArrowUp }, send_sz)
            .tooltip(tip(if stop { "Interrupt (Esc)" } else if busy { "Steer the running turn (Enter)" } else { "Send (Enter)" }))
            .on_click(cx.listener(move |this, _, window, cx| if stop { this.send(Cmd::Interrupt, cx) } else { this.enter(window, cx) }));

        let composer_inner_r = px(if lay.compact_v { 11.0 } else { 15.0 });
        let mut card = div()
            .relative()
            .w_full()
            .min_w_0()
            .flex()
            .flex_col()
            .debug_selector(|| "cw-composer-card".to_string())
            .bg(c.raised)
            .border_color(if ultra_on { alpha(theme::hex("#a855f7"), if c.light { 0.30 } else { 0.34 }) } else { c.edge })
            .shadow(if ultra_on {
                vec![
                    gpui::BoxShadow { color: alpha(theme::hex("#a855f7"), if c.light { 0.12 } else { 0.20 }), offset: gpui::point(px(0.0), px(0.0)), blur_radius: px(20.0), spread_radius: px(-2.0), inset: false },
                    gpui::BoxShadow { color: hsla(0.0, 0.0, 0.0, if c.light { 0.14 } else { 0.55 }), offset: gpui::point(px(0.0), px(8.0)), blur_radius: px(24.0), spread_radius: px(-2.0), inset: false },
                ]
            } else {
                vec![
                    gpui::BoxShadow { color: hsla(0.0, 0.0, 0.0, if c.light { 0.14 } else { 0.55 }), offset: gpui::point(px(0.0), px(8.0)), blur_radius: px(24.0), spread_radius: px(-2.0), inset: false },
                    gpui::BoxShadow { color: hsla(0.0, 0.0, 0.0, if c.light { 0.08 } else { 0.40 }), offset: gpui::point(px(0.0), px(2.0)), blur_radius: px(6.0), spread_radius: px(0.0), inset: false },
                ]
            })
            .when(ultra_on || ultra_cool_seq.is_some(), |d| {
                d.child(super::code_pickers::ultracode_composer_aurora(
                    c.light,
                    composer_inner_r,
                    ultra_ignite_seq,
                    ultra_cool_seq,
                ))
            })
            .when_some(ultra_ignite_seq, |d, seq| d.child(super::code_pickers::ultracode_composer_ignite(seq, c.light, composer_inner_r)))
            .drag_over::<ExternalPaths>(move |d, _, _, _| d.border_color(c.accent).bg(alpha(c.accent, 0.08)))
            .on_drop(cx.listener(|this, paths: &ExternalPaths, window, cx| {
                this.attach_paths(paths.paths().to_vec(), cx);
                this.focus_composer(window, cx);
                cx.stop_propagation();
            }));
        card = if lay.compact_v {
            card.border_1().rounded_xl()
        } else {
            card.border_1().rounded_2xl()
        };
        if !attachments.is_empty() {
            let thumb_sz = if lay.compact_v { 40.0 } else { 48.0 };
            let chip_max = if lay.tight_x { 150.0 } else { 220.0 };
            let mut chips = div()
                .w_full()
                .min_w_0()
                .flex()
                .flex_wrap()
                .items_center()
                .gap_1p5()
                .px_3()
                .pt_2p5()
                .pb_1()
                .debug_selector(|| "cw-attachments".to_string());
            for (i, a) in attachments.iter().enumerate() {
                let decoded = if a.mime.starts_with("image/") && !a.data.is_empty() {
                    let mut h = std::collections::hash_map::DefaultHasher::new();
                    a.mime.hash(&mut h);
                    a.data.hash(&mut h);
                    let key = format!("att-{:x}", h.finish());
                    let mut cache = self.images.borrow_mut();
                    cache
                        .entry(key)
                        .or_insert_with(|| {
                            let url = format!("data:{};base64,{}", a.mime, a.data);
                            data_image(&url).into_iter().collect()
                        })
                        .first()
                        .cloned()
                } else {
                    None
                };

                if let Some(image) = decoded {
                    let full = image.clone();
                    chips = chips.child(
                        div()
                            .id(("cw-att", i))
                            .relative()
                            .size(px(thumb_sz))
                            .flex_shrink_0()
                            .rounded_lg()
                            .overflow_hidden()
                            .bg(c.hover)
                            .border_1()
                            .border_color(c.border)
                            .cursor_pointer()
                            .tooltip(tip(a.name.clone()))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.preview = Some(full.clone());
                                cx.notify();
                            }))
                            .child(gpui_kit::img(image).size(px(thumb_sz)).object_fit(ObjectFit::Cover))
                            .child(
                                div()
                                    .id(("cw-att-x", i))
                                    .absolute()
                                    .top(px(2.0))
                                    .right(px(2.0))
                                    .size(px(16.0))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded_full()
                                    .bg(alpha(c.panel, 0.85))
                                    .text_color(c.fg)
                                    .cursor_pointer()
                                    .hover(move |d| d.bg(c.panel))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.model.update(cx, |m, _| {
                                            if i < m.attachments.len() {
                                                m.attachments.remove(i);
                                            }
                                        });
                                        cx.notify();
                                    }))
                                    .child(Icon::new(IconName::X).size(px(10.0))),
                            ),
                    );
                } else {
                    let tip_text = a.path.clone().unwrap_or_else(|| a.name.clone());
                    chips = chips.child(
                        div()
                            .id(("cw-att", i))
                            .flex()
                            .items_center()
                            .gap_1p5()
                            .h(px(24.0))
                            .max_w(px(chip_max))
                            .min_w_0()
                            .overflow_hidden()
                            .pl_2()
                            .pr_1()
                            .rounded_lg()
                            .bg(c.hover)
                            .border_1()
                            .border_color(c.border)
                            .text_size(px(11.5))
                            .text_color(c.muted)
                            .tooltip(tip(tip_text))
                            .child(Icon::new(if a.mime.starts_with("image/") { IconName::Image } else { IconName::FileText }).size(px(12.0)).flex_shrink_0())
                            .child(div().min_w_0().flex_1().truncate().child(a.name.clone()))
                            .child(
                                div()
                                    .id(("cw-att-x", i))
                                    .size(px(16.0))
                                    .flex_shrink_0()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded_md()
                                    .cursor_pointer()
                                    .hover(move |d| d.bg(c.active))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.model.update(cx, |m, _| {
                                            if i < m.attachments.len() {
                                                m.attachments.remove(i);
                                            }
                                        });
                                        cx.notify();
                                    }))
                                    .child(Icon::new(IconName::X).size(px(10.0))),
                            ),
                    );
                }
            }
            card = card.child(chips);
        }
        let view_for_paste = cx.entity();
        // Composer text follows the web UI text scale (Settings → winfer code), like the transcript.
        let scale = self.prefs(cx).0;
        // Textarea sizes its own text from `Size` (Size::Size(s) renders at s * 0.875), so the parent's text size is ignored.
        // Cap the rows to what the pane fits instead of clipping a taller textarea: a clipped third row
        // can't be scrolled to (the textarea itself isn't overflowing).
        let rows = if lay.ultra_v { 1 } else if lay.compact_v { 2 } else { 3 };
        if rows != self.composer_rows {
            self.composer_rows = rows;
            self.input.update(cx, |s, cx| s.set_auto_grow(1, rows, cx));
        }
        let composer_input = Textarea::new(&self.input).with_size(KitSize::Size(px(13.5 * scale / 0.875))).appearance(false).on_paste(move |item, _, cx| {
            view_for_paste.update(cx, |this, cx| this.paste_clipboard_item(item, cx))
        });
        if lay.single_row && attachments.is_empty() {
            // Very short pane with horizontal room: input and compact icon controls share one row.
            card = card.child(
                div().flex().items_end().gap_0p5().px_1p5().py_1()
                    .child(attach_btn)
                    .child(div().flex_1().min_w_0().max_h(px(46.0)).overflow_hidden().px_1().child(composer_input))
                    .child(mode_btn)
                    .child(model_btn)
                    .children(fast_btn)
                    .child(effort_btn)
                    .children(mic_btn)
                    .child(send_btn),
            );
        } else {
            // Line the text up with the paperclip glyph below: toolbar inset (6) + the icon's inset in
            // its button, minus the textarea's own 10px left padding, plus a little breathing room.
            const EXTRA: f32 = 3.0;
            let clip_sz = if lay.ultra_v { 13.0 } else { 15.0 };
            let input_pl = px((6.0 + (attach_sz - clip_sz) / 2.0 - 10.0f32).max(0.0) + EXTRA);
            // The textarea's own 8px plus the line's leading already roughly matches the left
            // inset, so the top gets the same extra room plus 5px to look even.
            let input_wrap = div().pl(input_pl).pt(px(EXTRA + 5.0));
            let input_wrap = if lay.ultra_v {
                input_wrap.pr_2p5().pb_0p5()
            } else if lay.compact_v {
                input_wrap.pr_2p5().pb_0p5()
            } else {
                input_wrap.pr_3().pb_1()
            };
            card = card.child(input_wrap.child(composer_input));
            let bar_h = if lay.ultra_v { 28.0 } else if lay.compact_v { 31.0 } else { 36.0 };
            let send_pb = if lay.ultra_v { 3.5 } else if lay.compact_v { 4.0 } else { 5.0 };
            let controls_pb = if lay.compact_v { 3.0 } else { 4.0 };
            let controls = div().flex_1().min_w_0().flex().items_center().gap_0p5().pb(px(controls_pb))
                .child(attach_btn)
                .when(lay.show_labels, |d| d.child(div().w(px(1.0)).h(px(14.0)).mx_1().bg(c.border)))
                .child(mode_btn)
                .child(model_btn)
                .children(fast_btn)
                .child(effort_btn)
                .when(lay.show_branch, |d| d.when_some(branch, |d, b| d.child(div().id("cw-branch").h(px(pill_h)).max_w(px(160.0)).flex().items_center().gap_1p5().px_2().rounded_lg().text_size(px(12.0)).text_color(c.subtle)
                    .tooltip(tip(format!("Branch {b}")))
                    .child(Icon::new(IconName::GitBranch).size(px(13.0)).flex_shrink_0())
                    .child(div().min_w_0().truncate().child(b)))))
                .child(div().flex_1())
                .when(lay.show_usage && ((show_cost && lay.show_cost && cost.is_some()) || show_context || has_done_metrics), |d| {
                    let mono = fonts::mono(cx);
                    let mut tip_parts = vec![format!("Context: {} / {} tokens", tokens(context), tokens(window_tokens))];
                    if has_done_metrics {
                        let cached_s = if total_cached > 0 { format!(" ({} cached)", tokens(total_cached)) } else { String::new() };
                        tip_parts.push(format!("Session: ↑ {} in{cached_s} · ↓ {} out", tokens(total_in), tokens(total_out)));
                    }
                    if let Some(r) = done_rate {
                        tip_parts.push(format!("Speed: {r:.0} tok/s"));
                    }
                    let usage_tip = tip_parts.join("\n");
                    d.child(
                        div().id("cw-usage").h(px(pill_h)).flex_shrink_0().flex().items_center().gap_1p5().px(px(if lay.show_labels { 8.0 } else { 5.0 })).mr_0p5().rounded_lg().cursor_pointer().text_size(px(11.0)).text_color(c.subtle).hover(move |d| d.bg(c.hover))
                            .tooltip(tip(usage_tip))
                            .on_click(open(Sheet::Usage))
                            .when(has_done_metrics && lay.show_token_metrics, |d| {
                                d.child(div().font_family(mono.clone()).text_size(px(10.5)).child(format!("↑{} ↓{}", tokens(total_in), tokens(total_out))))
                            })
                            .when(!busy && show_rate && lay.show_rate_inline, |d| {
                                d.when_some(done_rate, |d, r| d.child(div().font_family(mono.clone()).text_size(px(10.5)).child(format!("{r:.0} t/s"))))
                            })
                            .when(show_cost && lay.show_cost, |d| d.when_some(cost, |d, cost| d.child(format!("${cost:.2}"))))
                            .when(show_context, |d| d.child(super::root::pane_header::context_ring(
                                pct as f64,
                                if pct > 0.85 { c.danger } else if pct > 0.65 { c.warning } else { c.accent },
                                if pct > 0.65 { if pct > 0.85 { c.danger } else { c.warning } } else { c.fg },
                                22.0, mono.clone(), c,
                            ))),
                    )
                });
            let toolbar = div().flex().items_end().gap_0p5().px_1p5().h(px(bar_h))
                .child(controls)
                .when_some(mic_btn, |d, mic| d.child(div().flex_shrink_0().pb(px(controls_pb)).child(mic)))
                .child(div().flex_shrink_0().pb(px(send_pb)).child(send_btn));
            card = card.child(toolbar);
        }
        let tight = lay.compact_v || lay.tight_x;
        let edge = if tight { px(6.0) } else { px(8.0) };
        dock_inset(div().pt(if lay.compact_v { px(2.0) } else { px(4.0) }).pb(edge).child(card), tight).flex_shrink_0().into_any_element()
    }
}

impl Render for CodeView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme_id = self.app.read(cx).theme.clone();
        let c = theme::colors(&theme_id);
        let mono = fonts::mono(cx);
        let (scale, compact, expand_thinking, expand_shell, _, _, _, show_plan, _) = self.prefs(cx);
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
        if !self.picks.is_empty() {
            self.picks.retain(|id, _| approvals.iter().any(|a| a.id == *id));
        }
        if self.live(cx) {
            self.tick(cx);
        }
        self.ensure_picker(window, cx);
        let modal_open = self.has_blocking_modal(cx);
        if modal_open {
            if self.input.read(cx).focus_handle(cx).is_focused(window) {
                self.restore_composer_focus = true;
                self.focus.focus(window, cx);
            }
        } else if std::mem::take(&mut self.restore_composer_focus) && self.focus.is_focused(window) {
            self.focus_composer(window, cx);
        }
        let compact_tools = self.compact_tools(cx);
        let items = if self.prefs(cx).8 { items } else { items.into_iter().filter(|i| !matches!(i, Item::Notice { text, error: false, .. } if crate::code::bg_note::parse(text).is_some())).collect() };
        let home = crate::voice::home_pane(&self.model.read(cx).pane_id).is_some();
        self.sync_list(&items, busy, compact_tools, now, home);
        // A newly sent message resumes following the bottom, even when scrolled up.
        let last_user = items.iter().rev().find(|i| matches!(i, Item::User { .. })).map(|i| i.key().to_string());
        if last_user != self.last_user {
            if last_user.is_some() {
                self.list.scroll_to_end();
            }
            self.last_user = last_user;
        }
        if std::mem::take(&mut self.scroll_end_pending) {
            self.list.scroll_to_end();
        }
        let away = !items.is_empty() && !self.list.is_following_tail() && self.list.is_scrolled_to_end() == Some(false);
        let pane_id = self.model.read(cx).pane_id.clone();
        let remote = self.app.read(cx).pane(&pane_id).is_some_and(|p| p.remote.is_some());
        let env = Env { c, mono: mono.clone(), scale, compact, view: cx.entity().downgrade(), flipped: Rc::new(self.flipped.clone()), toggled: Rc::new(self.toggled.clone()), expand_thinking, expand_shell, images: self.images.clone(), wg_scrolls: self.wg_scrolls.clone(), now, cwd: self.model.read(cx).state.cwd.clone(), remote, born: Rc::new(self.born.clone()), width: (self.pane_size.width / px(1.0) - 32.0).max(240.0), height: self.pane_size.height / px(1.0), refs: self.refs.clone() };
        let model = self.model.clone();
        let rows = self.rows.clone();
        let transcript: AnyElement = if items.is_empty() {
            self.welcome(c, cx)
        } else {
            let list_env = env.clone();
            let hidden = self.hidden;
            let lm = (hidden > 0) as usize;
            let count = rows.len() + lm;
            list(self.list.clone(), move |ix, _, cx| {
                if lm == 1 && ix == 0 {
                    let (view, c) = (list_env.view.clone(), list_env.c);
                    let label = format!("Load more… ({hidden} older hidden)");
                    return div().w_full().flex().justify_center().pt_4().pb_2().child(
                        div().id("cw-load-more").h(px(28.0)).px_3().flex().items_center().rounded_full().cursor_pointer().border_1().border_color(c.border).bg(c.raised).text_size(px(12.0)).text_color(c.muted).hover(move |d| d.bg(c.hover).text_color(c.fg))
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_click(move |_, _, cx| { let _ = view.update(cx, |v, cx| v.load_more(cx)); })
                            .child(label),
                    ).into_any_element();
                }
                let ix = ix - lm;
                let items = &model.read(cx).state.items;
                let Some(row) = rows.get(ix) else {
                    return div().into_any_element();
                };
                let inner: AnyElement = match row {
                    DisplayRow::Working => working_row(&list_env),
                    DisplayRow::Events { key, events } => super::home_events::events_row(&list_env, items, key, events),
                    DisplayRow::Report { key, items: charts } => report_row(&list_env, key, charts.iter().filter_map(|ix| items.get(*ix))),
                    DisplayRow::WorkGroup { key, start, end, active } => {
                        work_group_row(&list_env, items, key, *start, *end, *active)
                    }
                    DisplayRow::Item(item_ix) => {
                        let (prev, next) = (item_ix.checked_sub(1).and_then(|i| items.get(i)), items.get(item_ix + 1));
                        match items.get(*item_ix) {
                            Some(item) => render_item(&list_env, item, ix == 0 && lm == 0, prev, next),
                            None => div().into_any_element(),
                        }
                    }
                };
                div().w_full().flex().justify_center().px_4().debug_selector(move || format!("cw-row-{ix}")).child(div().w_full().pt(px(if ix == 0 && lm == 0 { 16.0 } else { 0.0 })).pb(px(if ix + lm + 1 == count { 12.0 } else { 0.0 })).child(inner)).into_any_element()
            }).size_full().into_any_element()
        };
        let jump = away.then(|| {
            div().absolute().bottom(px(12.0)).left_0().right_0().flex().justify_center().child(
                div()
                    .id("cw-jump")
                    .size(px(32.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .cursor_pointer()
                    .bg(c.raised)
                    .border_1()
                    .border_color(c.border)
                    .shadow(vec![BoxShadow { color: alpha(black(), if c.light { 0.12 } else { 0.4 }), offset: point(px(0.0), px(6.0)), blur_radius: px(18.0), spread_radius: px(0.0), inset: false }])
                    .hover(move |d| d.bg(c.hover))
                    .tooltip(tip("Jump to latest"))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.list.scroll_to_end();
                        cx.notify();
                    }))
                    .child(Icon::new(IconName::ArrowDown).size(px(16.0)).text_color(c.fg))
                    .with_animation("cw-jump-in", Animation::new(Duration::from_millis(180)), |d, t| d.opacity(ease_out(t)).mt(px(6.0 * (1.0 - ease_out(t))))),
            )
        });
        let text = self.draft(cx);
        let popup = self.render_popup(&text, c, cx);
        let plan = if show_plan { self.render_plan(c, cx) } else { None };
        let agent_tree = self.render_agent_tree(c, mono.clone(), cx);
        let limit_wait = self.render_limit_wait(c, cx);
        let approval_cards: Vec<AnyElement> = approvals.iter().take(1).map(|a| self.render_approval(a, c, mono.clone(), cx)).collect();
        let more_approvals = approvals.len().saturating_sub(1);
        let status = self.render_status(c, cx);
        let dictating = self.render_dictation(c, cx);
        let composer = self.render_composer(c, cx);
        let preview = self.preview.clone().map(|image| {
            div()
                .id("cw-lightbox")
                .absolute()
                .inset_0()
                .size_full()
                .rounded_b(px(15.0))
                .flex()
                .items_center()
                .justify_center()
                .bg(alpha(black(), 0.82))
                .p_6()
                .occlude()
                .cursor_pointer()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(|this, _, _, cx| {
                    this.preview = None;
                    cx.notify();
                }))
                .child(gpui_kit::img(image).max_w_full().max_h_full().object_fit(ObjectFit::Contain))
                .with_animation("cw-lightbox-in", Animation::new(Duration::from_millis(180)), |d, t| d.opacity(ease_out(t)))
        });
        if sheet != Some(Sheet::TaskLog) {
            self.task_log_scroll.reset();
        }
        let sheet_el = sheet.filter(|s| !matches!(s, Sheet::Model | Sheet::Effort | Sheet::Mode)).map(|s| self.render_sheet(s, c, mono.clone(), window, cx));
        let exited_note = exited.map(|e| col(div().mb_2().rounded_xl().border_1().border_color(alpha(c.danger, 0.5)).bg(alpha(c.danger, 0.08)).px_3().py_2().text_size(px(12.5)).text_color(c.danger).child(e)));
        let view_for_size = cx.entity();
        let view_for_zoom = cx.entity();
        let measure = canvas(
            move |bounds, window, cx| {
                view_for_size.update(cx, |this, cx| {
                    let prev = (
                        composer_layout(this.pane_size.width / px(1.0), this.pane_size.height / px(1.0)),
                        welcome_layout(this.pane_size.width / px(1.0), this.pane_size.height / px(1.0)),
                    );
                    let next = (
                        composer_layout(bounds.size.width / px(1.0), bounds.size.height / px(1.0)),
                        welcome_layout(bounds.size.width / px(1.0), bounds.size.height / px(1.0)),
                    );
                    let prev_wg_h = work_group_max_h(this.pane_size.height / px(1.0));
                    let next_wg_h = work_group_max_h(bounds.size.height / px(1.0));
                    let has_open_wg = this.rows.iter().any(|r| {
                        matches!(r, DisplayRow::WorkGroup { key, .. } if this.flipped.contains(key) || this.toggled.contains_key(key))
                    });
                    this.pane_size = bounds.size;
                    if prev != next || (prev_wg_h != next_wg_h && has_open_wg) {
                        cx.notify();
                    }
                });
                window.insert_hitbox(bounds, HitboxBehavior::Normal)
            },
            move |_, hitbox, window, _| {
                // Ctrl/⌘ + wheel steps the text size; capture phase so the transcript doesn't scroll.
                // Checking the hitbox instead of raw bounds ensures occluding modals block zoom too.
                let view = view_for_zoom.clone();
                window.on_mouse_event(move |e: &ScrollEvent, phase, window, cx| {
                    if phase != DispatchPhase::Capture || !(e.modifiers.control || e.modifiers.platform) || !hitbox.should_handle_scroll(window) {
                        return;
                    }
                    cx.stop_propagation();
                    view.update(cx, |this, cx| this.zoom_by_wheel(e.delta.pixel_delta(px(20.0)).y, cx));
                });
            },
        )
        .absolute()
        .size_full();
        div().id("code-pane").relative().size_full().rounded_b(px(15.0)).flex().flex_col().text_color(c.fg).font_family(fonts::sans(cx))
            .track_focus(&self.focus)
            .child(measure)
            .capture_key_down(cx.listener(|this, e: &KeyDownEvent, _, cx| {
                let m = e.keystroke.modifiers;
                let primary = if cfg!(target_os = "macos") { m.platform && !m.control } else { m.control && !m.platform };
                if !this.has_blocking_modal(cx) && primary && !m.alt && !m.shift && e.keystroke.key.eq_ignore_ascii_case("v") && this.paste_attachments(cx) {
                    cx.stop_propagation();
                }
            }))
            // The composer Textarea binds shift-tab to OutdentInline and up/down/tab/escape to
            // input actions, which run before on_key_down, so intercept them in the capture phase.
            .capture_action(cx.listener(|this, _: &OutdentInline, _, cx| {
                if this.model.read(cx).state.sheet.is_none() && this.preview.is_none() {
                    this.cycle_mode(cx);
                    cx.stop_propagation();
                }
            }))
            .capture_action(cx.listener(|this, _: &MoveDown, window, cx| {
                if this.move_popup(true, window, cx) {
                    cx.stop_propagation();
                }
            }))
            .capture_action(cx.listener(|this, _: &MoveUp, window, cx| {
                if this.move_popup(false, window, cx) {
                    cx.stop_propagation();
                }
            }))
            .capture_action(cx.listener(|this, _: &MovePageUp, _, cx| {
                if this.scroll_key("pageup", cx) {
                    cx.stop_propagation();
                }
            }))
            .capture_action(cx.listener(|this, _: &MovePageDown, _, cx| {
                if this.scroll_key("pagedown", cx) {
                    cx.stop_propagation();
                }
            }))
            .capture_action(cx.listener(|this, _: &MoveHome, window, cx| {
                // cmd-left/right and ctrl-a/e map to the same action: only the bare key scrolls.
                if !window.modifiers().modified() && this.scroll_key("home", cx) {
                    cx.stop_propagation();
                }
            }))
            .capture_action(cx.listener(|this, _: &MoveEnd, window, cx| {
                // cmd-left/right and ctrl-a/e map to the same action: only the bare key scrolls.
                if !window.modifiers().modified() && this.scroll_key("end", cx) {
                    cx.stop_propagation();
                }
            }))
            .capture_action(cx.listener(|this, _: &IndentInline, window, cx| {
                if this.accept_popup(window, cx) {
                    cx.stop_propagation();
                }
            }))
            .capture_action(cx.listener(|this, e: &InputEnter, window, cx| {
                if !e.shift && !e.secondary && this.accept_popup_on_enter(window, cx) {
                    cx.stop_propagation();
                }
            }))
            .capture_action(cx.listener(|this, _: &InputEscape, window, cx| {
                if this.dismiss_mention_popup(window, cx) {
                    cx.stop_propagation();
                }
            }))
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
                    } else if this.dismiss_mention_popup(window, cx) {
                        cx.stop_propagation();
                    } else if !this.draft(cx).is_empty() && this.draft(cx).trim_start().starts_with('/') {
                        this.set_input("", window, cx);
                    } else if { let st = &this.model.read(cx).state; st.busy || st.paused_until.is_some() || st.queued > 0 } {
                        // While out of usage, Esc stops waiting (and drops the queued messages).
                        this.send(Cmd::Interrupt, cx);
                    }
                } else if !e.keystroke.modifiers.modified() && matches!(k, "pageup" | "pagedown" | "home" | "end") {
                    // Focus outside the composer (e.g. after clicking the transcript).
                    if this.scroll_key(k, cx) {
                        cx.stop_propagation();
                    }
                } else if k == "tab" && e.keystroke.modifiers.shift && this.model.read(cx).state.sheet.is_none() && this.preview.is_none() {
                    this.cycle_mode(cx);
                } else if k == "tab" && !e.keystroke.modifiers.modified() && this.model.read(cx).state.sheet == Some(Sheet::Effort) && crate::code::names::supports_ultracode(&this.model.read(cx).state.model) {
                    this.toggle_ultracode(cx);
                    cx.stop_propagation();
                } else if this.model.read(cx).state.sheet.is_none() && this.preview.is_none() && this.draft(cx).is_empty() {
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
            .drag_over::<ExternalPaths>(move |d, _, _, _| d.border_2().border_color(alpha(c.accent, 0.55)))
            .on_drop(cx.listener(|this, paths: &ExternalPaths, window, cx| {
                if this.has_blocking_modal(cx) {
                    return;
                }
                this.attach_paths(paths.paths().to_vec(), cx);
                this.focus_composer(window, cx);
            }))
            .child(div().flex_1().min_h_0().relative().overflow_hidden().debug_selector(|| "cw-transcript".to_string()).child(transcript).children(jump))
            .children(status)
            .children(plan)
            .children(agent_tree)
            .children(limit_wait)
            .children(approval_cards)
            .when(more_approvals > 0, |d| d.child(col(div().pb_2().px_1().text_size(px(11.0)).text_color(c.subtle).child(format!("{more_approvals} more waiting")))))
            .children(exited_note)
            .children(popup)
            .children(dictating)
            .child(composer)
            .children(sheet_el)
            .children(preview)
    }
}

#[cfg(test)]
mod tests {
    use super::{attachment_from_clipboard_image, attachment_from_path, composer_layout, welcome_layout};
    use gpui_kit::{Image, ImageFormat};

    #[::core::prelude::v1::test]
    fn attachments_handle_images_documents_and_skip_dirs() {
        let dir = std::env::temp_dir().join(format!("winfer-att-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        assert!(attachment_from_path(&dir).is_none());

        let doc = dir.join("very_long_document_name_for_testing_truncation.pdf");
        std::fs::write(&doc, b"%PDF-1.4").unwrap();
        let doc_att = attachment_from_path(&doc).expect("document attaches by path");
        assert!(doc_att.mime.is_empty());
        assert_eq!(doc_att.path.as_deref(), Some(doc.to_string_lossy().as_ref()));

        let png_path = dir.join("shot.png");
        let png_bytes = [0x89u8, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
        std::fs::write(&png_path, png_bytes).unwrap();
        let img_att = attachment_from_path(&png_path).expect("png attaches inline");
        assert_eq!(img_att.mime, "image/png");
        assert!(img_att.path.is_none());
        assert!(!img_att.data.is_empty());

        let clip = Image::from_bytes(ImageFormat::Png, png_bytes.to_vec());
        let pasted = attachment_from_clipboard_image(&clip, 0).expect("clipboard image attaches");
        assert_eq!(pasted.name, "pasted-image.png");
        assert_eq!(pasted.mime, "image/png");
        let pasted2 = attachment_from_clipboard_image(&clip, 1).expect("second clipboard image numbered");
        assert_eq!(pasted2.name, "pasted-image-2.png");

        let mut tiff_bytes = Vec::new();
        image::DynamicImage::new_rgba8(2, 2)
            .write_to(&mut std::io::Cursor::new(&mut tiff_bytes), image::ImageFormat::Tiff)
            .unwrap();
        let tiff_clip = Image::from_bytes(ImageFormat::Tiff, tiff_bytes);
        let pasted_tiff = attachment_from_clipboard_image(&tiff_clip, 0).expect("tiff clipboard image converts to png");
        assert_eq!(pasted_tiff.name, "pasted-image.png");
        assert_eq!(pasted_tiff.mime, "image/png");
        assert!(!pasted_tiff.data.is_empty());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[::core::prelude::v1::test]
    fn composer_compacts_when_narrow_or_short() {
        let wide = composer_layout(760.0, 620.0);
        assert!(wide.show_branch && wide.show_labels && wide.show_model_label && wide.show_meter && wide.show_cost && !wide.compact_v && !wide.single_row);

        let mid = composer_layout(460.0, 620.0);
        assert!(!mid.show_branch && !mid.show_labels && !mid.show_meter && mid.show_model_label && mid.show_cost);

        let narrow = composer_layout(320.0, 620.0);
        assert!(narrow.tight_x && !narrow.show_labels && !narrow.show_model_label && !narrow.show_cost && narrow.show_usage);

        let short_wide = composer_layout(600.0, 240.0);
        assert!(short_wide.compact_v && short_wide.ultra_v && short_wide.single_row && !short_wide.show_labels && !short_wide.show_model_label);

        let short_narrow = composer_layout(340.0, 240.0);
        assert!(short_narrow.compact_v && short_narrow.ultra_v && !short_narrow.single_row && !short_narrow.show_labels && !short_narrow.show_model_label);
    }

    #[::core::prelude::v1::test]
    fn welcome_compacts_when_narrow_or_short() {
        let wide = welcome_layout(720.0, 640.0);
        assert!(!wide.compact && wide.show_cwd && !wide.short_cwd && wide.two_col && wide.suggestions == 4 && !wide.compact_cards && wide.show_hint);

        let narrow_tall = welcome_layout(250.0, 640.0);
        assert!(narrow_tall.compact && narrow_tall.short_cwd && !narrow_tall.two_col && narrow_tall.compact_cards && narrow_tall.suggestions == 4 && !narrow_tall.show_hint);

        let narrow_mid = welcome_layout(250.0, 440.0);
        assert!(narrow_mid.compact && narrow_mid.suggestions == 2 && narrow_mid.compact_cards);

        let short = welcome_layout(500.0, 320.0);
        assert!(short.compact && short.show_cwd && short.suggestions == 0 && !short.show_hint);

        let tiny = welcome_layout(240.0, 220.0);
        assert!(tiny.compact && !tiny.show_cwd && tiny.suggestions == 0 && !tiny.show_hint);
    }

    #[test]
    fn a_chart_stands_apart_from_the_work_groups_around_it() {
        use super::{DisplayRow, build_display_rows};
        use crate::code::state::Item;
        use serde_json::{Value, json};

        let tool = |k: &str, name: &str, input: Value, ok: Option<bool>| Item::Tool { key: k.into(), name: name.into(), title: String::new(), input, output: String::new(), ok, diff: None, started: 1, ended: Some(2) };
        let chart = json!({ "blocks": [{ "type": "text", "markdown": "hi" }] });
        let items = vec![
            Item::User { key: "u".into(), text: "hi".into(), at: 1, images: vec![] },
            tool("t1", "Bash", Value::Null, Some(true)),
            tool("v", "winfer · visualize", chart.clone(), Some(true)),
            tool("t2", "Bash", Value::Null, Some(true)),
        ];
        let rows = build_display_rows(&items, false, true);
        assert_eq!(rows.len(), 4, "{rows:?}");
        assert_eq!(rows[1], DisplayRow::Item(1));
        assert_eq!(rows[2], DisplayRow::Report { key: "rep-v".into(), items: vec![2] });
        assert_eq!(rows[3], DisplayRow::Item(3));
        // Charts with only work between them become one report; text between them splits it.
        let text = |k: &str| Item::Text { key: k.into(), text: "x".into(), streaming: false };
        let merged = vec![items[0].clone(), tool("v1", "winfer · visualize", chart.clone(), Some(true)), tool("t1", "Bash", Value::Null, Some(true)), tool("v2", "winfer · visualize", chart.clone(), Some(true)), text("a"), tool("v3", "winfer · visualize", chart.clone(), Some(true))];
        let rows = build_display_rows(&merged, false, true);
        assert_eq!(rows[1], DisplayRow::Report { key: "rep-v1".into(), items: vec![1, 3] });
        assert_eq!(rows[2], DisplayRow::Item(2));
        assert_eq!(rows[3], DisplayRow::Item(4));
        assert_eq!(rows[4], DisplayRow::Report { key: "rep-v3".into(), items: vec![5] });
        // Without compact tools only charts right next to each other merge.
        let adjacent = vec![items[0].clone(), tool("v1", "winfer · visualize", chart.clone(), Some(true)), tool("v2", "winfer · visualize", chart.clone(), Some(true))];
        assert_eq!(build_display_rows(&adjacent, false, false)[1], DisplayRow::Report { key: "rep-v1".into(), items: vec![1, 2] });
        // A call that failed is an ordinary tool row again, so the error shows in its group.
        let failed = vec![items[0].clone(), items[1].clone(), tool("v", "winfer · visualize", chart, Some(false))];
        assert_eq!(build_display_rows(&failed, false, true).len(), 2);
    }

    #[::core::prelude::v1::test]
    fn display_rows_compact_tools_into_updating_work_group() {
        use super::{DisplayRow, build_display_rows, group_duration_ms};
        use crate::code::state::Item;
        use serde_json::Value;

        let user = |k: &str| Item::User { key: k.into(), text: "hi".into(), at: 1, images: vec![] };
        let text = |k: &str, streaming: bool| Item::Text { key: k.into(), text: "ok".into(), streaming };
        let reason = |k: &str, started: u64, ended: Option<u64>| Item::Reasoning { key: k.into(), text: "hmm".into(), streaming: ended.is_none(), started, ended };
        let tool = |k: &str, ok: Option<bool>, started: u64, ended: Option<u64>| Item::Tool { key: k.into(), name: "Bash".into(), title: "ls".into(), input: Value::Null, output: String::new(), ok, diff: None, started, ended };
        let notice = |k: &str| Item::Notice { key: k.into(), text: "note".into(), error: false };
        let worked = |k: &str, ms: u64| Item::Worked { key: k.into(), ms };

        // Right after sending a message while busy: "Working…"
        assert_eq!(build_display_rows(&[user("u1")], true, true), vec![DisplayRow::Item(0), DisplayRow::Working]);

        // Text splits a turn's work: each run gets its own group, the trailing one live.
        let live = vec![user("u1"), reason("r1", 1000, Some(2000)), tool("t1", Some(true), 2000, Some(3000)), text("a1", false), tool("t2", None, 3000, None)];
        assert_eq!(
            build_display_rows(&live, true, true),
            vec![
                DisplayRow::Item(0),
                DisplayRow::WorkGroup { key: "wg-r1".into(), start: 1, end: 2, active: false },
                DisplayRow::Item(3),
                DisplayRow::WorkGroup { key: "wg-t2".into(), start: 4, end: 4, active: true },
            ]
        );

        // Between finished text and the next step, "Working…" keeps the turn visibly alive.
        assert_eq!(build_display_rows(&live[..4], true, true).last(), Some(&DisplayRow::Working));

        // Finished turn: a one-step run shows directly, and the total "Worked" row stays.
        let done = vec![user("u1"), reason("r1", 1000, Some(2000)), tool("t1", Some(true), 2000, Some(4000)), text("a1", false), tool("t2", Some(true), 4000, Some(8000)), text("a2", false), worked("w1", 92_000)];
        assert_eq!(
            build_display_rows(&done, false, true),
            vec![
                DisplayRow::Item(0),
                DisplayRow::WorkGroup { key: "wg-r1".into(), start: 1, end: 2, active: false },
                DisplayRow::Item(3),
                DisplayRow::Item(4),
                DisplayRow::Item(5),
                DisplayRow::Item(6),
            ]
        );
        assert_eq!(group_duration_ms(&done, 1, 2), Some(3000));

        // Notice stays outside work groups
        let with_notice = vec![user("u1"), tool("t1", Some(true), 1000, Some(2000)), tool("t2", Some(true), 2000, Some(3000)), notice("n1"), text("a2", false), worked("w1", 5000)];
        assert_eq!(
            build_display_rows(&with_notice, false, true),
            vec![
                DisplayRow::Item(0),
                DisplayRow::WorkGroup { key: "wg-t1".into(), start: 1, end: 2, active: false },
                DisplayRow::Item(3),
                DisplayRow::Item(4),
                DisplayRow::Item(5),
            ]
        );

        // Handoff divider stays outside work groups too
        let handoff = Item::Handoff { key: "h1".into(), from_model: "claude/fable".into(), from_actual: Some("claude-fable-5-2".into()), to_model: "agy/gemini-4-neon".into() };
        let with_handoff = vec![user("u1"), text("a1", false), handoff, user("u2"), text("a2", false)];
        assert_eq!(
            build_display_rows(&with_handoff, false, true),
            vec![DisplayRow::Item(0), DisplayRow::Item(1), DisplayRow::Item(2), DisplayRow::Item(3), DisplayRow::Item(4)]
        );

        // Restored session without timestamps yields None duration
        let restored = vec![tool("t1", Some(true), 0, Some(0))];
        assert_eq!(group_duration_ms(&restored, 0, 0), None);

        // Compact disabled preserves flat rows
        assert_eq!(build_display_rows(&with_notice, false, false).len(), 6);

        // Active compact row uses short subtle descriptions rather than full tool cards
        use super::describe_step;
        assert_eq!(describe_step(None), "Working…");
        assert_eq!(describe_step(Some(&reason("r1", 1000, None))), "Thinking…");
        assert_eq!(describe_step(Some(&reason("r1", 1000, Some(2000)))), "Working…");
        assert_eq!(
            describe_step(Some(&Item::Tool { key: "t1".into(), name: "Read".into(), title: "src/ui/code_view.rs".into(), input: Value::Null, output: String::new(), ok: None, diff: None, started: 1, ended: None })),
            "Reading code_view.rs"
        );
        assert_eq!(
            describe_step(Some(&Item::Tool { key: "t2".into(), name: "Bash".into(), title: "cargo test".into(), input: serde_json::json!({ "description": "Run unit tests" }), output: String::new(), ok: None, diff: None, started: 1, ended: None })),
            "Run unit tests"
        );
        assert_eq!(
            describe_step(Some(&Item::Tool { key: "t3".into(), name: "WebFetch".into(), title: "https://docs.rs/gpui".into(), input: Value::Null, output: String::new(), ok: None, diff: None, started: 1, ended: None })),
            "Fetching docs.rs"
        );
        assert_eq!(
            describe_step(Some(&Item::Tool { key: "t4".into(), name: "Bash".into(), title: "ls".into(), input: serde_json::json!({ "description": "list project files" }), output: String::new(), ok: None, diff: None, started: 1, ended: None })),
            "List project files"
        );
        assert_eq!(
            describe_step(Some(&Item::Tool { key: "t5".into(), name: "winfer · bg".into(), title: "start web-os static server".into(), input: Value::Null, output: String::new(), ok: None, diff: None, started: 1, ended: None })),
            "Start web-os static server"
        );
        // Streaming tool with empty title yet, lowercase backend names, and visualize / todos.
        assert_eq!(
            describe_step(Some(&Item::Tool { key: "t6".into(), name: "Write".into(), title: "".into(), input: Value::Null, output: String::new(), ok: None, diff: None, started: 1, ended: None })),
            "Writing file"
        );
        assert_eq!(
            describe_step(Some(&Item::Tool { key: "t7".into(), name: "write".into(), title: "src/app.rs".into(), input: Value::Null, output: String::new(), ok: None, diff: None, started: 1, ended: None })),
            "Writing app.rs"
        );
        assert_eq!(
            describe_step(Some(&Item::Tool { key: "t8".into(), name: "winfer · visualize".into(), title: "Perf Report".into(), input: Value::Null, output: String::new(), ok: None, diff: None, started: 1, ended: None })),
            "Visualizing Perf Report"
        );
        assert_eq!(
            describe_step(Some(&Item::Tool { key: "t9".into(), name: "Todos".into(), title: "update todos".into(), input: Value::Null, output: String::new(), ok: None, diff: None, started: 1, ended: None })),
            "Updating plan"
        );
    }

    #[::core::prelude::v1::test]
    fn inside_rounded_corners_clips_all_four_corners() {
        use super::inside_rounded_corners;
        let (w, h, r) = (400.0, 300.0, 16.0);
        // Center and straight edges are inside
        assert!(inside_rounded_corners(200.0, 150.0, w, h, r, 1.0));
        assert!(inside_rounded_corners(200.0, 1.0, w, h, r, 1.0));
        assert!(inside_rounded_corners(200.0, h - 1.0, w, h, r, 1.0));
        assert!(inside_rounded_corners(1.0, 150.0, w, h, r, 1.0));
        assert!(inside_rounded_corners(w - 1.0, 150.0, w, h, r, 1.0));
        // Points right inside each corner arc are inside
        assert!(inside_rounded_corners(r, r, w, h, r, 1.0));
        assert!(inside_rounded_corners(w - r, r, w, h, r, 1.0));
        assert!(inside_rounded_corners(r, h - r, w, h, r, 1.0));
        assert!(inside_rounded_corners(w - r, h - r, w, h, r, 1.0));
        // Extreme corner pixels outside the 16px arc on all four corners are excluded
        assert!(!inside_rounded_corners(1.0, 1.0, w, h, r, 1.0));
        assert!(!inside_rounded_corners(w - 1.0, 1.0, w, h, r, 1.0));
        assert!(!inside_rounded_corners(1.0, h - 1.0, w, h, r, 1.0));
        assert!(!inside_rounded_corners(w - 1.0, h - 1.0, w, h, r, 1.0));
        // Bright star with 7px glint near a corner arc is also excluded
        assert!(!inside_rounded_corners(6.0, 6.0, w, h, r, 7.0));
        assert!(!inside_rounded_corners(w - 6.0, 6.0, w, h, r, 7.0));
        assert!(!inside_rounded_corners(6.0, h - 6.0, w, h, r, 7.0));
        assert!(!inside_rounded_corners(w - 6.0, h - 6.0, w, h, r, 7.0));
    }

    #[::core::prelude::v1::test]
    fn user_bubble_takes_full_width_only_for_wrapping_lists() {
        use super::list_wraps;
        let long = "- Please create the trip yourself with the linde dev cosmos db connection.\n- No historical PDFs\n1. If local.TripReportURL is empty, don't show a QR-Code";
        assert!(list_wraps(long, 13.0, 400.0), "a long list wraps");
        assert!(!list_wraps("- a\n- b", 13.0, 400.0), "a short list keeps its own width");
        assert!(!list_wraps(&"a long paragraph ".repeat(20), 13.0, 400.0), "paragraphs size themselves fine");
        assert!(list_wraps("12) numbered item that is quite long and keeps on going past the edge", 13.0, 400.0));
    }

    #[::core::prelude::v1::test]
    fn sky_image_bounds_insets_atlas_sub_tile_on_all_four_edges() {
        use super::sky_image_bounds;
        use crate::ui::nebula::{SKY_INNER_H, SKY_INNER_W, SKY_PAD};
        use gpui_kit::{Bounds, point, px, size};

        let tile_w = (SKY_INNER_W + SKY_PAD * 2) as f32;
        let tile_h = (SKY_INNER_H + SKY_PAD * 2) as f32;

        // Test tall/narrow, exact 16:10, and ultra-wide pane shapes
        for (bw, bh) in [(858.0, 700.0), (800.0, 500.0), (1400.0, 380.0), (260.0, 420.0)] {
            let bounds = Bounds::new(point(px(12.0), px(20.0)), size(px(bw), px(bh)));
            let image_bounds = sky_image_bounds(bounds);
            // visible_bounds must stay equal to the exact pane bounds so corner_radii clip the pane
            assert_eq!(bounds.intersect(&image_bounds), bounds);

            // Mirror GPUI's paint_image sub_tile math and confirm >= SKY_PAD texels margin on all 4 sides
            let x_off = ((bounds.origin.x - image_bounds.origin.x) / image_bounds.size.width * tile_w).round() as i32;
            let y_off = ((bounds.origin.y - image_bounds.origin.y) / image_bounds.size.height * tile_h).round() as i32;
            let sub_w = ((bounds.size.width / image_bounds.size.width) * tile_w).round() as i32;
            let sub_h = ((bounds.size.height / image_bounds.size.height) * tile_h).round() as i32;

            assert!(x_off >= SKY_PAD as i32, "left margin {x_off} for {bw}x{bh}");
            assert!(y_off >= SKY_PAD as i32, "top margin {y_off} for {bw}x{bh}");
            assert!(x_off + sub_w <= tile_w as i32 - SKY_PAD as i32, "right margin for {bw}x{bh}");
            assert!(y_off + sub_h <= tile_h as i32 - SKY_PAD as i32, "bottom margin for {bw}x{bh}");
        }
    }

    #[gpui_kit::test]
    fn space_sky_spans_full_pane_behind_header(cx: &mut gpui_kit::TestAppContext) {
        use gpui_kit::{AppContext, Bounds, Context, Entity, IntoElement, ParentElement, Pixels, Render, Styled, Window, canvas, div, px};
        use std::cell::RefCell;
        use std::rc::Rc;

        cx.update(gpui_kit::init);
        let sky_bounds: Rc<RefCell<Option<Bounds<Pixels>>>> = Rc::new(RefCell::new(None));

        struct ChildSkyView {
            sky_bounds: Rc<RefCell<Option<Bounds<Pixels>>>>,
        }

        impl Render for ChildSkyView {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                let sb = self.sky_bounds.clone();
                div()
                    .relative()
                    .size_full()
                    .rounded_2xl()
                    .overflow_hidden()
                    .child(canvas(move |b, _, _| { *sb.borrow_mut() = Some(b); }, |_, _, _, _| {}).absolute().size_full())
            }
        }

        struct PaneSkyHarness {
            sky: Entity<ChildSkyView>,
        }

        impl Render for PaneSkyHarness {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                div()
                    .w(px(600.0))
                    .h(px(400.0))
                    .relative()
                    .flex()
                    .flex_col()
                    .overflow_hidden()
                    .rounded_2xl()
                    .child(
                        div()
                            .absolute()
                            .inset_0()
                            .rounded_2xl()
                            .overflow_hidden()
                            .child(self.sky.clone()),
                    )
                    .child(div().flex_shrink_0().mx_2().mt_2().h(px(48.0)))
                    .child(div().flex_1().min_h_0())
            }
        }

        let sb = sky_bounds.clone();
        let (_, vcx) = cx.add_window_view(move |_, cx| {
            let sky = cx.new(|_| ChildSkyView { sky_bounds: sb });
            PaneSkyHarness { sky }
        });
        vcx.run_until_parked();
        vcx.update(|window, cx| {
            let _ = window.draw(cx);
        });

        let sky = sky_bounds.borrow().expect("sky bounds measured");
        assert_eq!(sky.origin.x, px(0.0));
        assert_eq!(sky.origin.y, px(0.0));
        assert_eq!(sky.size.width, px(600.0));
        assert_eq!(sky.size.height, px(400.0));
    }

    /// A task orchestrator's multiple-choice question (it waits in its approvals) shows in Home's
    /// drill-in chat as an option card above the composer, and picking + Answer answers it.
    #[gpui_kit::test]
    fn an_orchestrators_question_is_an_answerable_card_above_the_composer(cx: &mut gpui_kit::TestAppContext) {
        use super::CodeView;
        use crate::code::protocol::{Approval, Question, QuestionOption};
        use crate::code::session::CodeModel;
        use crate::model::AppState;
        use gpui_kit::{AppContext, Modifiers, px};

        cx.update(gpui_kit::init);
        let opt = |l: &str| QuestionOption { label: l.into(), description: String::new() };
        let (view, vcx) = cx.add_window_view(|window, cx| {
            let app = cx.new(|_| AppState::new());
            let model = cx.new(|_| {
                let mut m = CodeModel::for_test(app.clone());
                m.pane_id = crate::voice::task_pane("t2");
                m.state.approvals = vec![Approval { id: "oq".into(), questions: vec![Question { header: "Export scope".into(), question: "What should Export include?".into(), options: vec![opt("Current filters"), opt("Whole report")], multi: false }], ..Default::default() }];
                m
            });
            CodeView::new(model, app, window, cx)
        });
        for _ in 0..2 {
            vcx.run_until_parked();
            vcx.update(|window, cx| {
                let _ = window.draw(cx);
            });
        }
        let card = vcx.debug_bounds("cw-approval").expect("the question card is drawn");
        let composer = vcx.debug_bounds("cw-composer-card").expect("composer drawn");
        // Docked on the composer (the dock tucks it a few px into the composer's top edge).
        assert!(card.top() < composer.top() && card.bottom() <= composer.top() + px(4.0), "the card sits above the composer: {card:?} vs {composer:?}");
        let option = vcx.debug_bounds("cw-q-0-1").expect("its options are drawn");
        vcx.simulate_click(option.center(), Modifiers::default());
        vcx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        assert_eq!(vcx.read(|cx| view.read(cx).picks.get("oq").cloned()), Some(vec![vec![1]]));
        let answer = vcx.debug_bounds("cw-answer").expect("Answer button drawn");
        vcx.simulate_click(answer.center(), Modifiers::default());
        vcx.run_until_parked();
        assert!(vcx.read(|cx| view.read(cx).model.read(cx).state.approvals.is_empty()), "answering clears the question");
    }

    #[gpui_kit::test]
    fn popup_arrow_keys_and_scroll_work_for_slash_skills_and_mentions(cx: &mut gpui_kit::TestAppContext) {
        use super::CodeView;
        use crate::code::protocol::COMMANDS;
        use crate::code::session::CodeModel;
        use crate::model::AppState;
        use gpui_kit::{AppContext, px};

        cx.update(gpui_kit::init);
        let (view, vcx) = cx.add_window_view(|window, cx| {
            let app = cx.new(|_| AppState::new());
            let model = cx.new(|_| {
                let mut m = CodeModel::for_test(app.clone());
                m.state.skills = vec!["deploy".into(), "zebra-skill".into()];
                m.state.files = Some((0..20).map(|i| format!("src/file_{i:02}.rs")).collect());
                m
            });
            CodeView::new(model, app, window, cx)
        });
        vcx.run_until_parked();

        // 1. `/` popup: arrow keys navigate commands + skills, scroll the popup, and Tab inserts the selection.
        vcx.update(|window, cx| {
            view.update(cx, |v, cx| {
                v.focus_composer(window, cx);
                v.set_input("/", window, cx);
            });
            let _ = window.draw(cx);
        });
        vcx.run_until_parked();

        // Total rows = COMMANDS.len() + 2 skills; pressing Up wraps to the last item ("/zebra-skill") and scrolls down.
        vcx.simulate_keystrokes("up");
        vcx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        vcx.run_until_parked();

        let (popup_ix, scroll_y) = vcx.read(|cx| {
            let v = view.read(cx);
            (v.popup, v.popup_scroll.offset().y)
        });
        assert_eq!(popup_ix, COMMANDS.len() + 1, "Up from 0 wraps to the last skill");
        assert!(scroll_y < px(0.0), "popup scroll should move to reveal the bottom skill, got {scroll_y:?}");

        vcx.simulate_keystrokes("tab");
        vcx.run_until_parked();
        assert_eq!(vcx.read(|cx| view.read(cx).draft(cx)), "/zebra-skill ");

        // 2. `@` popup: shows >8 files, arrow keys move past 8 and scroll, Enter picks the selected file.
        vcx.update(|window, cx| {
            view.update(cx, |v, cx| {
                v.focus_composer(window, cx);
                v.set_input("look at @", window, cx);
            });
            let _ = window.draw(cx);
        });
        vcx.run_until_parked();

        for _ in 0..12 {
            vcx.simulate_keystrokes("down");
        }
        vcx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        vcx.run_until_parked();

        let (mention_ix, mention_scroll_y) = vcx.read(|cx| {
            let v = view.read(cx);
            (v.popup, v.popup_scroll.offset().y)
        });
        assert_eq!(mention_ix, 12, "Down 12 times selects the 13th file");
        assert!(mention_scroll_y < px(0.0), "mention popup should scroll past the first 8 items, got {mention_scroll_y:?}");

        vcx.simulate_keystrokes("enter");
        vcx.run_until_parked();
        assert_eq!(vcx.read(|cx| view.read(cx).draft(cx)), "look at @src/file_12.rs ");

        // 3. Escape dismisses an open `@` popup without clearing the draft.
        vcx.update(|window, cx| {
            view.update(cx, |v, cx| {
                v.focus_composer(window, cx);
                v.set_input("check @src", window, cx);
            });
            let _ = window.draw(cx);
        });
        vcx.run_until_parked();
        assert!(!vcx.read(|cx| view.read(cx).popup_rows("check @src", cx).is_empty()));

        vcx.simulate_keystrokes("escape");
        vcx.run_until_parked();
        assert_eq!(vcx.read(|cx| view.read(cx).draft(cx)), "check @src");
        assert!(vcx.read(|cx| view.read(cx).popup_rows("check @src", cx).is_empty()));
    }

    #[gpui_kit::test]
    fn expanded_work_group_caps_at_40_percent_height_and_scrolls_inside(cx: &mut gpui_kit::TestAppContext) {
        use super::{CodeView, scroll_edge_fade_strengths, wheel_passes_work_group, work_group_max_h};
        use crate::code::session::CodeModel;
        use crate::code::state::Item;
        use crate::model::AppState;
        use gpui_kit::{AppContext, ScrollDelta, ScrollEvent, point, px};
        use serde_json::Value;

        assert_eq!(work_group_max_h(0.0), 240.0);
        assert_eq!(work_group_max_h(1000.0), 400.0);
        assert_eq!(work_group_max_h(750.0), 300.0);
        assert_eq!(work_group_max_h(100.0), 80.0);

        // Edge fade strength math: no overflow -> (0, 0), top -> (0, 1), mid -> (1, 1), bottom -> (1, 0), partial ramp.
        assert_eq!(scroll_edge_fade_strengths(0.0, 0.0), (0.0, 0.0));
        assert_eq!(scroll_edge_fade_strengths(0.0, 300.0), (0.0, 1.0));
        assert_eq!(scroll_edge_fade_strengths(-16.0, 300.0), (0.5, 1.0));
        assert_eq!(scroll_edge_fade_strengths(-100.0, 300.0), (1.0, 1.0));
        assert_eq!(scroll_edge_fade_strengths(-284.0, 300.0), (1.0, 0.5));
        assert_eq!(scroll_edge_fade_strengths(-300.0, 300.0), (1.0, 0.0));

        cx.update(gpui_kit::init);
        let (view, vcx) = cx.add_window_view(|window, cx| {
            let app = cx.new(|_| AppState::new());
            let model = cx.new(|_| {
                let mut m = CodeModel::for_test(app.clone());
                let mut items = vec![Item::User { key: "u0".into(), text: "earlier turn".into(), at: 1, images: vec![] }];
                // Filler turns so the outer transcript list itself is scrollable.
                for i in 0..10 {
                    items.push(Item::Text {
                        key: format!("fill-{i}"),
                        text: format!("Filler paragraph {i}\n\nLine two of filler paragraph {i}."),
                        streaming: false,
                    });
                }
                items.push(Item::User { key: "u1".into(), text: "do 22 tools".into(), at: 2, images: vec![] });
                for i in 0..22 {
                    items.push(Item::Tool {
                        key: format!("t{i}"),
                        name: "Read".into(),
                        title: format!("src/file_{i}.rs"),
                        input: Value::Null,
                        output: String::new(),
                        ok: Some(true),
                        diff: None,
                        started: 1000 + i as u64 * 1000,
                        ended: Some(2000 + i as u64 * 1000),
                    });
                }
                items.push(Item::Text { key: "a1".into(), text: "Done with 22 tools.".into(), streaming: false });
                // Short 2-tool group to verify it doesn't pad out to 40%.
                items.push(Item::User { key: "u2".into(), text: "do 2 tools".into(), at: 3, images: vec![] });
                for i in 0..2 {
                    items.push(Item::Tool {
                        key: format!("s{i}"),
                        name: "Read".into(),
                        title: format!("src/short_{i}.rs"),
                        input: Value::Null,
                        output: String::new(),
                        ok: Some(true),
                        diff: None,
                        started: 30_000 + i as u64 * 1000,
                        ended: Some(31_000 + i as u64 * 1000),
                    });
                }
                m.state.items = items;
                m
            });
            CodeView::new(model, app, window, cx)
        });

        let draw = |vcx: &mut gpui_kit::VisualTestContext| {
            vcx.run_until_parked();
            vcx.update(|window, cx| {
                let _ = window.draw(cx);
            });
        };

        draw(vcx);
        draw(vcx);

        let pane_h = vcx.read(|cx| view.read(cx).pane_size.height / px(1.0));
        let expected_cap = work_group_max_h(pane_h);
        assert!(pane_h > 100.0, "pane should be measured, got {pane_h}");

        // Open both the 22-tool group ("wg-t0") and the 2-tool group ("wg-s0").
        vcx.update(|_, cx| {
            view.update(cx, |v, cx| {
                v.flipped.insert("wg-t0".into());
                v.flipped.insert("wg-s0".into());
                cx.notify();
            });
        });
        draw(vcx);
        draw(vcx);

        // 1. The 2-tool group stays at its natural content height (< 40% cap) and shows neither edge fade.
        let short_bounds = vcx.debug_bounds("wg-body-wg-s0").expect("short work group body rendered");
        assert!(
            short_bounds.size.height < px(expected_cap),
            "short 2-tool group should stay shorter than 40% cap ({expected_cap}), got {:?}",
            short_bounds.size.height
        );
        let fade_for = |vcx: &mut gpui_kit::VisualTestContext, key: &str| {
            vcx.read(|cx| {
                let h = view.read(cx).wg_scrolls.borrow().get(key).expect("scroll handle").clone();
                scroll_edge_fade_strengths(h.offset().y / px(1.0), h.max_offset().y / px(1.0))
            })
        };
        assert_eq!(fade_for(vcx, "wg-s0"), (0.0, 0.0), "non-overflowing group must show no fades");

        // 2. The 22-tool group is capped at ~40% of the pane height (+4px pt_1), NOT ~880px.
        // Initially at the top, only the bottom edge fades.
        let long_bounds = vcx.debug_bounds("wg-body-wg-t0").expect("22-tool work group body rendered");
        assert!(
            (long_bounds.size.height - px(expected_cap + 4.0)).abs() <= px(2.0),
            "22-tool group body should cap at 40% pane height ({expected_cap} + 4px pt_1), got {:?}",
            long_bounds.size.height
        );
        assert_eq!(fade_for(vcx, "wg-t0"), (0.0, 1.0), "initially at top: only bottom edge should fade");

        // 3. Scrolling inside the 22-tool group moves its tool rows without scrolling the outer transcript list,
        // and brings in the top edge fade while keeping the bottom fade.
        let list_offset_before = vcx.read(|cx| view.read(cx).list.scroll_px_offset_for_scrollbar());
        let first_top_before = vcx.debug_bounds("t-t0").expect("first tool rendered").top();

        vcx.simulate_event(ScrollEvent {
            position: long_bounds.center(),
            delta: ScrollDelta::Pixels(point(px(0.0), px(-120.0))),
            ..Default::default()
        });
        draw(vcx);

        let first_top_after = vcx.debug_bounds("t-t0").expect("first tool still laid out").top();
        let list_offset_after = vcx.read(|cx| view.read(cx).list.scroll_px_offset_for_scrollbar());
        assert!(
            first_top_after < first_top_before - px(50.0),
            "inner tool rows should scroll up inside the work group: before={first_top_before:?}, after={first_top_after:?}"
        );
        assert_eq!(
            list_offset_after, list_offset_before,
            "outer transcript list must not scroll while scrolling inside the work group"
        );
        assert_eq!(fade_for(vcx, "wg-t0"), (1.0, 1.0), "mid-scroll: both top and bottom edges should fade");

        // 4. Scrolling all the way to the bottom clears the bottom edge fade while keeping the top fade.
        vcx.simulate_event(ScrollEvent {
            position: long_bounds.center(),
            delta: ScrollDelta::Pixels(point(px(0.0), px(-2000.0))),
            ..Default::default()
        });
        draw(vcx);
        assert_eq!(fade_for(vcx, "wg-t0"), (1.0, 0.0), "at bottom: only top edge should fade");

        // 5. At the group's ends the wheel scrolls the transcript instead of staying with the group.
        assert!(wheel_passes_work_group(0.0, 300.0, 10.0) && wheel_passes_work_group(-300.0, 300.0, -10.0));
        assert!(!wheel_passes_work_group(0.0, 300.0, -10.0) && !wheel_passes_work_group(-300.0, 300.0, 10.0));
        assert!(!wheel_passes_work_group(-150.0, 300.0, 10.0) && !wheel_passes_work_group(0.0, 0.0, 10.0));
        let list_y = |vcx: &mut gpui_kit::VisualTestContext| vcx.read(|cx| view.read(cx).list.scroll_px_offset_for_scrollbar().y);
        let group_y = |vcx: &mut gpui_kit::VisualTestContext| vcx.read(|cx| view.read(cx).wg_scrolls.borrow()["wg-t0"].offset().y);
        // Move the transcript up from its end, then push past the group's bottom.
        let at = |vcx: &mut gpui_kit::VisualTestContext| vcx.debug_bounds("wg-body-wg-t0").expect("group body").center();
        vcx.update(|_, cx| view.update(cx, |v, cx| {
            let y = v.list.scroll_px_offset_for_scrollbar().y + px(150.0);
            v.list.set_offset_from_scrollbar(point(px(0.0), y));
            cx.notify();
        }));
        draw(vcx);
        let before = (list_y(vcx), group_y(vcx));
        let pos = at(vcx);
        vcx.simulate_event(ScrollEvent { position: pos, delta: ScrollDelta::Pixels(point(px(0.0), px(-60.0))), ..Default::default() });
        draw(vcx);
        let (list_after, group_after) = (list_y(vcx), group_y(vcx));
        assert_eq!(group_after, before.1, "group already at its end must not move");
        assert!(list_after < before.0, "transcript should scroll down past the group's end: {:?} -> {list_after:?}", before.0);
        // Back to the group's top, then one more upward packet scrolls the transcript up.
        let pos = at(vcx);
        vcx.simulate_event(ScrollEvent { position: pos, delta: ScrollDelta::Pixels(point(px(0.0), px(5000.0))), ..Default::default() });
        draw(vcx);
        assert_eq!(group_y(vcx), px(0.0), "group scrolled back to its top");
        let top_before = list_y(vcx);
        let pos = at(vcx);
        vcx.simulate_event(ScrollEvent { position: pos, delta: ScrollDelta::Pixels(point(px(0.0), px(60.0))), ..Default::default() });
        draw(vcx);
        assert!(list_y(vcx) > top_before, "transcript should scroll up past the group's start: {top_before:?} -> {:?}", list_y(vcx));
    }

    #[gpui_kit::test]
    fn composer_attachments_grow_composer_and_shift_transcript_without_overlap(cx: &mut gpui_kit::TestAppContext) {
        use super::CodeView;
        use crate::code::session::CodeModel;
        use crate::code::state::{Attachment, Item};
        use crate::model::AppState;
        use base64::Engine;
        use gpui_kit::{AppContext, px};

        let mut png_bytes = Vec::new();
        image::DynamicImage::new_rgba8(4, 4)
            .write_to(&mut std::io::Cursor::new(&mut png_bytes), image::ImageFormat::Png)
            .unwrap();
        let png_b64 = base64::engine::general_purpose::STANDARD.encode(&png_bytes);

        cx.update(gpui_kit::init);
        let (view, vcx) = cx.add_window_view(|window, cx| {
            let app = cx.new(|_| AppState::new());
            let model = cx.new(|_| {
                let mut m = CodeModel::for_test(app.clone());
                let mut items = vec![Item::User { key: "u0".into(), text: "earlier prompt".into(), at: 1, images: vec![] }];
                for i in 0..14 {
                    items.push(Item::Text {
                        key: format!("fill-{i}"),
                        text: format!("Filler message {i}\n\nSecond paragraph of filler message {i}."),
                        streaming: false,
                    });
                }
                items.push(Item::Text {
                    key: "last".into(),
                    text: "Final assistant message right above the composer.".into(),
                    streaming: false,
                });
                m.state.items = items;
                m
            });
            CodeView::new(model, app, window, cx)
        });

        let draw = |vcx: &mut gpui_kit::VisualTestContext| {
            vcx.run_until_parked();
            vcx.update(|window, cx| {
                let _ = window.draw(cx);
            });
        };

        draw(vcx);
        draw(vcx);

        let card_0 = vcx.debug_bounds("cw-composer-card").expect("composer card rendered");
        let tr_0 = vcx.debug_bounds("cw-transcript").expect("transcript rendered");
        let last_0 = vcx.debug_bounds("cw-row-15").expect("last transcript row rendered");
        assert!(tr_0.bottom() <= card_0.top(), "initial transcript bottom should sit above composer");
        assert!(last_0.bottom() <= card_0.top(), "initial last row should sit above composer");

        // 1. Attach one image: composer grows in-flow, transcript shrinks, last row shifts up without overlap.
        let one = png_b64.clone();
        vcx.update(|_, cx| {
            view.update(cx, |v, cx| {
                v.model.update(cx, |m, _| {
                    m.attachments.push(Attachment { data: one, mime: "image/png".into(), name: "shot-0.png".into(), path: None });
                });
                cx.notify();
            });
        });
        draw(vcx);
        draw(vcx);

        let card_1 = vcx.debug_bounds("cw-composer-card").expect("composer card rendered with 1 attachment");
        let att_1 = vcx.debug_bounds("cw-attachments").expect("attachments container rendered");
        let tr_1 = vcx.debug_bounds("cw-transcript").expect("transcript rendered with 1 attachment");
        let last_1 = vcx.debug_bounds("cw-row-15").expect("last row rendered with 1 attachment");

        let delta_1 = card_1.size.height - card_0.size.height;
        assert!(delta_1 >= px(48.0), "composer should grow by at least 48px for image thumbnail, got {delta_1:?}");
        assert!(
            att_1.top() >= card_1.top() && att_1.bottom() <= card_1.bottom(),
            "attachments must be contained inside composer card: att={att_1:?}, card={card_1:?}"
        );
        let tr_shrink_1 = tr_0.size.height - tr_1.size.height;
        assert!(
            (tr_shrink_1 - delta_1).abs() <= px(1.0),
            "transcript should shrink by composer growth: delta={delta_1:?}, shrink={tr_shrink_1:?}"
        );
        assert!(tr_1.bottom() <= card_1.top(), "transcript must not overlap composer: tr={tr_1:?}, card={card_1:?}");
        assert!(
            last_1.bottom() < last_0.bottom() - px(40.0),
            "last row should shift upward: before={last_0:?}, after={last_1:?}"
        );
        assert!(last_1.bottom() <= card_1.top(), "last row must stay above composer: last={last_1:?}, card={card_1:?}");

        // 2. Attach enough images to wrap onto multiple rows: composer grows again, transcript shrinks with no overlap.
        let total_to_wrap = (f32::from(card_1.size.width) / 48.0).ceil() as usize + 4;
        let more = png_b64.clone();
        vcx.update(|_, cx| {
            view.update(cx, |v, cx| {
                v.model.update(cx, |m, _| {
                    for i in 1..total_to_wrap {
                        m.attachments.push(Attachment { data: more.clone(), mime: "image/png".into(), name: format!("shot-{i}.png"), path: None });
                    }
                });
                cx.notify();
            });
        });
        draw(vcx);
        draw(vcx);

        let card_2 = vcx.debug_bounds("cw-composer-card").expect("composer card rendered with wrapped attachments");
        let att_2 = vcx.debug_bounds("cw-attachments").expect("wrapped attachments container rendered");
        let tr_2 = vcx.debug_bounds("cw-transcript").expect("transcript rendered with wrapped attachments");
        let last_2 = vcx.debug_bounds("cw-row-15").expect("last row rendered with wrapped attachments");

        let delta_2 = card_2.size.height - card_1.size.height;
        assert!(delta_2 >= px(48.0), "wrapping onto another row should grow composer further, got {delta_2:?}");
        assert!(
            att_2.top() >= card_2.top() && att_2.bottom() <= card_2.bottom(),
            "wrapped attachments must stay inside composer card: att={att_2:?}, card={card_2:?}"
        );
        let tr_shrink_2 = tr_1.size.height - tr_2.size.height;
        assert!(
            (tr_shrink_2 - delta_2).abs() <= px(1.0),
            "transcript should shrink by wrapped composer growth: delta={delta_2:?}, shrink={tr_shrink_2:?}"
        );
        assert!(tr_2.bottom() <= card_2.top(), "transcript must not overlap wrapped composer: tr={tr_2:?}, card={card_2:?}");
        assert!(
            last_2.bottom() < last_1.bottom() - px(40.0),
            "last row should shift upward further on wrapped attachments: before={last_1:?}, after={last_2:?}"
        );
        assert!(last_2.bottom() <= card_2.top(), "last row must stay above wrapped composer: last={last_2:?}, card={card_2:?}");

        // 3. Clearing attachments restores composer and transcript heights.
        vcx.update(|_, cx| {
            view.update(cx, |v, cx| {
                v.model.update(cx, |m, _| m.attachments.clear());
                cx.notify();
            });
        });
        draw(vcx);
        draw(vcx);

        let card_3 = vcx.debug_bounds("cw-composer-card").expect("composer card rendered after clear");
        let tr_3 = vcx.debug_bounds("cw-transcript").expect("transcript rendered after clear");
        assert!(
            (card_3.size.height - card_0.size.height).abs() <= px(1.0),
            "composer height should restore after clearing attachments: before={card_0:?}, after={card_3:?}"
        );
        assert!(
            (tr_3.size.height - tr_0.size.height).abs() <= px(1.0),
            "transcript height should restore after clearing attachments: before={tr_0:?}, after={tr_3:?}"
        );
        assert!(vcx.debug_bounds("cw-attachments").is_none());
    }

    #[gpui_kit::test]
    fn ultracode_aurora_animates_distinct_frames_inside_parent_view(cx: &mut gpui_kit::TestAppContext) {
        use super::CodeView;
        use crate::code::session::CodeModel;
        use crate::code::state::Sheet;
        use crate::model::AppState;
        use crate::ui::nebula;
        use gpui_kit::{AppContext, Context, Entity, IntoElement, ParentElement, Render, Styled, Window, div};
        use std::collections::HashSet;
        use std::time::{Duration, Instant};

        struct Host {
            child: Entity<CodeView>,
        }
        impl Render for Host {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                div().size_full().child(self.child.clone())
            }
        }

        cx.update(gpui_kit::init);
        nebula::warm_ultracode(false);
        let deadline = Instant::now() + Duration::from_secs(5);
        while nebula::ultracode_sequence(false).frame_count() < nebula::ULTRA_FRAMES {
            assert!(Instant::now() < deadline, "ultracode sequence bake timed out");
            std::thread::sleep(Duration::from_millis(5));
        }

        let (host, vcx) = cx.add_window_view(|window, cx| {
            let app = cx.new(|_| AppState::new());
            let model = cx.new(|_| {
                let mut m = CodeModel::for_test(app.clone());
                m.state.model = "claude/sonnet".into();
                m.state.ultracode = false;
                m
            });
            let child = cx.new(|cx| CodeView::new(model, app, window, cx));
            Host { child }
        });
        vcx.run_until_parked();

        let view = vcx.read(|cx| host.read(cx).child.clone());
        vcx.update(|window, cx| {
            view.update(cx, |v, cx| {
                v.set_sheet(Some(Sheet::Effort), cx);
                v.toggle_ultracode(cx);
            });
            let _ = window.draw(cx);
        });
        vcx.run_until_parked();

        let _ = super::super::code_pickers::take_ultra_paint_log();
        let mut composer_frames = HashSet::new();
        let mut card_frames = HashSet::new();
        for _ in 0..12 {
            vcx.background_executor.advance_clock(Duration::from_millis(50));
            vcx.run_until_parked();
            vcx.update(|window, cx| {
                let _ = window.draw(cx);
            });
            for (id, frame_idx, bounds) in super::super::code_pickers::take_ultra_paint_log() {
                assert!(
                    f32::from(bounds.size.width) > 20.0 && f32::from(bounds.size.height) > 10.0,
                    "aurora bounds for {id} must be non-zero, got {bounds:?}"
                );
                if id.as_ref() == "ultra-composer-aurora" {
                    composer_frames.insert(frame_idx);
                } else if id.as_ref() == "ultra-card-aurora-effort" {
                    card_frames.insert(frame_idx);
                }
            }
        }
        assert!(
            composer_frames.len() >= 8,
            "ultracode composer aurora must advance through many distinct frames (not stuck on 1-2), saw {:?}",
            composer_frames
        );
        assert!(
            card_frames.len() >= 8,
            "ultracode card aurora must advance through many distinct frames (not stuck on 1-2), saw {:?}",
            card_frames
        );

        // Wait past the 420ms elastic knob animation and verify the settled handle is evenly centered
        // vertically and horizontally inside the switch track.
        vcx.background_executor.advance_clock(Duration::from_millis(600));
        vcx.run_until_parked();
        vcx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        let sw = vcx.debug_bounds("ultra-switch-effort").expect("ultracode switch rendered");
        let kn = vcx.debug_bounds("ultra-knob-effort").expect("ultracode knob rendered");
        let top_gap = f32::from(kn.top() - sw.top());
        let bot_gap = f32::from(sw.bottom() - kn.bottom());
        let right_gap = f32::from(sw.right() - kn.right());
        assert!(
            (top_gap - bot_gap).abs() <= 0.5,
            "switch knob must be vertically centered: top_gap={top_gap}, bot_gap={bot_gap} (sw={sw:?}, kn={kn:?})"
        );
        assert!(
            (right_gap - top_gap).abs() <= 0.5,
            "switch knob right margin must match vertical margin: right_gap={right_gap}, top_gap={top_gap} (sw={sw:?}, kn={kn:?})"
        );
    }

    #[gpui_kit::test]
    fn clicking_a_file_link_in_the_transcript_emits_open_link(cx: &mut gpui_kit::TestAppContext) {
        use super::{CodeView, OpenLink};
        use crate::code::session::CodeModel;
        use crate::code::state::Item;
        use crate::model::AppState;
        use gpui_kit::{AppContext, Modifiers, point, px};
        use std::{cell::RefCell, rc::Rc};

        let dir = std::env::temp_dir().join(format!("winfer-linkclick-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src/a.rs"), "fn a() {}").unwrap();
        let cwd = dir.to_str().unwrap().to_string();

        cx.update(gpui_kit::init);
        let (view, vcx) = cx.add_window_view(|window, cx| {
            let app = cx.new(|_| AppState::new());
            let model = cx.new(|_| {
                let mut m = CodeModel::for_test(app.clone());
                m.state.cwd = Some(cwd.clone());
                m.state.items = vec![Item::Text { key: "a0".into(), text: "`src/a.rs:3` see".into(), streaming: false }];
                m
            });
            CodeView::new(model, app, window, cx)
        });
        let got = Rc::new(RefCell::new(Vec::<String>::new()));
        let sink = got.clone();
        vcx.update(|_, cx| {
            cx.subscribe(&view, move |_, OpenLink(h), _| sink.borrow_mut().push(h.clone())).detach();
        });
        for _ in 0..3 {
            vcx.run_until_parked();
            vcx.update(|window, cx| {
                let _ = window.draw(cx);
            });
        }
        let size = vcx.update(|window, _| window.viewport_size());
        let mut hit = None;
        let mut y = 4.0;
        while y < size.height / px(1.0) && hit.is_none() {
            let mut x = 8.0;
            while x < 400.0 && hit.is_none() {
                vcx.simulate_click(point(px(x), px(y)), Modifiers::default());
                if !got.borrow().is_empty() {
                    hit = Some(point(px(x), px(y)));
                }
                x += 6.0;
            }
            y += 6.0;
        }
        let hit = hit.expect("no OpenLink emitted by any click");
        // The way a real click goes: the view re-renders between the press and the release
        // (pressing focuses the pane, which refocuses the composer).
        got.borrow_mut().clear();
        vcx.simulate_mouse_down(hit, gpui_kit::MouseButton::Left, Modifiers::default());
        vcx.update(|_, cx| view.update(cx, |_, cx| cx.notify()));
        for _ in 0..2 {
            vcx.run_until_parked();
            vcx.update(|window, cx| {
                let _ = window.draw(cx);
            });
        }
        vcx.simulate_mouse_up(hit, gpui_kit::MouseButton::Left, Modifiers::default());
        assert!(!got.borrow().is_empty(), "a re-render between press and release swallowed the click");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[gpui_kit::test]
    fn modal_sheet_blocks_scroll_zoom_and_composer_actions_behind_it(cx: &mut gpui_kit::TestAppContext) {
        use super::CodeView;
        use crate::code::session::CodeModel;
        use crate::code::state::{Item, Sheet};
        use crate::model::AppState;
        use gpui_kit::{AppContext, Focusable, Modifiers, ScrollDelta, ScrollEvent, point, px};

        cx.update(gpui_kit::init);
        let (view, vcx) = cx.add_window_view(|window, cx| {
            let app = cx.new(|_| AppState::new());
            let model = cx.new(|_| {
                let mut m = CodeModel::for_test(app.clone());
                for i in 0..24 {
                    m.state.items.push(Item::User { key: format!("u{i}"), text: format!("User message {i}"), images: Vec::new(), at: 0 });
                    m.state.items.push(Item::Text {
                        key: format!("a{i}"),
                        text: format!("Assistant reply {i}\n\nMore detail on line two for message {i}."),
                        streaming: false,
                    });
                }
                let long_log = (0..120).map(|i| format!("log line {i}: running background shell step")).collect::<Vec<_>>().join("\n");
                m.state.log_task = Some("bg-1".into());
                m.state.task_log = Some(("bg-1".into(), Some(long_log), None));
                m
            });
            CodeView::new(model, app, window, cx)
        });
        let draw = |vcx: &mut gpui_kit::VisualTestContext| {
            for _ in 0..2 {
                vcx.run_until_parked();
                vcx.update(|window, cx| {
                    let _ = window.draw(cx);
                });
            }
        };
        draw(vcx);

        let list_y = |vcx: &mut gpui_kit::VisualTestContext| vcx.read(|cx| view.read(cx).list.scroll_px_offset_for_scrollbar().y);
        let size = vcx.update(|window, _| window.viewport_size());
        let center = point(size.width * 0.5, size.height * 0.5);

        // Sanity check: before opening the modal, scrolling the chat pane moves the transcript.
        vcx.update(|window, cx| view.update(cx, |v, cx| v.focus_composer(window, cx)));
        draw(vcx);
        assert!(vcx.update(|window, cx| view.read(cx).input.read(cx).focus_handle(cx).is_focused(window)), "composer starts focused");
        let y_start = list_y(vcx);
        vcx.simulate_event(ScrollEvent { position: center, delta: ScrollDelta::Pixels(point(px(0.0), px(200.0))), ..Default::default() });
        draw(vcx);
        let y_scrolled = list_y(vcx);
        assert_ne!(y_scrolled, y_start, "transcript should scroll when no modal is open");

        // Open the background task/shell log modal (`Sheet::TaskLog`).
        vcx.update(|_, cx| view.update(cx, |v, cx| v.set_sheet(Some(Sheet::TaskLog), cx)));
        draw(vcx);

        // Focus must move out of the composer while the blocking modal is open.
        assert!(
            !vcx.update(|window, cx| view.read(cx).input.read(cx).focus_handle(cx).is_focused(window)),
            "composer must not stay focused behind a blocking modal sheet"
        );

        // Scroll past both ends of the task log body AND over the scrim corner: transcript must not move.
        let y_modal_open = list_y(vcx);
        for dy in [-3000.0, -3000.0, 5000.0, 5000.0] {
            vcx.simulate_event(ScrollEvent { position: center, delta: ScrollDelta::Pixels(point(px(0.0), px(dy))), ..Default::default() });
            draw(vcx);
            assert_eq!(list_y(vcx), y_modal_open, "transcript behind TaskLog modal must not scroll (dy={dy})");
        }
        let scrim_corner = point(px(12.0), px(12.0));
        vcx.simulate_event(ScrollEvent { position: scrim_corner, delta: ScrollDelta::Pixels(point(px(0.0), px(400.0))), ..Default::default() });
        draw(vcx);
        assert_eq!(list_y(vcx), y_modal_open, "scrolling on the modal scrim must not scroll the transcript behind it");

        // Ctrl/⌘ + wheel over the open modal must not zoom the chat behind it.
        let scale_before = vcx.read(|cx| view.read(cx).prefs(cx).0);
        vcx.simulate_event(ScrollEvent {
            position: center,
            delta: ScrollDelta::Pixels(point(px(0.0), px(120.0))),
            modifiers: Modifiers { platform: true, control: true, ..Default::default() },
            ..Default::default()
        });
        draw(vcx);
        assert_eq!(vcx.read(|cx| view.read(cx).prefs(cx).0), scale_before, "zoom wheel must not pass through a modal");

        // Pressing Escape closes the modal and restores composer focus.
        vcx.simulate_keystrokes("escape");
        draw(vcx);
        assert_eq!(vcx.read(|cx| view.read(cx).model.read(cx).state.sheet), None, "Escape should close TaskLog sheet");
        assert!(
            vcx.update(|window, cx| view.read(cx).input.read(cx).focus_handle(cx).is_focused(window)),
            "closing the modal should restore focus to the composer"
        );
    }

    #[gpui_kit::test]
    fn task_log_modal_starts_at_bottom_follows_new_output_and_shows_jump_button(cx: &mut gpui_kit::TestAppContext) {
        use super::CodeView;
        use crate::code::protocol::Ev;
        use crate::code::session::CodeModel;
        use crate::code::state::Sheet;
        use crate::model::AppState;
        use gpui_kit::{AppContext, Modifiers, ScrollDelta, ScrollEvent, point, px};

        cx.update(gpui_kit::init);
        let (view, vcx) = cx.add_window_view(|window, cx| {
            let app = cx.new(|_| AppState::new());
            let model = cx.new(|_| CodeModel::for_test(app.clone()));
            CodeView::new(model, app, window, cx)
        });
        let draw = |vcx: &mut gpui_kit::VisualTestContext| {
            for _ in 0..2 {
                vcx.run_until_parked();
                vcx.update(|window, cx| {
                    let _ = window.draw(cx);
                });
            }
        };
        let scroll_state = |vcx: &mut gpui_kit::VisualTestContext| {
            vcx.read(|cx| {
                let h = &view.read(cx).task_log_scroll.handle;
                (h.offset().y / px(1.0), h.max_offset().y / px(1.0))
            })
        };
        let make_log = |n: usize| (0..n).map(|i| format!("log line {i}: running background shell step")).collect::<Vec<_>>().join("\n");

        // 1. Open while log is still loading, then deliver initial long output: must land at the bottom.
        vcx.update(|_, cx| {
            view.update(cx, |v, cx| {
                v.model.update(cx, |m, cx| {
                    m.state.open_task_log("bg-1");
                    cx.notify();
                });
            });
        });
        draw(vcx);
        vcx.update(|_, cx| {
            view.update(cx, |v, cx| {
                v.model.update(cx, |m, cx| {
                    m.state.apply(Ev::TaskLog { id: "bg-1".into(), text: Some(make_log(120)), file: None });
                    cx.notify();
                });
            });
        });
        draw(vcx);

        let (y0, max0) = scroll_state(vcx);
        assert!(max0 > 100.0, "expected overflowing task log, got max_offset={max0}");
        assert!((y0 + max0).abs() <= 1.0, "task log must open at bottom: offset_y={y0}, max_y={max0}");
        assert!(vcx.debug_bounds("cw-task-log-jump").is_none(), "jump button must stay hidden at bottom");

        // 2. While pinned to the bottom, new log lines auto-scroll to the new bottom.
        vcx.update(|_, cx| {
            view.update(cx, |v, cx| {
                v.model.update(cx, |m, cx| {
                    m.state.apply(Ev::TaskLog { id: "bg-1".into(), text: Some(make_log(160)), file: None });
                    cx.notify();
                });
            });
        });
        draw(vcx);

        let (y1, max1) = scroll_state(vcx);
        assert!(max1 > max0 + 50.0, "appending lines should increase scroll height: {max0} -> {max1}");
        assert!((y1 + max1).abs() <= 1.0, "new log lines must auto-scroll to bottom: offset_y={y1}, max_y={max1}");
        assert!(vcx.debug_bounds("cw-task-log-jump").is_none(), "jump button stays hidden while following");

        // 3. Scrolling upwards detaches from bottom and shows the centered bottom jump button.
        let body = vcx.debug_bounds("cw-task-log-body").expect("task log body rendered");
        let body_center = body.center();
        vcx.simulate_event(ScrollEvent { position: body_center, delta: ScrollDelta::Pixels(point(px(0.0), px(300.0))), ..Default::default() });
        draw(vcx);

        let (y_up, max_up) = scroll_state(vcx);
        assert!(y_up > -max_up + 50.0, "scrolling up should move away from bottom: offset_y={y_up}, max_y={max_up}");
        let jump = vcx.debug_bounds("cw-task-log-jump").expect("jump button must appear when scrolled up");
        let body_cx = f32::from(body.center().x);
        let jump_cx = f32::from(jump.center().x);
        assert!((body_cx - jump_cx).abs() <= 2.0, "jump button must be centered horizontally: body_cx={body_cx}, jump_cx={jump_cx}");
        assert!(jump.bottom() <= body.bottom() && jump.top() >= body.bottom() - px(60.0), "jump button must sit at bottom of body: body={body:?}, jump={jump:?}");

        // 4. While scrolled upwards, new log lines must NOT pull the scroll position back down.
        vcx.update(|_, cx| {
            view.update(cx, |v, cx| {
                v.model.update(cx, |m, cx| {
                    m.state.apply(Ev::TaskLog { id: "bg-1".into(), text: Some(make_log(200)), file: None });
                    cx.notify();
                });
            });
        });
        draw(vcx);

        let (y_after_append, max_after_append) = scroll_state(vcx);
        assert!(max_after_append > max_up + 50.0, "expected taller log after append: {max_up} -> {max_after_append}");
        assert!((y_after_append - y_up).abs() <= 1.0, "scroll position must stay put while scrolled up: before={y_up}, after={y_after_append}");
        assert!(vcx.debug_bounds("cw-task-log-jump").is_some(), "jump button must remain visible while scrolled up");

        // 5. Clicking the jump button snaps back to the bottom, hides the button, and resumes auto-scroll.
        let jump = vcx.debug_bounds("cw-task-log-jump").expect("jump button clickable");
        vcx.simulate_click(jump.center(), Modifiers::default());
        draw(vcx);

        let (y_snapped, max_snapped) = scroll_state(vcx);
        assert!((y_snapped + max_snapped).abs() <= 1.0, "clicking jump must scroll to bottom: offset_y={y_snapped}, max_y={max_snapped}");
        assert!(vcx.debug_bounds("cw-task-log-jump").is_none(), "jump button must hide after jumping to bottom");

        vcx.update(|_, cx| {
            view.update(cx, |v, cx| {
                v.model.update(cx, |m, cx| {
                    m.state.apply(Ev::TaskLog { id: "bg-1".into(), text: Some(make_log(230)), file: None });
                    cx.notify();
                });
            });
        });
        draw(vcx);
        let (y_resumed, max_resumed) = scroll_state(vcx);
        assert!(max_resumed > max_snapped + 30.0);
        assert!((y_resumed + max_resumed).abs() <= 1.0, "auto-scroll must resume after clicking jump: offset_y={y_resumed}, max_y={max_resumed}");

        // 6. Scroll up again, close and reopen the modal: reopening must start at the bottom again.
        vcx.simulate_event(ScrollEvent { position: body_center, delta: ScrollDelta::Pixels(point(px(0.0), px(400.0))), ..Default::default() });
        draw(vcx);
        assert!(vcx.debug_bounds("cw-task-log-jump").is_some());

        vcx.update(|_, cx| view.update(cx, |v, cx| v.set_sheet(None, cx)));
        draw(vcx);
        vcx.update(|_, cx| view.update(cx, |v, cx| v.set_sheet(Some(Sheet::TaskLog), cx)));
        draw(vcx);

        let (y_reopen, max_reopen) = scroll_state(vcx);
        assert!((y_reopen + max_reopen).abs() <= 1.0, "reopening task log must start at bottom: offset_y={y_reopen}, max_y={max_reopen}");
        assert!(vcx.debug_bounds("cw-task-log-jump").is_none(), "jump button must be hidden on reopen");
    }

    #[gpui_kit::test]
    fn home_composer_focus_leaves_the_focused_pane_alone(cx: &mut gpui_kit::TestAppContext) {
        use super::CodeView;
        use crate::code::session::CodeModel;
        use crate::model::AppState;
        use gpui_kit::AppContext;

        cx.update(gpui_kit::init);
        for (pane_id, expect) in [(crate::voice::CONDUCTOR, "agent-pane"), ("chat-pane", "chat-pane")] {
            let mut app_ref = None;
            let (view, vcx) = cx.add_window_view(|window, cx| {
                let app = cx.new(|_| {
                    let mut s = AppState::new();
                    s.focused_pane_id = Some("agent-pane".into());
                    s
                });
                app_ref = Some(app.clone());
                let model = cx.new(|_| {
                    let mut m = CodeModel::for_test(app.clone());
                    m.pane_id = pane_id.into();
                    m
                });
                CodeView::new(model, app, window, cx)
            });
            let app = app_ref.unwrap();
            // GPUI delivers focus events only for an active window.
            vcx.update(|window, _| window.activate_window());
            vcx.run_until_parked();
            vcx.update(|window, cx| {
                view.update(cx, |v, cx| v.focus_composer(window, cx));
                let _ = window.draw(cx);
            });
            vcx.run_until_parked();
            // Home's chats aren't panes: making one the focused pane would have Root focus a terminal for it.
            assert_eq!(vcx.update(|_, cx| app.read(cx).focused_pane_id.clone()).as_deref(), Some(expect), "{pane_id}");
        }
    }
}
