//! Color themes, the Tauri app's definitions exported to JSON (`assets/themes.json`).

use gpui_kit::{Hsla, Rgba, rgba};
use serde::Deserialize;
use std::sync::{LazyLock, RwLock};

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Ui {
    pub app: String,
    pub panel: String,
    pub panel_raised: String,
    pub hover: String,
    pub active: String,
    pub border: String,
    pub border_strong: String,
    pub fg: String,
    pub fg_muted: String,
    pub fg_subtle: String,
    pub accent: String,
    pub on_accent: String,
    pub success: String,
    pub warning: String,
    pub danger: String,
    pub claude: String,
}

#[derive(Clone, Deserialize)]
pub struct ThemeDef {
    pub id: String,
    pub name: String,
    pub mode: String,
    pub pair: String,
    pub ui: Ui,
    pub ansi: [String; 16],
}

pub static THEMES: LazyLock<Vec<ThemeDef>> =
    LazyLock::new(|| serde_json::from_str(include_str!("../assets/themes.json")).expect("themes.json"));

pub fn theme_by_id(id: &str) -> &'static ThemeDef {
    THEMES.iter().find(|t| t.id == id).unwrap_or(&THEMES[0])
}

pub fn hex(s: &str) -> Hsla {
    rgb_of(s).into()
}

fn rgb_of(s: &str) -> Rgba {
    let h = s.trim_start_matches('#');
    let expanded: String = if h.len() == 3 { h.chars().flat_map(|c| [c, c]).collect() } else { h.to_string() };
    let n = u32::from_str_radix(&expanded, 16).unwrap_or(0);
    if expanded.len() == 8 { rgba(n) } else { rgba((n << 8) | 0xff) }
}

/// A theme's colors, ready to paint.
#[derive(Clone, Copy)]
pub struct Colors {
    pub light: bool,
    pub app: Hsla,
    pub panel: Hsla,
    pub raised: Hsla,
    pub hover: Hsla,
    pub active: Hsla,
    pub border: Hsla,
    pub border_strong: Hsla,
    pub fg: Hsla,
    pub muted: Hsla,
    pub subtle: Hsla,
    pub accent: Hsla,
    pub on_accent: Hsla,
    pub success: Hsla,
    pub warning: Hsla,
    pub danger: Hsla,
    pub claude: Hsla,
}

pub fn colors(id: &str) -> Colors {
    let def = theme_by_id(id);
    let u = &def.ui;
    Colors {
        light: def.mode == "light",
        app: hex(&u.app),
        panel: hex(&u.panel),
        raised: hex(&u.panel_raised),
        hover: hex(&u.hover),
        active: hex(&u.active),
        border: hex(&u.border),
        border_strong: hex(&u.border_strong),
        fg: hex(&u.fg),
        muted: hex(&u.fg_muted),
        subtle: hex(&u.fg_subtle),
        accent: hex(&u.accent),
        on_accent: hex(&u.on_accent),
        success: hex(&u.success),
        warning: hex(&u.warning),
        danger: hex(&u.danger),
        claude: hex(&u.claude),
    }
}

/// The terminal's palette: 16 ANSI colors, background, foreground, cursor.
#[derive(Clone, Copy)]
pub struct Palette {
    pub ansi: [[u8; 3]; 16],
    pub background: [u8; 3],
    pub foreground: [u8; 3],
    pub light: bool,
}

fn bytes(s: &str) -> [u8; 3] {
    let c = rgb_of(s);
    [(c.r * 255.0).round() as u8, (c.g * 255.0).round() as u8, (c.b * 255.0).round() as u8]
}

fn palette_for(id: &str) -> Palette {
    let t = theme_by_id(id);
    Palette {
        ansi: std::array::from_fn(|i| bytes(&t.ansi[i])),
        background: bytes(&t.ui.panel),
        foreground: bytes(&t.ui.fg),
        light: t.mode == "light",
    }
}

static PALETTE: LazyLock<RwLock<Palette>> = LazyLock::new(|| RwLock::new(palette_for("dark")));

pub fn set_palette(id: &str) {
    *PALETTE.write().unwrap() = palette_for(id);
}

pub fn palette() -> Palette {
    *PALETTE.read().unwrap()
}
