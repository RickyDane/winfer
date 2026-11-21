//! The side panel's file viewer: markdown rendered, images shown, other text files with line
//! numbers, with live reload, editing and review comments
//! (port of `src/features/viewer/ViewerPanel.tsx`, `viewer.ts`, `projects.ts` and `comments.ts`).

use crate::backend;
use crate::model::{AppState, Workspace};
use crate::terminal::SESSIONS;
use crate::theme::{self, Colors};
use gpui_kit::assets::IconName;
use gpui_kit::component::Icon;
use gpui_kit::component::input::{Editor, EditorState, Position};
use gpui_kit::component::text::TextView;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use regex::Regex;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, LazyLock};
use std::time::Duration;
use winfer_backend::files;

const IMAGE: [&str; 8] = ["png", "jpg", "jpeg", "gif", "webp", "bmp", "svg", "ico"];
const POLL: Duration = Duration::from_millis(1500);

static COMMENT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)<!--\s*@([\w.-]+):\s*(.*?)\s*-->").unwrap());

#[derive(Clone, Debug, PartialEq)]
pub struct DocComment {
    pub author: String,
    pub body: String,
    pub start: usize,
    pub end: usize,
}

pub fn find_comments(text: &str) -> Vec<DocComment> {
    COMMENT
        .captures_iter(text)
        .map(|c| {
            let whole = c.get(0).unwrap();
            DocComment { author: c[1].to_string(), body: c[2].to_string(), start: whole.start(), end: whole.end() }
        })
        .collect()
}

pub fn format_comment(body: &str, author: &str) -> String {
    format!("<!-- @{author}: {} -->", body.trim().replace("-->", "-- >").replace("--!>", "-- !>"))
}

/// `text` with a comment added on its own after line `after_line` (1-based; 0 puts it first),
/// set apart by blank lines so markdown keeps it out of the block above.
pub fn insert_comment(text: &str, after_line: usize, body: &str, author: &str) -> String {
    let mut lines: Vec<String> = text.split('\n').map(str::to_string).collect();
    let at = after_line.min(lines.len());
    let mut add = vec![format_comment(body, author)];
    if at > 0 && !lines[at - 1].trim().is_empty() {
        add.insert(0, String::new());
    }
    if at < lines.len() && !lines[at].trim().is_empty() {
        add.push(String::new());
    }
    lines.splice(at..at, add);
    lines.join("\n")
}

/// `text` without the comment starting at `start`, along with its line when it had one to itself.
pub fn remove_comment(text: &str, start: usize) -> String {
    let Some(c) = find_comments(text).into_iter().find(|x| x.start == start) else { return text.to_string() };
    let line_start = text[..c.start].rfind('\n').map_or(0, |i| i + 1);
    let newline = text[c.end..].find('\n').map(|i| i + c.end);
    let line_end = newline.unwrap_or(text.len());
    let alone = text[line_start..c.start].trim().is_empty() && text[c.end..line_end].trim().is_empty();
    if !alone {
        return format!("{}{}", text[..c.start].trim_end_matches([' ', '\t']), &text[c.end..]);
    }
    let mut from = line_start;
    let mut to = newline.map_or(text.len(), |i| i + 1);
    let rest = &text[to..];
    let blank_len = rest.find('\n').filter(|i| rest[..*i].trim().is_empty()).map(|i| i + 1);
    if let Some(blank) = blank_len.filter(|_| to < text.len()) {
        to += blank;
    } else if from >= 2 {
        let before = &text[..from];
        if let Some(previous) = before[..before.len() - 1].rfind('\n') {
            if before[previous + 1..before.len() - 1].trim().is_empty() {
                from = previous + 1;
            }
        }
    }
    format!("{}{}", &text[..from], &text[to..])
}

/// A GitHub-style alert / callout blockquote (`> [!NOTE]`, `> [!IMPORTANT]`, …).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AlertKind {
    Note,
    Tip,
    Important,
    Warning,
    Caution,
}

impl AlertKind {
    fn parse(tag: &str) -> Option<Self> {
        match tag.to_ascii_uppercase().as_str() {
            "NOTE" => Some(Self::Note),
            "TIP" => Some(Self::Tip),
            "IMPORTANT" => Some(Self::Important),
            "WARNING" => Some(Self::Warning),
            "CAUTION" => Some(Self::Caution),
            _ => None,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Note => "Note",
            Self::Tip => "Tip",
            Self::Important => "Important",
            Self::Warning => "Warning",
            Self::Caution => "Caution",
        }
    }

    fn icon(self) -> IconName {
        match self {
            Self::Note => IconName::Info,
            Self::Tip => IconName::Lightbulb,
            Self::Important => IconName::MessageSquareWarning,
            Self::Warning => IconName::TriangleAlert,
            Self::Caution => IconName::OctagonAlert,
        }
    }

    fn color(self, c: Colors) -> Hsla {
        match self {
            Self::Note => theme::hex(if c.light { "#0969da" } else { "#58a6ff" }),
            Self::Tip => c.success,
            Self::Important => theme::hex(if c.light { "#8250df" } else { "#a371f7" }),
            Self::Warning => c.warning,
            Self::Caution => c.danger,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AlertBlock {
    pub kind: AlertKind,
    pub body: String,
}

fn strip_quote_prefix(line: &str) -> Option<&str> {
    let trimmed = line.trim_start_matches(' ');
    if line.len() - trimmed.len() > 3 {
        return None;
    }
    let rest = trimmed.strip_prefix('>')?;
    Some(rest.strip_prefix(' ').unwrap_or(rest))
}

fn parse_alert_opener(line: &str) -> Option<(AlertKind, &str)> {
    let inner = strip_quote_prefix(line)?.trim_start();
    let rest = inner.strip_prefix("[!")?;
    let (tag, after) = rest.split_once(']')?;
    let kind = AlertKind::parse(tag)?;
    let trailing = if after.is_empty() {
        ""
    } else if after.starts_with(char::is_whitespace) {
        after.trim_start()
    } else {
        return None;
    };
    Some((kind, trailing))
}

/// Parses a GitHub-style alert blockquote (`> [!NOTE]` / `[!TIP]` / `[!IMPORTANT]` / `[!WARNING]` / `[!CAUTION]`).
pub fn parse_alert(source: &str) -> Option<AlertBlock> {
    let mut lines = source.lines();
    let first = lines.next()?;
    let (kind, first_rest) = parse_alert_opener(first)?;
    let mut body_lines: Vec<&str> = Vec::new();
    if !first_rest.is_empty() {
        body_lines.push(first_rest);
    }
    for line in lines {
        if let Some(quoted) = strip_quote_prefix(line) {
            body_lines.push(quoted);
        } else {
            let t = line.trim_start();
            if t.starts_with('#') || t.starts_with("```") || t.starts_with("~~~") {
                return None;
            }
            body_lines.push(line);
        }
    }
    while body_lines.first().is_some_and(|l| l.trim().is_empty()) {
        body_lines.remove(0);
    }
    Some(AlertBlock { kind, body: body_lines.join("\n") })
}

/// Extracts the inner diagram markup of a closed `mermaid` fenced code block.
pub fn parse_mermaid_fence(source: &str) -> Option<String> {
    let lines: Vec<&str> = source.lines().collect();
    if lines.len() < 2 {
        return None;
    }
    let first = lines[0].trim();
    let ch = first.chars().next().filter(|c| *c == '`' || *c == '~')?;
    let n = first.chars().take_while(|c| *c == ch).count();
    if n < 3 {
        return None;
    }
    let lang = first[n..].trim().split_whitespace().next()?;
    if !lang.eq_ignore_ascii_case("mermaid") {
        return None;
    }
    let last = lines.last()?.trim();
    let close_n = last.chars().take_while(|c| *c == ch).count();
    if close_n < n || !last[close_n..].trim().is_empty() {
        return None;
    }
    Some(lines[1..lines.len() - 1].join("\n"))
}

/// A top-level block of a markdown file: its lines (1-based, inclusive) and source, or a review
/// comment standing on its own.
#[derive(Clone, Debug, PartialEq)]
pub struct MdBlock {
    pub start_line: usize,
    pub end_line: usize,
    pub source: String,
    pub comment: Option<DocComment>,
}

/// Splits markdown into the blocks a reader can comment on: runs of lines set apart by blank
/// lines, with fenced code kept whole and GitHub alerts / top-level fences isolated.
pub fn md_blocks(text: &str) -> Vec<MdBlock> {
    let mut blocks = Vec::new();
    let mut current: Vec<&str> = Vec::new();
    let mut start = 1;
    let mut fence: Option<String> = None;
    let mut offset = 0;
    let mut starts: Vec<usize> = Vec::new();
    let flush = |current: &mut Vec<&str>, start: usize, blocks: &mut Vec<MdBlock>, starts: &mut Vec<usize>| {
        if current.is_empty() {
            return;
        }
        let source = current.join("\n");
        let end_line = start + current.len() - 1;
        let comment = COMMENT.captures(&source).filter(|c| c.get(0).is_some_and(|m| m.as_str().trim() == source.trim())).and_then(|c| {
            let whole = c.get(0)?;
            Some(DocComment { author: c[1].to_string(), body: c[2].to_string(), start: starts.get(blocks.len()).copied().unwrap_or(0) + whole.start(), end: 0 })
        });
        blocks.push(MdBlock { start_line: start, end_line, source, comment });
        current.clear();
    };
    for (i, raw) in text.split('\n').enumerate() {
        let line = raw.trim_end_matches('\r');
        let trimmed = line.trim_start();
        if let Some(open) = &fence {
            current.push(line);
            if trimmed.starts_with(open.as_str()) && trimmed.trim_start_matches(open.chars().next().unwrap_or('`')).trim().is_empty() {
                fence = None;
                flush(&mut current, start, &mut blocks, &mut starts);
            }
        } else if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            if !current.is_empty() && !line.starts_with(' ') && !line.starts_with('\t') {
                flush(&mut current, start, &mut blocks, &mut starts);
            }
            if current.is_empty() {
                start = i + 1;
                starts.push(offset);
            }
            let ch = trimmed.chars().next().unwrap_or('`');
            let n = trimmed.chars().take_while(|c| *c == ch).count();
            fence = Some(ch.to_string().repeat(n));
            current.push(line);
        } else if line.trim().is_empty() {
            flush(&mut current, start, &mut blocks, &mut starts);
        } else {
            let in_alert = current.first().is_some_and(|l| parse_alert_opener(l).is_some());
            let starts_new_alert = parse_alert_opener(line).is_some();
            let breaks_alert = in_alert
                && strip_quote_prefix(line).is_none()
                && (trimmed.starts_with('#')
                    || trimmed.starts_with("- ")
                    || trimmed.starts_with("* ")
                    || trimmed.starts_with("+ ")
                    || trimmed.starts_with("---")
                    || trimmed.starts_with("***")
                    || trimmed.starts_with("___"));
            if !current.is_empty() && (starts_new_alert || breaks_alert) {
                flush(&mut current, start, &mut blocks, &mut starts);
            }
            if current.is_empty() {
                start = i + 1;
                starts.push(offset);
            }
            current.push(line);
        }
        offset += raw.len() + 1;
    }
    flush(&mut current, start, &mut blocks, &mut starts);
    // Comments know where they start in the file; recompute so remove_comment can find them.
    let all = find_comments(text);
    for b in &mut blocks {
        if let Some(c) = &mut b.comment {
            if let Some(found) = all.iter().find(|x| x.author == c.author && x.body == c.body && x.start >= c.start) {
                *c = found.clone();
            }
        }
    }
    blocks
}

#[derive(Clone, Debug, PartialEq)]
pub struct ViewerProject {
    pub id: String,
    pub name: String,
    pub root: Option<String>,
    pub docs: Vec<String>,
}

fn normalized(path: &str) -> String {
    path.replace('\\', "/").trim_end_matches('/').to_string()
}

/// Groups files by project: the most specific root wins, a worktree belongs to its project.
pub fn viewer_projects(docs: &[String], workspaces: &[Workspace]) -> Vec<ViewerProject> {
    let mut roots: Vec<(&Workspace, String)> = workspaces
        .iter()
        .flat_map(|w| std::iter::once(w.root_dir.clone()).chain(w.panes.iter().filter_map(|p| p.worktree.as_ref().map(|t| t.path.clone()))).map(move |r| (w, normalized(&expand(&r)))))
        .collect();
    roots.sort_by_key(|(_, r)| std::cmp::Reverse(r.len()));
    let mut groups: Vec<ViewerProject> = Vec::new();
    for path in docs {
        let file = normalized(path);
        let owner = roots.iter().find(|(_, r)| file.starts_with(&format!("{r}/"))).map(|(w, _)| *w);
        let id = owner.map_or_else(|| "__other_files__".to_string(), |w| w.id.clone());
        match groups.iter_mut().find(|g| g.id == id) {
            Some(g) => g.docs.push(path.clone()),
            None => groups.push(ViewerProject {
                id,
                name: owner.map_or_else(|| "Other files".to_string(), |w| w.name.clone()),
                root: owner.map(|w| w.root_dir.clone()),
                docs: vec![path.clone()],
            }),
        }
    }
    groups
}

fn expand(path: &str) -> String {
    match (path, dirs::home_dir()) {
        ("~", Some(h)) => h.display().to_string(),
        (p, Some(h)) if p.starts_with("~/") => format!("{}{}", h.display(), &p[1..]),
        (p, _) => p.to_string(),
    }
}

fn extension(path: &str) -> String {
    file_name(path).rsplit_once('.').filter(|(stem, _)| !stem.is_empty()).map_or(String::new(), |(_, e)| e.to_lowercase())
}

fn file_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

fn language_for(path: &str) -> String {
    let name = file_name(path);
    match name {
        "Makefile" | "makefile" | "GNUmakefile" => "make".to_string(),
        "Dockerfile" | "Containerfile" => "dockerfile".to_string(),
        ".bashrc" | ".bash_profile" | ".zshrc" | ".zprofile" | ".profile" => "bash".to_string(),
        _ => {
            let ext = extension(path);
            if ext.is_empty() { name.to_string() } else { ext }
        }
    }
}

/// A tab's name: the file name, with its folder when another open file shares it.
fn tab_label(path: &str, docs: &[String]) -> String {
    let name = file_name(path);
    if docs.iter().filter(|d| file_name(d) == name).count() < 2 {
        return name.to_string();
    }
    let parts: Vec<&str> = path.split(['/', '\\']).collect();
    parts.len().checked_sub(2).map_or(name.to_string(), |i| format!("{}/{name}", parts[i]))
}

/// The file a markdown link points at, resolved from the document's folder: `None` for web links
/// and in-page anchors. Keeps a `:12` or `#L12` line suffix, like terminal links.
fn link_target(href: &str, doc: &str) -> Option<(String, Option<usize>)> {
    let file = href.strip_prefix("file://");
    let scheme = href.split_once(':').is_some_and(|(s, _)| s.len() > 1 && s.chars().all(|c| c.is_ascii_alphanumeric() || "+.-".contains(c)) && !s.contains('.'));
    if href.starts_with('#') || (scheme && file.is_none() && !href.split_once(':').is_some_and(|(_, r)| r.chars().next().is_some_and(|c| c.is_ascii_digit()))) {
        return None;
    }
    let raw = file.unwrap_or(href).split('?').next().unwrap_or_default();
    let (raw, line) = match raw.rsplit_once("#L").or_else(|| raw.rsplit_once(':')) {
        Some((p, n)) if n.parse::<usize>().is_ok() => (p, n.parse().ok()),
        _ => (raw.split('#').next().unwrap_or_default(), None),
    };
    let decoded = percent_decode(raw);
    if decoded.is_empty() { return None; }
    let path = std::path::Path::new(&decoded);
    let full = if path.is_absolute() { path.to_path_buf() } else { std::path::Path::new(doc).parent()?.join(path) };
    Some((full.to_string_lossy().into_owned(), line))
}

/// Wraps link destinations that hold spaces or parentheses (`[a](file:///Dir (X)/a.rs)`) in `<…>`
/// so the markdown parser reads them as links; fenced code is left alone.
fn fix_link_destinations(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let mut fenced = false;
    for line in src.split_inclusive('\n') {
        if line.trim_start().starts_with("```") || line.trim_start().starts_with("~~~") {
            fenced = !fenced;
        }
        if fenced || !line.contains("](") {
            out.push_str(line);
            continue;
        }
        let mut rest = line;
        while let Some(i) = rest.find("](") {
            let (head, tail) = rest.split_at(i + 2);
            out.push_str(head);
            let mut depth = 1usize;
            let end = tail.char_indices().find_map(|(j, ch)| {
                match ch {
                    '(' => depth += 1,
                    ')' => {
                        depth -= 1;
                        if depth == 0 { return Some(j); }
                    }
                    _ => {}
                }
                None
            });
            match end {
                Some(j) if !tail.starts_with('<') && !tail[..j].contains(['<', '>', '"', '\n']) && tail[..j].contains([' ', '(']) => {
                    out.push('<');
                    out.push_str(&tail[..j]);
                    out.push('>');
                    rest = &tail[j..];
                }
                _ => rest = tail,
            }
        }
        out.push_str(rest);
    }
    out
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = (bytes[i] == b'%').then(|| s.get(i + 1..i + 3).and_then(|h| u8::from_str_radix(h, 16).ok())).flatten();
        match hex { Some(b) => { out.push(b); i += 3; } None => { out.push(bytes[i]); i += 1; } }
    }
    String::from_utf8_lossy(&out).into_owned()
}

enum Content {
    Loading,
    Failed(String),
    Text { text: SharedString, modified: u64, line_count: usize, comments: Vec<DocComment> },
    Image(PathBuf),
    Pdf { bytes: Arc<Vec<u8>>, pages: usize },
}

#[derive(Clone, Hash, Eq, PartialEq)]
struct MermaidKey {
    theme: String,
    source: String,
}

enum MermaidState {
    Pending,
    Ready { image: Arc<Image>, width: f32, height: f32 },
    Failed(String),
}

pub struct ViewerPanel {
    state: Entity<AppState>,
    shown: Option<String>,
    content: Content,
    source: bool,
    editor: Option<Entity<EditorState>>,
    editor_synced: Option<(String, u64)>,
    editing: bool,
    edit_base: u64,
    conflict: bool,
    composing: Option<Entity<EditorState>>,
    comment_line: usize,
    jump: Option<usize>,
    send_open: bool,
    poll: Option<Task<()>>,
    pdf: PdfView,
    mermaid_cache: HashMap<MermaidKey, MermaidState>,
    mermaid_source: HashSet<usize>,
    mermaid_zoom: HashMap<usize, f32>,
}

/// Which page of a PDF shows, how far in, and the rendered result.
struct PdfView {
    page: usize,
    zoom: f32,
    image: Option<(Arc<Image>, f32)>,
    busy: bool,
    error: Option<String>,
    modified: u64,
}

impl Default for PdfView {
    fn default() -> Self {
        Self { page: 1, zoom: 1.0, image: None, busy: false, error: None, modified: 0 }
    }
}

impl ViewerPanel {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        cx.observe(&state, |this, _, cx| this.sync(cx)).detach();
        Self { state, shown: None, content: Content::Loading, source: false, editor: None, editor_synced: None, editing: false, edit_base: 0, conflict: false, composing: None, comment_line: 0, jump: None, send_open: false, poll: None, pdf: PdfView::default(), mermaid_cache: HashMap::new(), mermaid_source: HashSet::new(), mermaid_zoom: HashMap::new() }
    }

    /// Shows `path` (absolute), opening its tab; `background` leaves the side panel as it is.
    pub fn open(&mut self, path: String, line: Option<usize>, background: bool, cx: &mut Context<Self>) {
        self.jump = line;
        self.state.update(cx, |s, cx| {
            let groups = viewer_projects(&s.viewer.docs, &s.workspaces);
            if !s.viewer.docs.contains(&path) {
                s.viewer.docs.push(path.clone());
            }
            let fresh = viewer_projects(&s.viewer.docs, &s.workspaces);
            let previous = groups.iter().find(|g| s.viewer.active.as_ref().is_some_and(|a| g.docs.contains(a)));
            let target = fresh.iter().find(|g| g.docs.contains(&path));
            if let (Some(previous), Some(target), Some(active)) = (previous, target, s.viewer.active.clone()) {
                if previous.id != target.id {
                    s.viewer.active_by_project.insert(previous.id.clone(), active);
                }
            }
            s.viewer.active = Some(path.clone());
            if !background {
                s.side_panel = Some(crate::model::SidePanel::Viewer);
                s.browser_open = true;
            }
            cx.notify();
        });
        if self.shown.as_ref() == Some(&path) {
            self.apply_jump(cx);
        }
    }

    fn sync(&mut self, cx: &mut Context<Self>) {
        let active = self.state.read(cx).viewer.active.clone();
        if active != self.shown {
            self.shown = active;
            self.editor = None;
            self.editor_synced = None;
            self.editing = false;
            self.composing = None;
            self.conflict = false;
            self.source = false;
            self.send_open = false;
            self.mermaid_source.clear();
            self.mermaid_zoom.clear();
            self.load(cx);
            self.start_polling(cx);
        }
    }

    fn start_polling(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.shown.clone() else {
            self.poll = None;
            return;
        };
        if IMAGE.contains(&extension(&path).as_str()) {
            self.poll = None;
            return;
        }
        self.poll = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(POLL).await;
                let p = path.clone();
                let Ok(modified) = backend::background(move || files::files_modified(p)).await else { continue };
                let stale = this.update(cx, |this, _| match &this.content {
                    Content::Text { modified: known, .. } => !this.editing && modified.is_some_and(|m| m != *known),
                    Content::Pdf { .. } => modified.is_some_and(|m| m != this.pdf.modified),
                    _ => false,
                });
                match stale {
                    Ok(true) => {
                        let _ = this.update(cx, |this, cx| this.load(cx));
                    }
                    Ok(false) => {}
                    Err(_) => break,
                }
            }
        }));
    }

    fn load(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.shown.clone() else {
            self.content = Content::Loading;
            return cx.notify();
        };
        if IMAGE.contains(&extension(&path).as_str()) {
            self.content = Content::Image(PathBuf::from(&path));
            return cx.notify();
        }
        if extension(&path) == "pdf" {
            return self.load_pdf(path, cx);
        }
        if matches!(self.content, Content::Failed(_)) {
            self.content = Content::Loading;
        }
        cx.spawn(async move |this, cx| {
            let target = path.clone();
            let read = tauri::async_runtime::spawn(async move {
                let file = files::files_read_text(target).await?;
                let v = serde_json::to_value(file).map_err(|e| e.to_string())?;
                let text = v["text"].as_str().unwrap_or_default().to_string();
                Ok::<_, String>((text, v["modified"].as_u64().unwrap_or(0)))
            })
            .await
            .map_err(|e| e.to_string())
            .and_then(|r| r);
            let _ = this.update(cx, |this, cx| {
                if this.shown.as_ref() != Some(&path) {
                    return;
                }
                this.content = match read {
                    Ok((text, modified)) => {
                        let line_count = text.split('\n').count();
                        let comments = find_comments(&text);
                        Content::Text { text: text.into(), modified, line_count, comments }
                    }
                    Err(err) => Content::Failed(err),
                };
                this.apply_jump(cx);
                cx.notify();
            });
        })
        .detach();
    }

    fn load_pdf(&mut self, path: String, cx: &mut Context<Self>) {
        if !matches!(self.content, Content::Pdf { .. }) {
            self.content = Content::Loading;
        }
        cx.spawn(async move |this, cx| {
            let target = path.clone();
            let read = backend::background(move || {
                let modified = files::files_modified(target.clone()).unwrap_or(0);
                let len = std::fs::metadata(expand(&target)).map_err(|e| format!("can't open {target}: {e}"))?.len();
                if len > 200 * 1024 * 1024 {
                    return Err(format!("{target} is too big"));
                }
                let bytes = Arc::new(std::fs::read(expand(&target)).map_err(|e| e.to_string())?);
                let pages = crate::pdf::page_count(&bytes)?;
                Ok::<_, String>((bytes, pages, modified))
            })
            .await
            .map_err(|e| e.to_string())
            .and_then(|r| r);
            let _ = this.update(cx, |this, cx| {
                if this.shown.as_ref() != Some(&path) {
                    return;
                }
                match read {
                    Ok((bytes, pages, modified)) => {
                        this.pdf.modified = modified;
                        this.pdf.page = this.pdf.page.clamp(1, pages.max(1));
                        this.content = Content::Pdf { bytes, pages };
                        this.render_pdf(cx);
                    }
                    Err(err) => this.content = Content::Failed(format!("Couldn't display PDF: {err}")),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Renders the current page at the current zoom (a page is about 1000 px wide at fit width).
    fn render_pdf(&mut self, cx: &mut Context<Self>) {
        let Content::Pdf { bytes, .. } = &self.content else { return };
        let (bytes, page, zoom, path) = (bytes.clone(), self.pdf.page, self.pdf.zoom, self.shown.clone());
        self.pdf.busy = true;
        self.pdf.error = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = backend::background(move || crate::pdf::render_page(&bytes, page - 1, 1.7 * zoom)).await.map_err(|e| e.to_string()).and_then(|r| r);
            let _ = this.update(cx, |this, cx| {
                if this.shown != path || this.pdf.page != page || this.pdf.zoom != zoom {
                    return;
                }
                this.pdf.busy = false;
                match result {
                    Ok(p) => this.pdf.image = Some((Arc::new(Image::from_bytes(ImageFormat::Png, p.png)), p.width as f32 / p.height.max(1) as f32)),
                    Err(err) => this.pdf.error = Some(err),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn pdf_go(&mut self, page: usize, zoom: f32, cx: &mut Context<Self>) {
        let Content::Pdf { pages, .. } = &self.content else { return };
        let (page, zoom) = (page.clamp(1, (*pages).max(1)), zoom.clamp(0.25, 3.0));
        if page == self.pdf.page && zoom == self.pdf.zoom {
            return;
        }
        self.pdf.page = page;
        self.pdf.zoom = zoom;
        self.render_pdf(cx);
    }

    fn render_pdf_view(&self, pages: usize, c: Colors, cx: &mut Context<Self>) -> AnyElement {
        let (page, zoom) = (self.pdf.page, self.pdf.zoom);
        let button = |id: &'static str, icon: IconName, enabled: bool| {
            div().id(id).p_1().rounded_md().text_color(if enabled { c.muted } else { c.subtle }).when(enabled, |d| d.cursor_pointer().hover(move |d| d.bg(c.hover))).child(Icon::new(icon).size(px(14.0)))
        };
        let controls = div().flex().items_center().justify_center().gap_2().px_2().py_1().text_xs().text_color(c.muted)
            .child(button("pdf-prev", IconName::ChevronLeft, page > 1).on_click(cx.listener(move |this, _, _, cx| this.pdf_go(page.saturating_sub(1), zoom, cx))))
            .child(format!("Page {page} of {pages}"))
            .child(button("pdf-next", IconName::ChevronRight, page < pages).on_click(cx.listener(move |this, _, _, cx| this.pdf_go(page + 1, zoom, cx))))
            .child(div().h(px(12.0)).w(px(1.0)).bg(c.border))
            .child(button("pdf-out", IconName::Minus, zoom > 0.25).on_click(cx.listener(move |this, _, _, cx| this.pdf_go(page, zoom - 0.25, cx))))
            .child(div().id("pdf-fit").px_1().rounded_sm().cursor_pointer().hover(move |d| d.bg(c.hover)).on_click(cx.listener(move |this, _, _, cx| this.pdf_go(page, 1.0, cx))).child(format!("{}%", (zoom * 100.0).round())))
            .child(button("pdf-in", IconName::Plus, zoom < 3.0).on_click(cx.listener(move |this, _, _, cx| this.pdf_go(page, zoom + 0.25, cx))))
            .when(self.pdf.busy, |d| d.child(div().text_color(c.subtle).child("Rendering…")));
        let surface: AnyElement = match (&self.pdf.error, &self.pdf.image) {
            (Some(err), _) => div().p_4().text_xs().text_color(c.danger).child(format!("Couldn't display PDF: {err}")).into_any_element(),
            (None, Some((image, aspect))) => div().w(relative(zoom)).min_w(px(120.0)).aspect_ratio(*aspect).bg(white()).shadow_md().child(img(image.clone()).size_full()).into_any_element(),
            (None, None) => div().p_4().text_xs().text_color(c.subtle).child("Loading…").into_any_element(),
        };
        div().size_full().flex().flex_col().child(controls)
            .child(div().id("pdf-scroll").flex_1().min_h_0().overflow_scroll().p_3().child(surface))
            .into_any_element()
    }

    fn apply_jump(&mut self, cx: &mut Context<Self>) {
        if self.jump.is_some() && matches!(self.content, Content::Text { .. }) {
            if matches!(extension(self.shown.as_deref().unwrap_or_default()).as_str(), "md" | "markdown" | "mdx" | "mmd" | "mermaid") {
                self.source = true;
            }
            cx.notify();
        }
    }

    fn close(&mut self, path: &str, cx: &mut Context<Self>) {
        self.state.update(cx, |s, cx| {
            let group = viewer_projects(&s.viewer.docs, &s.workspaces).into_iter().find(|g| g.docs.iter().any(|d| d == path));
            let siblings: Vec<String> = group.as_ref().map(|g| g.docs.iter().filter(|d| *d != path).cloned().collect()).unwrap_or_default();
            let index = if siblings.is_empty() { s.viewer.docs.iter().position(|d| d == path) } else { group.as_ref().and_then(|g| g.docs.iter().position(|d| d == path)) }.unwrap_or(0);
            s.viewer.docs.retain(|d| d != path);
            let candidates = if siblings.is_empty() { s.viewer.docs.clone() } else { siblings };
            if s.viewer.active.as_deref() == Some(path) {
                s.viewer.active = candidates.get(index.min(candidates.len().saturating_sub(1))).cloned();
            }
            s.viewer.active_by_project.retain(|_, v| v != path);
            cx.notify();
        });
    }

    fn select_project(&mut self, id: &str, cx: &mut Context<Self>) {
        self.state.update(cx, |s, cx| {
            let groups = viewer_projects(&s.viewer.docs, &s.workspaces);
            let Some(target) = groups.iter().find(|g| g.id == id) else { return };
            if let Some(active) = s.viewer.active.clone() {
                if let Some(current) = groups.iter().find(|g| g.docs.contains(&active)) {
                    s.viewer.active_by_project.insert(current.id.clone(), active);
                }
            }
            let remembered = s.viewer.active_by_project.get(id).filter(|r| target.docs.contains(r)).cloned();
            s.viewer.active = remembered.or_else(|| target.docs.first().cloned());
            cx.notify();
        });
    }

    fn pick_files(&mut self, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions { files: true, directories: false, multiple: true, prompt: Some("Open".into()) });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = paths.await {
                let _ = this.update(cx, |this, cx| {
                    for path in paths {
                        this.open(path.display().to_string(), None, false, cx);
                    }
                });
            }
        })
        .detach();
    }

    fn sync_editor(&mut self, path: &str, text: &SharedString, modified: u64, window: &mut Window, cx: &mut Context<Self>) -> Entity<EditorState> {
        if let Some(editor) = self.editor.clone() {
            let synced = self.editor_synced.as_ref().is_some_and(|(p, m)| p == path && *m == modified);
            if !self.editing && !synced {
                let next = text.to_string();
                editor.update(cx, |state, cx| {
                    if state.value().as_ref() != next.as_str() {
                        state.set_value(next, window, cx);
                    }
                });
                self.editor_synced = Some((path.to_string(), modified));
            }
            return editor;
        }
        let lang = language_for(path);
        let initial = text.to_string();
        let editor = cx.new(|cx| {
            let mut state = EditorState::new(window, cx).language(lang).soft_wrap(false);
            state.set_value(initial, window, cx);
            state
        });
        self.editor = Some(editor.clone());
        self.editor_synced = Some((path.to_string(), modified));
        editor
    }

    fn begin_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(path), Content::Text { text, modified, .. }) = (self.shown.clone(), &self.content) else { return };
        let (text, modified) = (text.clone(), *modified);
        let editor = self.sync_editor(&path, &text, modified, window, cx);
        editor.update(cx, |state, cx| {
            state.set_readonly(false, cx);
            state.focus(window, cx);
        });
        self.edit_base = modified;
        self.editing = true;
        self.conflict = false;
        self.composing = None;
        cx.notify();
    }

    fn cancel_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.editing = false;
        self.conflict = false;
        if let (Some(editor), Some(path), Content::Text { text, modified, .. }) = (self.editor.clone(), self.shown.clone(), &self.content) {
            let (text, modified) = (text.clone(), *modified);
            editor.update(cx, |state, cx| {
                state.set_readonly(true, cx);
                if state.value().as_ref() != text.as_ref() {
                    state.set_value(text.to_string(), window, cx);
                }
            });
            self.editor_synced = Some((path, modified));
        }
        cx.notify();
    }

    fn save_draft(&mut self, overwrite: bool, cx: &mut Context<Self>) {
        let (Some(editor), Some(path)) = (self.editor.clone(), self.shown.clone()) else { return };
        let text = editor.read(cx).value().to_string();
        let expected = (!overwrite).then_some(self.edit_base);
        self.write(path, text, expected, true, cx);
    }

    /// Saves `text`; a changed file reports a conflict instead of being overwritten.
    fn write(&mut self, path: String, text: String, expected: Option<u64>, from_editor: bool, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let saved = tauri::async_runtime::spawn(async move { files::files_write_text(path, text, expected).await }).await.map_err(|e| e.to_string()).and_then(|r| r);
            let _ = this.update(cx, |this, cx| {
                match saved {
                    Ok(_) => {
                        if from_editor {
                            this.editing = false;
                        }
                        this.conflict = false;
                        this.load(cx);
                    }
                    Err(_) if from_editor => this.conflict = true,
                    Err(err) => this.content = Content::Failed(err),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn begin_comment(&mut self, after_line: usize, window: &mut Window, cx: &mut Context<Self>) {
        let editor = cx.new(|cx| EditorState::new(window, cx));
        editor.read(cx).focus_handle(cx).focus(window, cx);
        self.comment_line = after_line;
        self.composing = Some(editor);
        cx.notify();
    }

    fn submit_comment(&mut self, cx: &mut Context<Self>) {
        let (Some(composer), Some(path), Content::Text { text, modified, .. }) = (self.composing.take(), self.shown.clone(), &self.content) else { return };
        let body = composer.read(cx).value().to_string();
        if body.trim().is_empty() {
            return cx.notify();
        }
        let updated = insert_comment(text, self.comment_line, &body, "user");
        let modified = *modified;
        self.write(path, updated, Some(modified), false, cx);
    }

    fn remove_comment_at(&mut self, start: usize, cx: &mut Context<Self>) {
        let (Some(path), Content::Text { text, modified, .. }) = (self.shown.clone(), &self.content) else { return };
        let updated = remove_comment(text, start);
        let modified = *modified;
        self.write(path, updated, Some(modified), false, cx);
    }

    fn send_comments(&mut self, pane: String, count: usize, cx: &mut Context<Self>) {
        let Some(path) = self.shown.clone() else { return };
        self.send_open = false;
        let what = if count == 1 { "a review comment".to_string() } else { format!("{count} review comments") };
        let text = format!("I left {what} in {path} (HTML comments like <!-- @user: … --> after the part they're about). Read the file, address them, and remove each comment once it's handled (or reply with <!-- @agent: … -->).");
        if let Some(session) = SESSIONS.get(&pane) {
            session.send(&format!("\x1b[200~{text}\x1b[201~"));
            cx.spawn(async move |_, cx| {
                // Lets the paste land before submitting it.
                cx.background_executor().timer(Duration::from_millis(80)).await;
                session.send("\r");
            })
            .detach();
        }
        cx.notify();
    }

    fn ensure_mermaid(&mut self, theme_id: &str, code: &str, cx: &mut Context<Self>) -> &MermaidState {
        let key = MermaidKey { theme: theme_id.to_string(), source: code.to_string() };
        if !self.mermaid_cache.contains_key(&key) {
            if self.mermaid_cache.len() >= 64 {
                self.mermaid_cache.clear();
            }
            self.mermaid_cache.insert(key.clone(), MermaidState::Pending);
            let k = key.clone();
            cx.spawn(async move |this, cx| {
                let (src, th) = (k.source.clone(), k.theme.clone());
                let res = backend::background(move || crate::mermaid::render_png(&src, &th))
                    .await
                    .map_err(|e| e.to_string())
                    .and_then(|r| r);
                let _ = this.update(cx, |this, cx| {
                    let state = match res {
                        Ok(d) => MermaidState::Ready {
                            image: Arc::new(Image::from_bytes(ImageFormat::Png, d.png)),
                            width: d.width,
                            height: d.height,
                        },
                        Err(err) => MermaidState::Failed(err),
                    };
                    this.mermaid_cache.insert(k, state);
                    cx.notify();
                });
            })
            .detach();
        }
        self.mermaid_cache.get(&key).expect("mermaid state inserted")
    }

    fn render_alert(&self, id: SharedString, alert: AlertBlock, doc: String, link_panel: WeakEntity<Self>, c: Colors) -> AnyElement {
        let accent = alert.kind.color(c);
        let bg = theme::blend(c.panel, accent, if c.light { 0.06 } else { 0.09 });
        let border = Hsla { a: if c.light { 0.25 } else { 0.22 }, ..accent };
        let mut inner = div()
            .flex_1()
            .min_w_0()
            .px_3()
            .py_2()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .text_size(px(12.5))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(accent)
                    .child(Icon::new(alert.kind.icon()).size(px(14.0)).text_color(accent))
                    .child(alert.kind.label()),
            );
        if !alert.body.trim().is_empty() {
            inner = inner.child(
                TextView::markdown(id, SharedString::from(fix_link_destinations(&alert.body)))
                    .selectable(true)
                    .on_link_click(move |href, _, _, cx| match link_target(href, &doc) {
                        Some((target, line)) => {
                            link_panel.update(cx, |this, cx| this.open(target, line, false, cx)).ok();
                        }
                        None if !href.starts_with('#') => cx.open_url(href),
                        None => {}
                    }),
            );
        }
        div()
            .w_full()
            .rounded_md()
            .overflow_hidden()
            .bg(bg)
            .border_1()
            .border_color(border)
            .flex()
            .flex_row()
            .child(div().w(px(3.0)).flex_shrink_0().bg(accent))
            .child(inner)
            .into_any_element()
    }

    fn render_mermaid_card(
        &mut self,
        key_id: String,
        block_line: usize,
        raw_fence: String,
        code: &str,
        theme_id: &str,
        allow_source_toggle: bool,
        pad_right_for_comment: bool,
        c: Colors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let show_source = allow_source_toggle && self.mermaid_source.contains(&block_line);
        let zoom = self.mermaid_zoom.get(&block_line).copied();
        let copy = code.to_string();
        let mut actions = div().flex().items_center().gap_1().when(pad_right_for_comment, |d| d.mr_6());
        if !show_source {
            let cur = zoom.unwrap_or(1.0);
            let can_out = zoom.map_or(true, |z| z > 0.25);
            let can_in = cur < 3.0;
            let zoom_label = match zoom {
                None => "Fit".to_string(),
                Some(z) => format!("{}%", (z * 100.0).round()),
            };
            let zoom_tip = SharedString::from(if zoom.is_none() { "Actual size (100%)" } else { "Fit to width" });
            actions = actions
                .child(
                    div()
                        .id(SharedString::from(format!("{key_id}-zout")))
                        .rounded_sm()
                        .p_1()
                        .text_color(if can_out { c.subtle } else { Hsla { a: 0.35, ..c.subtle } })
                        .when(can_out, |d| d.cursor_pointer().hover(move |d| d.bg(c.hover).text_color(c.fg)))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if !can_out {
                                return;
                            }
                            let next = (this.mermaid_zoom.get(&block_line).copied().unwrap_or(1.0) - 0.25).clamp(0.25, 3.0);
                            this.mermaid_zoom.insert(block_line, next);
                            cx.notify();
                        }))
                        .child(Icon::new(IconName::Minus).size(px(11.0))),
                )
                .child(
                    div()
                        .id(SharedString::from(format!("{key_id}-zfit")))
                        .rounded_sm()
                        .px_1()
                        .py_0p5()
                        .text_size(px(10.5))
                        .text_color(if zoom.is_some() { c.accent } else { c.muted })
                        .cursor_pointer()
                        .hover(move |d| d.bg(c.hover).text_color(c.fg))
                        .tooltip(move |window, cx| Tooltip::new(zoom_tip.clone()).build(window, cx))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if this.mermaid_zoom.remove(&block_line).is_none() {
                                this.mermaid_zoom.insert(block_line, 1.0);
                            }
                            cx.notify();
                        }))
                        .child(zoom_label),
                )
                .child(
                    div()
                        .id(SharedString::from(format!("{key_id}-zin")))
                        .rounded_sm()
                        .p_1()
                        .text_color(if can_in { c.subtle } else { Hsla { a: 0.35, ..c.subtle } })
                        .when(can_in, |d| d.cursor_pointer().hover(move |d| d.bg(c.hover).text_color(c.fg)))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if !can_in {
                                return;
                            }
                            let next = (this.mermaid_zoom.get(&block_line).copied().unwrap_or(1.0) + 0.25).clamp(0.25, 3.0);
                            this.mermaid_zoom.insert(block_line, next);
                            cx.notify();
                        }))
                        .child(Icon::new(IconName::Plus).size(px(11.0))),
                )
                .child(div().h(px(11.0)).w(px(1.0)).mx_0p5().bg(c.border));
        }
        if allow_source_toggle {
            let toggle_tip = SharedString::from(if show_source { "Show diagram" } else { "Show source" });
            actions = actions.child(
                div()
                    .id(SharedString::from(format!("{key_id}-toggle")))
                    .rounded_sm()
                    .p_1()
                    .cursor_pointer()
                    .text_color(if show_source { c.accent } else { c.subtle })
                    .hover(move |d| d.bg(c.hover).text_color(c.fg))
                    .tooltip(move |window, cx| Tooltip::new(toggle_tip.clone()).build(window, cx))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !this.mermaid_source.remove(&block_line) {
                            this.mermaid_source.insert(block_line);
                        }
                        cx.notify();
                    }))
                    .child(Icon::new(if show_source { IconName::Eye } else { IconName::Code }).size(px(12.0))),
            );
        }
        actions = actions.child(
            div()
                .id(SharedString::from(format!("{key_id}-copy")))
                .rounded_sm()
                .p_1()
                .cursor_pointer()
                .text_color(c.subtle)
                .hover(move |d| d.bg(c.hover).text_color(c.fg))
                .tooltip(|window, cx| Tooltip::new("Copy mermaid source").build(window, cx))
                .on_click(move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(copy.clone())))
                .child(Icon::new(IconName::Copy).size(px(12.0))),
        );
        let toolbar = div()
            .flex()
            .items_center()
            .justify_between()
            .px_2p5()
            .py_1()
            .border_b_1()
            .border_color(c.border)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .text_size(px(11.0))
                    .text_color(c.muted)
                    .child(Icon::new(IconName::Workflow).size(px(12.0)).text_color(c.subtle))
                    .child("mermaid"),
            )
            .child(actions);

        let body: AnyElement = if show_source {
            div()
                .p_2()
                .child(TextView::markdown(SharedString::from(format!("{key_id}-src")), SharedString::from(raw_fence)).selectable(true))
                .into_any_element()
        } else {
            match self.ensure_mermaid(theme_id, code, cx) {
                MermaidState::Pending => div()
                    .p_4()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_xs()
                    .text_color(c.subtle)
                    .child("Rendering diagram…")
                    .into_any_element(),
                MermaidState::Ready { image, width, height } => {
                    let aspect = (*width / height.max(1.0)).max(0.05);
                    let (img_w, img_h) = (*width, *height);
                    let image = image.clone();
                    if let Some(z) = zoom {
                        div()
                            .id(SharedString::from(format!("{key_id}-scroll")))
                            .w_full()
                            .overflow_x_scroll()
                            .p_3()
                            .child(
                                div()
                                    .w(px(img_w * z))
                                    .h(px(img_h * z))
                                    .flex_shrink_0()
                                    .child(img(image).size_full().object_fit(ObjectFit::Contain)),
                            )
                            .into_any_element()
                    } else {
                        div()
                            .w_full()
                            .p_3()
                            .flex()
                            .justify_center()
                            .child(
                                div()
                                    .w(px(img_w))
                                    .max_w_full()
                                    .aspect_ratio(aspect)
                                    .child(img(image).size_full().object_fit(ObjectFit::Contain)),
                            )
                            .into_any_element()
                    }
                }
                MermaidState::Failed(err) => {
                    let err = err.clone();
                    div()
                        .flex()
                        .flex_col()
                        .gap_1p5()
                        .p_2p5()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_1p5()
                                .text_xs()
                                .text_color(c.danger)
                                .child(Icon::new(IconName::TriangleAlert).size(px(13.0)).text_color(c.danger))
                                .child(format!("Couldn't render Mermaid diagram: {err}")),
                        )
                        .child(TextView::markdown(SharedString::from(format!("{key_id}-fallback")), SharedString::from(raw_fence)).selectable(true))
                        .into_any_element()
                }
            }
        };

        div()
            .w_full()
            .rounded_md()
            .overflow_hidden()
            .bg(c.raised)
            .border_1()
            .border_color(c.edge)
            .flex()
            .flex_col()
            .child(toolbar)
            .child(body)
            .into_any_element()
    }

    fn render_mermaid_file(&mut self, path: &str, text: SharedString, theme_id: &str, c: Colors, cx: &mut Context<Self>) -> AnyElement {
        let raw_fence = format!("```mermaid\n{text}\n```");
        let card = self.render_mermaid_card(format!("mmd-file-{path}"), 0, raw_fence, text.as_ref(), theme_id, false, false, c, cx);
        div().id("viewer-mermaid").size_full().overflow_y_scroll().p_4().child(card).into_any_element()
    }

    /// Rendered markdown, one block at a time so a reader can comment on the one they mean.
    fn render_markdown(&mut self, path: &str, text: SharedString, theme_id: &str, c: Colors, cx: &mut Context<Self>) -> AnyElement {
        let panel = cx.entity().downgrade();
        let commenting = self.composing.is_some().then_some(self.comment_line);
        let mut column = div().flex().flex_col().gap_2();
        for block in md_blocks(&text) {
            if let Some(note) = block.comment {
                let start = note.start;
                column = column.child(
                    div().flex().items_start().gap_2().rounded_md().bg(c.raised).border_1().border_color(c.edge).px_2().py_1().text_xs()
                        .child(div().text_color(c.accent).child(format!("@{}", note.author)))
                        .child(div().flex_1().min_w_0().text_color(c.fg).child(note.body.clone()))
                        .child(div().id(("block-note-remove", start)).cursor_pointer().on_click(cx.listener(move |this, _, _, cx| this.remove_comment_at(start, cx))).child(Icon::new(IconName::X).size(px(11.0)).text_color(c.subtle))),
                );
                continue;
            }
            let (end, id) = (block.end_line, SharedString::from(format!("md-{path}-{}", block.start_line)));
            let active = commenting == Some(end);
            let doc = path.to_string();
            let link_panel = panel.clone();
            let block_body: AnyElement = if let Some(alert) = parse_alert(&block.source) {
                self.render_alert(id, alert, doc, link_panel, c)
            } else if let Some(code) = parse_mermaid_fence(&block.source) {
                self.render_mermaid_card(
                    format!("mmd-{path}-{}", block.start_line),
                    block.start_line,
                    block.source,
                    &code,
                    theme_id,
                    true,
                    true,
                    c,
                    cx,
                )
            } else {
                TextView::markdown(id, SharedString::from(fix_link_destinations(&block.source)))
                    .selectable(true)
                    .on_link_click(move |href, _, _, cx| match link_target(href, &doc) {
                        Some((target, line)) => {
                            link_panel.update(cx, |this, cx| this.open(target, line, false, cx)).ok();
                        }
                        None if !href.starts_with('#') => cx.open_url(href),
                        None => {}
                    })
                    .into_any_element()
            };
            column = column.child(
                div().id(SharedString::from(format!("block-{path}-{}", block.start_line))).group("md-block").relative().rounded_md().px_2().py_0p5().border_1()
                    .when(active, |d| d.bg(Hsla { a: 0.06, ..c.accent }))
                    .child(block_body)
                    .child(
                        div().id(SharedString::from(format!("block-comment-{path}-{end}"))).absolute().top(px(2.0)).right(px(2.0)).p_1().rounded_md().bg(c.raised).border_1().border_color(c.edge).cursor_pointer()
                            .opacity(0.0).group_hover("md-block", |d| d.opacity(1.0)).tooltip({ let t = SharedString::from("Comment on this block"); move |window, cx| Tooltip::new(t.clone()).build(window, cx) })
                            .on_click(cx.listener(move |this, _, window, cx| this.begin_comment(end, window, cx)))
                            .child(Icon::new(IconName::MessageSquarePlus).size(px(12.0)).text_color(c.muted)),
                    ),
            );
        }
        div().id("viewer-markdown").size_full().overflow_y_scroll().p_4().child(column).into_any_element()
    }

    fn render_code(&mut self, path: &str, text: SharedString, modified: u64, line_count: usize, font: SharedString, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let editor = self.sync_editor(path, &text, modified, window, cx);
        if let Some(line) = self.jump.take() {
            let pos = Position::new(line.saturating_sub(1).min(line_count.saturating_sub(1)) as u32, 0);
            editor.update(cx, |state, cx| {
                state.unfold_at(pos, cx);
                state.set_cursor_position(pos, window, cx);
            });
        }
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                Editor::new(&editor)
                    .appearance(false)
                    .bordered(false)
                    .readonly(!self.editing)
                    .h(relative(1.0))
                    .size_full()
                    .font_family(font)
                    .text_size(px(12.0)),
            )
            .into_any_element()
    }

    fn render_header(&self, path: &str, is_markdown: bool, has_source_toggle: bool, c: Colors, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let _ = window;
        let button = move |id: &'static str, icon: IconName, active: bool| {
            div().id(id).rounded_sm().p_1().cursor_pointer().text_color(if active { c.accent } else { c.subtle }).when(active, |d| d.bg(Hsla { a: 0.15, ..c.accent })).hover(move |d| d.bg(c.hover).text_color(c.fg)).child(Icon::new(icon).size(px(13.0)))
        };
        let editing = self.editing;
        let comments = if let Content::Text { comments, .. } = &self.content { comments.iter().filter(|c| c.author == "user").count() } else { 0 };
        let shown = path.to_string();
        let copy = path.to_string();
        let reveal = path.to_string();
        let open_in = path.to_string();
        let row = div().flex().items_center().gap_1().px_2().py_1()
            .child(div().flex_1().min_w_0().truncate().mx_1().text_size(px(11.0)).text_color(c.muted).child(shown));
        let row = if editing {
            row.when(self.conflict, |d| d.child(div().text_xs().text_color(c.warning).child("Changed on disk")))
                .child(
                    div().id("viewer-save").rounded_md().bg(c.accent).text_color(c.on_accent).px_2().py_1().text_xs().cursor_pointer()
                        .on_click(cx.listener(|this, _, _, cx| { let overwrite = this.conflict; this.save_draft(overwrite, cx); }))
                        .child(if self.conflict { "Overwrite" } else { "Save" }),
                )
                .child(
                    div().id("viewer-cancel").rounded_md().px_2().py_1().text_xs().text_color(c.muted).cursor_pointer().hover(move |d| d.bg(c.hover))
                        .on_click(cx.listener(|this, _, window, cx| this.cancel_edit(window, cx)))
                        .child("Cancel"),
                )
        } else {
            row.when(has_source_toggle, |d| d.child(button("viewer-source", IconName::Code, self.source).on_click(cx.listener(|this, _, _, cx| { this.source = !this.source; cx.notify(); }))))
                .when(matches!(self.content, Content::Text { .. }), |d| d.child(button("viewer-edit", IconName::Pencil, false).on_click(cx.listener(|this, _, window, cx| this.begin_edit(window, cx)))))
                .when(is_markdown, |d| d.child(button("viewer-comment", IconName::MessageSquarePlus, false).on_click(cx.listener(|this, _, window, cx| {
                    let last = if let Content::Text { line_count, .. } = &this.content { *line_count } else { 0 };
                    this.begin_comment(last, window, cx);
                }))))
                .when(comments > 0, |d| d.child(button("viewer-send", IconName::Send, self.send_open).on_click(cx.listener(|this, _, _, cx| { this.send_open = !this.send_open; cx.notify(); }))))
                .child(button("viewer-reload", IconName::RotateCw, false).on_click(cx.listener(|this, _, _, cx| this.load(cx))))
        };
        row.child(button("viewer-copy", IconName::Copy, false).on_click(cx.listener(move |_, _, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(copy.clone())))))
            .child(button("viewer-reveal", IconName::FolderOpen, false).on_click(move |_, _, cx| cx.reveal_path(std::path::Path::new(&reveal))))
            .child(button("viewer-open", IconName::ExternalLink, false).on_click(cx.listener(move |this, _, _, cx| {
                let (path, editor) = (open_in.clone(), this.state.read(cx).editor.clone());
                tauri::async_runtime::spawn(async move {
                    let list = serde_json::to_value(winfer_backend::editors::editors_list().await).unwrap_or_default();
                    if let Some(id) = editor.or_else(|| list.as_array()?.first()?["id"].as_str().map(str::to_string)) {
                        let _ = winfer_backend::editors::editor_open(id, path).await;
                    }
                });
                let _ = cx;
            })))
    }
}

impl Render for ViewerPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (theme_id, c, docs, active, workspaces) = {
            let s = self.state.read(cx);
            (s.theme.clone(), theme::colors(&s.theme), s.viewer.docs.clone(), s.viewer.active.clone(), s.workspaces.clone())
        };
        let font = crate::fonts::mono(cx);
        let groups = viewer_projects(&docs, &workspaces);
        let current = groups.iter().find(|g| active.as_ref().is_some_and(|a| g.docs.contains(a))).cloned();
        let mut top = div().flex().flex_col();
        if groups.len() > 1 {
            let mut projects = div().flex().items_center().gap_1().px_2().pt_1();
            for g in &groups {
                let id = g.id.clone();
                let is_current = current.as_ref().is_some_and(|x| x.id == g.id);
                projects = projects.child(
                    div().id(SharedString::from(format!("project-{}", g.id))).rounded_md().px_2().py_0p5().text_size(px(11.0)).cursor_pointer()
                        .text_color(if is_current { c.fg } else { c.muted }).when(is_current, |d| d.bg(c.active))
                        .on_click(cx.listener(move |this, _, _, cx| this.select_project(&id, cx)))
                        .child(g.name.clone()),
                );
            }
            top = top.child(projects);
        }
        let mut tabs = div().flex().items_center().gap_1().px_2().py_1();
        for path in current.as_ref().map_or_else(Vec::new, |g| g.docs.clone()) {
            let is_active = active.as_ref() == Some(&path);
            let label = tab_label(&path, &docs);
            let (select, close, middle) = (path.clone(), path.clone(), path.clone());
            tabs = tabs.child(
                div().id(SharedString::from(format!("doc-{path}"))).flex().items_center().gap_1p5().max_w(px(180.0)).rounded_md().px_2().py_1().text_xs().cursor_pointer()
                    .text_color(if is_active { c.fg } else { c.muted }).when(is_active, |d| d.bg(c.active)).hover(move |d| d.bg(c.hover))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let path = select.clone();
                        this.state.update(cx, |s, cx| { s.viewer.active = Some(path); cx.notify(); });
                    }))
                    // Middle click closes, like any browser or editor.
                    .on_mouse_up(MouseButton::Middle, cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.close(&middle, cx);
                    }))
                    .child(div().flex_1().truncate().child(label))
                    .child(
                        div().id(SharedString::from(format!("doc-close-{path}"))).cursor_pointer()
                            .on_click(cx.listener(move |this, _, _, cx| { cx.stop_propagation(); this.close(&close, cx); }))
                            .child(Icon::new(IconName::X).size(px(11.0)).text_color(c.subtle)),
                    ),
            );
        }
        tabs = tabs.child(div().flex_1()).child(
            div().id("viewer-pick").rounded_sm().p_1().cursor_pointer().hover(move |d| d.bg(c.hover)).on_click(cx.listener(|this, _, _, cx| this.pick_files(cx)))
                .child(Icon::new(IconName::FolderOpen).size(px(13.0)).text_color(c.subtle)),
        );
        top = top.child(tabs);

        let Some(path) = active else {
            return div().size_full().flex().flex_col().child(top).child(div().flex_1().flex().items_center().justify_center().text_xs().text_color(c.subtle).child("No file open")).into_any_element();
        };
        let ext = extension(&path);
        let is_markdown = matches!(ext.as_str(), "md" | "markdown" | "mdx");
        let is_mermaid = matches!(ext.as_str(), "mmd" | "mermaid");
        let header = self.render_header(&path, is_markdown, is_markdown || is_mermaid, c, window, cx);
        let body: AnyElement = match &self.content {
            Content::Loading => div().p_4().text_xs().text_color(c.subtle).child("Loading…").into_any_element(),
            Content::Failed(err) => div().p_4().text_xs().text_color(c.danger).child(err.clone()).into_any_element(),
            Content::Pdf { pages, .. } => self.render_pdf_view(*pages, c, cx),
            Content::Image(p) => div().size_full().p_2().child(img(p.clone()).size_full().object_fit(ObjectFit::Contain)).into_any_element(),
            Content::Text { text, .. } if !self.editing && is_markdown && !self.source => {
                let text = text.clone();
                self.render_markdown(&path, text, &theme_id, c, cx)
            }
            Content::Text { text, .. } if !self.editing && is_mermaid && !self.source => {
                let text = text.clone();
                self.render_mermaid_file(&path, text, &theme_id, c, cx)
            }
            Content::Text { text, modified, line_count, .. } => {
                let (text, modified, line_count) = (text.clone(), *modified, *line_count);
                self.render_code(&path, text, modified, line_count, font.clone(), window, cx)
            }
        };
        let comments = match &self.content {
            Content::Text { comments, .. } if !self.editing && is_markdown => comments.clone(),
            _ => Vec::new(),
        };
        let mut notes = div().flex().flex_col().gap_1();
        for note in &comments {
            let start = note.start;
            notes = notes.child(
                div().flex().items_start().gap_2().rounded_md().bg(c.raised).border_1().border_color(c.edge).px_2().py_1().text_xs()
                    .child(div().text_color(c.accent).child(format!("@{}", note.author)))
                    .child(div().flex_1().min_w_0().text_color(c.fg).child(note.body.clone()))
                    .child(
                        div().id(("note-remove", start)).cursor_pointer().on_click(cx.listener(move |this, _, _, cx| this.remove_comment_at(start, cx)))
                            .child(Icon::new(IconName::X).size(px(11.0)).text_color(c.subtle)),
                    ),
            );
        }
        let composer = self.composing.clone();
        let send_targets: Vec<(usize, String, String)> = if self.send_open {
            let s = self.state.read(cx);
            s.active().map(|w| {
                w.layout.as_ref().map(crate::model::layout::leaves).unwrap_or_default().iter().enumerate().filter_map(|(i, id)| w.panes.iter().find(|p| p.id == *id).map(|p| (i + 1, p.id.clone(), s.provider_name(&p.provider)))).collect()
            }).unwrap_or_default()
        } else {
            Vec::new()
        };
        let count = comments.iter().filter(|n| n.author == "user").count();
        let mut send_list = div().flex().flex_col();
        for (n, id, name) in send_targets {
            send_list = send_list.child(
                div().id(SharedString::from(format!("send-{id}"))).flex().gap_2().rounded_sm().px_2().py_1().text_xs().cursor_pointer().hover(move |d| d.bg(c.hover))
                    .on_click(cx.listener(move |this, _, _, cx| this.send_comments(id.clone(), count, cx)))
                    .child(div().text_color(c.subtle).child(n.to_string()))
                    .child(name),
            );
        }
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(top)
            .child(header)
            .when(self.send_open, |d| d.child(div().p_1().child(div().px_2().text_size(px(10.0)).text_color(c.subtle).child("Ask an agent to address your comments")).child(send_list)))
            .child(div().flex_1().min_h_0().child(body))
            .when(!comments.is_empty() && self.source, |d| d.child(div().id("viewer-notes").max_h(px(180.0)).overflow_y_scroll().p_2().child(notes)))
            .when_some(composer, |d, editor| {
                d.child(
                    div().p_2().flex().flex_col().gap_1()
                        .child(div().text_size(px(11.0)).text_color(c.subtle).child(format!("Comment after line {}", self.comment_line)))
                        .child(div().h(px(72.0)).child(Editor::new(&editor)))
                        .child(
                            div().flex().justify_end().gap_1()
                                .child(div().id("comment-cancel").px_2().py_1().text_xs().text_color(c.muted).cursor_pointer().on_click(cx.listener(|this, _, _, cx| { this.composing = None; cx.notify(); })).child("Cancel"))
                                .child(div().id("comment-add").rounded_md().bg(c.accent).text_color(c.on_accent).px_2().py_1().text_xs().cursor_pointer().on_click(cx.listener(|this, _, _, cx| this.submit_comment(cx))).child("Comment")),
                        ),
                )
            })
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{AlertBlock, AlertKind, find_comments, format_comment, insert_comment, parse_alert, parse_mermaid_fence, remove_comment, tab_label};

    #[test]
    fn parses_github_alerts() {
        assert_eq!(
            parse_alert("> [!NOTE]\n> Useful info."),
            Some(AlertBlock { kind: AlertKind::Note, body: "Useful info.".into() })
        );
        assert_eq!(
            parse_alert("> [!IMPORTANT] Target version\n> compiles `src-tauri/src/`"),
            Some(AlertBlock { kind: AlertKind::Important, body: "Target version\ncompiles `src-tauri/src/`".into() })
        );
        assert_eq!(
            parse_alert("> [!note]\n>\n> - item one\n> - item two"),
            Some(AlertBlock { kind: AlertKind::Note, body: "- item one\n- item two".into() })
        );
        assert_eq!(parse_alert("> [!TIP]\n> Try this").unwrap().kind, AlertKind::Tip);
        assert_eq!(parse_alert("> [!WARNING]\n> Watch out").unwrap().kind, AlertKind::Warning);
        assert_eq!(parse_alert("> [!CAUTION]\n> Danger").unwrap().kind, AlertKind::Caution);
        assert_eq!(parse_alert("> Just a regular quote"), None);
        assert_eq!(parse_alert("> [!UNKNOWN]\n> Nope"), None);
        assert_eq!(parse_alert("> [!NOTE]no-space"), None);
    }

    #[test]
    fn parses_mermaid_fences() {
        assert_eq!(
            parse_mermaid_fence("```mermaid\nflowchart LR\n    A --> B\n```"),
            Some("flowchart LR\n    A --> B".into())
        );
        assert_eq!(
            parse_mermaid_fence("~~~Mermaid\nsequenceDiagram\n    A->>B: hi\n~~~"),
            Some("sequenceDiagram\n    A->>B: hi".into())
        );
        assert_eq!(parse_mermaid_fence("```rust\nfn main() {}\n```"), None);
        assert_eq!(parse_mermaid_fence("```mermaid\nflowchart LR\n    A --> B"), None);
    }

    #[test]
    fn splits_adjacent_alerts_and_fences() {
        let text = "## Review\n> [!IMPORTANT]\n> First\n> [!NOTE]\n> Second\n```mermaid\ngraph TD\nA-->B\n```\nAfter\n";
        let blocks = super::md_blocks(text);
        assert_eq!(blocks.len(), 5);
        assert_eq!(blocks[0].source, "## Review");
        assert_eq!(parse_alert(&blocks[1].source).unwrap().kind, AlertKind::Important);
        assert_eq!(parse_alert(&blocks[2].source).unwrap().kind, AlertKind::Note);
        assert_eq!(parse_mermaid_fence(&blocks[3].source).as_deref(), Some("graph TD\nA-->B"));
        assert_eq!(blocks[4].source, "After");
    }

    #[test]
    fn markdown_splits_into_blocks_and_keeps_fences_whole() {
        let text = "# Title\n\nFirst para\nstill first\n\n```rust\nlet a = 1;\n\nlet b = 2;\n```\n\n- one\n- two\n";
        let blocks = super::md_blocks(text);
        let spans: Vec<(usize, usize)> = blocks.iter().map(|b| (b.start_line, b.end_line)).collect();
        assert_eq!(spans, [(1, 1), (3, 4), (6, 10), (12, 13)]);
        assert!(blocks[2].source.contains("let b = 2;"));
    }

    #[test]
    fn standalone_comments_are_their_own_blocks() {
        let text = super::insert_comment("# T\n\nbody\n", 3, "fix this", "user");
        let blocks = super::md_blocks(&text);
        let note = blocks.iter().find_map(|b| b.comment.clone()).expect("a comment block");
        assert_eq!((note.author.as_str(), note.body.as_str()), ("user", "fix this"));
        assert_eq!(blocks.iter().filter(|b| b.comment.is_none()).count(), 2);
        assert_eq!(super::remove_comment(&text, note.start), "# T\n\nbody\n");
    }

    #[test]
    fn comments_round_trip() {
        let text = "# T\nbody\nmore";
        let with = insert_comment(text, 2, "fix this", "user");
        assert_eq!(with, "# T\nbody\n\n<!-- @user: fix this -->\n\nmore");
        let found = find_comments(&with);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].body, "fix this");
        assert_eq!(remove_comment(&with, found[0].start), "# T\nbody\n\nmore");
    }

    #[test]
    fn comment_bodies_cant_end_the_comment() {
        assert_eq!(format_comment("a --> b", "user"), "<!-- @user: a -- > b -->");
    }

    #[test]
    fn same_named_tabs_show_their_folder() {
        let docs = vec!["/a/x/README.md".to_string(), "/a/y/README.md".to_string(), "/a/z.txt".to_string()];
        assert_eq!(tab_label(&docs[0], &docs), "x/README.md");
        assert_eq!(tab_label(&docs[2], &docs), "z.txt");
    }

    #[test]
    fn language_for_resolves_extensions_and_filenames() {
        assert_eq!(super::language_for("/a/settings.rs"), "rs");
        assert_eq!(super::language_for("/a.b/Makefile"), "make");
        assert_eq!(super::language_for("/a/Dockerfile"), "dockerfile");
        assert_eq!(super::language_for("/a/.zshrc"), "bash");
        assert_eq!(super::language_for("/a.b/README"), "README");
    }
}

#[cfg(test)]
mod link_tests {
    use super::{fix_link_destinations, link_target};

    #[test]
    fn wraps_destinations_with_spaces_and_parens() {
        assert_eq!(fix_link_destinations("[a](file:///V/(D) E/a.rs): x [b](http://x.y)"), "[a](<file:///V/(D) E/a.rs>): x [b](http://x.y)");
        assert_eq!(fix_link_destinations("```\n[a](b c)\n```\n"), "```\n[a](b c)\n```\n");
    }

    #[test]
    fn resolves_links() {
        // As paths, so Windows' joined `\` compares equal to `/`.
        let link = |href: &str| link_target(href, "/d/a.md").map(|(p, line)| (std::path::PathBuf::from(p), line));
        let path = |p: &str, line: Option<usize>| Some((std::path::PathBuf::from(p), line));
        assert_eq!(link("https://a.dev/x"), None);
        assert_eq!(link("mailto:a@b.c"), None);
        assert_eq!(link("#top"), None);
        assert_eq!(link("b%20c.md#intro"), path("/d/b c.md", None));
        assert_eq!(link("src/x.rs:12"), path("/d/src/x.rs", Some(12)));
        assert_eq!(link("/e/x.rs#L3"), path("/e/x.rs", Some(3)));
        assert_eq!(link("file:///e/x.rs"), path("/e/x.rs", None));
    }
}
