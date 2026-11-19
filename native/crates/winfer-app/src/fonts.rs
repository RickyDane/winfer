//! The app's fonts: the first installed of the families the web UI asks for.

use gpui_kit::{App, SharedString};
use std::sync::OnceLock;

static MONO: OnceLock<SharedString> = OnceLock::new();
static SANS: OnceLock<SharedString> = OnceLock::new();

fn first_installed(wanted: &[&'static str], cx: &App) -> SharedString {
    let names = cx.text_system().all_font_names();
    wanted.iter().find(|w| names.iter().any(|n| n == *w)).map_or_else(|| SharedString::from(*wanted.last().unwrap()), |w| SharedString::from(*w))
}

pub fn mono(cx: &App) -> SharedString {
    MONO.get_or_init(|| first_installed(&["JetBrains Mono", "SF Mono", "Menlo", "Consolas", "DejaVu Sans Mono", "monospace"], cx)).clone()
}

pub fn sans(cx: &App) -> SharedString {
    SANS.get_or_init(|| first_installed(&["Inter", ".SystemUIFont", "Segoe UI", "Noto Sans", "sans-serif"], cx)).clone()
}
