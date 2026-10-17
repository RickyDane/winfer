//! Colors and small shared pieces. Minimal: text in the terminal's own colors, greys blended
//! between its foreground and background, and one nebula gradient (blue → violet → pink) for
//! accents. The palette follows the terminal's theme (queried at startup, see `detect`), so
//! every shade stays readable on dark, grey and light backgrounds alike.

#![allow(non_snake_case)]

use std::cell::Cell;
use std::sync::RwLock;

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;

use crate::harness::backend::Kind;

pub type Rgb = (u8, u8, u8);

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Palette {
    pub light: bool,
    pub fg: Color,
    pub muted: Color,
    pub dim: Color,
    pub faint: Color,
    pub void: Color,
    pub blue: Color,
    pub violet: Color,
    pub pink: Color,
    pub green: Color,
    pub red: Color,
    pub yellow: Color,
    pub selection: Color,
    pub diff_add_bg: Color,
    pub diff_del_bg: Color,
    pub claude: Color,
    pub chatgpt: Color,
}

/// The default when the terminal doesn't say: a near-black background.
const DARK_BG: Rgb = (0x0e, 0x0e, 0x13);
const DARK_FG: Rgb = (0xec, 0xec, 0xf2);

static GLOBAL: RwLock<Option<Palette>> = RwLock::new(None);

thread_local! {
    /// Per-thread override, so tests and snapshots can render other themes in parallel.
    static LOCAL: Cell<Option<Palette>> = const { Cell::new(None) };
}

/// Sets the palette from the terminal's background and foreground.
pub fn init(bg: Option<Rgb>, fg: Option<Rgb>) {
    *GLOBAL.write().unwrap() = Some(Palette::derive(bg.unwrap_or(DARK_BG), fg));
}

/// Uses `p` on this thread only (tests).
#[cfg(test)]
pub fn set_local(p: Option<Palette>) {
    LOCAL.with(|l| l.set(p));
}

pub fn pal() -> Palette {
    if let Some(p) = LOCAL.with(Cell::get) {
        return p;
    }
    GLOBAL.read().unwrap().unwrap_or_else(|| Palette::derive(DARK_BG, Some(DARK_FG)))
}

impl Palette {
    /// Builds a palette for a terminal with background `bg` (and foreground `fg`, if known).
    pub fn derive(bg: Rgb, fg: Option<Rgb>) -> Palette {
        let light = lum(bg) > 0.4;
        let ink: Rgb = if light { (0x16, 0x16, 0x1c) } else { (0xf4, 0xf4, 0xf8) };
        // The terminal's own text color, unless it's too weak to read.
        let fg = fg.filter(|&f| contrast(f, bg) >= 4.5).unwrap_or(ink);
        let grey = |t: f32, min: f32| rgb(reach(blend(bg, fg, t), bg, fg, min));
        let accent = |dark: Rgb, lite: Rgb| rgb(reach(if light { lite } else { dark }, bg, ink, 3.2));
        let violet = accent((0xa9, 0x8b, 0xff), (0x6a, 0x45, 0xe0));
        let green = accent((0x5e, 0xe6, 0xc4), (0x0f, 0x80, 0x5e));
        let red = accent((0xff, 0x6b, 0x81), (0xc8, 0x2a, 0x48));
        let tint = |c: Color, k: f32| match c {
            Color::Rgb(r, g, b) => rgb(blend(bg, (r, g, b), k)),
            c => c,
        };
        Palette {
            light,
            fg: rgb(fg),
            muted: grey(0.7, 4.5),
            dim: grey(0.5, 3.0),
            faint: grey(0.24, 1.45),
            void: rgb(bg),
            blue: accent((0x6f, 0xb6, 0xff), (0x1f, 0x6c, 0xd0)),
            violet,
            pink: accent((0xff, 0x7a, 0xc6), (0xc0, 0x2d, 0x7c)),
            green,
            red,
            yellow: accent((0xff, 0xc1, 0x6b), (0x9a, 0x62, 0x00)),
            selection: tint(violet, if light { 0.2 } else { 0.3 }),
            diff_add_bg: tint(green, if light { 0.13 } else { 0.14 }),
            diff_del_bg: tint(red, if light { 0.12 } else { 0.14 }),
            claude: accent((0xff, 0xa0, 0x7a), (0xc0, 0x55, 0x2c)),
            chatgpt: green,
        }
    }
}

pub fn FG() -> Color {
    pal().fg
}
pub fn MUTED() -> Color {
    pal().muted
}
pub fn DIM() -> Color {
    pal().dim
}
/// Rules and decoration only; never for text.
pub fn FAINT() -> Color {
    pal().faint
}
/// The background: what things fade from and to.
pub fn VOID() -> Color {
    pal().void
}
pub fn BLUE() -> Color {
    pal().blue
}
pub fn VIOLET() -> Color {
    pal().violet
}
pub fn PINK() -> Color {
    pal().pink
}
pub fn GREEN() -> Color {
    pal().green
}
pub fn RED() -> Color {
    pal().red
}
pub fn YELLOW() -> Color {
    pal().yellow
}
pub fn CYAN() -> Color {
    pal().blue
}
pub fn SELECTION() -> Color {
    pal().selection
}
pub fn DIFF_ADD_BG() -> Color {
    pal().diff_add_bg
}
pub fn DIFF_DEL_BG() -> Color {
    pal().diff_del_bg
}

pub fn backend(kind: Kind) -> Color {
    let p = pal();
    match kind {
        Kind::ChatGpt => p.chatgpt,
        Kind::Claude => p.claude,
        Kind::Codex => p.blue,
    }
}

pub fn fg(c: Color) -> Style {
    Style::default().fg(c)
}

pub fn bold(c: Color) -> Style {
    Style::default().fg(c).add_modifier(Modifier::BOLD)
}

pub fn dim() -> Style {
    fg(DIM())
}

pub fn muted() -> Style {
    fg(MUTED())
}

/// Steps per gradient or blend. Animated colors snap to these so a moving effect reuses a
/// small fixed set of colors: terminals (xterm.js' WebGL renderer especially) cache a glyph per
/// color, and a fresh color every frame floods that cache, garbling text and slowing every pane.
pub const STEPS: f32 = 8.0;

/// The nebula gradient at `x` (wraps every 1.0): blue → violet → pink → blue, in 24 steps.
pub fn nebula(x: f32) -> Color {
    let p = pal();
    let x = (x.rem_euclid(1.0) * 3.0 * STEPS).round() / STEPS;
    let x = if x >= 3.0 { 0.0 } else { x };
    match x {
        x if x < 1.0 => mix(p.blue, p.violet, x),
        x if x < 2.0 => mix(p.violet, p.pink, x - 1.0),
        x => mix(p.pink, p.blue, x - 2.0),
    }
}

/// A key hint: the key in muted, its meaning dim.
pub fn key_hint(k: &str, what: &str) -> [Span<'static>; 2] {
    [Span::styled(k.to_string(), fg(MUTED())), Span::styled(format!(" {what}"), fg(DIM()))]
}

/// Moon phase for a fraction 0..=1: ○ ◔ ◑ ◕ ●.
pub fn moon(p: f32) -> &'static str {
    const M: [&str; 5] = ["○", "◔", "◑", "◕", "●"];
    M[((p.clamp(0.0, 1.0) * 4.0).round() as usize).min(4)]
}

/// Blend of two RGB colors, `t` snapped to `STEPS` (see there).
pub fn mix(a: Color, b: Color, t: f32) -> Color {
    let t = (t.clamp(0.0, 1.0) * STEPS).round() / STEPS;
    match (a, b) {
        (Color::Rgb(r1, g1, b1), Color::Rgb(r2, g2, b2)) => rgb(blend((r1, g1, b1), (r2, g2, b2), t)),
        _ => if t < 0.5 { a } else { b },
    }
}

fn rgb((r, g, b): Rgb) -> Color {
    Color::Rgb(r, g, b)
}

fn blend(a: Rgb, b: Rgb, t: f32) -> Rgb {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    (l(a.0, b.0), l(a.1, b.1), l(a.2, b.2))
}

/// WCAG relative luminance.
fn lum((r, g, b): Rgb) -> f32 {
    let c = |v: u8| {
        let v = v as f32 / 255.0;
        if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
    };
    0.2126 * c(r) + 0.7152 * c(g) + 0.0722 * c(b)
}

/// WCAG contrast ratio (1..21).
pub fn contrast(a: Rgb, b: Rgb) -> f32 {
    let (x, y) = (lum(a), lum(b));
    (x.max(y) + 0.05) / (x.min(y) + 0.05)
}

/// `c`, pushed toward `toward` until it has at least `min` contrast against `bg`.
fn reach(c: Rgb, bg: Rgb, toward: Rgb, min: f32) -> Rgb {
    let mut out = c;
    let mut t = 0.0;
    while contrast(out, bg) < min && t < 1.0 {
        t += 0.05;
        out = blend(c, toward, t);
    }
    out
}

/// Parses an XParseColor reply body like `rgb:ffff/8080/0000` (1–4 hex digits per channel).
pub fn parse_rgb(s: &str) -> Option<Rgb> {
    let body = s.trim().strip_prefix("rgb:").or_else(|| s.trim().strip_prefix("rgba:"))?;
    let mut parts = body.split('/');
    let mut ch = || -> Option<u8> {
        let h = parts.next()?;
        let h = h.get(..h.len().min(4))?;
        let v = u32::from_str_radix(h, 16).ok()?;
        let max = (1u32 << (4 * h.len() as u32)) - 1;
        Some(((v * 255 + max / 2) / max) as u8)
    };
    Some((ch()?, ch()?, ch()?))
}

/// Asks the terminal for its foreground and background (OSC 10/11), waiting at most `wait`.
/// A device-attributes query goes last: every terminal answers it, so terminals that ignore
/// OSC 10/11 don't cost the full wait. Needs raw mode, and must run before anything else
/// reads stdin. Falls back to COLORFGBG.
pub fn detect(wait: std::time::Duration) -> (Option<Rgb>, Option<Rgb>) {
    let (mut bg, mut fg) = query(wait);
    if bg.is_none() {
        // COLORFGBG="15;0": ANSI indices; 0-6 and 8 are dark.
        if let Some(b) = std::env::var("COLORFGBG").ok().and_then(|v| v.rsplit(';').next().and_then(|n| n.parse::<u8>().ok())) {
            bg = Some(if b == 7 || b >= 9 { (0xfa, 0xfa, 0xfa) } else { DARK_BG });
            fg = None;
        }
    }
    (bg, fg)
}

#[cfg(unix)]
fn query(wait: std::time::Duration) -> (Option<Rgb>, Option<Rgb>) {
    use std::io::{Read, Write};
    use std::os::fd::AsRawFd;
    use std::time::Instant;
    let mut out = std::io::stdout();
    if out.write_all(b"\x1b]10;?\x1b\\\x1b]11;?\x1b\\\x1b[c").and_then(|_| out.flush()).is_err() {
        return (None, None);
    }
    let stdin = std::io::stdin();
    let fd = stdin.as_raw_fd();
    let deadline = Instant::now() + wait;
    let mut buf = Vec::new();
    let mut lock = stdin.lock();
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        let mut pfd = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
        // SAFETY: one valid pollfd for the duration of the call.
        let n = unsafe { libc::poll(&mut pfd, 1, left.as_millis().max(1) as i32) };
        if n <= 0 {
            break;
        }
        let mut chunk = [0u8; 256];
        match lock.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(k) => buf.extend_from_slice(&chunk[..k]),
        }
        // The device-attributes reply (ESC [ ? … c) ends the conversation.
        let s = String::from_utf8_lossy(&buf);
        if s.find("\x1b[?").is_some_and(|i| s[i..].contains('c')) {
            break;
        }
    }
    let s = String::from_utf8_lossy(&buf);
    let find = |code: &str| {
        let i = s.find(&format!("]{code};"))? + code.len() + 2;
        let end = s[i..].find(['\x07', '\x1b']).map(|e| i + e).unwrap_or(s.len());
        parse_rgb(&s[i..end])
    };
    (find("11"), find("10"))
}

#[cfg(not(unix))]
fn query(_wait: std::time::Duration) -> (Option<Rgb>, Option<Rgb>) {
    (None, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rgb_of(c: Color) -> Rgb {
        match c {
            Color::Rgb(r, g, b) => (r, g, b),
            _ => panic!("not rgb"),
        }
    }

    #[test]
    fn parses_osc_colors() {
        assert_eq!(parse_rgb("rgb:ffff/8080/0000"), Some((255, 128, 0)));
        assert_eq!(parse_rgb("rgb:ff/80/00"), Some((255, 128, 0)));
        assert_eq!(parse_rgb("rgb:f/8/0"), Some((255, 136, 0)));
        assert_eq!(parse_rgb("nope"), None);
    }

    /// Every winfer theme's terminal colors: text and accents stay readable.
    #[test]
    fn readable_on_all_themes() {
        let themes: [(Rgb, Rgb); 8] = [
            ((0x12, 0x12, 0x15), (0xe6, 0xe6, 0xea)), // winfer dark
            ((0xfb, 0xfb, 0xfc), (0x1b, 0x1b, 0x21)), // winfer light
            ((0x28, 0x2a, 0x36), (0xf8, 0xf8, 0xf2)), // dracula
            ((0x28, 0x28, 0x28), (0xeb, 0xdb, 0xb2)), // gruvbox dark
            ((0xfb, 0xf1, 0xc7), (0x3c, 0x38, 0x36)), // gruvbox light
            ((0x00, 0x2b, 0x36), (0x93, 0xa1, 0xa1)), // solarized dark
            ((0xfd, 0xf6, 0xe3), (0x58, 0x6e, 0x75)), // solarized light
            ((0xe1, 0xe2, 0xe7), (0x37, 0x60, 0xbf)), // tokyo night day
        ];
        for (bg, fg) in themes {
            let p = Palette::derive(bg, Some(fg));
            for (name, c, min) in [("fg", p.fg, 4.5), ("muted", p.muted, 4.5), ("dim", p.dim, 3.0), ("violet", p.violet, 3.0), ("green", p.green, 3.0), ("red", p.red, 3.0), ("yellow", p.yellow, 3.0), ("blue", p.blue, 3.0), ("pink", p.pink, 3.0)] {
                let k = contrast(rgb_of(c), bg);
                assert!(k >= min, "{name} on {bg:?}: {k:.2} < {min}");
            }
        }
    }
}
