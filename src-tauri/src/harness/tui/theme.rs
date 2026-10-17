//! Colors. Truecolor, matching winfer's app palette; the terminal's own background shows through.

use ratatui::style::{Color, Modifier, Style};

use crate::harness::backend::Kind;

pub const ACCENT: Color = Color::Rgb(0x6d, 0x7c, 0xff);
pub const FG: Color = Color::Rgb(0xe6, 0xe8, 0xee);
pub const MUTED: Color = Color::Rgb(0x9a, 0xa0, 0xad);
pub const DIM: Color = Color::Rgb(0x6b, 0x71, 0x7e);
pub const FAINT: Color = Color::Rgb(0x3a, 0x3f, 0x4b);
pub const SURFACE: Color = Color::Rgb(0x1c, 0x1f, 0x26);
pub const GREEN: Color = Color::Rgb(0x7e, 0xe7, 0x87);
pub const RED: Color = Color::Rgb(0xff, 0x7b, 0x72);
pub const YELLOW: Color = Color::Rgb(0xf5, 0xb5, 0x44);
pub const CYAN: Color = Color::Rgb(0x79, 0xc0, 0xff);
pub const SELECTION: Color = Color::Rgb(0x2f, 0x3a, 0x6b);
pub const DIFF_ADD_BG: Color = Color::Rgb(0x14, 0x33, 0x1d);
pub const DIFF_DEL_BG: Color = Color::Rgb(0x3d, 0x16, 0x18);

pub fn backend(kind: Kind) -> Color {
    match kind {
        Kind::ChatGpt => Color::Rgb(0x10, 0xa3, 0x7f),
        Kind::Claude => Color::Rgb(0xd9, 0x77, 0x57),
        Kind::Codex => Color::Rgb(0x8b, 0x9c, 0xff),
    }
}

pub fn fg(c: Color) -> Style {
    Style::default().fg(c)
}

pub fn bold(c: Color) -> Style {
    Style::default().fg(c).add_modifier(Modifier::BOLD)
}

pub fn dim() -> Style {
    fg(DIM)
}

pub fn muted() -> Style {
    fg(MUTED)
}

/// Linear blend of two RGB colors.
pub fn mix(a: Color, b: Color, t: f32) -> Color {
    let t = t.clamp(0.0, 1.0);
    match (a, b) {
        (Color::Rgb(r1, g1, b1), Color::Rgb(r2, g2, b2)) => {
            let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
            Color::Rgb(l(r1, r2), l(g1, g2), l(b1, b2))
        }
        _ => if t < 0.5 { a } else { b },
    }
}
