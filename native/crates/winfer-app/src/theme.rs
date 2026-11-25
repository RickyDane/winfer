//! Color themes, the Tauri app's definitions exported to JSON (`winfer-core/assets/themes.json`).

use gpui_kit::{Hsla, Rgba, rgba};
use serde::{Deserialize, Serialize};
use std::sync::LazyLock;

#[derive(Clone, Deserialize, Serialize)]
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
    LazyLock::new(|| serde_json::from_str(winfer_core::theme::JSON).expect("themes.json"));

pub fn theme_by_id(id: &str) -> &'static ThemeDef {
    THEMES.iter().find(|t| t.id == id).unwrap_or(&THEMES[0])
}

pub fn hex(s: &str) -> Hsla {
    rgb_of(s).into()
}

/// Opaque sRGB linear blend of `over` on top of `base` (`amount` in `0.0..=1.0`), with `a = 1.0`.
/// Keeps floating surfaces opaque so GPUI drop shadows never bleed through their interior.
pub fn blend(base: Hsla, over: Hsla, amount: f32) -> Hsla {
    let a = Rgba::from(base);
    let b = Rgba::from(over);
    let t = amount.clamp(0.0, 1.0);
    Rgba {
        r: a.r + (b.r - a.r) * t,
        g: a.g + (b.g - a.g) * t,
        b: a.b + (b.b - a.b) * t,
        a: 1.0,
    }
    .into()
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
    /// Subtle outer edge for cards, groups, inputs, buttons and floating surfaces.
    pub edge: Hsla,
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
    let (border, border_strong) = (hex(&u.border), hex(&u.border_strong));
    Colors {
        light: def.mode == "light",
        app: hex(&u.app),
        panel: hex(&u.panel),
        raised: hex(&u.panel_raised),
        hover: hex(&u.hover),
        active: hex(&u.active),
        border,
        border_strong,
        edge: blend(border, border_strong, 0.4),
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

pub use winfer_core::theme::{Palette, palette, set_palette};
