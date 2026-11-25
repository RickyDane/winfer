//! Color theme definitions, the Tauri app's exported to JSON (`assets/themes.json`). The app paints
//! with them (`winfer-app/src/theme.rs`); the core only needs a theme's mode and counterpart.

use serde::Deserialize;
use std::sync::{LazyLock, RwLock};

pub const JSON: &str = include_str!("../assets/themes.json");

#[derive(Clone, Deserialize)]
pub struct Meta {
    pub id: String,
    pub mode: String,
    pub pair: String,
    ansi: [String; 16],
    ui: Surface,
}

#[derive(Clone, Deserialize)]
struct Surface {
    panel: String,
    fg: String,
}

static METAS: LazyLock<Vec<Meta>> = LazyLock::new(|| serde_json::from_str(JSON).expect("themes.json"));

/// The theme with this id, else the first (default) one.
pub fn meta(id: &str) -> &'static Meta {
    METAS.iter().find(|t| t.id == id).unwrap_or(&METAS[0])
}

/// The terminal's palette: 16 ANSI colors, background, foreground (answers programs' color queries).
#[derive(Clone, Copy)]
pub struct Palette {
    pub ansi: [[u8; 3]; 16],
    pub background: [u8; 3],
    pub foreground: [u8; 3],
    pub light: bool,
}

/// `#rgb`, `#rrggbb` or `#rrggbbaa` (alpha ignored).
fn bytes(s: &str) -> [u8; 3] {
    let h = s.trim_start_matches('#');
    let expanded: String = if h.len() == 3 { h.chars().flat_map(|c| [c, c]).collect() } else { h.to_string() };
    let n = u32::from_str_radix(expanded.get(..6).unwrap_or(""), 16).unwrap_or(0);
    [(n >> 16) as u8, (n >> 8) as u8, n as u8]
}

fn palette_for(id: &str) -> Palette {
    let t = meta(id);
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

#[cfg(test)]
mod tests {
    #[test]
    fn parses_colors() {
        assert_eq!(super::bytes("#102030"), [0x10, 0x20, 0x30]);
        assert_eq!(super::bytes("#abc"), [0xaa, 0xbb, 0xcc]);
        assert_eq!(super::bytes("#10203040"), [0x10, 0x20, 0x30]);
    }
}
