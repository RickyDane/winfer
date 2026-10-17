//! Animation helpers. Everything is a pure function of time, so a frame is just "render now";
//! the app only ticks (~60fps) while something is moving.

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

const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

pub fn spinner(t: Duration) -> &'static str {
    SPINNER[(t.as_millis() / 80) as usize % SPINNER.len()]
}

const ORB: [&str; 6] = ["·", "✢", "✳", "✶", "✻", "✽"];

/// A breathing star for "thinking".
pub fn orb(t: Duration) -> &'static str {
    let n = ORB.len() * 2 - 2;
    let i = (t.as_millis() / 120) as usize % n;
    ORB[if i < ORB.len() { i } else { n - i }]
}

/// Text with a light band sweeping across it (the "working" shimmer).
pub fn shimmer(text: &str, t: Duration, base: Color, glow: Color) -> Vec<Span<'static>> {
    let chars: Vec<char> = text.chars().collect();
    let width = chars.len() as f32 + 10.0;
    let pos = (t.as_secs_f32() * 18.0) % width - 5.0;
    chars
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let d = (i as f32 - pos).abs();
            let k = (1.0 - d / 4.0).max(0.0);
            Span::styled(c.to_string(), Style::default().fg(theme::mix(base, glow, k)))
        })
        .collect()
}

/// 0..1..0 over `period`.
pub fn pulse(t: Duration, period_ms: u64) -> f32 {
    let p = (t.as_millis() % period_ms as u128) as f32 / period_ms as f32;
    0.5 - 0.5 * (p * std::f32::consts::TAU).cos()
}

/// Gradient across a line of text from `a` to `b`, shifted over time.
pub fn gradient(text: &str, t: Duration, a: Color, b: Color) -> Vec<Span<'static>> {
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len().max(1) as f32;
    let shift = t.as_secs_f32() * 0.35;
    chars
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let x = (i as f32 / n + shift) * std::f32::consts::TAU;
            let k = 0.5 + 0.5 * x.sin();
            Span::styled(c.to_string(), Style::default().fg(theme::mix(a, b, k)))
        })
        .collect()
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
