//! Mermaid diagrams as images, rendered on the CPU by `mermaid-rs-renderer` and `resvg`.

use crate::theme;
use mermaid_rs_renderer::{LayoutConfig, Theme, compute_layout, parse_mermaid, render_svg};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, OnceLock};

/// Diagrams bigger than this (physical pixels per side) are scaled down so huge graphs stay safe.
const MAX_SIDE: f32 = 4096.0;
/// Retina rasterization scale for crisp labels.
const SCALE: f32 = 2.0;

const LIGHT_SERIES: [&str; 12] = [
    "#2a78d6", "#eb6834", "#1baf7a", "#eda100", "#e87ba4", "#008300",
    "#6250d6", "#e34948", "#0f7f8c", "#9a6a00", "#8a3fc0", "#52525b",
];
const DARK_SERIES: [&str; 12] = [
    "#3987e5", "#d95926", "#199e70", "#c98500", "#d55181", "#0ca30c",
    "#9085e9", "#e66767", "#5fd4e0", "#f5b544", "#c08cff", "#a1a1aa",
];

pub struct Diagram {
    pub png: Vec<u8>,
    /// Logical (1x) dimensions in CSS pixels.
    pub width: f32,
    pub height: f32,
}

fn fontdb() -> Arc<resvg::usvg::fontdb::Database> {
    static DB: OnceLock<Arc<resvg::usvg::fontdb::Database>> = OnceLock::new();
    DB.get_or_init(|| {
        let mut db = resvg::usvg::fontdb::Database::new();
        db.load_system_fonts();
        db.set_sans_serif_family("Helvetica");
        db.set_monospace_family("Menlo");
        Arc::new(db)
    })
    .clone()
}

fn mermaid_theme(theme_id: &str) -> Theme {
    let def = theme::theme_by_id(theme_id);
    let u = &def.ui;
    let light = def.mode == "light";
    let mut t = if light { Theme::modern() } else { Theme::dark() };
    t.font_family = "Helvetica, Arial, sans-serif".to_string();
    t.font_size = 13.5;
    t.background = u.panel_raised.clone();
    t.primary_color = if light { u.panel.clone() } else { u.active.clone() };
    t.secondary_color = u.hover.clone();
    t.tertiary_color = if light { u.app.clone() } else { u.panel.clone() };
    t.primary_text_color = u.fg.clone();
    t.text_color = u.fg.clone();
    t.primary_border_color = u.border_strong.clone();
    t.line_color = u.fg_muted.clone();
    t.edge_label_background = u.panel_raised.clone();
    t.cluster_background = u.panel.clone();
    t.cluster_border = u.border_strong.clone();
    t.sequence_actor_fill = if light { u.panel.clone() } else { u.active.clone() };
    t.sequence_actor_border = u.border_strong.clone();
    t.sequence_actor_line = u.fg_subtle.clone();
    t.sequence_note_fill = u.hover.clone();
    t.sequence_note_border = u.border_strong.clone();
    t.sequence_activation_fill = u.hover.clone();
    t.sequence_activation_border = u.fg_muted.clone();
    t.pie_colors = (if light { LIGHT_SERIES } else { DARK_SERIES }).map(str::to_string);
    t.pie_title_text_color = u.fg.clone();
    t.pie_section_text_color = u.fg.clone();
    t.pie_legend_text_color = u.fg.clone();
    t.pie_stroke_color = u.panel_raised.clone();
    t.pie_outer_stroke_color = u.border_strong.clone();
    t.pie_opacity = if light { 0.88 } else { 0.82 };
    t
}

/// Renders Mermaid markup to an SVG string styled for `theme_id`.
pub fn render_to_svg(source: &str, theme_id: &str) -> Result<String, String> {
    let trimmed = source.trim();
    if trimmed.is_empty() {
        return Err("empty diagram".into());
    }
    catch_unwind(AssertUnwindSafe(|| {
        let parsed = parse_mermaid(trimmed).map_err(|e| e.to_string())?;
        let theme = mermaid_theme(theme_id);
        let config = LayoutConfig::default();
        let layout = compute_layout(&parsed.graph, &theme, &config);
        Ok(render_svg(&layout, &theme, &config))
    }))
    .map_err(|_| "diagram renderer panicked".to_string())?
}

/// Renders Mermaid markup to a 2x PNG image with logical dimensions.
pub fn render_png(source: &str, theme_id: &str) -> Result<Diagram, String> {
    let svg = render_to_svg(source, theme_id)?;
    let opts = resvg::usvg::Options { fontdb: fontdb(), ..Default::default() };
    let tree = resvg::usvg::Tree::from_str(&svg, &opts).map_err(|e| e.to_string())?;
    let logical = tree.size();
    let (w, h) = (logical.width().max(1.0), logical.height().max(1.0));
    let scale = SCALE.min(MAX_SIDE / w.max(h)).max(0.1);
    let pw = (w * scale).ceil().max(1.0) as u32;
    let ph = (h * scale).ceil().max(1.0) as u32;
    let mut pixmap = resvg::tiny_skia::Pixmap::new(pw, ph).ok_or("diagram is too large to rasterize")?;
    resvg::render(&tree, resvg::tiny_skia::Transform::from_scale(scale, scale), &mut pixmap.as_mut());
    let png = pixmap.encode_png().map_err(|e| e.to_string())?;
    Ok(Diagram { png, width: w, height: h })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_flowchart_and_sequence_to_png() {
        let flow = "flowchart LR\n    subgraph Providers\n        A[Claude / Codex] --> B[Harness]\n        C[Pi / OpenCode] --> B\n    end\n    B -->|Events| D{Viewer}\n    D -->|Render| E[Native GPUI]\n";
        let diagram = render_png(flow, "dark").expect("flowchart renders");
        assert!(diagram.png.starts_with(&[0x89, b'P', b'N', b'G']));
        assert!(diagram.width > 50.0 && diagram.height > 20.0);

        let seq = "sequenceDiagram\n    Alice->>Bob: Hello\n    Bob-->>Alice: Hi\n";
        let seq_light = render_png(seq, "light").expect("sequence diagram renders");
        assert!(seq_light.png.starts_with(&[0x89, b'P', b'N', b'G']));
        assert!(seq_light.width > 50.0 && seq_light.height > 20.0);
    }

    #[test]
    fn rejects_empty_and_invalid_diagrams() {
        assert!(render_png("   \n", "dark").is_err());
        assert!(render_png("not a valid mermaid diagram %%%", "dark").is_err());
    }
}
