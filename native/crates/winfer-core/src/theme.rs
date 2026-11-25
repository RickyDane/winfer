//! Color theme definitions, the Tauri app's exported to JSON (`assets/themes.json`). The app paints
//! with them (`winfer-app/src/theme.rs`); the core only needs a theme's mode and counterpart.

use serde::Deserialize;
use std::sync::LazyLock;

pub const JSON: &str = include_str!("../assets/themes.json");

#[derive(Clone, Deserialize)]
pub struct Meta {
    pub id: String,
    pub mode: String,
    pub pair: String,
}

static METAS: LazyLock<Vec<Meta>> = LazyLock::new(|| serde_json::from_str(JSON).expect("themes.json"));

/// The theme with this id, else the first (default) one.
pub fn meta(id: &str) -> &'static Meta {
    METAS.iter().find(|t| t.id == id).unwrap_or(&METAS[0])
}
