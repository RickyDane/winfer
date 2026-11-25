//! Paints a terminal session and feeds it the keyboard and mouse.

use crate::model::AppState;
use crate::terminal::{self, Event as SessionEvent, Events, Session, SpawnSettings};
use crate::theme::{self, Palette};
use alacritty_terminal::index::{Column, Line, Point as TermPoint, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::TermMode;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::vte::ansi::{Color, CursorShape, NamedColor};
use gpui_kit::*;
use std::ops::Range;
use std::sync::Arc;

// Tab/Shift-Tab are bound to focus cycling in gpui's "Root" context, which beats key-down listeners,
// so the terminal claims them with its own bindings.
actions!(terminal, [Copy, Paste, SendTab, SendBackTab]);

pub struct TerminalView {
    pub session: Arc<Session>,
    state: Entity<AppState>,
    events: Events,
    pub focus: FocusHandle,
    cell: Size<Pixels>,
    origin: Point<Pixels>,
    selecting: bool,
    marked: Option<String>,
    was_focused: bool,
    /// For shells that aren't panes in the state (the side terminal).
    pub own_pane: Option<crate::model::Pane>,
    press: Option<(usize, usize)>,
    /// The account a new Claude Code or Codex process starts on.
    pub start_account: Option<String>,
}

struct Segment {
    col: usize,
    text: String,
    fg: Hsla,
    bold: bool,
    italic: bool,
    underline: bool,
    strike: bool,
}

struct Band {
    line: usize,
    col: usize,
    cols: usize,
    color: Hsla,
}

struct Frame {
    lines: Vec<(usize, Vec<Segment>)>,
    bands: Vec<Band>,
    cursor: Option<(usize, usize, CursorShape, Hsla)>,
    cell: Size<Pixels>,
    font_size: Pixels,
    family: SharedString,
}

fn to_hsla(c: [u8; 3]) -> Hsla {
    rgb(((c[0] as u32) << 16) | ((c[1] as u32) << 8) | c[2] as u32).into()
}

fn cube(i: u8) -> [u8; 3] {
    if i >= 232 {
        let v = 8 + (i - 232) * 10;
        return [v, v, v];
    }
    let i = i - 16;
    let level = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
    [level(i / 36), level((i / 6) % 6), level(i % 6)]
}

fn dim(c: [u8; 3]) -> [u8; 3] {
    c.map(|v| (v as f32 * 0.66) as u8)
}

fn resolve(color: Color, overrides: &alacritty_terminal::term::color::Colors, p: &Palette) -> [u8; 3] {
    let lookup = |index: usize| overrides[index].map(|c| [c.r, c.g, c.b]);
    match color {
        Color::Spec(c) => [c.r, c.g, c.b],
        Color::Indexed(i) if i < 16 => lookup(i as usize).unwrap_or(p.ansi[i as usize]),
        Color::Indexed(i) => lookup(i as usize).unwrap_or_else(|| cube(i)),
        Color::Named(n) => {
            let index = n as usize;
            if let Some(c) = lookup(index) {
                return c;
            }
            match n {
                NamedColor::Foreground | NamedColor::BrightForeground | NamedColor::Cursor => p.foreground,
                NamedColor::Background => p.background,
                NamedColor::DimForeground => dim(p.foreground),
                _ if index < 16 => p.ansi[index],
                // DimBlack..DimWhite follow the named colors in order.
                _ => dim(p.ansi[index.saturating_sub(259) & 7]),
            }
        }
    }
}

fn luminance(c: [u8; 3]) -> f32 {
    let f = |v: u8| {
        let s = v as f32 / 255.0;
        if s <= 0.03928 { s / 12.92 } else { ((s + 0.055) / 1.055).powf(2.4) }
    };
    0.2126 * f(c[0]) + 0.7152 * f(c[1]) + 0.0722 * f(c[2])
}

fn contrast(a: [u8; 3], b: [u8; 3]) -> f32 {
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// xterm's `minimumContrastRatio`: moves the color toward black or white until it reads.
fn ensure_contrast(fg: [u8; 3], bg: [u8; 3], ratio: f32) -> [u8; 3] {
    if contrast(fg, bg) >= ratio {
        return fg;
    }
    let target = if luminance(bg) > 0.5 { 0.0 } else { 255.0 };
    let mut c = fg;
    for step in 1..=20 {
        let t = step as f32 / 20.0;
        c = std::array::from_fn(|i| (fg[i] as f32 + (target - fg[i] as f32) * t) as u8);
        if contrast(c, bg) >= ratio {
            break;
        }
    }
    c
}

fn snapshot(session: &Session, palette: &Palette, accent: Hsla, focused: bool, cell: Size<Pixels>, font_size: Pixels, family: SharedString) -> Frame {
    let term = session.term.lock();
    let content = term.renderable_content();
    let offset = content.display_offset as i32;
    let colors = content.colors;
    let mut lines: Vec<(usize, Vec<Segment>)> = Vec::new();
    let mut bands: Vec<Band> = Vec::new();
    let selection_color = Hsla { a: 0.35, ..accent };
    let selection = content.selection;
    for indexed in content.display_iter {
        let line = (indexed.point.line.0 + offset) as usize;
        let col = indexed.point.column.0;
        let c = indexed.cell;
        if c.flags.contains(Flags::WIDE_CHAR_SPACER) || c.flags.contains(Flags::LEADING_WIDE_CHAR_SPACER) {
            continue;
        }
        let (mut fg, mut bg) = (resolve(c.fg, colors, palette), resolve(c.bg, colors, palette));
        let is_default_bg = !c.flags.contains(Flags::INVERSE) && matches!(c.bg, Color::Named(NamedColor::Background));
        if c.flags.contains(Flags::INVERSE) {
            std::mem::swap(&mut fg, &mut bg);
        }
        if c.flags.contains(Flags::DIM) {
            fg = dim(fg);
        }
        if palette.light {
            fg = ensure_contrast(fg, bg, 4.5);
        }
        let width = if c.flags.contains(Flags::WIDE_CHAR) { 2 } else { 1 };
        let band = |color: Hsla, bands: &mut Vec<Band>| match bands.last_mut() {
            Some(b) if b.line == line && b.col + b.cols == col && b.color == color => b.cols += width,
            _ => bands.push(Band { line, col, cols: width, color }),
        };
        if !is_default_bg {
            band(to_hsla(bg), &mut bands);
        }
        if selection.is_some_and(|s| s.contains(indexed.point)) {
            band(selection_color, &mut bands);
        }
        if c.c == ' ' && !c.flags.intersects(Flags::UNDERLINE | Flags::STRIKEOUT) && c.zerowidth().is_none() {
            continue;
        }
        if c.flags.contains(Flags::HIDDEN) {
            continue;
        }
        if lines.last().is_none_or(|(l, _)| *l != line) {
            lines.push((line, Vec::new()));
        }
        let segments = &mut lines.last_mut().unwrap().1;
        let color = to_hsla(fg);
        let (bold, italic) = (c.flags.contains(Flags::BOLD), c.flags.contains(Flags::ITALIC));
        let (underline, strike) = (c.flags.intersects(Flags::UNDERLINE | Flags::DOUBLE_UNDERLINE | Flags::UNDERCURL), c.flags.contains(Flags::STRIKEOUT));
        // Fallback fonts have their own advances: anything that isn't plain ASCII or box drawing
        // starts a new segment, so it (and what follows) lands on the cell grid.
        let on_grid = |ch: char| ch.is_ascii() || ('\u{2500}'..='\u{259f}').contains(&ch);
        let extendable = segments.last().is_some_and(|s: &Segment| {
            s.col + s.text.chars().count() == col
                && s.fg == color
                && s.bold == bold
                && s.italic == italic
                && s.underline == underline
                && s.strike == strike
                && on_grid(c.c)
                && s.text.chars().all(on_grid)
        });
        if extendable {
            segments.last_mut().unwrap().text.push(c.c);
        } else {
            segments.push(Segment { col, text: c.c.to_string(), fg: color, bold, italic, underline, strike });
        }
        if let Some(zero) = c.zerowidth() {
            segments.last_mut().unwrap().text.extend(zero);
        }
    }
    let cursor = (content.cursor.shape != CursorShape::Hidden).then(|| {
        let shape = if focused { content.cursor.shape } else { CursorShape::HollowBlock };
        let color = colors[NamedColor::Cursor as usize].map_or(palette.foreground, |c| [c.r, c.g, c.b]);
        ((content.cursor.point.line.0 + offset) as usize, content.cursor.point.column.0, shape, to_hsla(color))
    });
    Frame { lines, bands, cursor, cell, font_size, family }
}

fn paint_frame(frame: Frame, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App) {
    let at = |line: usize, col: usize| point(bounds.origin.x + frame.cell.width * col as f32, bounds.origin.y + frame.cell.height * line as f32);
    for band in &frame.bands {
        window.paint_quad(fill(Bounds::new(at(band.line, band.col), size(frame.cell.width * band.cols as f32, frame.cell.height)), band.color));
    }
    for (line, segments) in &frame.lines {
        for s in segments {
            let mut base = font(frame.family.clone());
            if s.bold {
                base = base.bold();
            }
            if s.italic {
                base = base.italic();
            }
            let run = TextRun {
                len: s.text.len(),
                font: base,
                color: s.fg,
                background_color: None,
                underline: s.underline.then(|| UnderlineStyle { thickness: px(1.0), color: Some(s.fg), wavy: false }),
                strikethrough: s.strike.then(|| StrikethroughStyle { thickness: px(1.0), color: Some(s.fg) }),
            };
            let shaped = window.text_system().shape_line(s.text.clone().into(), frame.font_size, &[run], None);
            let _ = shaped.paint(at(*line, s.col), frame.cell.height, TextAlign::Left, None, window, cx);
        }
    }
    if let Some((line, col, shape, color)) = frame.cursor {
        let origin = at(line, col);
        match shape {
            CursorShape::Block => window.paint_quad(fill(Bounds::new(origin, frame.cell), Hsla { a: 0.75, ..color })),
            CursorShape::Beam => window.paint_quad(fill(Bounds::new(origin, size(px(2.0), frame.cell.height)), color)),
            CursorShape::Underline => window.paint_quad(fill(
                Bounds::new(point(origin.x, origin.y + frame.cell.height - px(2.0)), size(frame.cell.width, px(2.0))),
                color,
            )),
            CursorShape::HollowBlock => window.paint_quad(outline(Bounds::new(origin, frame.cell), color, BorderStyle::Solid)),
            CursorShape::Hidden => {}
        }
    }
}

/// Bytes for a keystroke that isn't plain text; `None` leaves it to the text input path.
fn key_bytes(k: &Keystroke, mode: TermMode) -> Option<String> {
    let m = k.modifiers;
    let app_cursor = mode.contains(TermMode::APP_CURSOR);
    let modifier = 1 + m.shift as u8 + (m.alt as u8) * 2 + (m.control as u8) * 4;
    let csi = |code: &str, letter: char| {
        if modifier > 1 { format!("\x1b[{code};{modifier}{letter}") } else { format!("\x1b[{code}{letter}") }
    };
    let arrow = |letter: char| {
        if modifier > 1 {
            format!("\x1b[1;{modifier}{letter}")
        } else if app_cursor {
            format!("\x1bO{letter}")
        } else {
            format!("\x1b[{letter}")
        }
    };
    let tilde = |code: u8| if modifier > 1 { format!("\x1b[{code};{modifier}~") } else { format!("\x1b[{code}~") };
    let alt_prefix = |s: &str| if m.alt { format!("\x1b{s}") } else { s.to_string() };
    Some(match k.key.as_str() {
        "enter" => alt_prefix("\r"),
        "tab" if m.shift => "\x1b[Z".into(),
        "tab" => alt_prefix("\t"),
        "backspace" if m.control => "\x08".into(),
        "backspace" => alt_prefix("\x7f"),
        "escape" => alt_prefix("\x1b"),
        "up" => arrow('A'),
        "down" => arrow('B'),
        "right" => arrow('C'),
        "left" => arrow('D'),
        "home" => arrow('H'),
        "end" => arrow('F'),
        "pageup" => tilde(5),
        "pagedown" => tilde(6),
        "delete" => tilde(3),
        "insert" => tilde(2),
        "f1" => csi("1", 'P').replace("[1P", "OP"),
        "f2" => csi("1", 'Q').replace("[1Q", "OQ"),
        "f3" => csi("1", 'R').replace("[1R", "OR"),
        "f4" => csi("1", 'S').replace("[1S", "OS"),
        "f5" => tilde(15),
        "f6" => tilde(17),
        "f7" => tilde(18),
        "f8" => tilde(19),
        "f9" => tilde(20),
        "f10" => tilde(21),
        "f11" => tilde(23),
        "f12" => tilde(24),
        key if m.control && !m.platform => {
            let c = key.chars().next().filter(|_| key.chars().count() == 1)?;
            let code = match c {
                'a'..='z' => c as u8 - b'a' + 1,
                '@' | ' ' | '2' => 0,
                '[' | '3' => 0x1b,
                '\\' | '4' => 0x1c,
                ']' | '5' => 0x1d,
                '^' | '6' => 0x1e,
                '_' | '7' | '-' => 0x1f,
                '8' => 0x7f,
                _ => return None,
            };
            alt_prefix(&(code as char).to_string())
        }
        // Option is Meta, except for the characters a layout types with it (Option+L is "@" on
        // a German Mac): those fall through to text input.
        key if m.alt && !m.platform => {
            let typed = k.key_char.as_deref().filter(|t| t.chars().count() == 1 && t.chars().all(|c| ('!'..='~').contains(&c)));
            if typed.is_some_and(|t| t.to_lowercase() != key) {
                return None;
            }
            format!("\x1b{key}")
        }
        _ => return None,
    })
}

fn mouse_report(mode: TermMode, button: u8, col: usize, line: usize, pressed: bool) -> Option<String> {
    if mode.contains(TermMode::SGR_MOUSE) {
        return Some(format!("\x1b[<{button};{};{}{}", col + 1, line + 1, if pressed { 'M' } else { 'm' }));
    }
    let code = if pressed { button } else { 3 };
    if col > 222 || line > 222 {
        return None;
    }
    Some(format!("\x1b[M{}{}{}", (32 + code) as char, (33 + col as u8) as char, (33 + line as u8) as char))
}

impl TerminalView {
    pub fn new(session: Arc<Session>, state: Entity<AppState>, events: Events, cx: &mut Context<Self>) -> Self {
        Self {
            session,
            state,
            events,
            focus: cx.focus_handle(),
            cell: size(px(8.0), px(16.0)),
            origin: Point::default(),
            selecting: false,
            marked: None,
            was_focused: false,
            own_pane: None,
            press: None,
            start_account: None,
        }
    }

    fn input(&self, text: &str, cx: &mut Context<Self>) {
        if let Some(line) = self.session.typed(text).filter(|_| self.own_pane.is_none()) {
            let provider = self.state.read(cx).pane(&self.session.pane_id).map(|p| p.provider.clone());
            if provider.is_some_and(|p| p != "shell") {
                let _ = self.events.try_send(SessionEvent::Output(self.session.pane_id.clone()));
                crate::status::submitted(&self.state, &self.session.pane_id, &line, cx);
            }
        }
    }

    fn scroll_to_bottom(&self) {
        let offset = self.session.term.lock().grid().display_offset();
        if offset != 0 {
            self.session.scroll(-(offset as i32));
        }
    }

    fn key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let mode = *self.session.term.lock().mode();
        if event.keystroke.modifiers.platform {
            return;
        }
        if let Some(bytes) = key_bytes(&event.keystroke, mode) {
            self.scroll_to_bottom();
            self.input(&bytes, cx);
            cx.stop_propagation();
        }
    }

    fn cell_at(&self, position: Point<Pixels>) -> (usize, usize, Side) {
        let relative = position - self.origin;
        let size = *self.session.size.lock();
        let x = (relative.x / self.cell.width).max(0.0);
        let col = (x as usize).min(size.cols - 1);
        let line = ((relative.y / self.cell.height).max(0.0) as usize).min(size.rows - 1);
        let side = if x.fract() < 0.5 { Side::Left } else { Side::Right };
        (line, col, side)
    }

    fn term_point(&self, line: usize, col: usize) -> TermPoint {
        let offset = self.session.term.lock().grid().display_offset();
        terminal::viewport_point(offset, line, col)
    }

    fn reports_mouse(&self, shift: bool) -> Option<TermMode> {
        let mode = *self.session.term.lock().mode();
        (mode.intersects(TermMode::MOUSE_MODE) && !shift).then_some(mode)
    }

    fn mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.focus.focus(window, cx);
        if self.own_pane.is_none() {
            self.state.update(cx, |s, cx| {
                s.focus_pane(&self.session.pane_id);
                cx.notify();
            });
        }
        let (line, col, side) = self.cell_at(event.position);
        if let Some(mode) = self.reports_mouse(event.modifiers.shift) {
            let button = match event.button {
                MouseButton::Left => 0,
                MouseButton::Middle => 1,
                MouseButton::Right => 2,
                _ => return,
            };
            if let Some(report) = mouse_report(mode, button, col, line, true) {
                self.session.send(&report);
            }
            return;
        }
        if event.button != MouseButton::Left {
            return;
        }
        self.press = (event.click_count == 1).then_some((line, col));
        let ty = match event.click_count {
            2 => SelectionType::Semantic,
            3 => SelectionType::Lines,
            _ => SelectionType::Simple,
        };
        let at = self.term_point(line, col);
        self.session.term.lock().selection = Some(Selection::new(ty, at, side));
        self.selecting = true;
        cx.notify();
    }

    fn mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        let (line, col, side) = self.cell_at(event.position);
        if let Some(mode) = self.reports_mouse(event.modifiers.shift) {
            let drag = event.pressed_button.is_some() && mode.intersects(TermMode::MOUSE_DRAG | TermMode::MOUSE_MOTION);
            let hover = event.pressed_button.is_none() && mode.contains(TermMode::MOUSE_MOTION);
            if drag || hover {
                let base = if hover { 3 } else { event.pressed_button.map_or(0, |b| if b == MouseButton::Right { 2 } else { 0 }) };
                if let Some(report) = mouse_report(mode, base + 32, col, line, true) {
                    self.session.send(&report);
                }
            }
            return;
        }
        if self.selecting {
            let at = self.term_point(line, col);
            if let Some(selection) = self.session.term.lock().selection.as_mut() {
                selection.update(at, side);
            }
            cx.notify();
        }
    }

    fn mouse_up(&mut self, event: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.selecting = false;
        let (line, col, _) = self.cell_at(event.position);
        if self.press.take() == Some((line, col)) && event.button == MouseButton::Left && !self.reports_mouse(event.modifiers.shift).is_some() {
            self.session.term.lock().selection = None;
            self.open_link(line, col, event.modifiers.platform || (cfg!(not(target_os = "macos")) && event.modifiers.control));
        }
        if let Some(mode) = self.reports_mouse(event.modifiers.shift) {
            let button = if event.button == MouseButton::Right { 2 } else { 0 };
            if let Some(report) = mouse_report(mode, button, col, line, false) {
                self.session.send(&report);
            }
        }
        cx.notify();
    }

    fn open_link(&self, line: usize, col: usize, editor: bool) {
        let link = self.session.hyperlink_at(line, col).map_or_else(
            || {
                let (text, index) = self.session.logical_line(line, col);
                terminal::link_at(&text, index)
            },
            |uri| {
                Some(match terminal::file_from_uri(&uri) {
                    Some((path, line)) => terminal::Link::File { path, line },
                    None => terminal::Link::Web(uri),
                })
            },
        );
        if let Some(link) = link {
            let _ = self.events.try_send(SessionEvent::OpenLink { pane_id: self.session.pane_id.clone(), link, editor });
        }
    }

    fn scroll(&mut self, event: &ScrollEvent, _: &mut Window, cx: &mut Context<Self>) {
        // Ctrl/⌘ + wheel (and pinch) zooms every terminal.
        if event.modifiers.control || event.modifiers.platform {
            let delta = event.delta.pixel_delta(self.cell.height).y;
            if delta.abs() > px(1.0) {
                self.state.update(cx, |s, cx| {
                    s.set_terminal_font_size(s.terminal_font_size + if delta > px(0.0) { 0.5 } else { -0.5 });
                    cx.notify();
                });
            }
            return;
        }
        let lines = (event.delta.pixel_delta(self.cell.height).y / self.cell.height) as i32;
        if lines == 0 {
            return;
        }
        let mode = *self.session.term.lock().mode();
        let (line, col, _) = self.cell_at(event.position);
        if mode.intersects(TermMode::MOUSE_MODE) {
            let button = if lines > 0 { 64 } else { 65 };
            for _ in 0..lines.abs().min(6) {
                if let Some(report) = mouse_report(mode, button, col, line, true) {
                    self.session.send(&report);
                }
            }
        } else if mode.contains(TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL) {
            let key = if lines > 0 { "A" } else { "B" };
            let intro = if mode.contains(TermMode::APP_CURSOR) { "\x1bO" } else { "\x1b[" };
            self.session.send(&format!("{intro}{key}").repeat(lines.unsigned_abs() as usize));
        } else {
            self.session.scroll(lines);
        }
        cx.notify();
    }

    fn send_tab(&mut self, _: &SendTab, _: &mut Window, cx: &mut Context<Self>) {
        self.scroll_to_bottom();
        self.input("\t", cx);
    }

    fn send_back_tab(&mut self, _: &SendBackTab, _: &mut Window, cx: &mut Context<Self>) {
        self.scroll_to_bottom();
        self.input("\x1b[Z", cx);
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = self.session.term.lock().selection_to_string() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    fn paste_action(&mut self, _: &Paste, _: &mut Window, cx: &mut Context<Self>) {
        let Some(item) = cx.read_from_clipboard() else { return };
        match item.text() {
            Some(text) => {
                let provider = self.state.read(cx).pane(&self.session.pane_id).map(|p| p.provider.clone()).unwrap_or_else(|| "shell".to_string());
                self.scroll_to_bottom();
                self.session.paste(&text, &provider);
            }
            // No text (a screenshot): agents read the clipboard themselves on Ctrl+V.
            None => self.session.send("\x16"),
        }
    }

    fn start_if_needed(&self, cx: &mut App) {
        if self.session.spawned.load(std::sync::atomic::Ordering::Relaxed) {
            return;
        }
        let state = self.state.read(cx);
        let Some(pane) = state.pane(&self.session.pane_id).cloned().or_else(|| self.own_pane.clone()) else { return };
        let settings = SpawnSettings::of(state, &pane);
        let mut pane = pane;
        if let Some(account) = self.start_account.clone().filter(|_| pane.account_id.is_none()) {
            pane.account_id = Some(account);
        }
        let phase = if pane.wants_worktree && pane.worktree.is_none() { "worktree" } else { "process" };
        let id = pane.id.clone();
        self.state.update(cx, |s, cx| {
            s.starting.insert(id, phase);
            cx.notify();
        });
        terminal::spawn(self.session.clone(), pane, settings, self.events.clone());
    }
}

impl Focusable for TerminalView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl EntityInputHandler for TerminalView {
    fn text_for_range(&mut self, _: Range<usize>, _: &mut Option<Range<usize>>, _: &mut Window, _: &mut Context<Self>) -> Option<String> {
        None
    }

    fn selected_text_range(&mut self, _: bool, _: &mut Window, _: &mut Context<Self>) -> Option<UTF16Selection> {
        Some(UTF16Selection { range: 0..0, reversed: false })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked.as_ref().map(|m| 0..m.encode_utf16().count())
    }

    fn unmark_text(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.marked = None;
        cx.notify();
    }

    fn replace_text_in_range(&mut self, _: Option<Range<usize>>, text: &str, _: &mut Window, cx: &mut Context<Self>) {
        self.marked = None;
        self.scroll_to_bottom();
        self.input(text, cx);
        cx.notify();
    }

    fn replace_and_mark_text_in_range(&mut self, _: Option<Range<usize>>, text: &str, _: Option<Range<usize>>, _: &mut Window, cx: &mut Context<Self>) {
        self.marked = (!text.is_empty()).then(|| text.to_string());
        cx.notify();
    }

    fn bounds_for_range(&mut self, _: Range<usize>, bounds: Bounds<Pixels>, _: &mut Window, _: &mut Context<Self>) -> Option<Bounds<Pixels>> {
        let term = self.session.term.lock();
        let cursor = term.grid().cursor.point;
        let offset = term.grid().display_offset() as i32;
        let line = (cursor.line.0 + offset).max(0) as f32;
        Some(Bounds::new(point(bounds.origin.x + self.cell.width * cursor.column.0 as f32, bounds.origin.y + self.cell.height * line), self.cell))
    }

    fn character_index_for_point(&mut self, _: Point<Pixels>, _: &mut Window, _: &mut Context<Self>) -> Option<usize> {
        None
    }
}

impl TerminalView {
    /// Winfer Connect: who sizes this pane, when it isn't this tile.
    fn size_banner(&self, cx: &mut Context<Self>) -> Option<Div> {
        let pane = self.session.pane_id.clone();
        let c = theme::colors(&self.state.read(cx).theme);
        let pill = |text: String| div().absolute().top(px(6.0)).right(px(8.0)).flex().items_center().gap_2().h(px(24.0)).pl_2p5().pr_1().rounded_full()
            .bg(c.raised).border_1().border_color(c.edge).text_size(px(11.5)).text_color(c.muted).shadow_md().child(text);
        let button = |id: &'static str, label: &'static str| div().id(id).h(px(18.0)).px_2().flex().items_center().rounded_full().bg(c.accent).text_color(c.on_accent).cursor_pointer().hover(|d| d.opacity(0.9)).child(label);
        if let Some(peer) = crate::connect_term::sized_by(&pane) {
            let name = crate::connect::peer_name(&peer).unwrap_or_else(|| "another machine".into());
            let session = self.session.clone();
            return Some(pill(format!("Viewed on {name}")).child(button("take-back", "Take back").on_click(cx.listener(move |_, _, _, cx| {
                crate::connect_term::reclaim(&session.pane_id);
                session.write_screen(&[]);
                cx.notify();
            }))));
        }
        let (peer, _) = crate::connect_term::remote_of(&pane)?;
        let name = crate::connect::peer_name(&peer).unwrap_or_else(|| "The other machine".into());
        if !crate::connect::get().is_some_and(|c| c.is_up(&peer)) {
            return Some(pill(format!("{name} is offline · reconnecting…")).pr_2p5());
        }
        if crate::connect_term::is_reclaimed(&pane) {
            let session = self.session.clone();
            return Some(pill(format!("Sized by {name}")).child(button("take-over", "Fit here").on_click(cx.listener(move |_, _, _, cx| {
                crate::connect_term::set_reclaimed(&session.pane_id, false);
                let size = *session.size.lock();
                crate::connect_term::send_resize(&session.pane_id, size.cols as u16, size.rows as u16);
                session.write_screen(&[]);
                cx.notify();
            }))));
        }
        None
    }
}

impl Render for TerminalView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let focused = self.focus.is_focused(window);
        if focused != self.was_focused {
            self.was_focused = focused;
            if self.session.term.lock().mode().contains(TermMode::FOCUS_IN_OUT) {
                self.session.send(if focused { "\x1b[I" } else { "\x1b[O" });
            }
        }
        let state = self.state.read(cx);
        let font_size = px(state.terminal_font_size as f32);
        let accent = theme::colors(&state.theme).accent;
        let family = crate::fonts::mono(cx);
        let view = cx.entity();
        let focus = self.focus.clone();
        let session = self.session.clone();
        self.session.painted();
        let size_banner = self.size_banner(cx);

        div()
            .size_full()
            .relative()
            .children(size_banner)
            .track_focus(&self.focus)
            .key_context("Terminal")
            .on_key_down(cx.listener(Self::key_down))
            .on_action(cx.listener(Self::send_tab))
            .on_action(cx.listener(Self::send_back_tab))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::paste_action))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::mouse_down))
            .on_mouse_down(MouseButton::Right, cx.listener(Self::mouse_down))
            .on_mouse_down(MouseButton::Middle, cx.listener(Self::mouse_down))
            .on_mouse_move(cx.listener(Self::mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::mouse_up))
            .on_mouse_up(MouseButton::Right, cx.listener(Self::mouse_up))
            .on_scroll(cx.listener(Self::scroll))
            .child(
                canvas(
                    {
                        let view = view.clone();
                        move |bounds, window, cx| {
                            let font_id = window.text_system().resolve_font(&font(family.clone()));
                            let width = window.text_system().advance(font_id, font_size, 'm').map_or(px(8.0), |a| a.width).max(px(1.0));
                            let cell = size(width, (font_size * 1.2).round().max(px(1.0)));
                            let cols = (bounds.size.width / cell.width) as usize;
                            let rows = (bounds.size.height / cell.height) as usize;
                            if cols >= 2 && rows >= 2 {
                                session.resize(cols, rows);
                                view.update(cx, |v, cx| {
                                    v.cell = cell;
                                    v.origin = bounds.origin;
                                    v.start_if_needed(cx);
                                });
                            }
                            snapshot(&session, &theme::palette(), accent, focused, cell, font_size, family.clone())
                        }
                    },
                    move |bounds, frame, window, cx| {
                        window.handle_input(&focus, ElementInputHandler::new(bounds, view.clone()), cx);
                        paint_frame(frame, bounds, window, cx);
                    },
                )
                .size_full(),
            )
    }
}


#[allow(unused)]
fn _assert_line_type(_: Line, _: Column) {}

#[cfg(test)]
mod tests {
    use super::{contrast, cube, ensure_contrast, key_bytes};
    use alacritty_terminal::term::TermMode;
    use gpui_kit::{Keystroke, Modifiers};

    fn key(name: &str, f: impl FnOnce(&mut Modifiers)) -> Keystroke {
        let mut modifiers = Modifiers::default();
        f(&mut modifiers);
        Keystroke { modifiers, key: name.to_string(), key_char: None }
    }

    #[test]
    fn arrows_follow_cursor_mode_and_modifiers() {
        assert_eq!(key_bytes(&key("up", |_| {}), TermMode::empty()).unwrap(), "\x1b[A");
        assert_eq!(key_bytes(&key("up", |_| {}), TermMode::APP_CURSOR).unwrap(), "\x1bOA");
        assert_eq!(key_bytes(&key("right", |m| m.alt = true), TermMode::empty()).unwrap(), "\x1b[1;3C");
    }

    #[test]
    fn control_keys() {
        assert_eq!(key_bytes(&key("c", |m| m.control = true), TermMode::empty()).unwrap(), "\x03");
        assert_eq!(key_bytes(&key("[", |m| m.control = true), TermMode::empty()).unwrap(), "\x1b");
        assert_eq!(key_bytes(&key("tab", |m| m.shift = true), TermMode::empty()).unwrap(), "\x1b[Z");
    }

    #[test]
    fn plain_text_uses_the_text_path() {
        assert_eq!(key_bytes(&key("a", |_| {}), TermMode::empty()), None);
    }

    #[test]
    fn option_is_meta_except_for_layout_characters() {
        let us = Keystroke { modifiers: Modifiers { alt: true, ..Default::default() }, key: "b".into(), key_char: Some("∫".into()) };
        assert_eq!(key_bytes(&us, TermMode::empty()).unwrap(), "\x1bb");
        let german = Keystroke { modifiers: Modifiers { alt: true, ..Default::default() }, key: "l".into(), key_char: Some("@".into()) };
        assert_eq!(key_bytes(&german, TermMode::empty()), None);
    }

    #[test]
    fn color_cube() {
        assert_eq!(cube(16), [0, 0, 0]);
        assert_eq!(cube(231), [255, 255, 255]);
        assert_eq!(cube(232), [8, 8, 8]);
    }

    #[test]
    fn contrast_is_enforced() {
        let c = ensure_contrast([200, 200, 200], [255, 255, 255], 4.5);
        assert!(contrast(c, [255, 255, 255]) >= 4.5);
    }
}

#[cfg(test)]
mod typing_tests {
    use super::TerminalView;
    use crate::terminal::{CodeHooks, Session};
    use gpui_kit::{AppContext, TestAppContext};
    use std::sync::{Arc, Mutex};

    #[gpui_kit::test]
    fn typing_reaches_the_shell(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            crate::ui::root::bind_keys(cx);
        });
        let sent = Arc::new(Mutex::new(String::new()));
        let session = Session::new("p1", async_channel::unbounded().0);
        let log = sent.clone();
        session.attach_code(CodeHooks { tap: Box::new(|_| {}), input: Box::new(move |s| log.lock().unwrap().push_str(s)) });
        let state = cx.new(|_| crate::model::AppState::default());
        let (view, vcx) = cx.add_window_view(|_, cx| TerminalView::new(session, state, async_channel::unbounded().0, cx));
        vcx.update(|window, cx| { let f = view.read(cx).focus.clone(); window.focus(&f, cx) });
        vcx.run_until_parked();
        vcx.simulate_input("ls");
        vcx.simulate_keystrokes("tab shift-tab enter");
        assert_eq!(*sent.lock().unwrap(), "ls\t\x1b[Z\r");
    }
}
