//! Terminal sessions: an `alacritty_terminal` screen fed by the backend's PTY stream, kept
//! outside any view so layout changes and workspace switches never touch the running agent
//! (port of `src/features/terminal/xtermRegistry.ts`).

use crate::backend;
use crate::model::Pane;
use crate::theme;
use alacritty_terminal::event::{Event as TermEvent, EventListener, WindowSize};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line, Point};
use alacritty_terminal::term::{Config, Term};
use alacritty_terminal::vte::ansi::{self, Rgb};
use parking_lot::Mutex;
use regex::Regex;
use serde_json::json;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, mpsc};
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::Manager;
use tauri::ipc::{Channel, InvokeResponseBody};
use winfer_backend::pty;

/// Something a session reports to the app.
pub enum Event {
    Dirty(String),
    Title(String, String),
    /// OSC 9/777 message or bell.
    Notice(String, Option<String>),
    Output(String),
    DevUrl(String, String),
    Clipboard(String),
    FirstOutput(String),
    Spawned { pane_id: String, busy: bool, task: Option<String> },
    SpawnFailed(String),
    Worktree { pane_id: String, info: Option<crate::model::WorktreeInfo> },
    StartingProcess(String),
    Backend(String, serde_json::Value),
    OpenLink { pane_id: String, link: Link, editor: bool },
}

/// Something clickable in a terminal.
#[derive(Clone, Debug, PartialEq)]
pub enum Link {
    Web(String),
    File { path: String, line: Option<usize> },
}

pub type Events = async_channel::Sender<Event>;

#[derive(Clone, Copy)]
pub struct Size {
    pub cols: usize,
    pub rows: usize,
}

impl Dimensions for Size {
    fn total_lines(&self) -> usize {
        self.rows
    }

    fn screen_lines(&self) -> usize {
        self.rows
    }

    fn columns(&self) -> usize {
        self.cols
    }
}

pub struct Proxy {
    pane_id: String,
    events: Events,
    input: mpsc::Sender<String>,
    size: Arc<Mutex<Size>>,
}

impl EventListener for Proxy {
    fn send_event(&self, event: TermEvent) {
        match event {
            TermEvent::Title(title) => drop(self.events.try_send(Event::Title(self.pane_id.clone(), title))),
            TermEvent::Bell => drop(self.events.try_send(Event::Notice(self.pane_id.clone(), None))),
            TermEvent::PtyWrite(text) => drop(self.input.send(text)),
            TermEvent::ClipboardStore(_, text) => drop(self.events.try_send(Event::Clipboard(text))),
            TermEvent::ColorRequest(index, format) => {
                let p = theme::palette();
                let c = match index {
                    257 => p.background,
                    256 | 258 => p.foreground,
                    i if i < 16 => p.ansi[i],
                    _ => return,
                };
                let _ = self.input.send(format(Rgb { r: c[0], g: c[1], b: c[2] }));
            }
            TermEvent::TextAreaSizeRequest(format) => {
                let s = *self.size.lock();
                let _ = self.input.send(format(WindowSize { num_lines: s.rows as u16, num_cols: s.cols as u16, cell_width: 8, cell_height: 16 }));
            }
            _ => {}
        }
    }
}

/// Picks OSC 9 and 777 notifications out of the output (the terminal core ignores them).
#[derive(Default)]
struct OscScanner {
    state: u8,
    buffer: Vec<u8>,
}

impl OscScanner {
    fn feed(&mut self, bytes: &[u8], mut found: impl FnMut(&str)) {
        const MAX: usize = 4096;
        for &b in bytes {
            match self.state {
                0 if b == 0x1b => self.state = 1,
                1 if b == b']' => {
                    self.state = 2;
                    self.buffer.clear();
                }
                1 => self.state = 0,
                2 if b == 0x07 => {
                    self.finish(&mut found);
                    self.state = 0;
                }
                2 if b == 0x1b => self.state = 3,
                2 => {
                    if self.buffer.len() < MAX {
                        self.buffer.push(b);
                    } else {
                        self.state = 0;
                    }
                }
                3 => {
                    if b == b'\\' {
                        self.finish(&mut found);
                    }
                    self.state = 0;
                }
                _ => {}
            }
        }
    }

    fn finish(&self, found: &mut impl FnMut(&str)) {
        let text = String::from_utf8_lossy(&self.buffer);
        let Some((code, data)) = text.split_once(';') else { return };
        match code {
            "9" => found(data),
            "777" => {
                let mut parts = data.splitn(3, ';');
                if parts.next() == Some("notify") {
                    let joined: Vec<&str> = parts.filter(|p| !p.is_empty()).collect();
                    found(&joined.join(": "));
                }
            }
            _ => {}
        }
    }
}

static ANSI: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\x1b\[[0-9;?]*[A-Za-z]").unwrap());
static LOCAL_URL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"https?://(?:localhost|127\.0\.0\.1|0\.0\.0\.0|\[::1\]):\d{2,5}(?:/[^\s"'<>)\]]*)?"#).unwrap());

/// The local dev-server URL in a chunk of output, if any (often colored inside the URL).
fn detect_dev_url(text: &str) -> Option<String> {
    let clean = ANSI.replace_all(text, "");
    let url = LOCAL_URL.find_iter(&clean).last()?;
    Some(url.as_str().replace("0.0.0.0", "localhost"))
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// What a winfer code chat pane plugs into a session: the process speaks JSON lines instead of
/// drawing a screen, and text typed into the pane from elsewhere (send_to_pane, Approve) goes to
/// the chat's composer.
pub struct CodeHooks {
    pub tap: Box<dyn Fn(&[u8]) + Send + Sync>,
    pub input: Box<dyn Fn(&str) + Send + Sync>,
}

pub struct Session {
    pub pane_id: String,
    code: Mutex<Option<Arc<CodeHooks>>>,
    pub term: Mutex<Term<Proxy>>,
    parser: Mutex<ansi::Processor>,
    osc: Mutex<OscScanner>,
    pub size: Arc<Mutex<Size>>,
    input: mpsc::Sender<String>,
    pub spawned: AtomicBool,
    ready: AtomicBool,
    pub exited: AtomicBool,
    last_output: AtomicU64,
    /// A repaint was requested and not yet done.
    dirty: AtomicBool,
    pid: Mutex<Option<Option<u32>>>,
    /// Exits that came before the spawn returned (a restarted pane's old process).
    early_exits: Mutex<Vec<(u32, Option<u32>)>>,
    typed: Mutex<String>,
    events: Events,
    resize_tx: mpsc::Sender<(u16, u16)>,
}

fn is_hover_report(data: &str) -> bool {
    data.strip_prefix("\x1b[<35;").is_some_and(|r| r.ends_with('M') && r[..r.len() - 1].chars().all(|c| c.is_ascii_digit() || c == ';'))
}

fn writer_thread(pane_id: String, rx: mpsc::Receiver<String>) {
    while let Ok(first) = rx.recv() {
        // Programs tracking the mouse get a report per cell crossed; only the last position
        // matters to one that's still catching up.
        let mut pending = vec![first];
        while let Ok(next) = rx.try_recv() {
            if is_hover_report(&next) && pending.last().is_some_and(|p| is_hover_report(p)) {
                pending.pop();
            }
            pending.push(next);
        }
        let app = backend::app();
        let _ = pty::pty_write(app.state(), pane_id.clone(), pending.concat());
    }
}

fn resize_thread(pane_id: String, rx: mpsc::Receiver<(u16, u16)>) {
    while let Ok(mut size) = rx.recv() {
        // Each resize makes a TUI repaint everything: during a drag it only gets the final one.
        while let Ok(next) = rx.recv_timeout(std::time::Duration::from_millis(60)) {
            size = next;
        }
        let _ = pty::pty_resize(backend::app().state(), pane_id.clone(), size.0, size.1);
    }
}

impl Session {
    pub fn new(pane_id: &str, events: Events) -> Arc<Self> {
        let size = Arc::new(Mutex::new(Size { cols: 120, rows: 32 }));
        let (input, input_rx) = mpsc::channel();
        let (resize_tx, resize_rx) = mpsc::channel();
        let config = Config { scrolling_history: 10_000, ..Config::default() };
        let proxy = Proxy { pane_id: pane_id.to_string(), events: events.clone(), input: input.clone(), size: size.clone() };
        let term = Term::new(config, &*size.lock(), proxy);
        let writer_id = pane_id.to_string();
        std::thread::spawn(move || writer_thread(writer_id, input_rx));
        let resize_id = pane_id.to_string();
        std::thread::spawn(move || resize_thread(resize_id, resize_rx));
        Arc::new(Self {
            pane_id: pane_id.to_string(),
            code: Mutex::new(None),
            term: Mutex::new(term),
            parser: Mutex::new(ansi::Processor::new()),
            osc: Mutex::new(OscScanner::default()),
            size,
            input,
            spawned: AtomicBool::new(false),
            ready: AtomicBool::new(false),
            exited: AtomicBool::new(false),
            last_output: AtomicU64::new(0),
            dirty: AtomicBool::new(false),
            pid: Mutex::new(None),
            early_exits: Mutex::new(Vec::new()),
            typed: Mutex::new(String::new()),
            events,
            resize_tx,
        })
    }

    pub fn write_screen(&self, bytes: &[u8]) {
        {
            let mut term = self.term.lock();
            self.parser.lock().advance(&mut *term, bytes);
        }
        self.mark_dirty();
    }

    fn mark_dirty(&self) {
        if !self.dirty.swap(true, Ordering::AcqRel) {
            let _ = self.events.try_send(Event::Dirty(self.pane_id.clone()));
        }
    }

    pub fn painted(&self) {
        self.dirty.store(false, Ordering::Release);
    }

    fn on_output(self: &Arc<Self>, bytes: &[u8]) {
        let first = self.last_output.swap(now_ms(), Ordering::Relaxed) == 0;
        if first {
            let _ = self.events.try_send(Event::FirstOutput(self.pane_id.clone()));
        }
        if let Some(code) = self.code.lock().clone() {
            (code.tap)(bytes);
            return;
        }
        self.write_screen(bytes);
        let _ = self.events.try_send(Event::Output(self.pane_id.clone()));
        let mut notices = Vec::new();
        self.osc.lock().feed(bytes, |text| notices.push(text.to_string()));
        for notice in notices {
            let _ = self.events.try_send(Event::Notice(self.pane_id.clone(), Some(notice)));
        }
        if let Some(url) = detect_dev_url(&String::from_utf8_lossy(bytes)) {
            let _ = self.events.try_send(Event::DevUrl(self.pane_id.clone(), url));
        }
    }

    pub fn output_age_ms(&self) -> u64 {
        now_ms().saturating_sub(self.last_output.load(Ordering::Relaxed))
    }

    /// Input the user typed: tracks the current line, returns it when it's submitted.
    pub fn typed(&self, data: &str) -> Option<String> {
        self.send(data);
        if data.starts_with('\x1b') {
            return None;
        }
        let mut submitted = None;
        let mut typed = self.typed.lock();
        for ch in data.chars() {
            match ch {
                '\r' => submitted = Some(std::mem::take(&mut *typed)),
                '\x7f' => {
                    typed.pop();
                }
                c if c >= ' ' || c == '\n' => typed.push(c),
                _ => {}
            }
        }
        submitted
    }

    /// Makes this a winfer code chat session (before the process starts).
    pub fn attach_code(&self, hooks: CodeHooks) {
        *self.code.lock() = Some(Arc::new(hooks));
    }

    pub fn is_code(&self) -> bool {
        self.code.lock().is_some()
    }

    /// Writes protocol bytes to the process, bypassing the chat's input hook.
    pub fn write_process(&self, data: &str) -> bool {
        if self.ready.load(Ordering::Acquire) && !self.exited.load(Ordering::Relaxed) {
            let _ = self.input.send(data.to_string());
            true
        } else {
            false
        }
    }

    /// Replaces what the screen shows (the chat's transcript as text, for read_pane and summaries).
    pub fn set_screen_text(&self, text: &str) {
        let mut bytes = b"\x1b[2J\x1b[3J\x1b[H".to_vec();
        bytes.extend_from_slice(text.replace('\n', "\r\n").as_bytes());
        self.write_screen(&bytes);
    }

    /// Writes to the process as if typed.
    pub fn send(&self, data: &str) {
        if let Some(code) = self.code.lock().clone() {
            (code.input)(data);
            return;
        }
        if self.spawned.load(Ordering::Relaxed) && !self.exited.load(Ordering::Relaxed) {
            let _ = self.input.send(data.to_string());
        }
    }

    /// Pastes as one block when the program or the pane calls for it.
    pub fn paste(&self, text: &str, provider: &str) {
        let bracketed = self.term.lock().mode().contains(alacritty_terminal::term::TermMode::BRACKETED_PASTE);
        let multi_line = text.trim_end().contains(['\r', '\n']);
        let clean = text.replace("\x1b[200~", "").replace("\x1b[201~", "").replace("\r\n", "\r").replace('\n', "\r");
        if bracketed || (multi_line && provider != "shell") {
            self.send(&format!("\x1b[200~{clean}\x1b[201~"));
        } else {
            self.send(&clean);
        }
    }

    pub fn resize(&self, cols: usize, rows: usize) {
        let new = Size { cols: cols.max(2), rows: rows.max(2) };
        {
            let mut size = self.size.lock();
            if size.cols == new.cols && size.rows == new.rows {
                return;
            }
            *size = new;
        }
        self.term.lock().resize(new);
        if self.spawned.load(Ordering::Relaxed) {
            let _ = self.resize_tx.send((new.cols as u16, new.rows as u16));
        }
    }

    pub fn scroll(&self, lines: i32) {
        self.term.lock().scroll_display(Scroll::Delta(lines));
        self.mark_dirty();
    }

    /// Finds `query` (case-insensitive) in the scrollback and selects the next or previous match,
    /// scrolling it into view: (index, count) of matches. An empty query clears the selection.
    pub fn search(&self, query: &str, forward: bool) -> Option<(usize, usize)> {
        use alacritty_terminal::index::Side;
        use alacritty_terminal::selection::{Selection, SelectionType};
        let mut term = self.term.lock();
        if query.is_empty() {
            term.selection = None;
            drop(term);
            self.mark_dirty();
            return None;
        }
        let needle: Vec<char> = query.to_lowercase().chars().collect();
        let grid = term.grid();
        let (rows, cols) = (grid.screen_lines() as i32, grid.columns());
        let mut matches: Vec<(i32, usize)> = Vec::new();
        for line in -(grid.history_size() as i32)..rows {
            let row = &grid[Line(line)];
            let chars: Vec<char> = (0..cols).map(|c| row[Column(c)].c.to_lowercase().next().unwrap_or(' ')).collect();
            if chars.len() >= needle.len() {
                for start in 0..=chars.len() - needle.len() {
                    if chars[start..start + needle.len()] == needle[..] {
                        matches.push((line, start));
                    }
                }
            }
        }
        if matches.is_empty() {
            term.selection = None;
            drop(term);
            self.mark_dirty();
            return Some((0, 0));
        }
        let current = term.selection.as_ref().and_then(|s| s.to_range(&term)).map(|r| (r.start.line.0, r.start.column.0));
        let index = match current.and_then(|c| matches.iter().position(|m| *m == c)) {
            Some(i) if forward => (i + 1) % matches.len(),
            Some(i) => (i + matches.len() - 1) % matches.len(),
            // A new search starts at the latest output.
            None => matches.len() - 1,
        };
        let (line, col) = matches[index];
        let mut selection = Selection::new(SelectionType::Simple, Point::new(Line(line), Column(col)), Side::Left);
        selection.update(Point::new(Line(line), Column(col + needle.len() - 1)), Side::Right);
        term.selection = Some(selection);
        // Scrolled so the match sits in view, about mid-screen when it was off screen.
        let offset = term.grid().display_offset() as i32;
        if line < -offset || line >= rows - offset {
            let target = (rows / 2 - line).clamp(0, term.grid().history_size() as i32);
            term.scroll_display(Scroll::Delta(target - offset));
        }
        drop(term);
        self.mark_dirty();
        Some((index, matches.len()))
    }

    pub fn write_note(&self, text: &str) {
        self.write_screen(text.as_bytes());
    }

    /// Last `lines` lines of screen and scrollback as plain text.
    pub fn read_text(&self, lines: usize) -> String {
        let term = self.term.lock();
        let grid = term.grid();
        let top = -(grid.history_size() as i32);
        let text_of = |line: i32| {
            let row = &grid[Line(line)];
            let mut text: String = (0..grid.columns()).map(|c| row[Column(c)].c).collect();
            text.truncate(text.trim_end().len());
            text
        };
        // Blank rows below the last output don't count, so a short screen still reads.
        let mut bottom = grid.screen_lines() as i32;
        while bottom > top && text_of(bottom - 1).is_empty() {
            bottom -= 1;
        }
        let start = (bottom - lines as i32).max(top);
        (start..bottom).map(text_of).collect::<Vec<_>>().join("\n")
    }

    /// The logical line at a viewport row (wrapped rows joined) and the column's char index in it.
    pub fn logical_line(&self, row: usize, col: usize) -> (String, usize) {
        use alacritty_terminal::term::cell::Flags;
        let term = self.term.lock();
        let grid = term.grid();
        let offset = grid.display_offset() as i32;
        let columns = grid.columns();
        let wrapped = |line: i32| grid[Line(line)][Column(columns - 1)].flags.contains(Flags::WRAPLINE);
        let top_limit = -(grid.history_size() as i32);
        let bottom_limit = grid.screen_lines() as i32 - 1;
        let target = row as i32 - offset;
        let mut top = target;
        while top > top_limit && wrapped(top - 1) {
            top -= 1;
        }
        let mut bottom = target;
        while bottom < bottom_limit && wrapped(bottom) {
            bottom += 1;
        }
        let mut text = String::new();
        let mut index = 0;
        for line in top..=bottom {
            for c in 0..columns {
                let cell = &grid[Line(line)][Column(c)];
                if line == target && c == col {
                    index = text.chars().count();
                }
                if !cell.flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER) {
                    text.push(cell.c);
                }
            }
        }
        (text, index)
    }

    /// An OSC 8 hyperlink at a viewport cell.
    pub fn hyperlink_at(&self, row: usize, col: usize) -> Option<String> {
        let term = self.term.lock();
        let grid = term.grid();
        let offset = grid.display_offset() as i32;
        grid[Line(row as i32 - offset)][Column(col)].hyperlink().map(|h| h.uri().to_string())
    }

    /// `true` if this exit belongs to the running process (see `earlyExits` in the original).
    fn claim_exit(&self, code: u32, pid: Option<u32>) -> bool {
        let known = *self.pid.lock();
        let Some(current) = known else {
            self.early_exits.lock().push((code, pid));
            return false;
        };
        if pid.is_some() && current.is_some() && pid != current {
            return false;
        }
        self.mark_exited(code);
        true
    }

    fn mark_exited(&self, code: u32) {
        self.exited.store(true, Ordering::Relaxed);
        self.write_note(&format!("\r\n\x1b[2m[process exited with code {code}]\x1b[0m\r\n"));
    }
}

static WEB_URL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"(?i)\b[a-z][\w+.-]*://[^\s"'<>]+"#).unwrap());

/// A file URI's path, if `uri` is one (`file://…`, `vscode://file/…`).
pub fn file_from_uri(uri: &str) -> Option<(String, Option<usize>)> {
    let rest = uri.strip_prefix("file://").or_else(|| uri.strip_prefix("vscode://file"))?;
    let rest = rest.strip_prefix("localhost").unwrap_or(rest);
    let decoded = url::Url::parse(&format!("file://{rest}")).ok().and_then(|u| u.to_file_path().ok()).map_or(rest.to_string(), |p| p.display().to_string());
    Some(split_position(&decoded))
}

/// Takes `:line`, `:line:col`, `:from-to` or `#L12` off the end of a path.
fn split_position(token: &str) -> (String, Option<usize>) {
    static POS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(.*?)(?::|#L)(\d+)(?:[:-]L?\d+)?$").unwrap());
    match POS.captures(token) {
        Some(c) => (c[1].to_string(), c[2].parse().ok()),
        None => (token.to_string(), None),
    }
}

/// The URL or file-looking token at char `index` of a line.
pub fn link_at(text: &str, index: usize) -> Option<Link> {
    let chars: Vec<char> = text.chars().collect();
    if index >= chars.len() {
        return None;
    }
    for m in WEB_URL.find_iter(text) {
        let start = text[..m.start()].chars().count();
        let end = start + m.as_str().chars().count();
        if (start..end).contains(&index) {
            let url = m.as_str().trim_end_matches(['.', ',', ';', ':', ')', ']', '}']);
            return Some(Link::Web(url.to_string()));
        }
    }
    let path_char = |c: char| c.is_alphanumeric() || "_.@+-/\\~:#".contains(c);
    if !path_char(chars[index]) {
        return None;
    }
    let mut start = index;
    while start > 0 && path_char(chars[start - 1]) {
        start -= 1;
    }
    let mut end = index + 1;
    while end < chars.len() && path_char(chars[end]) {
        end += 1;
    }
    let token: String = chars[start..end].iter().collect();
    let token = token.trim_end_matches(['.', ',', ':', ';']).trim_start_matches(':');
    let (path, line) = split_position(token);
    let name = path.rsplit(['/', '\\']).next().unwrap_or(&path);
    let has_extension = name.rsplit_once('.').is_some_and(|(_, e)| (1..=12).contains(&e.len()) && e.chars().any(char::is_alphabetic) && e.chars().all(char::is_alphanumeric));
    let known = ["Makefile", "Dockerfile", "LICENSE", "README", "Gemfile", "Justfile"].contains(&name);
    let dotfile = name.starts_with('.') && name.len() > 1;
    (has_extension || known || dotfile || path.contains('/')).then_some(Link::File { path, line })
}

/// Every live session, by pane id.
#[derive(Default)]
pub struct Sessions {
    map: Mutex<HashMap<String, Arc<Session>>>,
}

pub static SESSIONS: LazyLock<Sessions> = LazyLock::new(Sessions::default);

impl Sessions {
    pub fn get(&self, id: &str) -> Option<Arc<Session>> {
        self.map.lock().get(id).cloned()
    }

    pub fn get_or_create(&self, id: &str, events: &Events) -> Arc<Session> {
        self.map.lock().entry(id.to_string()).or_insert_with(|| Session::new(id, events.clone())).clone()
    }

    pub fn remove(&self, id: &str) -> Option<Arc<Session>> {
        self.map.lock().remove(id)
    }

}

/// A `pty://exit` event: `true` if it ended the pane's current process.
pub fn on_exit(pane_id: &str, code: u32, pid: Option<u32>) -> bool {
    SESSIONS.get(pane_id).is_some_and(|s| s.claim_exit(code, pid))
}

/// What `pty_spawn` needs to know, from the app's state at the moment the pane starts.
pub struct SpawnSettings {
    pub auto_approve: bool,
    pub system_prompt: String,
    pub lean: bool,
    pub hidden_tools: Vec<String>,
}

impl SpawnSettings {
    pub fn of(state: &crate::model::AppState, pane: &Pane) -> Self {
        Self {
            auto_approve: state.auto_approve.contains(&pane.provider),
            system_prompt: state.system_prompt_of(),
            // Codex CLI panes follow Settings → WinferCode → Lean setup → Codex.
            lean: pane.provider == "codex" && winfer_backend::harness::config::Config::load().lean_codex,
            hidden_tools: state.hidden_tools(),
        }
    }
}

/// Gives an agent pane its worktree, then starts its process. Runs off the UI thread.
pub fn spawn(session: Arc<Session>, pane: Pane, settings: SpawnSettings, events: Events) {
    if session.spawned.swap(true, Ordering::AcqRel) {
        return;
    }
    tauri::async_runtime::spawn(async move {
        let id = pane.id.clone();
        let mut pane = pane;
        if pane.wants_worktree && pane.worktree.is_none() {
            let required = pane.race_id.is_some();
            let created = winfer_backend::worktree::worktree_create(backend::app().clone(), pane.cwd.clone(), pane.provider.clone()).await;
            let info = created.and_then(|v| serde_json::to_value(v).map_err(|e| e.to_string())).map(|v| serde_json::from_value::<Option<crate::model::WorktreeInfo>>(v).unwrap_or(None));
            match info {
                Ok(Some(info)) => {
                    crate::telemetry::track("worktree_created", serde_json::json!({ "provider": crate::telemetry::provider_label(&pane.provider) }));
                    session.write_note(&format!("\x1b[2mworktree {} (from {}) at {}\x1b[0m\r\n", info.branch, info.base, info.path));
                    pane.cwd = info.path.clone();
                    let _ = events.try_send(Event::Worktree { pane_id: id.clone(), info: Some(info) });
                }
                Ok(None) => {
                    let _ = events.try_send(Event::Worktree { pane_id: id.clone(), info: None });
                    if required {
                        session.write_note(&format!("\x1b[31m{} isn't a git repository. Races and A/B tests need a worktree per agent, so this agent wasn't started.\x1b[0m\r\n", pane.cwd));
                        session.exited.store(true, Ordering::Relaxed);
                        let _ = events.try_send(Event::SpawnFailed(id));
                        return;
                    }
                }
                Err(err) => {
                    let _ = events.try_send(Event::Worktree { pane_id: id.clone(), info: None });
                    if required {
                        session.write_note(&format!("\x1b[31mCouldn't create a worktree, so this agent wasn't started (races and A/B tests need one per agent): {err}\x1b[0m\r\n"));
                        session.exited.store(true, Ordering::Relaxed);
                        let _ = events.try_send(Event::SpawnFailed(id));
                        return;
                    }
                    session.write_note(&format!("\x1b[33mCouldn't create a worktree, starting in {}: {err}\x1b[0m\r\n", pane.cwd));
                }
            }
        }
        let _ = events.try_send(Event::StartingProcess(id.clone()));
        let size = *session.size.lock();
        let request = json!({
            "paneId": pane.id, "providerId": pane.provider, "cwd": pane.cwd, "workspaceId": pane.workspace_id,
            "resume": pane.resume, "session": if pane.resume { pane.session_id.clone() } else { None },
            // Pasted images go through the session itself (see CodeModel::new), not the start prompt.
            "prompt": if pane.attachments.is_empty() { pane.prompt.clone() } else { None }, "model": pane.model, "effort": pane.effort, "autoApprove": settings.auto_approve,
            "systemPrompt": settings.system_prompt, "plain": pane.plain, "lean": settings.lean,
            "hiddenTools": settings.hidden_tools, "account": pane.account_id, "cols": size.cols, "rows": size.rows,
            "serve": session.is_code(),
        });
        let request = match serde_json::from_value(request) {
            Ok(r) => r,
            Err(err) => {
                session.write_note(&format!("\x1b[31mFailed to start {}: {err}\x1b[0m\r\n", pane.provider));
                let _ = events.try_send(Event::SpawnFailed(id));
                return;
            }
        };
        let output = session.clone();
        let channel: Channel = Channel::new(move |body| {
            if let InvokeResponseBody::Raw(bytes) = body {
                output.on_output(&bytes);
            }
            Ok(())
        });
        match pty::pty_spawn(backend::app().clone(), request, channel).await {
            Ok(result) => {
                session.ready.store(true, Ordering::Release);
                let pid = serde_json::to_value(&result).ok().and_then(|v| v["pid"].as_u64()).map(|p| p as u32);
                *session.pid.lock() = Some(pid);
                let busy = pane.provider != "shell" && !pane.resume && pane.attachments.is_empty() && pane.prompt.as_ref().is_some_and(|p| !p.trim().is_empty());
                let task = if busy { pane.prompt.clone() } else { None };
                let _ = events.try_send(Event::Spawned { pane_id: id, busy, task });
                let early: Vec<_> = std::mem::take(&mut *session.early_exits.lock());
                for (code, exit_pid) in early {
                    session.claim_exit(code, exit_pid);
                }
            }
            Err(err) => {
                *session.pid.lock() = Some(None);
                session.exited.store(true, Ordering::Relaxed);
                session.write_note(&format!("\x1b[31mFailed to start {}: {err}\x1b[0m\r\n", pane.provider));
                let _ = events.try_send(Event::SpawnFailed(id));
            }
        }
    });
}

/// Kills the process and forgets the session.
pub fn close(pane_id: &str) {
    SESSIONS.remove(pane_id);
    let id = pane_id.to_string();
    backend::background(move || pty::pty_kill(backend::app().state(), id));
}

/// A point on the visible screen, in the terminal's coordinates.
pub fn viewport_point(display_offset: usize, line: usize, col: usize) -> Point {
    Point::new(Line(line as i32 - display_offset as i32), Column(col))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_walks_matches_from_the_latest() {
        let (tx, _rx) = async_channel::unbounded();
        let session = Session::new("search-test", tx);
        session.resize(40, 5);
        session.write_screen(b"alpha one\r\nbeta\r\nalpha two\r\n");
        assert_eq!(session.search("ALPHA", true), Some((1, 2)));
        assert_eq!(session.search("alpha", true), Some((0, 2)));
        assert_eq!(session.search("alpha", false), Some((1, 2)));
        assert_eq!(session.search("zzz", true), Some((0, 0)));
        assert_eq!(session.search("", true), None);
    }

    #[test]
    fn finds_notifications_across_chunks() {
        let mut scanner = OscScanner::default();
        let mut found = Vec::new();
        scanner.feed(b"x\x1b]9;hel", |t| found.push(t.to_string()));
        scanner.feed(b"lo\x07y\x1b]777;notify;Title;Body\x1b\\", |t| found.push(t.to_string()));
        scanner.feed(b"\x1b]0;a title\x07", |t| found.push(t.to_string()));
        assert_eq!(found, ["hello", "Title: Body"]);
    }

    #[test]
    fn detects_colored_dev_urls() {
        let url = detect_dev_url("  Local: \x1b[36mhttp://localhost:\x1b[1m5173\x1b[22m/\x1b[39m\n");
        assert_eq!(url.as_deref(), Some("http://localhost:5173/"));
        assert_eq!(detect_dev_url("http://0.0.0.0:3000").as_deref(), Some("http://localhost:3000"));
        assert_eq!(detect_dev_url("https://example.com:8080"), None);
    }

    #[test]
    fn links_are_found_under_the_pointer() {
        let line = "Read(src/app.ts:12) see https://example.com/a. done";
        assert_eq!(link_at(line, 7), Some(Link::File { path: "src/app.ts".to_string(), line: Some(12) }));
        assert_eq!(link_at(line, 30), Some(Link::Web("https://example.com/a".to_string())));
        assert_eq!(link_at(line, 48), None);
    }

    #[test]
    fn file_uris_parse() {
        assert_eq!(file_from_uri("file:///a/b.md"), Some(("/a/b.md".to_string(), None)));
        assert_eq!(file_from_uri("vscode://file/a/b.md:7:2"), Some(("/a/b.md".to_string(), Some(7))));
    }

    #[test]
    fn hover_reports_are_recognized() {
        assert!(is_hover_report("\x1b[<35;10;20M"));
        assert!(!is_hover_report("\x1b[<0;10;20M"));
    }

    #[test]
    fn write_process_waits_until_session_is_ready() {
        let (tx, _rx) = async_channel::unbounded();
        let session = Session::new("ready-test", tx);
        session.spawned.store(true, Ordering::Release);
        assert!(!session.write_process("{\"type\":\"set_mode\",\"mode\":\"yolo\"}\n"));
        session.ready.store(true, Ordering::Release);
        assert!(session.write_process("{\"type\":\"set_mode\",\"mode\":\"yolo\"}\n"));
        session.exited.store(true, Ordering::Release);
        assert!(!session.write_process("{\"type\":\"set_mode\",\"mode\":\"ask\"}\n"));
    }
}
