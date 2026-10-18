//! Animation helpers. Everything is a pure function of time, so a frame is just "render now";
//! the app only ticks (~25fps) while something is moving.

use std::time::{Duration, Instant};

use ratatui::style::{Color, Style};
use ratatui::text::Span;

use super::theme;

pub fn ease_out(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

/// A value that moves smoothly toward its target.
#[derive(Debug, Clone, Copy)]
pub struct Tween {
    from: f32,
    to: f32,
    start: Instant,
    dur: Duration,
}

impl Tween {
    pub fn new(v: f32) -> Self {
        Tween { from: v, to: v, start: Instant::now(), dur: Duration::from_millis(1) }
    }
    pub fn set(&mut self, to: f32, dur_ms: u64) {
        if (to - self.to).abs() < f32::EPSILON {
            return;
        }
        self.from = self.value();
        self.to = to;
        self.start = Instant::now();
        self.dur = Duration::from_millis(dur_ms);
    }
    pub fn value(&self) -> f32 {
        let t = self.start.elapsed().as_secs_f32() / self.dur.as_secs_f32();
        self.from + (self.to - self.from) * ease_out(t)
    }
    pub fn moving(&self) -> bool {
        self.start.elapsed() < self.dur
    }
}

/// 0..1..0 over `period`.
pub fn pulse(t: Duration, period_ms: u64) -> f32 {
    let p = (t.as_millis() % period_ms as u128) as f32 / period_ms as f32;
    0.5 - 0.5 * (p * std::f32::consts::TAU).cos()
}

pub fn elapsed(d: Duration) -> String {
    let s = d.as_secs();
    if s < 60 { format!("{s}s") } else { format!("{}m {:02}s", s / 60, s % 60) }
}

pub fn tokens(n: u64) -> String {
    match n {
        0..1_000 => n.to_string(),
        1_000..1_000_000 => format!("{:.1}k", n as f32 / 1000.0),
        _ => format!("{:.2}M", n as f32 / 1_000_000.0),
    }
}

const MOON: [&str; 4] = ["◐", "◓", "◑", "◒"];

/// A turning moon, the mark for running background agents on the status line.
pub fn moon_spin(t: Duration) -> &'static str {
    MOON[(t.as_millis() / 130) as usize % MOON.len()]
}

const DOTS: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// Spinning braille dots ("dots" from sindresorhus/cli-spinners, 80ms a frame): the spinner
/// for async and background work.
pub fn dots_spin(t: Duration) -> &'static str {
    DOTS[(t.as_millis() / 80) as usize % DOTS.len()]
}

/// How every dots spinner is drawn: one steady violet, never bold (bold braille renders as a
/// heavy full grid in many terminal fonts).
pub fn spin_style() -> Style {
    theme::fg(theme::nebula(0.37))
}

const STAR: [&str; 6] = ["✶", "✷", "✸", "✹", "✺", "✹"];

/// A pulsing star, the "agent is working" mark (tools, plan items and background work use the dots).
pub fn star_spin(t: Duration) -> &'static str {
    STAR[(t.as_millis() / 140) as usize % STAR.len()]
}

/// Text painted with the nebula gradient, flowing to the right over time.
pub fn nebula_text(text: &str, t: Duration, style: Style) -> Vec<Span<'static>> {
    let n = text.chars().count().max(8) as f32;
    let shift = t.as_secs_f32() * 0.25;
    text.chars()
        .enumerate()
        .map(|(i, c)| Span::styled(c.to_string(), style.fg(theme::nebula(i as f32 / (n * 1.6) - shift))))
        .collect()
}

/// `base`-colored text with a nebula-tinted light band sweeping across it.
pub fn nebula_shimmer(text: &str, t: Duration, base: Color) -> Vec<Span<'static>> {
    let chars: Vec<char> = text.chars().collect();
    let width = chars.len() as f32 + 12.0;
    let pos = (t.as_secs_f32() * 14.0) % width - 6.0;
    chars
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let d = (i as f32 - pos).abs();
            let k = (1.0 - d / 5.0).max(0.0);
            let glow = theme::mix(theme::nebula(i as f32 / 24.0 - t.as_secs_f32() * 0.2), theme::FG(), 0.35);
            Span::styled(c.to_string(), Style::default().fg(theme::mix(base, glow, k)))
        })
        .collect()
}

/// A hairline `width` wide. With `moving`, a comet with a nebula tail travels along it;
/// otherwise it glows faintly from the left and fades out.
pub fn comet(width: usize, t: Duration, moving: bool) -> Vec<Span<'static>> {
    let w = width.max(1) as f32;
    let head = if moving { (t.as_secs_f32() * 0.55).fract() * (w + 30.0) - 10.0 } else { -1000.0 };
    (0..width)
        .map(|i| {
            let x = i as f32;
            let base = theme::mix(theme::mix(theme::VIOLET(), theme::FAINT(), 0.55), theme::FAINT(), (x / (w * 0.35)).min(1.0));
            let d = head - x;
            let c = if moving && (0.0..24.0).contains(&d) {
                let k = 1.0 - d / 24.0;
                theme::mix(base, theme::mix(theme::nebula(d / 48.0 + 0.1), theme::FG(), if d < 1.0 { 0.6 } else { 0.0 }), k.powf(1.6))
            } else {
                base
            };
            Span::styled("─", Style::default().fg(c))
        })
        .collect()
}

/// Fades lines in: k=0 is nearly invisible, k=1 is as drawn.
pub fn fade(lines: &mut [ratatui::text::Line<'static>], k: f32) {
    if k >= 1.0 {
        return;
    }
    let k = ease_out(k);
    for l in lines {
        for s in &mut l.spans {
            if let Some(c) = s.style.fg {
                s.style.fg = Some(theme::mix(theme::VOID(), c, k));
            }
            if let Some(c) = s.style.bg {
                s.style.bg = Some(theme::mix(theme::VOID(), c, k));
            }
        }
    }
}

/// A cheap stable hash for placing stars.
fn hash(mut x: u32) -> u32 {
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb_352d);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846c_a68b);
    x ^ (x >> 16)
}

/// A twinkling starfield: (x, y, glyph, color) for a `w`×`h` area. Now and then a shooting
/// star crosses the top third.
pub fn stars(w: u16, h: u16, t: Duration) -> Vec<(u16, u16, &'static str, Color)> {
    const GLYPHS: [&str; 5] = ["·", "·", "˙", "✦", "✧"];
    let mut out = Vec::new();
    let secs = t.as_secs_f32();
    let count = (w as u32 * h as u32) / 70;
    for i in 0..count {
        let r = hash(i.wrapping_mul(2654435761).wrapping_add(17));
        let x = (r % w.max(1) as u32) as u16;
        let y = ((r >> 12) % h.max(1) as u32) as u16;
        let g = GLYPHS[((r >> 24) % 5) as usize];
        let phase = ((r >> 4) & 0xff) as f32 / 255.0;
        let speed = 0.25 + ((r >> 20) & 0xf) as f32 / 30.0;
        let tw = 0.5 + 0.5 * ((secs * speed + phase) * std::f32::consts::TAU).sin();
        let peak = if g == "✦" || g == "✧" { 0.75 } else { 0.45 };
        let tint = theme::nebula(phase);
        out.push((x, y, g, theme::mix(theme::VOID(), theme::mix(theme::DIM(), tint, 0.35), 0.15 + peak * tw)));
    }
    // Shooting star: every 7s, for 1.1s.
    let cycle = 7.0;
    let n = (secs / cycle) as u32;
    let p = (secs % cycle) / 1.1;
    if p < 1.0 && w > 20 && h > 4 {
        let r = hash(n.wrapping_add(99));
        let x0 = (r % (w as u32 / 2)) as f32 + w as f32 * 0.3;
        let y0 = ((r >> 8) % (h as u32 / 3).max(1)) as f32;
        let len = 9;
        for k in 0..len {
            let pos = p * 26.0 - k as f32 * 1.0;
            if pos < 0.0 {
                continue;
            }
            let x = x0 - pos * 1.0;
            let y = y0 + pos * 0.35;
            if x < 0.0 || y >= h as f32 {
                continue;
            }
            let fade = (1.0 - k as f32 / len as f32) * (1.0 - p).clamp(0.0, 1.0).sqrt();
            let glyph = if k == 0 { "✦" } else { "·" };
            out.push((x as u16, y as u16, glyph, theme::mix(theme::VOID(), theme::mix(theme::FG(), theme::PINK(), k as f32 / len as f32), fade)));
        }
    }
    out
}
